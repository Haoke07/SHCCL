use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};

use crate::auth::Account;
use crate::config::LauncherConfig;
use crate::error::{Error, Result};
use crate::java::JavaRuntime;
use crate::paths::LauncherPaths;
use crate::progress::{LogStream, ProgressEvent, ProgressSink};
use crate::util::read_json;
use crate::version::{AssetIndex, VersionDetails};

/// Everything needed to start one game process.
#[derive(Debug, Clone)]
pub struct LaunchRequest {
    pub version_id: String,
    pub account: Account,
    pub java_path: Option<PathBuf>,
    pub memory_mb: Option<u32>,
    pub jvm_args: Vec<String>,
    pub game_args: Vec<String>,
    /// Overrides the configured game directory.
    pub game_dir: Option<PathBuf>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Capture stdout/stderr and forward lines through the progress sink.
    /// When false the game inherits the terminal's standard streams.
    pub capture_output: bool,
}

impl LaunchRequest {
    pub fn new(version_id: impl Into<String>, account: Account) -> Self {
        Self {
            version_id: version_id.into(),
            account,
            java_path: None,
            memory_mb: None,
            jvm_args: Vec::new(),
            game_args: Vec::new(),
            game_dir: None,
            width: None,
            height: None,
            capture_output: false,
        }
    }
}

/// The resolved process invocation. Useful for frontends that want to preview
/// exactly what will be executed.
#[derive(Debug, Clone)]
pub struct LaunchPlan {
    pub version_id: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub working_dir: PathBuf,
}

impl LaunchPlan {
    pub fn command_line(&self) -> String {
        let mut parts = Vec::with_capacity(self.args.len() + 1);
        parts.push(shell_quote(&self.program.to_string_lossy()));
        parts.extend(self.args.iter().map(|arg| shell_quote(arg)));
        parts.join(" ")
    }
}

/// A spawned game process.
pub struct LaunchHandle {
    pub child: Child,
    pub plan: LaunchPlan,
}

impl LaunchHandle {
    pub async fn wait(&mut self) -> Result<std::process::ExitStatus> {
        Ok(self.child.wait().await?)
    }
}

pub(crate) fn build_plan(
    paths: &LauncherPaths,
    details: &VersionDetails,
    request: &LaunchRequest,
    config: &LauncherConfig,
    java: &JavaRuntime,
) -> Result<LaunchPlan> {
    let mut features = HashMap::new();
    features.insert("is_demo_user".to_string(), false);
    features.insert("has_quick_plays_support".to_string(), false);
    features.insert("is_quick_play_singleplayer".to_string(), false);
    features.insert("is_quick_play_multiplayer".to_string(), false);
    features.insert("is_quick_play_realms".to_string(), false);
    if request.width.is_some() && request.height.is_some() {
        features.insert("has_custom_resolution".to_string(), true);
    }

    let client_jar = paths.version_jar(&request.version_id);
    if !client_jar.exists() {
        return Err(Error::VersionNotInstalled(request.version_id.clone()));
    }

    let mut classpath = Vec::new();
    for library in details.classpath_libraries(&features) {
        let relative = library.artifact_path().ok_or_else(|| {
            Error::Launch(format!("library '{}' has no artifact path", library.name))
        })?;
        let path = paths.libraries_dir().join(&relative);
        if !path.exists() {
            return Err(Error::Launch(format!(
                "missing library {}; reinstall version '{}'",
                relative.display(),
                request.version_id
            )));
        }
        classpath.push(path);
    }
    classpath.push(client_jar.clone());

    let classpath = classpath
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join(CLASSPATH_SEPARATOR);

    let natives_dir = paths.natives_dir(&request.version_id);
    let working_dir = request
        .game_dir
        .clone()
        .or_else(|| config.game_dir.clone())
        .map(|dir| {
            if dir.is_absolute() {
                dir
            } else {
                paths.root().join(dir)
            }
        })
        .unwrap_or_else(|| paths.root().to_path_buf());
    std::fs::create_dir_all(&working_dir)?;

    let memory = request
        .memory_mb
        .unwrap_or(config.memory_mb)
        .clamp(512, 65_536);
    let asset_index_name = details
        .assets
        .clone()
        .unwrap_or_else(|| "legacy".to_string());
    let game_assets = game_assets_dir(paths, &asset_index_name);

    let vars: HashMap<&str, String> = HashMap::from([
        ("auth_player_name", request.account.username.clone()),
        ("version_name", request.version_id.clone()),
        ("game_directory", working_dir.to_string_lossy().into_owned()),
        (
            "assets_root",
            paths.assets_dir().to_string_lossy().into_owned(),
        ),
        ("game_assets", game_assets.to_string_lossy().into_owned()),
        ("assets_index_name", asset_index_name),
        ("auth_uuid", request.account.uuid.replace('-', "")),
        (
            "auth_access_token",
            request.account.access_token().to_string(),
        ),
        ("auth_session", request.account.access_token().to_string()),
        ("auth_xuid", String::new()),
        ("user_type", request.account.user_type().to_string()),
        ("user_properties", "{}".to_string()),
        ("clientid", "shccl".to_string()),
        ("version_type", details.version_type().to_string()),
        (
            "natives_directory",
            natives_dir.to_string_lossy().into_owned(),
        ),
        ("classpath", classpath.clone()),
        (
            "library_directory",
            paths.libraries_dir().to_string_lossy().into_owned(),
        ),
        ("classpath_separator", CLASSPATH_SEPARATOR.to_string()),
        ("launcher_name", "SHCCL".to_string()),
        ("launcher_version", env!("CARGO_PKG_VERSION").to_string()),
        ("resolution_width", request.width.unwrap_or(854).to_string()),
        (
            "resolution_height",
            request.height.unwrap_or(480).to_string(),
        ),
        ("quickPlayPath", String::new()),
        ("quickPlaySingleplayer", String::new()),
        ("quickPlayMultiplayer", String::new()),
        ("quickPlayRealms", String::new()),
    ]);

    let mut jvm_args = Vec::new();
    if !contains_prefix(&config.jvm_args, "-Xmx") && !contains_prefix(&request.jvm_args, "-Xmx") {
        jvm_args.push(format!("-Xmx{memory}M"));
        jvm_args.push(format!("-Xms{}M", memory.min(2048)));
    }

    if let Some(arguments) = &details.arguments {
        for argument in &arguments.jvm {
            if argument.is_allowed(&features) {
                jvm_args.extend(
                    argument
                        .values()
                        .into_iter()
                        .map(|value| substitute(&value, &vars)),
                );
            }
        }
    }

    if cfg!(target_os = "macos") && !jvm_args.iter().any(|arg| arg == "-XstartOnFirstThread") {
        jvm_args.push("-XstartOnFirstThread".to_string());
    }

    if !jvm_args
        .iter()
        .any(|arg| arg.starts_with("-Djava.library.path="))
    {
        jvm_args.push(format!(
            "-Djava.library.path={}",
            natives_dir.to_string_lossy()
        ));
    }

    if !jvm_args
        .iter()
        .any(|arg| arg == "-cp" || arg == "-classpath")
    {
        jvm_args.push("-cp".to_string());
        jvm_args.push(classpath.clone());
    }

    jvm_args.push("-Dminecraft.launcher.brand=SHCCL".to_string());
    jvm_args.push(format!(
        "-Dminecraft.launcher.version={}",
        env!("CARGO_PKG_VERSION")
    ));

    jvm_args.extend(config.jvm_args.iter().cloned());
    jvm_args.extend(request.jvm_args.iter().cloned());

    let mut game_args = Vec::new();
    if let Some(arguments) = &details.arguments {
        for argument in &arguments.game {
            if argument.is_allowed(&features) {
                game_args.extend(
                    argument
                        .values()
                        .into_iter()
                        .map(|value| substitute(&value, &vars)),
                );
            }
        }
    } else if let Some(legacy) = &details.minecraft_arguments {
        game_args.extend(
            legacy
                .split_whitespace()
                .map(|value| substitute(value, &vars)),
        );
    }

    game_args.extend(config.game_args.iter().cloned());
    game_args.extend(request.game_args.iter().cloned());

    let main_class = details
        .main_class
        .clone()
        .ok_or_else(|| Error::Launch("version JSON has no mainClass".into()))?;

    let mut args = Vec::with_capacity(jvm_args.len() + game_args.len() + 1);
    args.extend(jvm_args);
    args.push(main_class);
    args.extend(game_args);

    // Make sure an unresolved variable does not silently reach the game.
    if let Some(bad) = args.iter().find(|arg| arg.contains("${")) {
        return Err(Error::Launch(format!(
            "could not resolve launch placeholder in argument: {bad}"
        )));
    }

    Ok(LaunchPlan {
        version_id: request.version_id.clone(),
        program: java.path.clone(),
        args,
        working_dir,
    })
}

pub(crate) fn spawn(
    plan: LaunchPlan,
    capture_output: bool,
    sink: &ProgressSink,
) -> Result<LaunchHandle> {
    let mut command = Command::new(&plan.program);
    command.args(&plan.args).current_dir(&plan.working_dir);

    if capture_output {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
    } else {
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
    }

    let mut child = command.spawn().map_err(|error| {
        Error::Launch(format!(
            "could not start {} in {}: {error}",
            plan.program.display(),
            plan.working_dir.display()
        ))
    })?;

    if capture_output {
        if let Some(stdout) = child.stdout.take() {
            spawn_log_reader(stdout, LogStream::Stdout, sink.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_log_reader(stderr, LogStream::Stderr, sink.clone());
        }
    }

    Ok(LaunchHandle { child, plan })
}

fn spawn_log_reader<R>(reader: R, stream: LogStream, sink: ProgressSink)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            sink(ProgressEvent::Log { stream, line });
        }
    });
}

fn game_assets_dir(paths: &LauncherPaths, index_name: &str) -> PathBuf {
    let index_path = paths.indexes_dir().join(format!("{index_name}.json"));
    if let Ok(index) = read_json::<AssetIndex>(&index_path)
        && (index.is_virtual || index.map_to_resources)
    {
        return paths.virtual_assets_dir(index_name);
    }
    paths.assets_dir()
}

fn contains_prefix(args: &[String], prefix: &str) -> bool {
    args.iter().any(|arg| arg.starts_with(prefix))
}

fn substitute(value: &str, vars: &HashMap<&str, String>) -> String {
    let mut result = value.to_string();
    for (name, replacement) in vars {
        result = result.replace(&format!("${{{name}}}"), replacement);
    }
    result
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:=+".contains(c))
    {
        value.to_string()
    } else if cfg!(windows) {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

#[cfg(windows)]
const CLASSPATH_SEPARATOR: &str = ";";
#[cfg(not(windows))]
const CLASSPATH_SEPARATOR: &str = ":";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Account;

    #[test]
    fn builds_a_complete_vanilla_command() {
        let unique = format!(
            "shccl-launch-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        let paths = LauncherPaths::resolve(Some(root.clone()));
        paths.ensure().unwrap();

        let details: VersionDetails = serde_json::from_value(serde_json::json!({
            "id": "test",
            "type": "release",
            "mainClass": "net.minecraft.client.main.Main",
            "assets": "test",
            "libraries": [{
                "name": "example:library:1.0",
                "downloads": {
                    "artifact": {
                        "path": "example/library/1.0/library-1.0.jar",
                        "url": "https://example.invalid/library.jar",
                        "size": 1
                    }
                }
            }],
            "arguments": {
                "jvm": ["-Djava.library.path=${natives_directory}", "-cp", "${classpath}"],
                "game": ["--username", "${auth_player_name}", "--uuid", "${auth_uuid}", "--gameDir", "${game_directory}"]
            }
        }))
        .unwrap();

        std::fs::create_dir_all(paths.version_dir("test")).unwrap();
        std::fs::write(paths.version_jar("test"), b"jar").unwrap();
        let library = paths
            .libraries_dir()
            .join("example/library/1.0/library-1.0.jar");
        std::fs::create_dir_all(library.parent().unwrap()).unwrap();
        std::fs::write(&library, b"jar").unwrap();

        let request = LaunchRequest::new("test", Account::offline("Steve").unwrap());
        let java = JavaRuntime {
            path: PathBuf::from("java"),
            major: 21,
            version: "21.0.2".into(),
        };
        let plan = build_plan(
            &paths,
            &details,
            &request,
            &LauncherConfig::default(),
            &java,
        )
        .unwrap();

        let joined = plan.args.join(" ");
        assert!(joined.contains("net.minecraft.client.main.Main"));
        assert!(joined.contains("--username Steve"));
        assert!(joined.contains("5627dd98e6be3c21b8a8e92344183641"));
        assert!(joined.contains(library.to_string_lossy().as_ref()));
        assert!(!joined.contains("${"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_asset_directory_follows_index_flags() {
        let unique = format!(
            "shccl-assets-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        let paths = LauncherPaths::resolve(Some(root.clone()));
        paths.ensure().unwrap();

        let index_path = paths.indexes_dir().join("pre-1.6.json");
        std::fs::write(&index_path, r#"{"objects":{},"map_to_resources":true}"#).unwrap();
        assert_eq!(
            game_assets_dir(&paths, "pre-1.6"),
            paths.virtual_assets_dir("pre-1.6")
        );

        std::fs::write(&index_path, r#"{"objects":{},"virtual":true}"#).unwrap();
        assert_eq!(
            game_assets_dir(&paths, "pre-1.6"),
            paths.virtual_assets_dir("pre-1.6")
        );

        std::fs::remove_dir_all(root).unwrap();
    }
}
