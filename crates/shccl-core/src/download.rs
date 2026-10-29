use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::stream::{self, StreamExt};
use reqwest::Client;
use serde::de::DeserializeOwned;
use tokio::io::AsyncWriteExt;

use crate::error::{Error, Result};
use crate::progress::{Phase, ProgressEvent, ProgressSink};
use crate::util::sha1_file;

const USER_AGENT: &str = concat!("SHCCL/", env!("CARGO_PKG_VERSION"));

pub(crate) fn build_http_client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(20))
        .build()?)
}

#[derive(Clone)]
pub(crate) struct Downloader {
    http: Client,
    concurrency: usize,
}

impl Downloader {
    pub(crate) fn new(http: Client, concurrency: usize) -> Self {
        Self {
            http,
            concurrency: concurrency.clamp(1, 64),
        }
    }

    pub(crate) async fn fetch_json<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        let response = self.http.get(url).send().await?.error_for_status()?;
        Ok(response.json::<T>().await?)
    }

    pub(crate) async fn download_all(
        &self,
        tasks: Vec<DownloadTask>,
        phase: Phase,
        sink: &ProgressSink,
    ) -> Result<()> {
        let total = tasks.len() as u64;
        let completed = Arc::new(AtomicU64::new(0));
        let concurrency = self.concurrency;

        let results: Vec<std::result::Result<(), String>> = stream::iter(tasks)
            .map(|task| {
                let completed = completed.clone();
                let sink = sink.clone();
                async move {
                    let result = self.download_one(&task).await;
                    if result.is_ok() {
                        let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                        sink(ProgressEvent::Batch {
                            phase,
                            completed: done,
                            total,
                            file: file_label(&task.path),
                        });
                    }
                    result
                }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;

        let mut errors = Vec::new();
        for result in results {
            if let Err(error) = result {
                errors.push(error);
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            let shown = errors.into_iter().take(3).collect::<Vec<_>>().join("; ");
            Err(Error::Download(shown))
        }
    }

    async fn download_one(&self, task: &DownloadTask) -> std::result::Result<(), String> {
        if existing_file_is_valid(task).await {
            return Ok(());
        }

        let mut last_error = String::new();
        for attempt in 1..=3 {
            if let Some(parent) = task.path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|error| format!("{}: {error}", parent.display()))?;
            }

            let mut part = task.path.clone().into_os_string();
            part.push(".part");
            let part = PathBuf::from(part);

            let result = async {
                let response = self
                    .http
                    .get(&task.url)
                    .send()
                    .await
                    .map_err(|error| format!("GET {}: {error}", task.url))?;

                let status = response.status();
                if !status.is_success() {
                    return Err(format!("GET {} returned HTTP {status}", task.url));
                }

                let mut file = tokio::fs::File::create(&part)
                    .await
                    .map_err(|error| format!("create {}: {error}", part.display()))?;

                let mut response = response;
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(|error| format!("read body: {error}"))?
                {
                    file.write_all(&chunk)
                        .await
                        .map_err(|error| format!("write {}: {error}", part.display()))?;
                }
                file.flush()
                    .await
                    .map_err(|error| format!("flush {}: {error}", part.display()))?;

                if let Some(expected) = &task.sha1
                    && task.verify_hash
                {
                    let actual = tokio::task::spawn_blocking({
                        let part = part.clone();
                        move || sha1_file(&part)
                    })
                    .await
                    .map_err(|error| format!("checksum task failed: {error}"))?
                    .map_err(|error| error.to_string())?;
                    if !actual.eq_ignore_ascii_case(expected) {
                        let _ = tokio::fs::remove_file(&part).await;
                        return Err(format!(
                            "checksum mismatch for {}: expected {expected}, got {actual}",
                            task.path.display()
                        ));
                    }
                }

                if task.path.exists() {
                    tokio::fs::remove_file(&task.path)
                        .await
                        .map_err(|error| format!("remove old {}: {error}", task.path.display()))?;
                }
                tokio::fs::rename(&part, &task.path)
                    .await
                    .map_err(|error| format!("rename {}: {error}", task.path.display()))?;
                Ok::<(), String>(())
            }
            .await;

            match result {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let _ = tokio::fs::remove_file(&part).await;
                    last_error = error;
                    if attempt < 3 {
                        tokio::time::sleep(Duration::from_millis(300 * attempt)).await;
                    }
                }
            }
        }

        Err(format!("{}: {last_error}", task.path.display()))
    }
}

async fn existing_file_is_valid(task: &DownloadTask) -> bool {
    let Ok(metadata) = tokio::fs::metadata(&task.path).await else {
        return false;
    };
    if let Some(expected) = task.size
        && metadata.len() != expected
    {
        return false;
    }
    let Some(expected) = task.sha1.as_ref() else {
        return true;
    };
    if !task.verify_hash && task.size.is_some() {
        return true;
    }
    let path = task.path.clone();
    tokio::task::spawn_blocking(move || sha1_file(&path))
        .await
        .ok()
        .and_then(|result| result.ok())
        .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
}

fn file_label(path: &Path) -> String {
    path.components()
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[derive(Debug, Clone)]
pub(crate) struct DownloadTask {
    pub url: String,
    pub path: PathBuf,
    pub sha1: Option<String>,
    pub size: Option<u64>,
    pub verify_hash: bool,
}

impl DownloadTask {
    pub(crate) fn new(url: impl Into<String>, path: PathBuf) -> Self {
        Self {
            url: url.into(),
            path,
            sha1: None,
            size: None,
            verify_hash: true,
        }
    }

    pub(crate) fn with_checksum(mut self, sha1: Option<String>, size: Option<u64>) -> Self {
        self.sha1 = sha1;
        self.size = size;
        self
    }

    /// Asset objects are stored by hash and can be checked by size on warm
    /// starts without hashing thousands of files.
    pub(crate) fn trust_size(mut self) -> Self {
        self.verify_hash = false;
        self
    }
}
