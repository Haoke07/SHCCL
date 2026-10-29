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
    /// Game directory. `None` uses the launcher root, matching the standard
    /// `.minecraft` layout. Relative paths are resolved against the root.
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
