//! Application state, background tasks and input handling for the SHCCL TUI.
//!
//! The structure mirrors the SDL GUI frontend: every CLI area has a matching
//! page, long-running work happens on the Tokio runtime, and progress/log events
//! arrive through an unbounded channel that the event loop drains each frame.

use std::path::PathBuf;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use shccl_core::{
    Account, AccountStore, DeviceCode, InstalledVersionSummary, InstanceConfig, LaunchRequest,
    Launcher, LauncherConfig, LogStream, ProgressEvent, ProgressSink, VersionManifest, detect_java,
};
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// Maximum number of log lines kept in memory.
const LOG_LIMIT: usize = 4000;

/// Pages shown in the tab bar. Every CLI area has a matching page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    Dashboard,
    Versions,
    Accounts,
    Settings,
    Logs,
}

impl Page {
    pub(crate) const ALL: [Page; 5] = [
        Page::Dashboard,
        Page::Versions,
        Page::Accounts,
        Page::Settings,
        Page::Logs,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Page::Dashboard => "Dashboard",
            Page::Versions => "Versions",
            Page::Accounts => "Accounts",
            Page::Settings => "Settings",
            Page::Logs => "Logs",
        }
    }

    pub(crate) fn hotkey(self) -> &'static str {
        match self {
            Page::Dashboard => "1",
            Page::Versions => "2",
            Page::Accounts => "3",
            Page::Settings => "4",
            Page::Logs => "5",
        }
    }

    fn next(self) -> Self {
        match self {
            Page::Dashboard => Page::Versions,
            Page::Versions => Page::Accounts,
            Page::Accounts => Page::Settings,
            Page::Settings => Page::Logs,
            Page::Logs => Page::Dashboard,
        }
    }

    fn previous(self) -> Self {
        match self {
            Page::Dashboard => Page::Logs,
            Page::Versions => Page::Dashboard,
            Page::Accounts => Page::Versions,
            Page::Settings => Page::Accounts,
            Page::Logs => Page::Settings,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VersionTab {
    Installed,
    Remote,
}

/// Which settings file the Settings page edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SettingsScope {
    Global,
    Version(String),
}

impl SettingsScope {
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Global => "Global defaults".to_string(),
            Self::Version(version) => format!("version {version}"),
        }
    }

    pub(crate) fn version(&self) -> Option<&str> {
        match self {
            Self::Global => None,
            Self::Version(version) => Some(version),
        }
    }

    pub(crate) fn is_version(&self) -> bool {
        matches!(self, Self::Version(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Setting {
    Scope,
    Java,
    Memory,
    Concurrency,
    CheckFiles,
    DefaultVersion,
    GameDir,
}

impl Setting {
    pub(crate) const ALL: [Setting; 7] = [
        Setting::Scope,
        Setting::Java,
        Setting::Memory,
        Setting::Concurrency,
        Setting::CheckFiles,
        Setting::DefaultVersion,
        Setting::GameDir,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Setting::Scope => "Target",
            Setting::Java => "Java path",
            Setting::Memory => "Memory",
            Setting::Concurrency => "Download concurrency",
            Setting::CheckFiles => "File checks",
            Setting::DefaultVersion => "Default version",
            Setting::GameDir => "Game directory",
        }
    }

    pub(crate) fn clearable(self, version_scope: bool) -> bool {
        match self {
            Setting::Scope => false,
            Setting::DefaultVersion => !version_scope,
            Setting::Java | Setting::GameDir => true,
            Setting::Memory | Setting::Concurrency | Setting::CheckFiles => version_scope,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone)]
pub(crate) struct Status {
    pub level: Level,
    pub text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Busy {
    pub label: String,
    pub phase: String,
    pub completed: u64,
    pub total: u64,
    pub file: String,
}

impl Busy {
    fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            phase: String::new(),
            completed: 0,
            total: 0,
            file: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputAction {
    LoginOffline,
    InstallVersion,
    RemoteFilter,
    SetSettingsTarget,
    SetJava,
    SetMemory,
    SetConcurrency,
    SetCheckFiles,
    SetDefaultVersion,
    SetGameDir,
}

#[derive(Debug, Clone)]
pub(crate) struct InputModal {
    pub title: String,
    pub prompt: String,
    pub value: String,
    /// Cursor position measured in characters.
    pub cursor: usize,
    /// Selection anchor; the selection spans from here to the cursor.
    pub selection: Option<usize>,
    pub action: InputAction,
}

impl InputModal {
    fn new(
        title: impl Into<String>,
        prompt: impl Into<String>,
        value: impl Into<String>,
        action: InputAction,
    ) -> Self {
        let value = value.into();
        let cursor = value.chars().count();
        Self {
            title: title.into(),
            prompt: prompt.into(),
            value,
            cursor,
            selection: None,
            action,
        }
    }

    pub(crate) fn selected_range(&self) -> Option<(usize, usize)> {
        let anchor = self.selection?;
        Some((anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    pub(crate) fn insert_str(&mut self, text: &str) {
        let filtered: String = text.chars().filter(|c| !c.is_control()).collect();
        if filtered.is_empty() {
            return;
        }
        self.remove_selection();
        let mut chars: Vec<char> = self.value.chars().collect();
        self.cursor = self.cursor.min(chars.len());
        for (offset, ch) in filtered.chars().enumerate() {
            chars.insert(self.cursor + offset, ch);
        }
        self.cursor += filtered.chars().count();
        self.value = chars.into_iter().collect();
    }

    fn remove_selection(&mut self) -> bool {
        let Some((start, end)) = self.selected_range() else {
            return false;
        };
        let mut chars: Vec<char> = self.value.chars().collect();
        chars.drain(start..end);
        self.value = chars.into_iter().collect();
        self.cursor = start;
        self.selection = None;
        true
    }

    fn backspace(&mut self) {
        if self.remove_selection() {
            return;
        }
        let mut chars: Vec<char> = self.value.chars().collect();
        if self.cursor > 0 && self.cursor <= chars.len() {
            chars.remove(self.cursor - 1);
            self.cursor -= 1;
        }
        self.value = chars.into_iter().collect();
    }

    fn delete(&mut self) {
        if self.remove_selection() {
            return;
        }
        let mut chars: Vec<char> = self.value.chars().collect();
        if self.cursor < chars.len() {
            chars.remove(self.cursor);
        }
        self.value = chars.into_iter().collect();
    }

    fn move_cursor(&mut self, delta: isize) {
        self.selection = None;
        let len = self.value.chars().count();
        if delta < 0 {
            self.cursor = self.cursor.saturating_sub(delta.unsigned_abs());
        } else {
            self.cursor = (self.cursor + delta as usize).min(len);
        }
    }

    fn cursor_home(&mut self) {
        self.selection = None;
        self.cursor = 0;
    }

    fn cursor_end(&mut self) {
        self.selection = None;
        self.cursor = self.value.chars().count();
    }

    fn select_all(&mut self) {
        self.selection = Some(0);
        self.cursor = self.value.chars().count();
    }
}

/// A clickable row recorded while drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowTarget {
    Installed,
    Remote,
    Account,
    Setting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskKind {
    Install,
    Launch,
    Manifest,
    Login,
}

enum BackendEvent {
    Step {
        phase: String,
        message: String,
    },
    Batch {
        phase: String,
        completed: u64,
        total: u64,
        file: String,
    },
    Log {
        stream: LogStream,
        line: String,
    },
    Installed(Vec<InstalledVersionSummary>),
    Manifest(VersionManifest),
    JavaResult {
        result: std::result::Result<String, String>,
    },
    Running {
        version: String,
    },
    MicrosoftCode(DeviceCode),
    TaskDone {
        task: TaskKind,
        ok: bool,
        message: String,
    },
}

pub(crate) struct App {
    pub(crate) launcher: Launcher,
    pub(crate) installed: Vec<InstalledVersionSummary>,
    pub(crate) accounts: AccountStore,
    pub(crate) config: LauncherConfig,
    /// Which settings file the Settings page edits.
    pub(crate) settings_scope: SettingsScope,
    /// Cached overrides for the selected per-version target.
    pub(crate) instance_config: InstanceConfig,
    pub(crate) manifest: Option<VersionManifest>,
    pub(crate) java: Option<std::result::Result<String, String>>,
    pub(crate) page: Page,
    pub(crate) version_tab: VersionTab,
    pub(crate) installed_sel: usize,
    pub(crate) installed_state: ListState,
    pub(crate) remote_sel: usize,
    pub(crate) remote_state: ListState,
    pub(crate) remote_filter: String,
    pub(crate) account_sel: usize,
    pub(crate) accounts_state: ListState,
    pub(crate) setting_sel: usize,
    pub(crate) settings_state: ListState,
    pub(crate) logs: Vec<String>,
    /// How many wrapped log lines above the tail are skipped.
    pub(crate) log_offset: usize,
    /// Total wrapped log lines produced during the last draw.
    pub(crate) log_visual_rows: usize,
    /// Number of log rows that fit on screen.
    pub(crate) log_view_rows: usize,
    pub(crate) status: Status,
    pub(crate) busy: Option<Busy>,
    pub(crate) running: Option<String>,
    pub(crate) modal: Option<InputModal>,
    /// Pending Microsoft device-code sign-in shown as an overlay.
    pub(crate) microsoft: Option<DeviceCode>,
    pub(crate) help: bool,
    pub(crate) quit: bool,
    /// Rows available in the current list, updated while drawing.
    pub(crate) view_rows: usize,
    pub(crate) tab_hits: Vec<(Rect, Page)>,
    pub(crate) version_tab_hits: Vec<(Rect, VersionTab)>,
    pub(crate) row_hits: Vec<(Rect, RowTarget, usize)>,
    pub(crate) log_area: Option<Rect>,

    runtime: Handle,
    events_tx: UnboundedSender<BackendEvent>,
    events_rx: UnboundedReceiver<BackendEvent>,
    microsoft_task: Option<tokio::task::JoinHandle<()>>,
}

impl App {
    pub(crate) fn new(launcher: Launcher, runtime: Handle) -> Self {
        let (events_tx, events_rx) = unbounded_channel();
        let config = launcher.config();
        let accounts = launcher.accounts();
        let mut app = Self {
            launcher,
            installed: Vec::new(),
            accounts,
            config,
            settings_scope: SettingsScope::Global,
            instance_config: InstanceConfig::default(),
            manifest: None,
            java: None,
            page: Page::Dashboard,
            version_tab: VersionTab::Installed,
            installed_sel: 0,
            installed_state: ListState::default(),
            remote_sel: 0,
            remote_state: ListState::default(),
            remote_filter: String::new(),
            account_sel: 0,
            accounts_state: ListState::default(),
            setting_sel: 0,
            settings_state: ListState::default(),
            logs: Vec::new(),
            log_offset: 0,
            log_visual_rows: 0,
            log_view_rows: 1,
            status: Status {
                level: Level::Info,
                text: "Ready.".to_string(),
            },
            busy: None,
            running: None,
            modal: None,
            microsoft: None,
            help: false,
            quit: false,
            view_rows: 10,
            tab_hits: Vec::new(),
            version_tab_hits: Vec::new(),
            row_hits: Vec::new(),
            log_area: None,
            runtime,
            events_tx,
            events_rx,
            microsoft_task: None,
        };
        app.refresh_local();
        app.detect_java();
        app.set_status(
            Level::Info,
            format!("Root: {}", app.launcher.paths().root().display()),
        );
        app
    }

    // ------------------------------------------------------------------
    // Local state
    // ------------------------------------------------------------------

    pub(crate) fn refresh_local(&mut self) {
        match self.launcher.installed_versions() {
            Ok(installed) => {
                self.installed = installed;
            }
            Err(error) => {
                self.installed.clear();
                self.set_status(
                    Level::Error,
                    format!("Could not read installed versions: {error}"),
                );
            }
        }
        self.accounts = self.launcher.accounts();
        self.config = self.launcher.config();
        self.reload_instance_config();
        self.clamp_selections();
    }

    fn reload_instance_config(&mut self) {
        let Some(version) = self.settings_scope.version().map(str::to_string) else {
            self.instance_config = InstanceConfig::default();
            return;
        };
        match self.launcher.instance_config(&version) {
            Ok(config) => self.instance_config = config,
            Err(error) => {
                self.instance_config = InstanceConfig::default();
                self.set_status(
                    Level::Error,
                    format!("Could not read settings for version {version}: {error}"),
                );
            }
        }
    }

    /// Settings actually used for the current target.
    fn settings_config(&self) -> LauncherConfig {
        if self.settings_scope.is_version() {
            self.instance_config.apply_to(&self.config)
        } else {
            self.config.clone()
        }
    }

    fn clamp_selections(&mut self) {
        self.installed_sel = self
            .installed_sel
            .min(self.installed.len().saturating_sub(1));
        let remote_len = self.remote_indices().len();
        self.remote_sel = self.remote_sel.min(remote_len.saturating_sub(1));
        self.account_sel = self
            .account_sel
            .min(self.accounts.accounts.len().saturating_sub(1));
        self.setting_sel = self.setting_sel.min(Setting::ALL.len() - 1);
    }

    fn set_status(&mut self, level: Level, text: impl Into<String>) {
        self.status = Status {
            level,
            text: text.into(),
        };
    }

    pub(crate) fn push_log(&mut self, text: impl AsRef<str>) {
        let text = text.as_ref().replace('\t', "    ");
        for line in text.split('\n') {
            let cleaned: String = line
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            self.logs.push(cleaned);
        }
        if self.logs.len() > LOG_LIMIT {
            let excess = self.logs.len() - LOG_LIMIT;
            self.logs.drain(0..excess);
        }
    }

    fn max_log_offset(&self) -> usize {
        self.log_visual_rows.saturating_sub(self.log_view_rows)
    }

    pub(crate) fn remote_indices(&self) -> Vec<usize> {
        let Some(manifest) = &self.manifest else {
            return Vec::new();
        };
        let filter = self.remote_filter.trim().to_lowercase();
        manifest
            .versions
            .iter()
            .enumerate()
            .filter(|(_, version)| {
                filter.is_empty()
                    || version.id.to_lowercase().contains(&filter)
                    || version.kind.to_lowercase().contains(&filter)
            })
            .map(|(index, _)| index)
            .collect()
    }

    pub(crate) fn remote_selected_id(&self) -> Option<String> {
        let indices = self.remote_indices();
        let index = *indices.get(self.remote_sel)?;
        self.manifest
            .as_ref()
            .and_then(|manifest| manifest.versions.get(index))
            .map(|version| version.id.clone())
    }

    pub(crate) fn setting_value(&self, setting: Setting) -> String {
        let config = self.settings_config();
        match setting {
            Setting::Scope => self.settings_scope.label(),
            Setting::Java => config
                .java_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "(JAVA_HOME / PATH)".to_string()),
            Setting::Memory => format!("{} MiB", config.memory_mb),
            Setting::Concurrency => config.download_concurrency.to_string(),
            Setting::CheckFiles => {
                if config.check_files {
                    "enabled".to_string()
                } else {
                    "disabled".to_string()
                }
            }
            Setting::DefaultVersion => self
                .config
                .default_version
                .clone()
                .unwrap_or_else(|| "(none)".to_string()),
            Setting::GameDir => config
                .game_dir
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| {
                    if self.settings_scope.is_version() {
                        "(version directory / inherited)".to_string()
                    } else {
                        "(per-version directory)".to_string()
                    }
                }),
        }
    }

    pub(crate) fn current_account_name(&self) -> Option<&str> {
        self.accounts
            .current()
            .map(|account| account.username.as_str())
    }

    // ------------------------------------------------------------------
    // Backend events
    // ------------------------------------------------------------------

    pub(crate) fn poll_backend(&mut self) {
        while let Ok(event) = self.events_rx.try_recv() {
            self.handle_backend(event);
        }
    }

    fn handle_backend(&mut self, event: BackendEvent) {
        match event {
            BackendEvent::Step { phase, message } => {
                self.push_log(format!("==> [{phase}] {message}"));
                if let Some(busy) = &mut self.busy {
                    busy.phase = phase;
                    busy.file = message;
                    busy.completed = 0;
                    busy.total = 0;
                }
            }
            BackendEvent::Batch {
                phase,
                completed,
                total,
                file,
            } => {
                if let Some(busy) = &mut self.busy {
                    busy.phase = phase;
                    busy.completed = completed;
                    busy.total = total;
                    busy.file = file;
                }
            }
            BackendEvent::Log { stream, line } => match stream {
                LogStream::Stdout => self.push_log(line),
                LogStream::Stderr => self.push_log(format!("[stderr] {line}")),
            },
            BackendEvent::Installed(installed) => {
                self.installed = installed;
                self.clamp_selections();
            }
            BackendEvent::Manifest(manifest) => {
                let count = manifest.versions.len();
                let latest = manifest.latest.release.clone();
                self.manifest = Some(manifest);
                self.remote_sel = 0;
                self.busy = None;
                self.set_status(
                    Level::Success,
                    format!("Loaded {count} remote versions (latest release {latest})."),
                );
                self.push_log(format!(
                    "==> [versions] loaded {count} remote versions, latest release {latest}"
                ));
            }
            BackendEvent::JavaResult { result } => {
                match &result {
                    Ok(display) => self.set_status(Level::Info, format!("Java: {display}")),
                    Err(error) => self.set_status(Level::Error, format!("Java: {error}")),
                }
                self.java = Some(result);
            }
            BackendEvent::Running { version } => {
                self.busy = None;
                self.running = Some(version.clone());
                self.set_status(
                    Level::Success,
                    format!("Minecraft {version} is running; output is shown in Logs."),
                );
                self.push_log(format!("==> [launch] Minecraft {version} is running"));
                self.page = Page::Logs;
            }
            BackendEvent::MicrosoftCode(code) => {
                self.push_log(format!(
                    "==> [auth] open {} and enter code {}",
                    code.verification_uri, code.user_code
                ));
                self.set_status(
                    Level::Info,
                    format!(
                        "Microsoft sign-in: enter {} at {}",
                        code.user_code, code.verification_uri
                    ),
                );
                self.microsoft = Some(code);
            }
            BackendEvent::TaskDone { task, ok, message } => {
                self.busy = None;
                if !message.is_empty() {
                    self.push_log(if ok {
                        format!("==> {message}")
                    } else {
                        format!("!!! {message}")
                    });
                    self.set_status(if ok { Level::Success } else { Level::Error }, message);
                }
                match task {
                    TaskKind::Install => {
                        self.refresh_local();
                        if ok {
                            self.version_tab = VersionTab::Installed;
                        }
                    }
                    TaskKind::Launch => {
                        self.running = None;
                        self.refresh_local();
                    }
                    TaskKind::Manifest => {
                        if !ok {
                            self.busy = None;
                        }
                    }
                    TaskKind::Login => {
                        self.microsoft = None;
                        self.microsoft_task = None;
                        self.refresh_local();
                        if ok {
                            let selected = self.accounts.selected.clone();
                            if let Some(index) = selected.as_deref().and_then(|uuid| {
                                self.accounts
                                    .accounts
                                    .iter()
                                    .position(|account| account.uuid == uuid)
                            }) {
                                self.account_sel = index;
                            }
                            self.page = Page::Accounts;
                        }
                    }
                }
            }
        }
    }

    fn progress_sink(tx: UnboundedSender<BackendEvent>) -> ProgressSink {
        Arc::new(move |event| {
            let event = match event {
                ProgressEvent::Step { phase, message } => BackendEvent::Step {
                    phase: phase.to_string(),
                    message,
                },
                ProgressEvent::Batch {
                    phase,
                    completed,
                    total,
                    file,
                } => BackendEvent::Batch {
                    phase: phase.to_string(),
                    completed,
                    total,
                    file,
                },
                ProgressEvent::Log { stream, line } => BackendEvent::Log { stream, line },
            };
            let _ = tx.send(event);
        })
    }

    // ------------------------------------------------------------------
    // Background tasks
    // ------------------------------------------------------------------

    fn detect_java(&mut self) {
        let configured = self.config.java_path.clone();
        let tx = self.events_tx.clone();
        self.runtime.spawn_blocking(move || {
            let result = detect_java(configured.as_deref(), None)
                .map(|java| java.display())
                .map_err(|error| error.to_string());
            let _ = tx.send(BackendEvent::JavaResult { result });
        });
    }

    fn load_manifest(&mut self) {
        if !self.ensure_idle() {
            return;
        }
        self.busy = Some(Busy::new("Loading remote version manifest"));
        self.push_log("==> [versions] fetching version_manifest_v2.json");
        let launcher = self.launcher.clone();
        let tx = self.events_tx.clone();
        self.runtime.spawn(async move {
            match launcher.fetch_manifest().await {
                Ok(manifest) => {
                    let _ = tx.send(BackendEvent::Manifest(manifest));
                    let _ = tx.send(BackendEvent::TaskDone {
                        task: TaskKind::Manifest,
                        ok: true,
                        message: String::new(),
                    });
                }
                Err(error) => {
                    let _ = tx.send(BackendEvent::TaskDone {
                        task: TaskKind::Manifest,
                        ok: false,
                        message: format!("Could not load the version manifest: {error}"),
                    });
                }
            }
        });
    }

    fn start_install(&mut self, version: String) {
        if version.trim().is_empty() {
            self.set_status(Level::Error, "Version id cannot be empty.");
            return;
        }
        if self.launcher.is_installed(&version) {
            self.refresh_local();
            self.version_tab = VersionTab::Installed;
            if let Some(index) = self.installed.iter().position(|item| item.id == version) {
                self.installed_sel = index;
            }
            self.set_status(
                Level::Info,
                format!("Minecraft {version} is already installed."),
            );
            return;
        }
        if !self.ensure_idle() {
            return;
        }

        self.busy = Some(Busy::new(format!("Installing Minecraft {version}")));
        self.run_install(version);
    }

    fn repair_version(&mut self, version: String) {
        if !self.ensure_idle() {
            return;
        }
        self.set_status(Level::Info, format!("Verifying Minecraft {version}..."));
        self.busy = Some(Busy::new(format!("Verifying Minecraft {version}")));
        self.run_install(version);
    }

    fn run_install(&mut self, version: String) {
        self.push_log(format!("==> [install] installing Minecraft {version}"));
        let launcher = self.launcher.clone();
        let tx = self.events_tx.clone();
        self.runtime.spawn(async move {
            let sink = App::progress_sink(tx.clone());
            let result = launcher.install_version(&version, &sink).await;
            if let Ok(list) = launcher.installed_versions() {
                let _ = tx.send(BackendEvent::Installed(list));
            }
            let (ok, message) = match result {
                Ok(report) => (
                    true,
                    format!(
                        "Installed {}: {} libraries, {} native archives, {} assets.",
                        report.version_id, report.libraries, report.natives, report.assets
                    ),
                ),
                Err(error) => (false, format!("Install failed for {version}: {error}")),
            };
            let _ = tx.send(BackendEvent::TaskDone {
                task: TaskKind::Install,
                ok,
                message,
            });
        });
    }

    fn start_microsoft_login(&mut self) {
        if !self.ensure_idle() {
            return;
        }
        if self.launcher.microsoft_client_id().is_none() {
            self.push_log(format!("!!! {}", shccl_core::microsoft::CLIENT_ID_HELP));
            self.set_status(
                Level::Error,
                "Microsoft login needs an Azure client ID; details are in Logs.",
            );
            self.page = Page::Logs;
            return;
        }

        self.busy = Some(Busy::new("Signing in with Microsoft"));
        self.push_log("==> [auth] requesting a Microsoft device code");
        self.set_status(Level::Info, "Starting Microsoft sign-in...");

        let launcher = self.launcher.clone();
        let tx = self.events_tx.clone();
        let handle = self.runtime.spawn(async move {
            let code = match launcher.microsoft_device_code().await {
                Ok(code) => code,
                Err(error) => {
                    let _ = tx.send(BackendEvent::TaskDone {
                        task: TaskKind::Login,
                        ok: false,
                        message: format!("Microsoft login failed: {error}"),
                    });
                    return;
                }
            };
            let _ = tx.send(BackendEvent::MicrosoftCode(code.clone()));
            let sink = App::progress_sink(tx.clone());
            let (ok, message) = match launcher.microsoft_login(&code, &sink).await {
                Ok(account) => (
                    true,
                    format!(
                        "Microsoft account '{}' signed in and selected.",
                        account.username
                    ),
                ),
                Err(error) => (false, format!("Microsoft login failed: {error}")),
            };
            let _ = tx.send(BackendEvent::TaskDone {
                task: TaskKind::Login,
                ok,
                message,
            });
        });
        self.microsoft_task = Some(handle);
    }

    fn cancel_microsoft_login(&mut self) {
        if let Some(handle) = self.microsoft_task.take() {
            handle.abort();
        }
        if self.microsoft.take().is_some() {
            self.busy = None;
            self.set_status(Level::Info, "Microsoft sign-in cancelled.");
        }
    }

    fn start_launch(&mut self, version: String) {
        if let Some(running) = &self.running {
            self.set_status(
                Level::Error,
                format!("Minecraft {running} is already running."),
            );
            return;
        }
        if !self.ensure_idle() {
            return;
        }
        let Some(account) = self.accounts.current().cloned() else {
            self.page = Page::Accounts;
            self.set_status(
                Level::Error,
                "No account selected. Add an offline account on the Accounts page.",
            );
            return;
        };

        self.busy = Some(Busy::new(format!("Preparing Minecraft {version}")));
        self.push_log(format!(
            "==> [launch] launching Minecraft {version} as {}",
            account.username
        ));
        let launcher = self.launcher.clone();
        let tx = self.events_tx.clone();
        self.runtime.spawn(async move {
            let sink = App::progress_sink(tx.clone());
            if !launcher.is_installed(&version) {
                let _ = tx.send(BackendEvent::Step {
                    phase: "version".to_string(),
                    message: format!("Minecraft {version} is not installed; installing it first"),
                });
                if let Err(error) = launcher.install_version(&version, &sink).await {
                    let _ = tx.send(BackendEvent::TaskDone {
                        task: TaskKind::Launch,
                        ok: false,
                        message: format!("Could not install {version}: {error}"),
                    });
                    return;
                }
                if let Ok(list) = launcher.installed_versions() {
                    let _ = tx.send(BackendEvent::Installed(list));
                }
            }

            let mut request = LaunchRequest::new(version.clone(), account);
            request.capture_output = true;
            match launcher.launch(request, &sink).await {
                Ok(mut handle) => {
                    let _ = tx.send(BackendEvent::Running {
                        version: version.clone(),
                    });
                    let (ok, message) = match handle.wait().await {
                        Ok(status) if status.success() => {
                            (true, format!("Minecraft {version} exited successfully."))
                        }
                        Ok(status) => (false, format!("Minecraft {version} exited with {status}.")),
                        Err(error) => (
                            false,
                            format!("Could not wait for Minecraft {version}: {error}"),
                        ),
                    };
                    let _ = tx.send(BackendEvent::TaskDone {
                        task: TaskKind::Launch,
                        ok,
                        message,
                    });
                }
                Err(error) => {
                    let _ = tx.send(BackendEvent::TaskDone {
                        task: TaskKind::Launch,
                        ok: false,
                        message: format!("Launch failed for {version}: {error}"),
                    });
                }
            }
        });
    }

    fn preview_launch(&mut self, version: String) {
        let Some(account) = self.accounts.current().cloned() else {
            self.page = Page::Accounts;
            self.set_status(
                Level::Error,
                "No account selected. Add an offline account on the Accounts page.",
            );
            return;
        };
        let request = LaunchRequest::new(version.clone(), account);
        match self.launcher.prepare_launch(&request) {
            Ok(plan) => {
                self.page = Page::Logs;
                self.log_offset = 0;
                self.push_log(format!("==> [launch] dry run for {version}"));
                self.push_log(format!("Working directory: {}", plan.working_dir.display()));
                self.push_log(plan.command_line());
                self.set_status(
                    Level::Info,
                    format!("Launch command for {version} written to Logs."),
                );
            }
            Err(error) => self.set_status(
                Level::Error,
                format!("Could not prepare the launch command: {error}"),
            ),
        }
    }

    fn ensure_idle(&mut self) -> bool {
        if let Some(version) = &self.running {
            self.set_status(
                Level::Info,
                format!("Minecraft {version} is running; exit the game first."),
            );
            return false;
        }
        if let Some(busy) = &self.busy {
            self.set_status(
                Level::Info,
                format!("Please wait: {} is still running.", busy.label),
            );
            false
        } else {
            true
        }
    }

    // ------------------------------------------------------------------
    // Input handling
    // ------------------------------------------------------------------

    pub(crate) fn handle_mouse(&mut self, kind: MouseEventKind, column: u16, row: u16) {
        if self.help {
            if matches!(kind, MouseEventKind::Down(_)) {
                self.help = false;
            }
            return;
        }
        if self.modal.is_some() {
            return;
        }
        if self.microsoft.is_some() {
            return;
        }
        match kind {
            MouseEventKind::ScrollUp => self.move_selection(-3),
            MouseEventKind::ScrollDown => self.move_selection(3),
            MouseEventKind::Down(MouseButton::Left) => self.handle_click(column, row),
            _ => {}
        }
    }

    fn handle_click(&mut self, x: u16, y: u16) {
        if let Some((_, page)) = self
            .tab_hits
            .iter()
            .find(|(rect, _)| contains(*rect, x, y))
            .copied()
        {
            self.page = page;
            return;
        }
        if let Some((_, tab)) = self
            .version_tab_hits
            .iter()
            .find(|(rect, _)| contains(*rect, x, y))
            .copied()
        {
            self.version_tab = tab;
            return;
        }
        if let Some((_, target, index)) = self
            .row_hits
            .iter()
            .find(|(rect, _, _)| contains(*rect, x, y))
            .copied()
        {
            match target {
                RowTarget::Installed => self.installed_sel = index,
                RowTarget::Remote => self.remote_sel = index,
                RowTarget::Account => self.account_sel = index,
                RowTarget::Setting => self.setting_sel = index,
            }
        }
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) {
        if self.help {
            if matches!(
                key.code,
                KeyCode::Esc
                    | KeyCode::Char('q')
                    | KeyCode::Char('Q')
                    | KeyCode::Char('?')
                    | KeyCode::F(1)
            ) {
                self.help = false;
            }
            return;
        }
        if self.modal.is_some() {
            self.handle_modal_key(key);
            return;
        }
        if self.microsoft.is_some() {
            if key.code == KeyCode::Esc {
                self.cancel_microsoft_login();
            }
            return;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc => self.quit = true,
            KeyCode::Char('c') | KeyCode::Char('C') if ctrl => self.quit = true,
            KeyCode::Char('q') | KeyCode::Char('Q') => self.quit = true,
            KeyCode::Char('?') | KeyCode::F(1) => self.help = true,
            KeyCode::Char('1') => self.page = Page::Dashboard,
            KeyCode::Char('2') => self.page = Page::Versions,
            KeyCode::Char('3') => self.page = Page::Accounts,
            KeyCode::Char('4') => self.page = Page::Settings,
            KeyCode::Char('5') => self.page = Page::Logs,
            KeyCode::Tab => {
                self.page = if shift {
                    self.page.previous()
                } else {
                    self.page.next()
                }
            }
            KeyCode::Char('r') | KeyCode::Char('R') if !ctrl => {
                self.refresh_local();
                self.detect_java();
                self.set_status(Level::Info, "Refreshed local state and Java detection.");
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-(self.view_rows.max(1) as isize)),
            KeyCode::PageDown => self.move_selection(self.view_rows.max(1) as isize),
            KeyCode::Home => self.jump_selection(false),
            KeyCode::End => self.jump_selection(true),
            KeyCode::Enter => self.primary_action(),
            _ => match self.page {
                Page::Versions => self.handle_versions_key(key.code),
                Page::Accounts => self.handle_accounts_key(key.code),
                Page::Settings => self.handle_settings_key(key.code),
                Page::Logs => self.handle_logs_key(key.code),
                Page::Dashboard => {}
            },
        }
    }

    fn handle_modal_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.modal = None,
            KeyCode::Enter => self.submit_modal(),
            KeyCode::Char('a') | KeyCode::Char('A') if ctrl => {
                if let Some(modal) = &mut self.modal {
                    modal.select_all();
                }
            }
            KeyCode::Backspace => {
                if let Some(modal) = &mut self.modal {
                    modal.backspace();
                }
            }
            KeyCode::Delete => {
                if let Some(modal) = &mut self.modal {
                    modal.delete();
                }
            }
            KeyCode::Left => {
                if let Some(modal) = &mut self.modal {
                    modal.move_cursor(-1);
                }
            }
            KeyCode::Right => {
                if let Some(modal) = &mut self.modal {
                    modal.move_cursor(1);
                }
            }
            KeyCode::Home => {
                if let Some(modal) = &mut self.modal {
                    modal.cursor_home();
                }
            }
            KeyCode::End => {
                if let Some(modal) = &mut self.modal {
                    modal.cursor_end();
                }
            }
            KeyCode::Char(ch) if !ctrl => {
                if let Some(modal) = &mut self.modal {
                    modal.insert_str(&ch.to_string());
                }
            }
            _ => {}
        }
    }

    fn handle_versions_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Left | KeyCode::Char('h') => self.version_tab = VersionTab::Installed,
            KeyCode::Right => self.version_tab = VersionTab::Remote,
            KeyCode::Char('i') | KeyCode::Char('I') => self.open_install_modal(),
            KeyCode::Char('l') | KeyCode::Char('L') => self.load_manifest(),
            KeyCode::Char('f') | KeyCode::Char('F') => self.open_filter_modal(),
            KeyCode::Char('d') | KeyCode::Char('D') => self.set_default_selected(),
            KeyCode::Char('p') | KeyCode::Char('P') => {
                if let Some(version) = self.installed.get(self.installed_sel) {
                    self.preview_launch(version.id.clone());
                }
            }
            KeyCode::Char('v') | KeyCode::Char('V') => {
                if let Some(version) = self.installed.get(self.installed_sel) {
                    self.repair_version(version.id.clone());
                }
            }
            _ => {}
        }
    }

    fn handle_accounts_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('a') | KeyCode::Char('A') => self.open_login_modal(),
            KeyCode::Char('m') | KeyCode::Char('M') => self.start_microsoft_login(),
            _ => {}
        }
    }

    fn handle_settings_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('t') | KeyCode::Char('T') => self.open_setting_modal(Setting::Scope),
            KeyCode::Char('c') | KeyCode::Char('C') => {
                let setting = Setting::ALL[self.setting_sel.min(Setting::ALL.len() - 1)];
                self.clear_setting(setting);
            }
            KeyCode::Char('s') | KeyCode::Char('S') => self.show_config(),
            _ => {}
        }
    }

    fn handle_logs_key(&mut self, code: KeyCode) {
        if matches!(code, KeyCode::Char('c') | KeyCode::Char('C')) {
            self.logs.clear();
            self.log_offset = 0;
            self.log_visual_rows = 0;
            self.set_status(Level::Info, "Logs cleared.");
        }
    }

    fn move_selection(&mut self, delta: isize) {
        match self.page {
            Page::Versions => match self.version_tab {
                VersionTab::Installed => {
                    if self.installed.is_empty() {
                        return;
                    }
                    self.installed_sel = step(self.installed_sel, delta, self.installed.len());
                }
                VersionTab::Remote => {
                    let len = self.remote_indices().len();
                    if len == 0 {
                        return;
                    }
                    self.remote_sel = step(self.remote_sel, delta, len);
                }
            },
            Page::Accounts => {
                if self.accounts.accounts.is_empty() {
                    return;
                }
                self.account_sel = step(self.account_sel, delta, self.accounts.accounts.len());
            }
            Page::Settings => {
                self.setting_sel = step(self.setting_sel, delta, Setting::ALL.len());
            }
            Page::Logs => {
                let amount = delta.unsigned_abs().max(1);
                if delta < 0 {
                    self.log_offset = (self.log_offset + amount).min(self.max_log_offset());
                } else {
                    self.log_offset = self.log_offset.saturating_sub(amount);
                }
            }
            Page::Dashboard => {}
        }
    }

    fn jump_selection(&mut self, end: bool) {
        match self.page {
            Page::Versions => match self.version_tab {
                VersionTab::Installed => {
                    self.installed_sel = if end {
                        self.installed.len().saturating_sub(1)
                    } else {
                        0
                    };
                }
                VersionTab::Remote => {
                    let len = self.remote_indices().len();
                    self.remote_sel = if end { len.saturating_sub(1) } else { 0 };
                }
            },
            Page::Accounts => {
                self.account_sel = if end {
                    self.accounts.accounts.len().saturating_sub(1)
                } else {
                    0
                };
            }
            Page::Settings => {
                self.setting_sel = if end { Setting::ALL.len() - 1 } else { 0 };
            }
            Page::Logs => {
                self.log_offset = if end { 0 } else { self.max_log_offset() };
            }
            Page::Dashboard => {}
        }
    }

    fn primary_action(&mut self) {
        match self.page {
            Page::Versions => match self.version_tab {
                VersionTab::Installed => {
                    if let Some(version) = self.installed.get(self.installed_sel) {
                        self.start_launch(version.id.clone());
                    }
                }
                VersionTab::Remote => {
                    if let Some(id) = self.remote_selected_id() {
                        self.start_install(id);
                    }
                }
            },
            Page::Accounts => {
                if let Some(account) = self.accounts.accounts.get(self.account_sel) {
                    self.select_account(&account.uuid.clone());
                }
            }
            Page::Settings => {
                self.open_setting_modal(Setting::ALL[self.setting_sel.min(Setting::ALL.len() - 1)]);
            }
            Page::Dashboard => {
                self.refresh_local();
                self.detect_java();
                self.set_status(Level::Info, "Refreshed local state and Java detection.");
            }
            Page::Logs => {}
        }
    }

    fn set_default_selected(&mut self) {
        let version = match self.version_tab {
            VersionTab::Installed => self
                .installed
                .get(self.installed_sel)
                .map(|version| version.id.clone()),
            VersionTab::Remote => self.remote_selected_id(),
        };
        if let Some(version) = version {
            self.set_default_version(version);
        }
    }

    fn set_default_version(&mut self, version: String) {
        let saved = version.clone();
        if self.update_config(|config| config.default_version = Some(saved.clone())) {
            self.set_status(Level::Success, format!("Default version set to {saved}."));
        }
    }

    fn select_account(&mut self, query: &str) {
        match self.launcher.select_account(query) {
            Ok(account) => {
                self.accounts = self.launcher.accounts();
                self.set_status(
                    Level::Success,
                    format!("Selected account '{}'.", account.username),
                );
            }
            Err(error) => self.set_status(Level::Error, error.to_string()),
        }
    }

    fn update_config<F>(&mut self, update: F) -> bool
    where
        F: FnOnce(&mut LauncherConfig),
    {
        match self.launcher.update_config(update) {
            Ok(config) => {
                self.config = config;
                true
            }
            Err(error) => {
                self.set_status(
                    Level::Error,
                    format!("Could not save configuration: {error}"),
                );
                false
            }
        }
    }

    fn update_instance_config<F>(&mut self, update: F) -> bool
    where
        F: FnOnce(&mut InstanceConfig),
    {
        let Some(version) = self.settings_scope.version().map(str::to_string) else {
            self.set_status(Level::Error, "Select a version target first.");
            return false;
        };
        match self.launcher.update_instance_config(&version, update) {
            Ok(config) => {
                self.instance_config = config;
                true
            }
            Err(error) => {
                self.set_status(
                    Level::Error,
                    format!("Could not save settings for version {version}: {error}"),
                );
                false
            }
        }
    }

    fn setting_modal_title(&self, title: &str) -> String {
        match &self.settings_scope {
            SettingsScope::Global => title.to_string(),
            SettingsScope::Version(version) => format!("{title} · {version}"),
        }
    }

    fn setting_suffix(&self) -> String {
        match &self.settings_scope {
            SettingsScope::Global => String::new(),
            SettingsScope::Version(version) => format!(" for version {version}"),
        }
    }

    fn open_login_modal(&mut self) {
        self.modal = Some(InputModal::new(
            "Add offline account",
            "Player name (letters, digits, underscore; max 16):",
            String::new(),
            InputAction::LoginOffline,
        ));
    }

    fn open_install_modal(&mut self) {
        let value = self.remote_selected_id().or_else(|| {
            self.config
                .default_version
                .clone()
                .or_else(|| self.installed.first().map(|version| version.id.clone()))
        });
        self.modal = Some(InputModal::new(
            "Install version",
            "Version id (for example 1.21.4):",
            value.unwrap_or_default(),
            InputAction::InstallVersion,
        ));
    }

    fn open_filter_modal(&mut self) {
        self.modal = Some(InputModal::new(
            "Filter remote versions",
            "Match version id or type (leave empty to show all):",
            self.remote_filter.clone(),
            InputAction::RemoteFilter,
        ));
    }

    fn open_setting_modal(&mut self, setting: Setting) {
        let config = self.settings_config();
        let version_scope = self.settings_scope.is_version();
        let (title, prompt, value, action) = match setting {
            Setting::Scope => (
                "Settings target".to_string(),
                "Global defaults or version id (empty for global):".to_string(),
                self.settings_scope
                    .version()
                    .unwrap_or_default()
                    .to_string(),
                InputAction::SetSettingsTarget,
            ),
            Setting::Java => (
                self.setting_modal_title("Java path"),
                "Java executable or JAVA_HOME:".to_string(),
                config
                    .java_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
                InputAction::SetJava,
            ),
            Setting::Memory => (
                self.setting_modal_title("Memory"),
                "Maximum Java heap in MiB (512-65536):".to_string(),
                config.memory_mb.to_string(),
                InputAction::SetMemory,
            ),
            Setting::Concurrency => (
                self.setting_modal_title("Download concurrency"),
                "Parallel downloads (1-64):".to_string(),
                config.download_concurrency.to_string(),
                InputAction::SetConcurrency,
            ),
            Setting::CheckFiles => (
                self.setting_modal_title("File checks"),
                "Check library files before launch (true/false):".to_string(),
                config.check_files.to_string(),
                InputAction::SetCheckFiles,
            ),
            Setting::DefaultVersion => (
                "Default version (global)".to_string(),
                "Version id (launcher-wide, e.g. 1.21.4):".to_string(),
                self.config.default_version.clone().unwrap_or_default(),
                InputAction::SetDefaultVersion,
            ),
            Setting::GameDir => (
                self.setting_modal_title("Game directory"),
                if version_scope {
                    "Game directory (empty inherits; relative to root):"
                } else {
                    "Game directory (relative paths use root):"
                }
                .to_string(),
                config
                    .game_dir
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
                InputAction::SetGameDir,
            ),
        };
        self.modal = Some(InputModal::new(title, prompt, value, action));
    }

    fn submit_modal(&mut self) {
        let Some(modal) = self.modal.take() else {
            return;
        };
        let action = modal.action;
        let value = modal.value.trim().to_string();

        match action {
            InputAction::LoginOffline => match Account::offline(&value) {
                Ok(account) => {
                    let username = account.username.clone();
                    match self.launcher.upsert_account(account) {
                        Ok(()) => {
                            self.refresh_local();
                            if let Some(index) = self
                                .accounts
                                .accounts
                                .iter()
                                .position(|item| item.username == username)
                            {
                                self.account_sel = index;
                            }
                            self.set_status(
                                Level::Success,
                                format!("Offline account '{username}' added and selected."),
                            );
                        }
                        Err(error) => {
                            self.set_status(
                                Level::Error,
                                format!("Could not save account: {error}"),
                            );
                            self.modal = Some(modal);
                        }
                    }
                }
                Err(error) => {
                    self.set_status(Level::Error, error.to_string());
                    self.modal = Some(modal);
                }
            },
            InputAction::InstallVersion => {
                if value.is_empty() {
                    self.set_status(Level::Error, "Version id cannot be empty.");
                    self.modal = Some(modal);
                } else {
                    self.start_install(value);
                }
            }
            InputAction::RemoteFilter => {
                self.remote_filter = value;
                self.remote_sel = 0;
                self.version_tab = VersionTab::Remote;
                let message = if self.remote_filter.is_empty() {
                    "Remote filter cleared.".to_string()
                } else {
                    format!("Filtering remote versions by '{}'.", self.remote_filter)
                };
                self.set_status(Level::Info, message);
            }
            InputAction::SetSettingsTarget => {
                if value.is_empty() {
                    self.settings_scope = SettingsScope::Global;
                    self.instance_config = InstanceConfig::default();
                    self.set_status(Level::Info, "Editing launcher-wide defaults.");
                } else if !self.launcher.is_installed(&value) {
                    self.set_status(
                        Level::Error,
                        format!("Version '{value}' is not installed. Install it first."),
                    );
                    self.modal = Some(modal);
                } else {
                    match self.launcher.instance_config(&value) {
                        Ok(config) => {
                            self.settings_scope = SettingsScope::Version(value.clone());
                            self.instance_config = config;
                            self.set_status(
                                Level::Info,
                                format!("Editing settings for version {value}."),
                            );
                        }
                        Err(error) => {
                            self.set_status(Level::Error, error.to_string());
                            self.modal = Some(modal);
                        }
                    }
                }
            }
            InputAction::SetJava => {
                let path = PathBuf::from(&value);
                if value.is_empty() || !path.exists() {
                    self.set_status(
                        Level::Error,
                        format!("Java path does not exist: {}", path.display()),
                    );
                    self.modal = Some(modal);
                    return;
                }
                let path = path.canonicalize().unwrap_or(path);
                let display = path.display().to_string();
                let saved = path.clone();
                let saved_ok = if self.settings_scope.is_version() {
                    self.update_instance_config(|config| config.java_path = Some(saved))
                } else {
                    self.update_config(|config| config.java_path = Some(saved))
                };
                if saved_ok {
                    self.set_status(
                        Level::Success,
                        format!("Java path{} set to {display}.", self.setting_suffix()),
                    );
                    if !self.settings_scope.is_version() {
                        self.detect_java();
                    }
                }
            }
            InputAction::SetMemory => match value.parse::<u32>() {
                Ok(memory) if (512..=65_536).contains(&memory) => {
                    let saved = if self.settings_scope.is_version() {
                        self.update_instance_config(|config| config.memory_mb = Some(memory))
                    } else {
                        self.update_config(|config| config.memory_mb = memory)
                    };
                    if saved {
                        self.set_status(
                            Level::Success,
                            format!("Memory{} set to {memory} MiB.", self.setting_suffix()),
                        );
                    }
                }
                _ => {
                    self.set_status(Level::Error, "Memory must be between 512 and 65536 MiB.");
                    self.modal = Some(modal);
                }
            },
            InputAction::SetConcurrency => match value.parse::<usize>() {
                Ok(concurrency) if (1..=64).contains(&concurrency) => {
                    let saved = if self.settings_scope.is_version() {
                        self.update_instance_config(|config| {
                            config.download_concurrency = Some(concurrency)
                        })
                    } else {
                        self.update_config(|config| config.download_concurrency = concurrency)
                    };
                    if saved {
                        self.set_status(
                            Level::Success,
                            format!(
                                "Download concurrency{} set to {concurrency}.",
                                self.setting_suffix()
                            ),
                        );
                    }
                }
                _ => {
                    self.set_status(
                        Level::Error,
                        "Download concurrency must be between 1 and 64.",
                    );
                    self.modal = Some(modal);
                }
            },
            InputAction::SetCheckFiles => match parse_boolish(&value) {
                Some(enabled) => {
                    let saved = if self.settings_scope.is_version() {
                        self.update_instance_config(|config| config.check_files = Some(enabled))
                    } else {
                        self.update_config(|config| config.check_files = enabled)
                    };
                    if saved {
                        self.set_status(
                            Level::Success,
                            format!(
                                "File checks{} {}.",
                                self.setting_suffix(),
                                if enabled { "enabled" } else { "disabled" }
                            ),
                        );
                    }
                }
                None => {
                    self.set_status(Level::Error, "File checks must be true or false.");
                    self.modal = Some(modal);
                }
            },
            InputAction::SetDefaultVersion => {
                if value.is_empty() {
                    self.set_status(Level::Error, "Version id cannot be empty.");
                    self.modal = Some(modal);
                } else if self.update_config(|config| config.default_version = Some(value.clone()))
                {
                    self.set_status(
                        Level::Success,
                        format!("Default version set to {value} (launcher-wide)."),
                    );
                }
            }
            InputAction::SetGameDir => {
                if value.is_empty() {
                    if self.settings_scope.is_version() {
                        if self.update_instance_config(|config| config.game_dir = None) {
                            self.set_status(
                                Level::Success,
                                format!(
                                    "Game directory override{} cleared; the launcher-wide value or versions/<id> will be used.",
                                    self.setting_suffix()
                                ),
                            );
                        }
                    } else {
                        self.set_status(Level::Error, "Game directory cannot be empty.");
                        self.modal = Some(modal);
                    }
                } else {
                    let path = PathBuf::from(&value);
                    let display = path.display().to_string();
                    let saved_path = path.clone();
                    let saved = if self.settings_scope.is_version() {
                        self.update_instance_config(|config| config.game_dir = Some(saved_path))
                    } else {
                        self.update_config(|config| config.game_dir = Some(saved_path))
                    };
                    if saved {
                        self.set_status(
                            Level::Success,
                            format!("Game directory{} set to {display}.", self.setting_suffix()),
                        );
                    }
                }
            }
        }
    }

    fn clear_setting(&mut self, setting: Setting) {
        let version_scope = self.settings_scope.is_version();
        let suffix = self.setting_suffix();
        match setting {
            Setting::Scope => self.set_status(
                Level::Info,
                "Select Target and press Enter to switch between global defaults and a version.",
            ),
            Setting::Java => {
                let cleared = if version_scope {
                    self.update_instance_config(|config| config.java_path = None)
                } else {
                    self.update_config(|config| config.java_path = None)
                };
                if cleared {
                    let message = if version_scope {
                        format!(
                            "Java path{suffix} cleared; the launcher-wide value (or JAVA_HOME/PATH) will be used."
                        )
                    } else {
                        "Java path cleared; JAVA_HOME/PATH will be used.".to_string()
                    };
                    self.set_status(Level::Success, message);
                    if !version_scope {
                        self.detect_java();
                    }
                }
            }
            Setting::Memory => {
                if version_scope {
                    if self.update_instance_config(|config| config.memory_mb = None) {
                        self.set_status(
                            Level::Success,
                            format!(
                                "Memory{suffix} cleared; the launcher-wide value will be used."
                            ),
                        );
                    }
                } else {
                    self.set_status(
                        Level::Info,
                        "This setting cannot be cleared; edit it instead.",
                    );
                }
            }
            Setting::Concurrency => {
                if version_scope {
                    if self.update_instance_config(|config| config.download_concurrency = None) {
                        self.set_status(
                            Level::Success,
                            format!(
                                "Download concurrency{suffix} cleared; the launcher-wide value will be used."
                            ),
                        );
                    }
                } else {
                    self.set_status(
                        Level::Info,
                        "This setting cannot be cleared; edit it instead.",
                    );
                }
            }
            Setting::CheckFiles => {
                if version_scope {
                    if self.update_instance_config(|config| config.check_files = None) {
                        self.set_status(
                            Level::Success,
                            format!(
                                "File checks{suffix} cleared; the launcher-wide value will be used."
                            ),
                        );
                    }
                } else {
                    self.set_status(
                        Level::Info,
                        "This setting cannot be cleared; edit it instead.",
                    );
                }
            }
            Setting::DefaultVersion => {
                if version_scope {
                    self.set_status(
                        Level::Info,
                        "Default version is launcher-wide; switch Target to Global defaults to clear it.",
                    );
                } else if self.update_config(|config| config.default_version = None) {
                    self.set_status(Level::Success, "Default version cleared.");
                }
            }
            Setting::GameDir => {
                let cleared = if version_scope {
                    self.update_instance_config(|config| config.game_dir = None)
                } else {
                    self.update_config(|config| config.game_dir = None)
                };
                if cleared {
                    let message = if version_scope {
                        format!(
                            "Game directory{suffix} cleared; the launcher-wide value or versions/<id> will be used."
                        )
                    } else {
                        "Game directory cleared; each version will use its own directory unless it overrides game_dir."
                            .to_string()
                    };
                    self.set_status(Level::Success, message);
                }
            }
        }
    }

    fn show_config(&mut self) {
        match self.settings_scope.version().map(str::to_string) {
            None => match serde_json::to_string_pretty(&self.config) {
                Ok(json) => {
                    self.push_log("==> [config] shccl.json");
                    self.push_log(json);
                    self.page = Page::Logs;
                    self.log_offset = 0;
                    self.set_status(Level::Info, "Configuration written to Logs.");
                }
                Err(error) => self.set_status(
                    Level::Error,
                    format!("Could not serialize configuration: {error}"),
                ),
            },
            Some(version) => {
                let mut effective = self.settings_config();
                if let Ok(game_dir) = self.launcher.game_dir(&version) {
                    effective.game_dir = Some(game_dir);
                }
                let effective_json = serde_json::to_string_pretty(&effective);
                let overrides_json = serde_json::to_string_pretty(&self.instance_config);
                match (effective_json, overrides_json) {
                    (Ok(effective), Ok(overrides)) => {
                        self.push_log(format!(
                            "==> [config] effective settings for version {version}"
                        ));
                        self.push_log(effective);
                        self.push_log(format!("==> [config] versions/{version}/shccl.json"));
                        self.push_log(overrides);
                        self.page = Page::Logs;
                        self.log_offset = 0;
                        self.set_status(Level::Info, "Configuration written to Logs.");
                    }
                    (Err(error), _) | (_, Err(error)) => self.set_status(
                        Level::Error,
                        format!("Could not serialize configuration: {error}"),
                    ),
                }
            }
        }
    }
}

fn contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.width && y < rect.y + rect.height
}

fn parse_boolish(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn step(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    if delta < 0 {
        current.saturating_sub(delta.unsigned_abs())
    } else {
        (current + delta as usize).min(len - 1)
    }
}
