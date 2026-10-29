use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};

/// A Java runtime detected on this machine.
#[derive(Debug, Clone)]
pub struct JavaRuntime {
    /// Executable used for `Command::new` (may be the literal `java`).
    pub path: PathBuf,
    /// Major version (8 for `1.8.0_392`, 21 for `21.0.2`).
    pub major: u32,
    /// Version string reported by `java -version`.
    pub version: String,
}

impl JavaRuntime {
    pub fn display(&self) -> String {
        format!("Java {} ({})", self.version, self.path.display())
    }
}

/// Detect Java from an explicit path, `JAVA_HOME`, or `PATH`.
///
/// If `required` is set, an older runtime is rejected with
/// [`Error::JavaTooOld`].
pub fn detect_java(configured: Option<&Path>, required: Option<u32>) -> Result<JavaRuntime> {
    let candidate = match configured {
        Some(path) => normalize_java_path(path),
        None => match std::env::var_os("JAVA_HOME") {
            Some(java_home) => normalize_java_path(Path::new(&java_home)),
            None => PathBuf::from("java"),
        },
    };
    let candidate = if candidate.exists() {
        candidate.canonicalize().unwrap_or(candidate)
    } else {
        candidate
    };

    let output = Command::new(&candidate)
        .arg("-version")
        .output()
        .map_err(|error| {
            Error::JavaNotFound(format!(
                "could not run {}: {error}; install Java 21+ or set `java_path` in shccl.json",
                candidate.display()
            ))
        })?;

    if !output.status.success() {
        return Err(Error::JavaNotFound(format!(
            "{} -version exited with {}",
            candidate.display(),
            output.status
        )));
    }

    let mut text = String::from_utf8_lossy(&output.stderr).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&output.stdout));

    let (version, major) = parse_java_version(&text).ok_or_else(|| {
        Error::JavaNotFound(format!(
            "could not parse version output from {}",
            candidate.display()
        ))
    })?;

    if let Some(required) = required
        && major < required
    {
        return Err(Error::JavaTooOld {
            found: major,
            required,
        });
    }

    Ok(JavaRuntime {
        path: candidate,
        major,
        version,
    })
}

fn normalize_java_path(path: &Path) -> PathBuf {
    // Accept both a JAVA_HOME directory and the executable itself.
    let executable = if cfg!(windows) { "java.exe" } else { "java" };
    if path.is_dir() {
        let in_bin = path.join("bin").join(executable);
        if in_bin.exists() {
            return in_bin;
        }
        let direct = path.join(executable);
        if direct.exists() {
            return direct;
        }
    }
    path.to_path_buf()
}

fn parse_java_version(output: &str) -> Option<(String, u32)> {
    let line = output.lines().find(|line| line.contains("version"))?;
    let start = line.find('"')? + 1;
    let rest = &line[start..];
    let end = rest.find('"')?;
    let version = rest[..end].to_string();

    let first = version.split('.').next()?;
    let major = if first == "1" {
        version.split('.').nth(1)?.parse().ok()?
    } else {
        first.parse().ok()?
    };

    Some((version, major))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modern_and_legacy_versions() {
        assert_eq!(
            parse_java_version("openjdk version \"21.0.2\" 2024-01-16"),
            Some(("21.0.2".into(), 21))
        );
        assert_eq!(
            parse_java_version("java version \"1.8.0_392\""),
            Some(("1.8.0_392".into(), 8))
        );
    }
}
