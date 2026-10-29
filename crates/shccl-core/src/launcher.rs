use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::auth::{Account, AccountStore};
use crate::config::LauncherConfig;
use crate::download::{DownloadTask, Downloader, build_http_client};
use crate::error::{Error, Result};
use crate::java::detect_java;
use crate::launch::{self, LaunchHandle, LaunchPlan, LaunchRequest};
use crate::paths::{LauncherPaths, validate_version_id};
use crate::progress::{Phase, ProgressEvent, ProgressSink};
use crate::util::{read_json, read_lock, write_json, write_lock};
use crate::version::{AssetIndex, VersionDetails, VersionManifest};

const VERSION_MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const RESOURCES_BASE_URL: &str = "https://resources.download.minecraft.net";

struct Inner {
    paths: LauncherPaths,
    downloader: Downloader,
    config: RwLock<LauncherConfig>,
    accounts: RwLock<AccountStore>,
}

/// The public entry point used by all SHCCL frontends.
#[derive(Clone)]
pub struct Launcher {
    inner: Arc<Inner>,
}

impl Launcher {
    /// Open (or create) a launcher root.
    pub fn new(root: Option<PathBuf>) -> Result<Self> {
        let paths = LauncherPaths::resolve(root);
        paths.ensure()?;
        let config = LauncherConfig::load(&paths.config_file())?;
        let accounts = AccountStore::load(&paths.accounts_file())?;
        let http = build_http_client()?;
        let downloader = Downloader::new(http, config.download_concurrency());

        Ok(Self {
            inner: Arc::new(Inner {
                paths,
                downloader,
                config: RwLock::new(config),
                accounts: RwLock::new(accounts),
            }),
        })
    }

    pub fn paths(&self) -> &LauncherPaths {
        &self.inner.paths
    }

    pub fn config(&self) -> LauncherConfig {
        read_lock(&self.inner.config).clone()
    }

    pub fn update_config<F>(&self, update: F) -> Result<LauncherConfig>
    where
        F: FnOnce(&mut LauncherConfig),
    {
        let mut config = self.config();
        update(&mut config);
        config.save(&self.inner.paths.config_file())?;
        *write_lock(&self.inner.config) = config.clone();
        Ok(config)
    }

    pub fn accounts(&self) -> AccountStore {
        read_lock(&self.inner.accounts).clone()
    }

    /// Add or replace an account and select it.
    pub fn upsert_account(&self, account: Account) -> Result<()> {
        let path = self.inner.paths.accounts_file();
        let mut accounts = write_lock(&self.inner.accounts);
        accounts.upsert(account.clone());
        accounts.selected = Some(account.uuid);
        accounts.save(&path)
    }

    pub fn select_account(&self, query: &str) -> Result<Account> {
        let path = self.inner.paths.accounts_file();
        let mut accounts = write_lock(&self.inner.accounts);
        let account = accounts.select(query)?;
        accounts.save(&path)?;
        Ok(account)
    }

    pub fn current_account(&self) -> Option<Account> {
        read_lock(&self.inner.accounts).current().cloned()
    }

    pub async fn fetch_manifest(&self) -> Result<VersionManifest> {
        self.inner.downloader.fetch_json(VERSION_MANIFEST_URL).await
    }

    pub fn installed_versions(&self) -> Result<Vec<InstalledVersionSummary>> {
        let versions_dir = self.inner.paths.versions_dir();
        let mut result = Vec::new();
        let entries = match std::fs::read_dir(&versions_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(result),
            Err(error) => return Err(error.into()),
        };

        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            if validate_version_id(&id).is_err() {
                continue;
            }
            if self.inner.paths.incomplete_marker(&id).exists() {
                continue;
            }
            let json_path = self.inner.paths.version_json(&id);
            let details = match read_json::<VersionDetails>(&json_path) {
                Ok(details) => details,
                Err(error) => {
                    tracing::warn!(
                        version = %id,
                        path = %json_path.display(),
                        %error,
                        "skipping version with an unreadable JSON file"
                    );
                    continue;
                }
            };
            result.push(InstalledVersionSummary {
                id: details.id.clone(),
                version_type: details.version_type().to_string(),
                main_class: details.main_class.clone(),
                path: entry.path(),
            });
        }

        result.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(result)
    }

    pub fn is_installed(&self, version_id: &str) -> bool {
        self.inner.paths.version_json(version_id).exists()
            && self.inner.paths.version_jar(version_id).exists()
            && !self.inner.paths.incomplete_marker(version_id).exists()
    }

    /// Download everything required to launch a version.
    pub async fn install_version(
        &self,
        version_id: &str,
        sink: &ProgressSink,
    ) -> Result<InstallReport> {
        validate_version_id(version_id)?;
        let incomplete = self.inner.paths.incomplete_marker(version_id);
        let marker_needed = incomplete.exists()
            || !self.inner.paths.version_json(version_id).exists()
            || !self.inner.paths.version_jar(version_id).exists();
        if marker_needed {
            if let Some(parent) = incomplete.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&incomplete, b"")?;
        }
        sink(ProgressEvent::Step {
            phase: Phase::Version,
            message: "fetching version manifest".to_string(),
        });
        let manifest = self.fetch_manifest().await?;
        sink(ProgressEvent::Step {
            phase: Phase::Version,
            message: format!("resolving Minecraft {version_id}"),
        });

        let mut visited = HashSet::new();
        let details = self
            .resolve_version(&manifest, version_id, &mut visited, sink)
            .await?;

        write_json(&self.inner.paths.version_json(version_id), &details)?;

        // Client jar.
        let client = details.downloads.client.clone().ok_or_else(|| {
            Error::Download(format!("version {version_id} has no client jar download"))
        })?;
        let client_url = client.url.clone().ok_or_else(|| {
            Error::Download(format!("version {version_id} client jar has no URL"))
        })?;
        let client_task = DownloadTask::new(client_url, self.inner.paths.version_jar(version_id))
            .with_checksum(client.sha1.clone(), client.size);
        self.inner
            .downloader
            .download_all(vec![client_task], Phase::Version, sink)
            .await?;

        // Libraries and native archives.
        let features = HashMap::new();
        let natives_dir = self.inner.paths.natives_dir(version_id);
        let mut library_tasks: BTreeMap<PathBuf, DownloadTask> = BTreeMap::new();
        let mut native_archives: Vec<NativeArchive> = Vec::new();

        for library in &details.libraries {
            if !library.is_allowed(&features) {
                continue;
            }

            if library.artifact().is_some() {
                let relative = library.artifact_path().ok_or_else(|| {
                    Error::Download(format!("library '{}' has no path", library.name))
                })?;
                let path = self.inner.paths.libraries_dir().join(&relative);
                let artifact = library.artifact().expect("checked above");
                let url = artifact.url.clone().or_else(|| {
                    Some(format!(
                        "https://libraries.minecraft.net/{}",
                        relative.to_string_lossy()
                    ))
                });
                if let Some(url) = url {
                    let task = DownloadTask::new(url, path.clone())
                        .with_checksum(artifact.sha1.clone(), artifact.size);
                    library_tasks.entry(path).or_insert(task);
                } else if !path.exists() {
                    return Err(Error::Download(format!(
                        "library {} has no download URL",
                        library.name
                    )));
                }
            }

            if let Some((_classifier, artifact)) = library.native_artifact() {
                let relative = artifact.path.clone().map(PathBuf::from).ok_or_else(|| {
                    Error::Download(format!("native library {} has no path", library.name))
                })?;
                let path = self.inner.paths.libraries_dir().join(&relative);
                let url = artifact.url.clone().ok_or_else(|| {
                    Error::Download(format!("native library {} has no URL", library.name))
                })?;
                let task = DownloadTask::new(url, path.clone())
                    .with_checksum(artifact.sha1.clone(), artifact.size);
                library_tasks.entry(path.clone()).or_insert(task);
                native_archives.push(NativeArchive {
                    archive: path,
                    destination: natives_dir.clone(),
                    excludes: library
                        .extract
                        .as_ref()
                        .map(|extract| extract.exclude.clone())
                        .unwrap_or_default(),
                });
            }
        }

        let library_count = library_tasks.len();
        let library_tasks = library_tasks.into_values().collect();
        self.inner
            .downloader
            .download_all(library_tasks, Phase::Libraries, sink)
            .await?;

        // Extract native libraries from a clean directory so stale platform
        // binaries cannot be mixed into the new install.
        sink(ProgressEvent::Step {
            phase: Phase::Natives,
            message: "extracting native libraries".to_string(),
        });
        let _ = tokio::fs::remove_dir_all(&natives_dir).await;
        tokio::fs::create_dir_all(&natives_dir).await?;
        let native_archives = native_archives.clone();
        let native_count = native_archives.len();
        tokio::task::spawn_blocking(move || {
            for archive in native_archives {
                extract_zip(&archive.archive, &archive.destination, &archive.excludes)?;
            }
            Ok::<(), Error>(())
        })
        .await
        .map_err(|error| Error::Launch(format!("native extraction task failed: {error}")))??;

        // Assets.
        let asset_count = self.install_assets(&details, sink).await?;

        sink(ProgressEvent::Step {
            phase: Phase::Version,
            message: format!("Minecraft {version_id} is ready"),
        });
        if marker_needed {
            std::fs::remove_file(&incomplete)?;
        }

        Ok(InstallReport {
            version_id: version_id.to_string(),
            client_jar: self.inner.paths.version_jar(version_id),
            libraries: library_count,
            natives: native_count,
            assets: asset_count,
        })
    }

    /// Build and spawn the game process.
    pub async fn launch(
        &self,
        request: LaunchRequest,
        sink: &ProgressSink,
    ) -> Result<LaunchHandle> {
        validate_version_id(&request.version_id)?;
        if !self.is_installed(&request.version_id) {
            return Err(Error::VersionNotInstalled(request.version_id.clone()));
        }
        let details = self.load_installed_details(&request.version_id, &mut HashSet::new())?;
        let config = self.config();
        let java_path = request
            .java_path
            .clone()
            .or_else(|| config.java_path.clone());
        let java = detect_java(java_path.as_deref(), details.required_java())?;
        sink(ProgressEvent::Step {
            phase: Phase::Launch,
            message: format!(
                "launching Minecraft {} with {}",
                request.version_id,
                java.display()
            ),
        });
        let plan = launch::build_plan(&self.inner.paths, &details, &request, &config, &java)?;
        launch::spawn(plan, request.capture_output, sink)
    }

    /// Resolve a plan without starting a process.
    pub fn prepare_launch(&self, request: &LaunchRequest) -> Result<LaunchPlan> {
        if !self.is_installed(&request.version_id) {
            return Err(Error::VersionNotInstalled(request.version_id.clone()));
        }
        let details = self.load_installed_details(&request.version_id, &mut HashSet::new())?;
        let config = self.config();
        let java_path = request
            .java_path
            .clone()
            .or_else(|| config.java_path.clone());
        let java = detect_java(java_path.as_deref(), details.required_java())?;
        launch::build_plan(&self.inner.paths, &details, request, &config, &java)
    }

    async fn resolve_version(
        &self,
        manifest: &VersionManifest,
        version_id: &str,
        visited: &mut HashSet<String>,
        sink: &ProgressSink,
    ) -> Result<VersionDetails> {
        if !visited.insert(version_id.to_string()) {
            return Err(Error::Config(format!(
                "cyclic version inheritance involving '{version_id}'"
            )));
        }

        let raw = self.load_or_fetch_details(manifest, version_id).await?;

        let merged = if let Some(parent_id) = raw.inherits_from.clone() {
            sink(ProgressEvent::Step {
                phase: Phase::Version,
                message: format!("merging {version_id} with parent {parent_id}"),
            });
            let parent =
                Box::pin(self.resolve_version(manifest, &parent_id, visited, sink)).await?;
            raw.merge(parent)
        } else {
            raw
        };

        visited.remove(version_id);
        Ok(merged)
    }

    async fn load_or_fetch_details(
        &self,
        manifest: &VersionManifest,
        version_id: &str,
    ) -> Result<VersionDetails> {
        validate_version_id(version_id)?;
        let path = self.inner.paths.version_json(version_id);
        if path.exists()
            && let Ok(details) = read_json::<VersionDetails>(&path)
        {
            return Ok(details);
        }

        let summary = manifest
            .versions
            .iter()
            .find(|entry| entry.id == version_id)
            .ok_or_else(|| Error::VersionNotFound(version_id.to_string()))?;
        let details: VersionDetails = self.inner.downloader.fetch_json(&summary.url).await?;
        write_json(&path, &details)?;
        Ok(details)
    }

    fn load_installed_details(
        &self,
        version_id: &str,
        visited: &mut HashSet<String>,
    ) -> Result<VersionDetails> {
        validate_version_id(version_id)?;
        if !visited.insert(version_id.to_string()) {
            return Err(Error::Config(format!(
                "cyclic version inheritance involving '{version_id}'"
            )));
        }
        let path = self.inner.paths.version_json(version_id);
        if !path.exists() {
            return Err(Error::VersionNotInstalled(version_id.to_string()));
        }
        let details: VersionDetails = read_json(&path)?;
        let merged = if let Some(parent_id) = details.inherits_from.clone() {
            let parent = self.load_installed_details(&parent_id, visited)?;
            details.merge(parent)
        } else {
            details
        };
        visited.remove(version_id);
        Ok(merged)
    }

    async fn install_assets(&self, details: &VersionDetails, sink: &ProgressSink) -> Result<usize> {
        let Some(index_ref) = details.asset_index.as_ref() else {
            sink(ProgressEvent::Step {
                phase: Phase::Assets,
                message: "version has no asset index; skipping assets".to_string(),
            });
            return Ok(0);
        };

        sink(ProgressEvent::Step {
            phase: Phase::Assets,
            message: format!("downloading asset index {}", index_ref.id),
        });

        let index_path = self
            .inner
            .paths
            .indexes_dir()
            .join(format!("{}.json", index_ref.id));
        if let Some(url) = index_ref.url.clone() {
            let task = DownloadTask::new(url, index_path.clone())
                .with_checksum(index_ref.sha1.clone(), index_ref.size);
            self.inner
                .downloader
                .download_all(vec![task], Phase::Assets, sink)
                .await?;
        } else if !index_path.exists() {
            return Err(Error::Download(format!(
                "asset index {} has no URL and is not installed",
                index_ref.id
            )));
        }

        let index: AssetIndex = read_json(&index_path)?;
        let mut tasks: BTreeMap<PathBuf, DownloadTask> = BTreeMap::new();
        for object in index.objects.values() {
            let Some((prefix, _)) = object.hash.split_at_checked(2) else {
                continue;
            };
            let relative = PathBuf::from("objects").join(prefix).join(&object.hash);
            let path = self.inner.paths.assets_dir().join(&relative);
            let url = format!("{RESOURCES_BASE_URL}/{prefix}/{}", object.hash);
            let task = DownloadTask::new(url, path.clone())
                .with_checksum(Some(object.hash.clone()), Some(object.size))
                .trust_size();
            tasks.entry(path).or_insert(task);
        }

        let asset_count = tasks.len();
        sink(ProgressEvent::Step {
            phase: Phase::Assets,
            message: format!("downloading {asset_count} asset objects"),
        });
        self.inner
            .downloader
            .download_all(tasks.into_values().collect(), Phase::Assets, sink)
            .await?;

        if index.is_virtual || index.map_to_resources {
            sink(ProgressEvent::Step {
                phase: Phase::Assets,
                message: "materializing legacy virtual assets".to_string(),
            });
            let objects_dir = self.inner.paths.objects_dir();
            let destination = self.inner.paths.virtual_assets_dir(&index_ref.id);
            let mappings = index
                .objects
                .iter()
                .filter_map(|(name, object)| {
                    let (prefix, _) = object.hash.split_at_checked(2)?;
                    Some((
                        objects_dir.join(prefix).join(&object.hash),
                        destination.join(name),
                    ))
                })
                .collect::<Vec<_>>();
            tokio::task::spawn_blocking(move || {
                for (source, destination) in mappings {
                    if let Some(parent) = destination.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    let needs_copy =
                        match (std::fs::metadata(&source), std::fs::metadata(&destination)) {
                            (Ok(source_meta), Ok(destination_meta)) => {
                                source_meta.len() != destination_meta.len()
                            }
                            _ => true,
                        };
                    if needs_copy {
                        std::fs::copy(&source, &destination)?;
                    }
                }
                Ok::<(), std::io::Error>(())
            })
            .await
            .map_err(|error| Error::Download(format!("asset copy task failed: {error}")))??;
        }

        Ok(asset_count)
    }
}

/// Summary of an installed version directory.
#[derive(Debug, Clone)]
pub struct InstalledVersionSummary {
    pub id: String,
    pub version_type: String,
    pub main_class: Option<String>,
    pub path: PathBuf,
}

/// Result of a successful installation.
#[derive(Debug, Clone)]
pub struct InstallReport {
    pub version_id: String,
    pub client_jar: PathBuf,
    pub libraries: usize,
    pub natives: usize,
    pub assets: usize,
}

#[derive(Clone)]
struct NativeArchive {
    archive: PathBuf,
    destination: PathBuf,
    excludes: Vec<String>,
}

fn extract_zip(archive: &Path, destination: &Path, excludes: &[String]) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let mut archive = zip::ZipArchive::new(file)?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let relative_string = relative.to_string_lossy();
        if excludes.iter().any(|exclude| {
            let exclude = exclude.trim_end_matches('/');
            !exclude.is_empty()
                && (relative_string == exclude
                    || relative_string.starts_with(&format!("{exclude}/")))
        }) {
            continue;
        }

        let output = destination.join(&relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&output)?;
            continue;
        }

        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut output_file = std::fs::File::create(&output)?;
        std::io::copy(&mut entry, &mut output_file)?;
    }

    Ok(())
}
