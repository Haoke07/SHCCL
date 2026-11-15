use anyhow::Result;
use sdl3::pixels::Color;
use sdl3::render::{Canvas, FRect};
use sdl3::ttf::{Font, TextEngine};
use sdl3::video::Window;
use shccl_core::DeviceCode;

use crate::app::{
    Action, App, Busy, Hit, InputModal, Level, Page, ScrollTarget, Setting, VersionTab,
};
use crate::font::UiFont;

pub(crate) const WINDOW_WIDTH: u32 = 1280;
pub(crate) const WINDOW_HEIGHT: u32 = 800;
pub(crate) const LOGICAL_WIDTH: f32 = 640.0;
pub(crate) const LOGICAL_HEIGHT: f32 = 400.0;

/// Number of list rows that fit on each page. Kept in sync with the layout
/// below so keyboard navigation and scrolling agree with the drawing code.
pub(crate) const VERSION_ROWS: usize = 14;
pub(crate) const ACCOUNT_ROWS: usize = 9;
pub(crate) const LOG_ROWS: usize = 19;

const LINE_HEIGHT: f32 = 14.0;

const BACKGROUND: Color = Color::RGB(12, 15, 20);
const PANEL: Color = Color::RGB(24, 30, 38);
const PANEL_HOVER: Color = Color::RGB(36, 45, 56);
const PANEL_LIGHT: Color = Color::RGB(30, 38, 48);
const ROW_SELECTED: Color = Color::RGB(36, 66, 54);
const BORDER: Color = Color::RGB(44, 54, 67);
const LIST_BACKGROUND: Color = Color::RGB(18, 22, 28);
const ACCENT: Color = Color::RGB(86, 214, 134);
const ACCENT_DIM: Color = Color::RGB(34, 78, 58);
const TEXT: Color = Color::RGB(226, 232, 240);
const MUTED: Color = Color::RGB(132, 145, 162);
const DANGER: Color = Color::RGB(248, 113, 113);
const WARNING: Color = Color::RGB(250, 204, 21);
const SHADOW: Color = Color::RGB(6, 8, 11);

const MAIN_X: f32 = 148.0;
const MAIN_Y: f32 = 52.0;
const MAIN_WIDTH: f32 = LOGICAL_WIDTH - 158.0;
const MAIN_HEIGHT: f32 = 304.0;

fn header_rect() -> FRect {
    FRect::new(10.0, 8.0, LOGICAL_WIDTH - 20.0, 38.0)
}

fn sidebar_rect() -> FRect {
    FRect::new(10.0, MAIN_Y, 130.0, MAIN_HEIGHT)
}

fn main_rect() -> FRect {
    FRect::new(MAIN_X, MAIN_Y, MAIN_WIDTH, MAIN_HEIGHT)
}

fn footer_rect() -> FRect {
    FRect::new(10.0, 362.0, LOGICAL_WIDTH - 20.0, 30.0)
}

struct Painter<'a> {
    canvas: &'a mut Canvas<Window>,
    text_engine: &'a TextEngine,
    font: &'a Font<'static>,
    scale: f32,
    char_width: f32,
    line_height: f32,
    hits: &'a mut Vec<Hit>,
    scroll_hits: &'a mut Vec<(FRect, ScrollTarget)>,
    mouse: (f32, f32),
    interactive: bool,
}

impl Painter<'_> {
    /// Converts a rect from the 640x400 layout space into device pixels,
    /// snapping the edges so neighbouring rectangles do not leave seams.
    fn device(&self, rect: FRect) -> FRect {
        let left = (rect.x * self.scale).round();
        let top = (rect.y * self.scale).round();
        let right = ((rect.x + rect.w) * self.scale).round();
        let bottom = ((rect.y + rect.h) * self.scale).round();
        FRect::new(left, top, right - left, bottom - top)
    }

    fn fill(&mut self, rect: FRect, color: Color) -> Result<()> {
        self.canvas.set_draw_color(color);
        self.canvas.fill_rect(self.device(rect))?;
        Ok(())
    }

    fn outline(&mut self, rect: FRect, color: Color) -> Result<()> {
        self.canvas.set_draw_color(color);
        self.canvas.draw_rect(self.device(rect))?;
        Ok(())
    }

    fn text(&mut self, text: &str, x: f32, y: f32, color: Color) -> Result<()> {
        let mut rendered = self.text_engine.create_text(self.font, &sanitize(text))?;
        rendered.set_color(color)?;
        // The glyphs were rasterized at the device resolution, so place them on
        // whole device pixels to keep them sharp.
        rendered.draw((x * self.scale).round(), (y * self.scale).round())?;
        Ok(())
    }

    /// Vertical position that centers one line of text inside `rect`.
    fn line_y(&self, rect: FRect) -> f32 {
        rect.y + (rect.h - self.line_height) / 2.0
    }

    fn text_clipped(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        max_chars: usize,
        color: Color,
    ) -> Result<()> {
        self.text(&clip(text, max_chars), x, y, color)
    }

    fn hovering(&self, rect: FRect) -> bool {
        contains(rect, self.mouse.0, self.mouse.1)
    }

    fn button(&mut self, rect: FRect, label: &str, action: Action) -> Result<()> {
        let hover = self.hovering(rect);
        self.fill(rect, if hover { PANEL_HOVER } else { PANEL_LIGHT })?;
        self.outline(rect, if hover { ACCENT } else { BORDER })?;

        let max_chars = ((rect.w - 8.0) / self.char_width).max(1.0) as usize;
        let label = clip(label, max_chars);
        let width = label.chars().count() as f32 * self.char_width;
        self.text(
            &label,
            rect.x + (rect.w - width) / 2.0,
            self.line_y(rect),
            if hover { TEXT } else { MUTED },
        )?;

        if self.interactive {
            self.hits.push(Hit { rect, action });
        }
        Ok(())
    }

    fn tab(&mut self, rect: FRect, label: &str, selected: bool, action: Action) -> Result<()> {
        self.fill(rect, if selected { ACCENT_DIM } else { PANEL_LIGHT })?;
        self.outline(rect, if selected { ACCENT } else { BORDER })?;
        let max_chars = ((rect.w - 8.0) / self.char_width).max(1.0) as usize;
        let label = clip(label, max_chars);
        let width = label.chars().count() as f32 * self.char_width;
        self.text(
            &label,
            rect.x + (rect.w - width) / 2.0,
            self.line_y(rect),
            if selected { TEXT } else { MUTED },
        )?;
        if self.interactive {
            self.hits.push(Hit { rect, action });
        }
        Ok(())
    }

    fn list_row(&mut self, rect: FRect, selected: bool, action: Action) -> Result<()> {
        if selected {
            self.fill(rect, ROW_SELECTED)?;
            self.fill(FRect::new(rect.x, rect.y, 2.0, rect.h), ACCENT)?;
        } else if self.hovering(rect) {
            self.fill(rect, PANEL_HOVER)?;
        }
        if self.interactive {
            self.hits.push(Hit { rect, action });
        }
        Ok(())
    }

    fn scroll_region(&mut self, rect: FRect, target: ScrollTarget) {
        if self.interactive {
            self.scroll_hits.push((rect, target));
        }
    }

    fn scrollbar(
        &mut self,
        area: FRect,
        offset: usize,
        total: usize,
        visible: usize,
    ) -> Result<()> {
        if total <= visible || visible == 0 {
            return Ok(());
        }
        let track = FRect::new(area.x + area.w - 3.0, area.y + 2.0, 3.0, area.h - 4.0);
        self.fill(track, BACKGROUND)?;
        let height = (track.h * (visible as f32 / total as f32)).max(8.0);
        let max_offset = (total - visible) as f32;
        let position = if max_offset > 0.0 {
            (offset as f32 / max_offset) * (track.h - height)
        } else {
            0.0
        };
        self.fill(FRect::new(track.x, track.y + position, 3.0, height), ACCENT)
    }

    fn page_heading(&mut self, main: FRect, title: &str, subtitle: &str) -> Result<()> {
        self.text(title, main.x + 12.0, main.y + 10.0, ACCENT)?;
        let x = main.x + 12.0 + (title.chars().count() as f32 + 1.5) * self.char_width;
        self.text(subtitle, x, main.y + 10.0, MUTED)
    }

    fn header(&mut self, app: &App) -> Result<()> {
        let rect = header_rect();
        self.fill(rect, PANEL)?;
        self.fill(FRect::new(rect.x, rect.y, 5.0, rect.h), ACCENT)?;
        self.text("SHCCL", rect.x + 16.0, rect.y + 8.0, ACCENT)?;
        self.text(
            env!("CARGO_PKG_VERSION"),
            rect.x + 16.0 + 6.0 * self.char_width,
            rect.y + 8.0,
            MUTED,
        )?;
        self.text(
            "Smart Home Control Craft Launcher",
            rect.x + 16.0,
            rect.y + 22.0,
            MUTED,
        )?;

        let title = app.page.title();
        let title_width = title.chars().count() as f32 * self.char_width;
        self.text(
            title,
            rect.x + rect.w - 12.0 - title_width,
            rect.y + 8.0,
            TEXT,
        )?;

        let (state, color) = if let Some(version) = &app.running {
            (format!("running {version}"), ACCENT)
        } else if let Some(busy) = &app.busy {
            (busy.label.clone(), WARNING)
        } else {
            ("idle".to_string(), MUTED)
        };
        let state = clip(&state, 30);
        let state_width = state.chars().count() as f32 * self.char_width;
        self.text(
            &state,
            rect.x + rect.w - 12.0 - state_width,
            rect.y + 22.0,
            color,
        )
    }

    fn sidebar(&mut self, app: &App) -> Result<()> {
        let rect = sidebar_rect();
        self.fill(rect, PANEL)?;
        self.text("PAGES", rect.x + 12.0, rect.y + 10.0, MUTED)?;

        let mut y = rect.y + 26.0;
        for page in Page::ALL {
            let item = FRect::new(rect.x + 8.0, y, rect.w - 16.0, 22.0);
            let selected = app.page == page;
            let hover = self.hovering(item);
            if selected {
                self.fill(item, ACCENT_DIM)?;
            } else if hover {
                self.fill(item, PANEL_HOVER)?;
            }
            self.text(
                &format!("{}  {}", page.hotkey(), page.title()),
                item.x + 8.0,
                item.y + (item.h - self.line_height) / 2.0,
                if selected { TEXT } else { MUTED },
            )?;
            if self.interactive {
                self.hits.push(Hit {
                    rect: item,
                    action: Action::SetPage(page),
                });
            }
            y += 24.0;
        }

        self.fill(
            FRect::new(rect.x + 12.0, y + 4.0, rect.w - 24.0, 1.0),
            BORDER,
        )?;

        let account = app
            .accounts
            .current()
            .map(|account| account.username.as_str())
            .unwrap_or("none");
        self.text("account", rect.x + 12.0, y + 16.0, MUTED)?;
        self.text_clipped(account, rect.x + 12.0, y + 28.0, 14, TEXT)?;

        let default = app.config.default_version.as_deref().unwrap_or("none");
        self.text("default", rect.x + 12.0, y + 46.0, MUTED)?;
        self.text_clipped(default, rect.x + 12.0, y + 58.0, 14, TEXT)?;

        self.text("state", rect.x + 12.0, y + 76.0, MUTED)?;
        let (state, color) = if app.running.is_some() {
            ("game running", ACCENT)
        } else if app.busy.is_some() {
            ("working", WARNING)
        } else {
            ("idle", TEXT)
        };
        self.text(state, rect.x + 12.0, y + 88.0, color)?;

        self.button(
            FRect::new(rect.x + 8.0, rect.y + rect.h - 26.0, rect.w - 16.0, 22.0),
            "Quit",
            Action::Quit,
        )
    }

    fn dashboard(&mut self, app: &App) -> Result<()> {
        let main = main_rect();
        self.fill(main, PANEL)?;
        self.page_heading(main, "Dashboard", "doctor")?;

        let label_x = main.x + 12.0;
        let value_x = label_x + 96.0;
        let max_chars = (((main.x + main.w - 14.0) - value_x) / self.char_width) as usize;
        let mut y = main.y + 30.0;

        let paths = app.launcher.paths();
        let selected = app
            .accounts
            .current()
            .map(|account| account.username.as_str())
            .unwrap_or("none");

        info(
            self,
            label_x,
            value_x,
            y,
            "Root",
            &paths.root().display().to_string(),
            max_chars,
        )?;
        y += LINE_HEIGHT;
        info(
            self,
            label_x,
            value_x,
            y,
            "Config",
            &paths.config_file().display().to_string(),
            max_chars,
        )?;
        y += LINE_HEIGHT;
        info(
            self,
            label_x,
            value_x,
            y,
            "Accounts",
            &paths.accounts_file().display().to_string(),
            max_chars,
        )?;
        y += LINE_HEIGHT;
        info(
            self,
            label_x,
            value_x,
            y,
            "Profiles",
            &format!(
                "{} known, selected: {selected}",
                app.accounts.accounts.len()
            ),
            max_chars,
        )?;
        y += LINE_HEIGHT;
        info(
            self,
            label_x,
            value_x,
            y,
            "Installed",
            &format!("{} versions", app.installed.len()),
            max_chars,
        )?;
        y += LINE_HEIGHT;
        info(
            self,
            label_x,
            value_x,
            y,
            "Memory",
            &format!("{} MiB", app.config.memory_mb),
            max_chars,
        )?;
        y += LINE_HEIGHT;
        info(
            self,
            label_x,
            value_x,
            y,
            "Default",
            app.config.default_version.as_deref().unwrap_or("none"),
            max_chars,
        )?;
        y += LINE_HEIGHT;
        info(
            self,
            label_x,
            value_x,
            y,
            "Game dir",
            &app.config
                .game_dir
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "per-version directories".to_string()),
            max_chars,
        )?;
        y += LINE_HEIGHT;
        let java = match &app.java {
            None => "checking...".to_string(),
            Some(Ok(display)) => display.clone(),
            Some(Err(error)) => error.clone(),
        };
        info(self, label_x, value_x, y, "Java", &java, max_chars)?;
        y += LINE_HEIGHT;
        info(
            self,
            label_x,
            value_x,
            y,
            "Game",
            app.running.as_deref().unwrap_or("idle"),
            max_chars,
        )?;

        let button_y = main.y + main.h - 26.0;
        self.button(
            FRect::new(main.x + 12.0, button_y, 72.0, 22.0),
            "Refresh",
            Action::Refresh,
        )?;
        self.button(
            FRect::new(main.x + 90.0, button_y, 92.0, 22.0),
            "Detect Java",
            Action::DetectJava,
        )?;
        self.button(
            FRect::new(main.x + 188.0, button_y, 84.0, 22.0),
            "Open logs",
            Action::SetPage(Page::Logs),
        )
    }

    fn versions(&mut self, app: &App) -> Result<()> {
        let main = main_rect();
        self.fill(main, PANEL)?;

        let installed_label = format!("Installed {}", app.installed.len());
        let remote_count = app
            .manifest
            .as_ref()
            .map(|manifest| manifest.versions.len())
            .unwrap_or(0);
        let remote_label = format!("Remote {remote_count}");
        let tab_y = main.y + 8.0;
        self.tab(
            FRect::new(main.x + 12.0, tab_y, 116.0, 22.0),
            &installed_label,
            app.version_tab == VersionTab::Installed,
            Action::VersionTab(VersionTab::Installed),
        )?;
        self.tab(
            FRect::new(main.x + 134.0, tab_y, 116.0, 22.0),
            &remote_label,
            app.version_tab == VersionTab::Remote,
            Action::VersionTab(VersionTab::Remote),
        )?;

        let toolbar_y = main.y + 38.0;
        match app.version_tab {
            VersionTab::Installed => {
                self.button(
                    FRect::new(main.x + 12.0, toolbar_y, 114.0, 20.0),
                    "Install version...",
                    Action::InstallInput,
                )?;
                if let Some(version) = app.installed.get(app.installed_sel) {
                    self.button(
                        FRect::new(main.x + 132.0, toolbar_y, 60.0, 20.0),
                        "Repair",
                        Action::RepairVersion(version.id.clone()),
                    )?;
                }
                self.button(
                    FRect::new(main.x + 198.0, toolbar_y, 64.0, 20.0),
                    "Refresh",
                    Action::Refresh,
                )?;
            }
            VersionTab::Remote => {
                let label = if app.manifest.is_some() {
                    "Reload"
                } else {
                    "Load remote"
                };
                self.button(
                    FRect::new(main.x + 12.0, toolbar_y, 82.0, 20.0),
                    label,
                    Action::LoadRemote,
                )?;
                self.button(
                    FRect::new(main.x + 100.0, toolbar_y, 64.0, 20.0),
                    "Filter...",
                    Action::EditRemoteFilter,
                )?;
                let filter = if app.remote_filter.is_empty() {
                    "filter: all".to_string()
                } else {
                    format!("filter: {}", app.remote_filter)
                };
                self.text_clipped(&filter, main.x + 174.0, toolbar_y + 6.0, 34, MUTED)?;
            }
        }

        let list = FRect::new(
            main.x + 12.0,
            main.y + 60.0,
            main.w - 24.0,
            VERSION_ROWS as f32 * LINE_HEIGHT,
        );
        self.fill(list, LIST_BACKGROUND)?;
        match app.version_tab {
            VersionTab::Installed => self.installed_list(app, list)?,
            VersionTab::Remote => self.remote_list(app, list)?,
        }
        let target = match app.version_tab {
            VersionTab::Installed => ScrollTarget::Installed,
            VersionTab::Remote => ScrollTarget::Remote,
        };
        self.scroll_region(list, target);

        let action_y = main.y + main.h - 26.0;
        match app.version_tab {
            VersionTab::Installed => {
                if let Some(version) = app.installed.get(app.installed_sel) {
                    self.button(
                        FRect::new(main.x + 12.0, action_y, 64.0, 22.0),
                        "Launch",
                        Action::LaunchVersion(version.id.clone()),
                    )?;
                    self.button(
                        FRect::new(main.x + 82.0, action_y, 88.0, 22.0),
                        "Set default",
                        Action::SetDefaultVersion(version.id.clone()),
                    )?;
                    self.button(
                        FRect::new(main.x + 176.0, action_y, 64.0, 22.0),
                        "Preview",
                        Action::PreviewLaunch(version.id.clone()),
                    )?;
                }
                self.button(
                    FRect::new(main.x + 246.0, action_y, 64.0, 22.0),
                    "Logs",
                    Action::SetPage(Page::Logs),
                )?;
            }
            VersionTab::Remote => {
                if let Some(id) = app.remote_selected_id() {
                    self.button(
                        FRect::new(main.x + 12.0, action_y, 64.0, 22.0),
                        "Install",
                        Action::InstallVersion(id.clone()),
                    )?;
                    self.button(
                        FRect::new(main.x + 82.0, action_y, 64.0, 22.0),
                        "Launch",
                        Action::LaunchVersion(id.clone()),
                    )?;
                    self.button(
                        FRect::new(main.x + 152.0, action_y, 88.0, 22.0),
                        "Set default",
                        Action::SetDefaultVersion(id),
                    )?;
                }
            }
        }
        Ok(())
    }

    fn installed_list(&mut self, app: &App, list: FRect) -> Result<()> {
        if app.installed.is_empty() {
            self.text(
                "No installed versions. Install one from the Remote tab.",
                list.x + 8.0,
                list.y + 6.0,
                MUTED,
            )?;
            return Ok(());
        }

        let total = app.installed.len();
        let start = app.installed_scroll.min(total);
        let end = (start + VERSION_ROWS).min(total);
        for index in start..end {
            let version = &app.installed[index];
            let row = FRect::new(
                list.x,
                list.y + (index - start) as f32 * LINE_HEIGHT,
                list.w,
                LINE_HEIGHT,
            );
            let selected = index == app.installed_sel;
            self.list_row(row, selected, Action::SelectInstalled(index))?;

            let marker = if app.config.default_version.as_deref() == Some(version.id.as_str()) {
                "*"
            } else {
                " "
            };
            let main_class = version.main_class.as_deref().unwrap_or("-");
            let line = format!(
                "{marker} {:<20} {:<10} {}",
                version.id, version.version_type, main_class
            );
            self.text_clipped(
                &line,
                row.x + 6.0,
                self.line_y(row),
                ((row.w - 24.0) / self.char_width) as usize,
                if selected { TEXT } else { MUTED },
            )?;
        }
        self.scrollbar(list, app.installed_scroll, total, VERSION_ROWS)
    }

    fn remote_list(&mut self, app: &App, list: FRect) -> Result<()> {
        let Some(manifest) = &app.manifest else {
            let message = if app.busy.is_some() {
                "Loading the version manifest..."
            } else {
                "Remote manifest not loaded. Press L or click Load remote."
            };
            return self.text(message, list.x + 8.0, list.y + 6.0, MUTED);
        };

        let indices = app.remote_indices();
        if indices.is_empty() {
            return self.text(
                "No remote versions match the filter.",
                list.x + 8.0,
                list.y + 6.0,
                MUTED,
            );
        }

        let total = indices.len();
        let start = app.remote_scroll.min(total);
        let end = (start + VERSION_ROWS).min(total);
        for (position, &index) in indices.iter().enumerate().skip(start).take(end - start) {
            let version = &manifest.versions[index];
            let row = FRect::new(
                list.x,
                list.y + (position - start) as f32 * LINE_HEIGHT,
                list.w,
                LINE_HEIGHT,
            );
            let selected = position == app.remote_sel;
            self.list_row(row, selected, Action::SelectRemote(position))?;

            let marker = if app.config.default_version.as_deref() == Some(version.id.as_str()) {
                "*"
            } else {
                " "
            };
            let line = format!(
                "{marker} {:<20} {:<10} {}",
                version.id,
                version.kind,
                short_date(&version.release_time)
            );
            self.text_clipped(
                &line,
                row.x + 6.0,
                self.line_y(row),
                ((row.w - 24.0) / self.char_width) as usize,
                if selected { TEXT } else { MUTED },
            )?;
        }
        self.scrollbar(list, app.remote_scroll, total, VERSION_ROWS)
    }

    fn accounts(&mut self, app: &App) -> Result<()> {
        let main = main_rect();
        self.fill(main, PANEL)?;
        self.page_heading(main, "Accounts", "offline profiles")?;

        let list = FRect::new(
            main.x + 12.0,
            main.y + 30.0,
            main.w - 24.0,
            ACCOUNT_ROWS as f32 * 26.0,
        );
        self.fill(list, LIST_BACKGROUND)?;

        if app.accounts.accounts.is_empty() {
            self.text(
                "No accounts yet. Add an offline account or sign in with Microsoft.",
                list.x + 8.0,
                list.y + 6.0,
                MUTED,
            )?;
        } else {
            let total = app.accounts.accounts.len();
            let start = app.account_scroll.min(total);
            let end = (start + ACCOUNT_ROWS).min(total);
            for index in start..end {
                let account = &app.accounts.accounts[index];
                let row = FRect::new(list.x, list.y + (index - start) as f32 * 26.0, list.w, 26.0);
                let selected = index == app.account_sel;
                self.list_row(row, selected, Action::SelectAccount(index))?;
                let text_y = row.y + (row.h - 2.0 * self.line_height) / 2.0;

                let marker = if app.accounts.selected.as_deref() == Some(account.uuid.as_str()) {
                    "*"
                } else {
                    " "
                };
                self.text_clipped(
                    &format!("{marker} {:<16} {}", account.username, account.kind),
                    row.x + 8.0,
                    text_y,
                    ((row.w - 24.0) / self.char_width) as usize,
                    if selected { TEXT } else { MUTED },
                )?;
                self.text_clipped(
                    &account.uuid,
                    row.x + 16.0,
                    text_y + self.line_height,
                    ((row.w - 32.0) / self.char_width) as usize,
                    MUTED,
                )?;
            }
            self.scrollbar(list, app.account_scroll, total, ACCOUNT_ROWS)?;
        }
        self.scroll_region(list, ScrollTarget::Accounts);

        let action_y = main.y + main.h - 26.0;
        if let Some(account) = app.accounts.accounts.get(app.account_sel) {
            self.button(
                FRect::new(main.x + 12.0, action_y, 84.0, 22.0),
                "Use account",
                Action::UseAccount(account.uuid.clone()),
            )?;
        }
        self.button(
            FRect::new(main.x + 102.0, action_y, 96.0, 22.0),
            "Add offline...",
            Action::AddOffline,
        )?;
        self.button(
            FRect::new(main.x + 204.0, action_y, 116.0, 22.0),
            "Microsoft sign-in",
            Action::AddMicrosoft,
        )?;
        self.button(
            FRect::new(main.x + 326.0, action_y, 64.0, 22.0),
            "Refresh",
            Action::Refresh,
        )
    }

    fn settings(&mut self, app: &App) -> Result<()> {
        let main = main_rect();
        self.fill(main, PANEL)?;
        let subtitle = match app.settings_scope.version() {
            Some(version) => format!("versions/{version}/shccl.json"),
            None => "shccl.json".to_string(),
        };
        self.page_heading(main, "Settings", &subtitle)?;

        let x = main.x + 12.0;
        let width = main.w - 24.0;
        let mut y = main.y + 28.0;
        let count = Setting::ALL.len() as f32;
        let step = ((main.h - 56.0) / count).min(40.0);

        for setting in Setting::ALL {
            let value = app.setting_value(setting);
            self.setting_row(
                FRect::new(x, y, width, step - 2.0),
                setting.label(),
                &value,
                setting,
                app.settings_scope.is_version(),
            )?;
            y += step;
        }

        let action_y = main.y + main.h - 24.0;
        self.button(
            FRect::new(x, action_y, 122.0, 22.0),
            "Show config JSON",
            Action::ShowConfig,
        )?;
        let stored_at = match app.settings_scope.version() {
            Some(version) => format!(
                "stored at {}",
                app.launcher.paths().version_config_file(version).display()
            ),
            None => format!("stored at {}", app.launcher.paths().config_file().display()),
        };
        self.text_clipped(&stored_at, x + 132.0, action_y + 7.0, 42, MUTED)
    }

    fn setting_row(
        &mut self,
        rect: FRect,
        label: &str,
        value: &str,
        setting: Setting,
        version_scope: bool,
    ) -> Result<()> {
        self.fill(rect, PANEL_LIGHT)?;
        self.outline(rect, BORDER)?;
        self.text(label, rect.x + 10.0, rect.y + 4.0, MUTED)?;
        self.text_clipped(
            value,
            rect.x + 10.0,
            rect.y + 19.0,
            ((rect.w - 140.0) / self.char_width) as usize,
            TEXT,
        )?;

        let clear_x = rect.x + rect.w - 10.0 - 48.0;
        let set_x = clear_x - 6.0 - 58.0;
        let set_label = if setting == Setting::Scope {
            "Change"
        } else {
            "Set..."
        };
        self.button(
            FRect::new(set_x, rect.y + 8.0, 58.0, 20.0),
            set_label,
            Action::EditSetting(setting),
        )?;
        if setting.clearable(version_scope) {
            self.button(
                FRect::new(clear_x, rect.y + 8.0, 48.0, 20.0),
                "Clear",
                Action::ClearSetting(setting),
            )?;
        }
        Ok(())
    }

    fn logs(&mut self, app: &App) -> Result<()> {
        let main = main_rect();
        self.fill(main, PANEL)?;
        self.page_heading(main, "Logs", "progress and game output")?;
        self.button(
            FRect::new(main.x + main.w - 12.0 - 64.0, main.y + 8.0, 64.0, 20.0),
            "Clear",
            Action::ClearLogs,
        )?;

        let area = FRect::new(
            main.x + 12.0,
            main.y + 34.0,
            main.w - 24.0,
            LOG_ROWS as f32 * self.line_height + 4.0,
        );
        self.fill(area, LIST_BACKGROUND)?;

        if app.logs.is_empty() {
            self.text(
                "No log output yet. Install or launch a version to see progress here.",
                area.x + 6.0,
                area.y + 6.0,
                MUTED,
            )?;
        } else {
            let total = app.logs.len();
            let offset = app.log_scroll.min(total.saturating_sub(LOG_ROWS));
            let end = total - offset;
            let start = end.saturating_sub(LOG_ROWS);
            for (line, text) in app.logs[start..end].iter().enumerate() {
                let color = if text.starts_with("==>") {
                    ACCENT
                } else if text.starts_with("!!!") || text.starts_with("[stderr]") {
                    DANGER
                } else {
                    TEXT
                };
                self.text(
                    text,
                    area.x + 6.0,
                    area.y + 4.0 + line as f32 * self.line_height,
                    color,
                )?;
            }
            self.scrollbar(area, offset, total, LOG_ROWS)?;
        }
        self.scroll_region(area, ScrollTarget::Logs);
        Ok(())
    }

    fn footer(&mut self, app: &App) -> Result<()> {
        let rect = footer_rect();
        self.fill(rect, PANEL)?;

        let color = match app.status.level {
            Level::Info => MUTED,
            Level::Success => ACCENT,
            Level::Error => DANGER,
        };
        self.text_clipped(
            &app.status.text,
            rect.x + 8.0,
            rect.y + 4.0,
            ((rect.w - 16.0) / self.char_width) as usize,
            color,
        )?;

        let hints = if app.modal.is_some() {
            "Enter save   Esc cancel"
        } else if app.microsoft.is_some() {
            "Esc cancel sign-in"
        } else {
            "1-5 pages   Enter   R refresh   Q quit"
        };
        let hints_width = hints.chars().count() as f32 * self.char_width;
        self.text(
            hints,
            rect.x + rect.w - 8.0 - hints_width,
            rect.y + 16.0,
            MUTED,
        )?;

        if let Some(busy) = &app.busy {
            self.busy_line(app, rect, busy, hints_width)?;
        } else {
            self.text_clipped(
                page_hint(app.page),
                rect.x + 8.0,
                rect.y + 16.0,
                ((rect.w - 24.0 - hints_width) / self.char_width) as usize,
                MUTED,
            )?;
        }
        Ok(())
    }

    fn busy_line(&mut self, app: &App, rect: FRect, busy: &Busy, hints_width: f32) -> Result<()> {
        let max_chars = ((rect.w - 24.0 - hints_width) / self.char_width).max(8.0) as usize;
        let line = if busy.total > 0 {
            format!(
                "{} | {} {}/{} {}",
                busy.label, busy.phase, busy.completed, busy.total, busy.file
            )
        } else if busy.file.is_empty() {
            format!("{} | {}", busy.label, busy.phase)
        } else {
            format!("{} | {} {}", busy.label, busy.phase, busy.file)
        };
        self.text_clipped(&line, rect.x + 8.0, rect.y + 16.0, max_chars, WARNING)?;

        let bar = FRect::new(rect.x + 1.0, rect.y + rect.h - 3.0, rect.w - 2.0, 2.0);
        self.fill(bar, BACKGROUND)?;
        if busy.total > 0 {
            let ratio = (busy.completed as f32 / busy.total as f32).clamp(0.0, 1.0);
            self.fill(FRect::new(bar.x, bar.y, bar.w * ratio, bar.h), ACCENT)?;
        } else {
            let phase = (app.started.elapsed().as_secs_f32() * 0.6) % 1.0;
            let width = bar.w * 0.25;
            self.fill(
                FRect::new(bar.x + (bar.w - width) * phase, bar.y, width, bar.h),
                ACCENT,
            )?;
        }
        Ok(())
    }

    fn modal(&mut self, modal: &InputModal) -> Result<()> {
        let width = 400.0;
        let height = 124.0;
        let rect = FRect::new(
            (LOGICAL_WIDTH - width) / 2.0,
            (LOGICAL_HEIGHT - height) / 2.0,
            width,
            height,
        );
        self.fill(
            FRect::new(rect.x + 4.0, rect.y + 4.0, rect.w, rect.h),
            SHADOW,
        )?;
        self.fill(rect, PANEL)?;
        self.outline(rect, ACCENT)?;

        self.text(modal.title.as_str(), rect.x + 14.0, rect.y + 12.0, ACCENT)?;
        self.text_clipped(
            modal.prompt.as_str(),
            rect.x + 14.0,
            rect.y + 30.0,
            ((rect.w - 28.0) / self.char_width) as usize,
            MUTED,
        )?;

        let input = FRect::new(rect.x + 14.0, rect.y + 48.0, rect.w - 28.0, 20.0);
        self.fill(input, BACKGROUND)?;
        self.outline(input, BORDER)?;

        let max_chars = ((input.w - 10.0) / self.char_width).max(1.0) as usize;
        let chars: Vec<char> = modal.value.chars().collect();
        let cursor = modal.cursor.min(chars.len());
        let start = cursor.saturating_sub(max_chars.saturating_sub(1));
        let end = (start + max_chars).min(chars.len());
        if let Some((selection_start, selection_end)) = modal.selected_range() {
            let visible_start = selection_start.max(start);
            let visible_end = selection_end.min(end);
            if visible_start < visible_end {
                let x = input.x + 4.0 + (visible_start - start) as f32 * self.char_width;
                let width = (visible_end - visible_start) as f32 * self.char_width;
                self.fill(
                    FRect::new(x, self.line_y(input), width, self.line_height),
                    ACCENT_DIM,
                )?;
            }
        }
        let shown: String = chars[start..end].iter().collect();
        self.text(&shown, input.x + 4.0, self.line_y(input), TEXT)?;
        let cursor_x = input.x + 4.0 + (cursor - start) as f32 * self.char_width;
        self.fill(
            FRect::new(cursor_x, self.line_y(input), 1.0, self.line_height),
            ACCENT,
        )?;

        self.text(
            "Enter: save   Esc: cancel",
            rect.x + 14.0,
            rect.y + height - 18.0,
            MUTED,
        )?;
        self.button(
            FRect::new(
                rect.x + rect.w - 14.0 - 58.0,
                rect.y + height - 24.0,
                58.0,
                20.0,
            ),
            "Cancel",
            Action::CancelModal,
        )?;
        self.button(
            FRect::new(
                rect.x + rect.w - 14.0 - 58.0 - 6.0 - 58.0,
                rect.y + height - 24.0,
                58.0,
                20.0,
            ),
            "Save",
            Action::SubmitModal,
        )
    }

    fn microsoft_login(&mut self, code: &DeviceCode) -> Result<()> {
        let width = 440.0;
        let height = 190.0;
        let rect = FRect::new(
            (LOGICAL_WIDTH - width) / 2.0,
            (LOGICAL_HEIGHT - height) / 2.0,
            width,
            height,
        );
        self.fill(
            FRect::new(rect.x + 4.0, rect.y + 4.0, rect.w, rect.h),
            SHADOW,
        )?;
        self.fill(rect, PANEL)?;
        self.outline(rect, ACCENT)?;

        let max_chars = ((rect.w - 28.0) / self.char_width) as usize;
        self.text("Microsoft sign-in", rect.x + 14.0, rect.y + 12.0, ACCENT)?;
        self.text(
            "1. Open this page in a browser:",
            rect.x + 14.0,
            rect.y + 32.0,
            MUTED,
        )?;
        self.text_clipped(
            &code.verification_uri,
            rect.x + 20.0,
            rect.y + 46.0,
            max_chars.saturating_sub(6),
            ACCENT,
        )?;
        self.text("2. Enter this code:", rect.x + 14.0, rect.y + 68.0, MUTED)?;
        self.text_clipped(
            &code.user_code,
            rect.x + 20.0,
            rect.y + 82.0,
            max_chars.saturating_sub(6),
            WARNING,
        )?;
        if !code.message.is_empty() {
            self.text_clipped(
                &code.message,
                rect.x + 14.0,
                rect.y + 104.0,
                max_chars,
                MUTED,
            )?;
        }
        self.text(
            "Waiting for Microsoft sign-in...",
            rect.x + 14.0,
            rect.y + 128.0,
            ACCENT,
        )?;
        self.text(
            "The account is added and selected automatically.",
            rect.x + 14.0,
            rect.y + 146.0,
            MUTED,
        )?;
        self.button(
            FRect::new(
                rect.x + rect.w - 14.0 - 58.0,
                rect.y + height - 26.0,
                58.0,
                20.0,
            ),
            "Cancel",
            Action::CancelMicrosoft,
        )
    }
}

pub(crate) fn draw(
    app: &mut App,
    canvas: &mut Canvas<Window>,
    font: &UiFont,
    text_engine: &TextEngine,
) -> Result<()> {
    canvas.set_draw_color(BACKGROUND);
    canvas.clear();

    let mut hits = Vec::new();
    let mut scroll_hits = Vec::new();
    {
        let mut painter = Painter {
            canvas,
            text_engine,
            font: font.font(),
            scale: font.scale(),
            char_width: font.char_width(),
            line_height: font.line_height(),
            hits: &mut hits,
            scroll_hits: &mut scroll_hits,
            mouse: app.mouse,
            interactive: app.modal.is_none() && app.microsoft.is_none(),
        };
        painter.header(app)?;
        painter.sidebar(app)?;
        match app.page {
            Page::Dashboard => painter.dashboard(app)?,
            Page::Versions => painter.versions(app)?,
            Page::Accounts => painter.accounts(app)?,
            Page::Settings => painter.settings(app)?,
            Page::Logs => painter.logs(app)?,
        }
        painter.footer(app)?;
        if let Some(modal) = &app.modal {
            painter.interactive = true;
            painter.modal(modal)?;
        }
        if let Some(code) = &app.microsoft {
            painter.interactive = true;
            painter.microsoft_login(code)?;
        }
    }

    app.hits = hits;
    app.scroll_hits = scroll_hits;
    canvas.present();
    Ok(())
}

fn info(
    painter: &mut Painter,
    label_x: f32,
    value_x: f32,
    y: f32,
    label: &str,
    value: &str,
    max_chars: usize,
) -> Result<()> {
    painter.text(label, label_x, y, MUTED)?;
    painter.text_clipped(value, value_x, y, max_chars, TEXT)
}

fn page_hint(page: Page) -> &'static str {
    match page {
        Page::Dashboard => "Root, Java, account overview",
        Page::Versions => "Up/Down select   Enter action",
        Page::Accounts => "Up/Down select   Enter use   M Microsoft",
        Page::Settings => "Click Set.../Change   T target",
        Page::Logs => "Up/Down scroll   C clear",
    }
}

fn sanitize(text: &str) -> String {
    text.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}

fn clip(text: &str, max_chars: usize) -> String {
    let text = sanitize(text);
    if text.chars().count() <= max_chars {
        return text;
    }
    if max_chars == 0 {
        return String::new();
    }
    let mut clipped: String = text.chars().take(max_chars - 1).collect();
    clipped.push('~');
    clipped
}

fn short_date(date: &str) -> &str {
    date.get(0..10).unwrap_or(date)
}

fn contains(rect: FRect, x: f32, y: f32) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.w && y < rect.y + rect.h
}
