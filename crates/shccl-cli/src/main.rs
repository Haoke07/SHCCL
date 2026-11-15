use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use shccl_core::{Account, LaunchRequest, Launcher, ProgressEvent, ProgressSink, detect_java};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "shccl",
    version,
    about = "SHCCL - Smart Home Control Craft Launcher"
)]
struct Cli {
    /// Launcher root (defaults to SHCCL_ROOT or the platform data directory).
    #[arg(long, global = true, env = "SHCCL_ROOT", value_name = "DIR")]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show paths, Java, accounts and installed versions.
    Doctor,

    /// List installed versions, or remote versions with --remote.
    Versions(VersionsArgs),

    /// Download a version, its libraries and its assets.
    Install {
        /// Version id, for example 1.21.4.
        version: String,
    },

    /// Create and select an account.
    Login {
        #[command(subcommand)]
        method: LoginMethod,
    },

    /// List known accounts.
    Accounts,

    /// Select an account by username or UUID.
    Select {
        /// Username or UUID.
        account: String,
    },

    /// Launch a version. Installs it automatically when missing.
    Launch(LaunchArgs),

    /// Show or change launcher settings.
    Config {
        /// Edit settings for a specific version instead of the launcher-wide
        /// defaults.
        #[arg(long, global = true, value_name = "VERSION")]
        version: Option<String>,

        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Debug, Args)]
struct VersionsArgs {
    /// Fetch the official Mojang version manifest.
    #[arg(long)]
    remote: bool,

    /// Maximum number of entries to print.
    #[arg(long, default_value_t = 20)]
    limit: usize,
}

#[derive(Debug, Subcommand)]
enum LoginMethod {
    /// Create an offline-mode profile with a Java-compatible UUID.
    Offline {
        /// Player name (letters, digits and underscore, up to 16 characters).
        username: String,
    },
    /// Sign in with a Microsoft account.
    Microsoft {
        /// Use the legacy Live Connect browser flow with the official
        /// launcher's shared client id (manual redirect paste, unsupported).
        #[arg(long)]
        legacy: bool,
    },
}

#[derive(Debug, Args)]
struct LaunchArgs {
    /// Version to launch. Falls back to `default_version`.
    version: Option<String>,

    /// Account username or UUID. Falls back to the selected account.
    #[arg(long)]
    account: Option<String>,

    /// Java executable or JAVA_HOME. Falls back to configuration and PATH.
    #[arg(long)]
    java: Option<PathBuf>,

    /// Game directory (defaults to the configured directory or the version
    /// directory).
    #[arg(long)]
    game_dir: Option<PathBuf>,

    /// Maximum Java heap in MiB.
    #[arg(long)]
    memory: Option<u32>,

    /// Requested window width.
    #[arg(long, requires = "height")]
    width: Option<u32>,

    /// Requested window height.
    #[arg(long, requires = "width")]
    height: Option<u32>,

    /// Print the command instead of starting the game.
    #[arg(long)]
    dry_run: bool,

    /// Extra arguments passed to the game; put them after `--`.
    #[arg(last = true)]
    game_args: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum ConfigAction {
    /// Print the current configuration as JSON. With --version, prints the
    /// effective settings used to launch that version.
    Show,
    /// Set the Java executable or JAVA_HOME.
    SetJava { path: PathBuf },
    /// Remove the configured Java and fall back to JAVA_HOME/PATH.
    ClearJava,
    /// Set the maximum heap size in MiB.
    SetMemory { mb: u32 },
    /// Set parallel download count (1-64).
    SetConcurrency { value: usize },
    /// Enable or disable library file checks before launch.
    SetCheckFiles {
        /// `true` checks and reports missing files; `false` launches anyway.
        #[arg(
            action = clap::ArgAction::Set,
            value_parser = clap::builder::BoolishValueParser::new()
        )]
        enabled: bool,
    },
    /// Set the version used when `launch` has no argument.
    SetDefaultVersion {
        /// Version id, for example 1.21.4.
        #[arg(value_name = "VERSION")]
        id: String,
    },
    /// Clear the default version.
    ClearDefaultVersion,
    /// Set a game directory (relative paths are resolved against the root).
    SetGameDir { path: PathBuf },
    /// Use the default game directory for the selected version.
    ClearGameDir,
    /// Remove every per-version override (requires --version).
    Reset,
}

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let launcher = Launcher::new(cli.root.clone())
        .with_context(|| format!("failed to open launcher root {:?}", cli.root))?;

    match cli.command {
        Command::Doctor => doctor(&launcher),
        Command::Versions(args) => versions(&launcher, args).await,
        Command::Install { version } => install(&launcher, &version).await,
        Command::Login { method } => login(&launcher, method).await,
        Command::Accounts => accounts(&launcher),
        Command::Select { account } => select(&launcher, &account),
        Command::Launch(args) => launch(&launcher, args).await,
        Command::Config { version, action } => config(&launcher, version.as_deref(), action),
    }
}

fn doctor(launcher: &Launcher) -> Result<()> {
    let config = launcher.config();
    let accounts = launcher.accounts();
    let installed = launcher.installed_versions()?;

    println!("SHCCL {}", env!("CARGO_PKG_VERSION"));
    println!("Root:     {}", launcher.paths().root().display());
    println!("Config:   {}", launcher.paths().config_file().display());
    println!("Accounts: {}", launcher.paths().accounts_file().display());
    println!(
        "Profiles: {} known, selected: {}",
        accounts.accounts.len(),
        accounts
            .current()
            .map(|account| account.username.as_str())
            .unwrap_or("none")
    );
    println!("Installed versions: {}", installed.len());
    println!("Memory:   {} MiB", config.memory_mb);
    println!(
        "Default version: {}",
        config.default_version.as_deref().unwrap_or("none")
    );

    match detect_java(config.java_path.as_deref(), None) {
        Ok(java) => println!("Java:     {}", java.display()),
        Err(error) => println!("Java:     unavailable ({error})"),
    }

    Ok(())
}

async fn versions(launcher: &Launcher, args: VersionsArgs) -> Result<()> {
    let installed = launcher.installed_versions()?;
    let installed_ids = installed
        .iter()
        .map(|version| version.id.as_str())
        .collect::<std::collections::HashSet<_>>();

    if args.remote {
        let manifest = launcher.fetch_manifest().await?;
        println!(
            "Latest release: {}  Latest snapshot: {}",
            manifest.latest.release, manifest.latest.snapshot
        );
        for version in manifest.versions.iter().take(args.limit) {
            let marker = if installed_ids.contains(version.id.as_str()) {
                "*"
            } else {
                " "
            };
            println!(
                "{marker} {:<24} {:<10} {}",
                version.id, version.kind, version.release_time
            );
        }
    } else if installed.is_empty() {
        println!("No installed versions. Run `shccl versions --remote` to browse.");
    } else {
        for version in installed {
            println!(
                "{:<24} {:<10} {}",
                version.id,
                version.version_type,
                version.path.display()
            );
        }
    }

    Ok(())
}

async fn install(launcher: &Launcher, version: &str) -> Result<()> {
    let report = launcher.install_version(version, &cli_progress()).await?;
    println!(
        "Installed {}: {} libraries, {} native archives, {} assets -> {}",
        report.version_id,
        report.libraries,
        report.natives,
        report.assets,
        report.client_jar.display()
    );
    Ok(())
}

async fn login(launcher: &Launcher, method: LoginMethod) -> Result<()> {
    match method {
        LoginMethod::Offline { username } => {
            let account = Account::offline(&username)?;
            launcher.upsert_account(account.clone())?;
            println!(
                "Offline account '{}' added and selected (UUID {}).",
                account.username, account.uuid
            );
        }
        LoginMethod::Microsoft { legacy } => login_microsoft(launcher, legacy).await?,
    }
    Ok(())
}

async fn login_microsoft(launcher: &Launcher, legacy: bool) -> Result<()> {
    if legacy {
        return login_microsoft_legacy(launcher).await;
    }

    if launcher.microsoft_client_id().is_none() {
        eprintln!("{}", shccl_core::microsoft::CLIENT_ID_HELP);
        bail!("this build has no Microsoft client ID configured");
    }

    let code = launcher.microsoft_device_code().await?;
    println!("Microsoft device code login");
    println!("  1. Open {}", code.verification_uri);
    println!("  2. Enter the code {}", code.user_code);
    if !code.message.is_empty() {
        println!("  ({})", code.message);
    }
    println!("Waiting for Microsoft sign-in... (Ctrl+C to cancel)");

    let account = launcher.microsoft_login(&code, &cli_progress()).await?;
    println!(
        "Microsoft account '{}' added and selected (UUID {}).",
        account.username, account.uuid
    );
    Ok(())
}

async fn login_microsoft_legacy(launcher: &Launcher) -> Result<()> {
    println!("Legacy Microsoft login (Live Connect, unsupported fallback)");
    println!("  1. Open this URL in a browser:");
    println!("     {}", launcher.microsoft_legacy_url());
    println!("  2. Sign in with your Microsoft account.");
    println!("  3. After the redirect, copy the full URL from the address bar.");
    print!("Paste the redirect URL or code: ");
    std::io::stdout().flush().ok();

    let input = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        Ok::<_, std::io::Error>(line)
    })
    .await??;
    if input.trim().is_empty() {
        bail!("no redirect URL was provided");
    }

    let account = launcher
        .microsoft_legacy_login(input.trim(), &cli_progress())
        .await?;
    println!(
        "Microsoft account '{}' added and selected (UUID {}).",
        account.username, account.uuid
    );
    Ok(())
}

fn accounts(launcher: &Launcher) -> Result<()> {
    let store = launcher.accounts();
    if store.accounts.is_empty() {
        println!("No accounts. Run `shccl login offline <name>` or `shccl login microsoft`.");
        return Ok(());
    }

    for account in &store.accounts {
        let selected = store.selected.as_deref() == Some(account.uuid.as_str());
        println!(
            "{} {:<16} {:<12} {}",
            if selected { "*" } else { " " },
            account.username,
            account.kind,
            account.uuid
        );
    }
    Ok(())
}

fn select(launcher: &Launcher, query: &str) -> Result<()> {
    let account = launcher.select_account(query)?;
    println!(
        "Selected account '{}' ({}).",
        account.username, account.uuid
    );
    Ok(())
}

async fn launch(launcher: &Launcher, args: LaunchArgs) -> Result<()> {
    let config = launcher.config();
    let version_id = args
        .version
        .clone()
        .or_else(|| config.default_version.clone())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no version specified; pass a version or set one with `shccl config set-default-version`"
            )
        })?;

    let account = match args.account.as_deref() {
        Some(query) => launcher.accounts().find(query).cloned().ok_or_else(|| {
            anyhow::anyhow!("account '{query}' not found; run `shccl accounts` to list accounts")
        })?,
        None => launcher.current_account().ok_or_else(|| {
            anyhow::anyhow!(
                "no account selected; run `shccl login offline <name>` or `shccl login microsoft`"
            )
        })?,
    };
    // Renew expired Microsoft tokens before preparing or starting the game.
    let account = launcher.refresh_account(&account).await?;

    if !launcher.is_installed(&version_id) {
        if args.dry_run {
            bail!(
                "Minecraft {version_id} is not installed; run `shccl install {version_id}` first"
            );
        }
        eprintln!("Minecraft {version_id} is not installed; installing it first.");
        launcher
            .install_version(&version_id, &cli_progress())
            .await?;
    }

    let mut request = LaunchRequest::new(version_id.clone(), account.clone());
    request.java_path = args.java;
    request.memory_mb = args.memory;
    request.game_dir = args.game_dir;
    request.width = args.width;
    request.height = args.height;
    request.game_args = args.game_args;
    request.capture_output = false;

    if args.dry_run {
        let plan = launcher.prepare_launch(&request)?;
        println!("Working directory: {}", plan.working_dir.display());
        println!("{}", plan.command_line());
        return Ok(());
    }

    println!(
        "Launching Minecraft {} as {}...",
        version_id, account.username
    );
    let mut handle = launcher.launch(request, &cli_progress()).await?;
    println!("Game command started; waiting for exit.");
    let status = handle.wait().await?;
    if status.success() {
        println!("Minecraft exited successfully.");
        Ok(())
    } else {
        match status.code() {
            Some(code) => bail!("Minecraft exited with status {code}"),
            None => bail!("Minecraft was terminated by a signal"),
        }
    }
}

fn config(launcher: &Launcher, version: Option<&str>, action: ConfigAction) -> Result<()> {
    let suffix = version
        .map(|version| format!(" for version {version}"))
        .unwrap_or_default();

    match action {
        ConfigAction::Show => {
            let config = match version {
                Some(version) => {
                    let mut config = launcher.effective_config(version)?;
                    config.game_dir = Some(launcher.game_dir(version)?);
                    config
                }
                None => launcher.config(),
            };
            println!("{}", serde_json::to_string_pretty(&config)?);
        }
        ConfigAction::SetJava { path } => {
            if !path.exists() {
                bail!("Java path does not exist: {}", path.display());
            }
            let path = path.canonicalize().unwrap_or(path);
            match version {
                Some(version) => {
                    launcher.update_instance_config(version, |config| {
                        config.java_path = Some(path.clone())
                    })?;
                }
                None => {
                    launcher.update_config(|config| config.java_path = Some(path.clone()))?;
                }
            }
            println!("Java path{suffix} set to {}.", path.display());
        }
        ConfigAction::ClearJava => {
            match version {
                Some(version) => {
                    launcher.update_instance_config(version, |config| config.java_path = None)?;
                }
                None => {
                    launcher.update_config(|config| config.java_path = None)?;
                }
            }
            println!("Java path{suffix} cleared; JAVA_HOME/PATH will be used.");
        }
        ConfigAction::SetMemory { mb } => {
            if !(512..=65_536).contains(&mb) {
                bail!("memory must be between 512 and 65536 MiB");
            }
            match version {
                Some(version) => {
                    launcher
                        .update_instance_config(version, |config| config.memory_mb = Some(mb))?;
                }
                None => {
                    launcher.update_config(|config| config.memory_mb = mb)?;
                }
            }
            println!("Memory{suffix} set to {mb} MiB.");
        }
        ConfigAction::SetConcurrency { value } => {
            if !(1..=64).contains(&value) {
                bail!("download concurrency must be between 1 and 64");
            }
            match version {
                Some(version) => {
                    launcher.update_instance_config(version, |config| {
                        config.download_concurrency = Some(value)
                    })?;
                }
                None => {
                    launcher.update_config(|config| config.download_concurrency = value)?;
                }
            }
            println!("Download concurrency{suffix} set to {value}.");
        }
        ConfigAction::SetCheckFiles { enabled } => {
            match version {
                Some(version) => {
                    launcher.update_instance_config(version, |config| {
                        config.check_files = Some(enabled)
                    })?;
                }
                None => {
                    launcher.update_config(|config| config.check_files = enabled)?;
                }
            }
            println!(
                "File checks{suffix} {}.",
                if enabled { "enabled" } else { "disabled" }
            );
        }
        ConfigAction::SetDefaultVersion { id: default } => {
            if version.is_some() {
                bail!("the default version is a launcher-wide setting; omit --version");
            }
            launcher.update_config(|config| config.default_version = Some(default.clone()))?;
            println!("Default version set to {default}.");
        }
        ConfigAction::ClearDefaultVersion => {
            if version.is_some() {
                bail!("the default version is a launcher-wide setting; omit --version");
            }
            launcher.update_config(|config| config.default_version = None)?;
            println!("Default version cleared.");
        }
        ConfigAction::SetGameDir { path } => {
            match version {
                Some(version) => {
                    launcher.update_instance_config(version, |config| {
                        config.game_dir = Some(path.clone())
                    })?;
                }
                None => {
                    launcher.update_config(|config| config.game_dir = Some(path.clone()))?;
                }
            }
            println!("Game directory{suffix} set to {}.", path.display());
        }
        ConfigAction::ClearGameDir => match version {
            Some(version) => {
                launcher.update_instance_config(version, |config| config.game_dir = None)?;
                println!(
                    "Game directory for version {version} cleared; the launcher-wide directory (or versions/{version}) will be used."
                );
            }
            None => {
                launcher.update_config(|config| config.game_dir = None)?;
                println!(
                    "Game directory cleared; each version will use its own directory unless it overrides game_dir."
                );
            }
        },
        ConfigAction::Reset => {
            let Some(version) = version else {
                bail!("reset requires --version VERSION");
            };
            launcher.reset_instance_config(version)?;
            println!("Per-version settings for {version} cleared.");
        }
    }
    Ok(())
}

fn cli_progress() -> ProgressSink {
    Arc::new(|event| match event {
        ProgressEvent::Step { phase, message } => {
            eprintln!("==> [{phase}] {message}");
        }
        ProgressEvent::Batch {
            phase,
            completed,
            total,
            file,
        } => {
            let should_print =
                total <= 20 || completed == 1 || completed == total || completed.is_multiple_of(50);
            if should_print {
                eprintln!("    [{phase}] {completed}/{total} {file}");
            }
        }
        ProgressEvent::Log { .. } => {}
    })
}
