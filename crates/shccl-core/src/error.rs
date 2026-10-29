use std::io;

/// Errors produced by `shccl-core`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),

    #[error("ZIP archive error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("checksum mismatch for {path}: expected {expected}, got {actual}")]
    Checksum {
        path: String,
        expected: String,
        actual: String,
    },

    #[error("version '{0}' was not found in the remote manifest")]
    VersionNotFound(String),

    #[error("version '{0}' is not installed; run `shccl install {0}` first")]
    VersionNotInstalled(String),

    #[error("no account is selected; run `shccl login offline <name>` or use `--account`")]
    NoAccount,

    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("Java is not available: {0}")]
    JavaNotFound(String),

    #[error("Java {found} is too old for this version (requires Java {required}+)")]
    JavaTooOld { found: u32, required: u32 },

    #[error("invalid configuration: {0}")]
    Config(String),

    #[error("launch failed: {0}")]
    Launch(String),

    #[error("download failed: {0}")]
    Download(String),

    #[error("unsupported operation: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, Error>;
