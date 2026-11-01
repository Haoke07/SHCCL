use std::env;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Directory layout used by SHCCL.
///
/// The layout intentionally mirrors the standard `.minecraft` directory so an
/// existing installation can be reused by pointing `SHCCL_ROOT` (or `--root`)
/// at it.
#[derive(Debug, Clone)]
pub struct LauncherPaths {
    root: PathBuf,
}

impl LauncherPaths {
    /// Resolve the launcher root.
    ///
    /// Priority: explicit path, `SHCCL_ROOT`, then a platform-specific data
    /// directory.
    pub fn resolve(root: Option<PathBuf>) -> Self {
        let root = root
            .or_else(|| env::var_os("SHCCL_ROOT").map(PathBuf::from))
            .unwrap_or_else(default_root);
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn versions_dir(&self) -> PathBuf {
        self.root.join("versions")
    }

    pub fn libraries_dir(&self) -> PathBuf {
        self.root.join("libraries")
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.root.join("assets")
    }

    pub fn indexes_dir(&self) -> PathBuf {
        self.assets_dir().join("indexes")
    }

    pub fn objects_dir(&self) -> PathBuf {
        self.assets_dir().join("objects")
    }

    pub fn virtual_assets_dir(&self, index: &str) -> PathBuf {
        self.assets_dir().join("virtual").join(index)
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn version_dir(&self, version_id: &str) -> PathBuf {
        self.versions_dir().join(version_id)
    }

    /// Per-version settings. Keeping them next to the version files means a
    /// version directory can be copied or backed up with its configuration.
    pub fn version_config_file(&self, version_id: &str) -> PathBuf {
        self.version_dir(version_id).join("shccl.json")
    }

    /// Default working directory for an isolated version.
    ///
    /// Following the standard `.minecraft` layout, each version gets its own
    /// `versions/<version_id>` directory for `saves`, `mods`, `resourcepacks`,
    /// `options.txt` and logs, while `libraries`, `assets` and the installed
    /// version files remain shared under the launcher root.
    pub fn version_game_dir(&self, version_id: &str) -> PathBuf {
        self.version_dir(version_id)
    }

    pub fn version_json(&self, version_id: &str) -> PathBuf {
        self.version_dir(version_id)
            .join(format!("{version_id}.json"))
    }

    pub fn version_jar(&self, version_id: &str) -> PathBuf {
        self.version_dir(version_id)
            .join(format!("{version_id}.jar"))
    }

    pub fn natives_dir(&self, version_id: &str) -> PathBuf {
        self.version_dir(version_id).join("natives")
    }

    /// Present while an SHCCL installation is in progress. External
    /// `.minecraft` installations do not have this file.
    pub fn incomplete_marker(&self, version_id: &str) -> PathBuf {
        self.version_dir(version_id).join(".shccl-incomplete")
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("shccl.json")
    }

    pub fn accounts_file(&self) -> PathBuf {
        self.root.join("accounts.json")
    }

    /// Create the base directories. Safe to call repeatedly.
    pub fn ensure(&self) -> Result<()> {
        for dir in [
            self.root.clone(),
            self.versions_dir(),
            self.libraries_dir(),
            self.assets_dir(),
            self.indexes_dir(),
            self.objects_dir(),
            self.logs_dir(),
        ] {
            std::fs::create_dir_all(&dir)?;
        }
        Ok(())
    }
}

/// Reject version IDs that could escape the versions directory.
pub(crate) fn validate_version_id(version_id: &str) -> Result<()> {
    if version_id.is_empty()
        || version_id == "."
        || version_id == ".."
        || version_id.contains('/')
        || version_id.contains('\\')
        || version_id.contains('\0')
    {
        return Err(Error::Config(format!("invalid version id: {version_id:?}")));
    }
    Ok(())
}

fn default_root() -> PathBuf {
    if cfg!(windows) {
        if let Some(appdata) = env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("SHCCL");
        }
    } else if cfg!(target_os = "macos") {
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("SHCCL");
        }
    } else {
        if let Some(data_home) = env::var_os("XDG_DATA_HOME") {
            return PathBuf::from(data_home).join("shccl");
        }
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("shccl");
        }
    }

    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".shccl")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let unique = format!(
            "shccl-paths-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::env::temp_dir().join(unique)
    }

    #[test]
    fn version_layout_holds_settings_and_game_data() {
        let root = temp_root("layout");
        let paths = LauncherPaths::resolve(Some(root.clone()));
        assert_eq!(
            paths.version_config_file("1.21.4"),
            root.join("versions/1.21.4/shccl.json")
        );
        assert_eq!(
            paths.version_game_dir("1.21.4"),
            root.join("versions/1.21.4")
        );
    }

    #[test]
    fn ensure_does_not_create_instances_dir() {
        let root = temp_root("no-instances");
        let paths = LauncherPaths::resolve(Some(root.clone()));
        paths.ensure().unwrap();
        assert!(!root.join("instances").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ensure_keeps_existing_instances_dir() {
        let root = temp_root("legacy-data");
        std::fs::create_dir_all(root.join("instances")).unwrap();
        std::fs::write(root.join("instances/file.txt"), b"data").unwrap();
        let paths = LauncherPaths::resolve(Some(root.clone()));
        paths.ensure().unwrap();
        assert_eq!(
            std::fs::read(root.join("instances/file.txt")).unwrap(),
            b"data"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
