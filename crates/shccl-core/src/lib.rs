//! Core download, authentication and launch logic for SHCCL.
//!
//! The crate intentionally keeps UI concerns out of the core. Frontends create a
//! [`Launcher`], pass a [`ProgressSink`] for status updates, and own any terminal
//! or window event loop.

pub mod auth;
pub mod config;
mod download;
pub mod error;
pub mod java;
pub mod launch;
mod launcher;
pub mod microsoft;
pub mod paths;
pub mod progress;
mod util;
pub mod version;

pub use auth::{Account, AccountKind, AccountStore, offline_uuid};
pub use config::LauncherConfig;
pub use error::{Error, Result};
pub use java::{JavaRuntime, detect_java};
pub use launch::{LaunchHandle, LaunchPlan, LaunchRequest};
pub use launcher::{InstallReport, InstalledVersionSummary, Launcher};
pub use microsoft::{DeviceCode, MicrosoftAuth, MinecraftProfile, MinecraftSession, MsTokens};
pub use paths::LauncherPaths;
pub use progress::{
    LogStream, Phase, ProgressEvent, ProgressSink, channel_progress, noop_progress,
};
pub use version::{
    Argument, Arguments, AssetIndex, AssetObject, DownloadInfo, Library, VersionDetails,
    VersionManifest, VersionSummary,
};
