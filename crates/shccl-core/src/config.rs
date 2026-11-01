use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::util::{read_json, write_json};

/// Persistent launcher settings stored in `<root>/shccl.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LauncherConfig {
    /// Java executable. `None` uses `JAVA_HOME`, then `java` from `PATH`.
    pub java_path: Option<PathBuf>,
    /// Maximum heap size passed to the game.
    pub memory_mb: u32,
    /// Extra JVM arguments appended after the version's own arguments.
    pub jvm_args: Vec<String>,
    /// Extra game arguments appended after the version's own arguments.
    pub game_args: Vec<String>,
    /// Version launched when the CLI/UI does not specify one.
    pub default_version: Option<String>,
    /// Launcher-wide default game directory. `None` lets each version use its
    /// isolated `versions/<version_id>` directory. Relative paths are resolved
    /// against the root.
    pub game_dir: Option<PathBuf>,
    /// Number of parallel downloads.
    pub download_concurrency: usize,
}

impl Default for LauncherConfig {
    fn default() -> Self {
        Self {
            java_path: None,
            memory_mb: 4096,
            jvm_args: Vec::new(),
            game_args: Vec::new(),
            default_version: None,
            game_dir: None,
            download_concurrency: 12,
        }
    }
}

impl LauncherConfig {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        read_json(path)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_json(path, self)
    }

    pub fn download_concurrency(&self) -> usize {
        self.download_concurrency.clamp(1, 64)
    }
}

/// Per-version settings stored in `<root>/versions/<version_id>/shccl.json`.
///
/// An instance is one version directory, so settings travel with the installed
/// version while libraries and assets stay shared at the launcher root.
///
/// Every field is an override on top of [`LauncherConfig`]. `None` inherits the
/// launcher-wide value; `Some` replaces it for this version only. This keeps
/// versions independent: two versions can point `game_dir` at the same
/// directory while keeping their own memory, Java and argument settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InstanceConfig {
    /// Java executable. `None` inherits the launcher-wide setting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub java_path: Option<PathBuf>,
    /// Maximum heap size passed to the game.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u32>,
    /// Extra JVM arguments. `Some` replaces the launcher-wide list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jvm_args: Option<Vec<String>>,
    /// Extra game arguments. `Some` replaces the launcher-wide list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_args: Option<Vec<String>>,
    /// Working directory. `None` inherits the launcher-wide setting, falling
    /// back to `versions/<version_id>` when neither is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_dir: Option<PathBuf>,
    /// Number of parallel downloads used when installing this version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_concurrency: Option<usize>,
}

impl InstanceConfig {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        read_json(path)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_json(path, self)
    }

    /// Whether this version has no overrides and therefore uses the
    /// launcher-wide settings and the default isolated working directory.
    pub fn is_empty(&self) -> bool {
        self.java_path.is_none()
            && self.memory_mb.is_none()
            && self.jvm_args.is_none()
            && self.game_args.is_none()
            && self.game_dir.is_none()
            && self.download_concurrency.is_none()
    }

    /// Apply these overrides to the launcher-wide settings.
    pub fn apply_to(&self, base: &LauncherConfig) -> LauncherConfig {
        LauncherConfig {
            java_path: self.java_path.clone().or_else(|| base.java_path.clone()),
            memory_mb: self.memory_mb.unwrap_or(base.memory_mb),
            jvm_args: self
                .jvm_args
                .clone()
                .unwrap_or_else(|| base.jvm_args.clone()),
            game_args: self
                .game_args
                .clone()
                .unwrap_or_else(|| base.game_args.clone()),
            default_version: base.default_version.clone(),
            game_dir: self.game_dir.clone().or_else(|| base.game_dir.clone()),
            download_concurrency: self
                .download_concurrency
                .unwrap_or(base.download_concurrency),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_settings_override_only_their_own_fields() {
        let base = LauncherConfig {
            java_path: Some(PathBuf::from("/usr/bin/java")),
            memory_mb: 4096,
            jvm_args: vec!["-Dbase=true".into()],
            game_args: vec!["--base".into()],
            default_version: Some("1.21.4".into()),
            game_dir: Some(PathBuf::from("shared")),
            download_concurrency: 12,
        };
        let instance = InstanceConfig {
            java_path: None,
            memory_mb: Some(8192),
            jvm_args: Some(Vec::new()),
            game_args: Some(vec!["--instance".into()]),
            game_dir: Some(PathBuf::from("shared")),
            download_concurrency: None,
        };

        let merged = instance.apply_to(&base);
        assert_eq!(merged.java_path, base.java_path);
        assert_eq!(merged.memory_mb, 8192);
        assert!(merged.jvm_args.is_empty());
        assert_eq!(merged.game_args, vec!["--instance"]);
        assert_eq!(merged.default_version, base.default_version);
        assert_eq!(merged.game_dir, base.game_dir);
        assert_eq!(merged.download_concurrency, 12);
    }

    #[test]
    fn empty_instance_inherits_everything() {
        let base = LauncherConfig {
            memory_mb: 2048,
            ..LauncherConfig::default()
        };
        let merged = InstanceConfig::default().apply_to(&base);
        assert_eq!(merged.memory_mb, 2048);
        assert_eq!(merged.download_concurrency, base.download_concurrency);
        assert!(merged.game_dir.is_none());
    }
}
