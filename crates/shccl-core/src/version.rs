use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Mojang's version manifest (`version_manifest_v2.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionManifest {
    pub latest: LatestVersions,
    pub versions: Vec<VersionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionSummary {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(default)]
    pub time: String,
    #[serde(rename = "releaseTime", default)]
    pub release_time: String,
    #[serde(default)]
    pub sha1: Option<String>,
}

/// A version JSON document.
///
/// Unknown fields are kept in `extra` so re-serializing does not drop data that
/// this launcher does not understand yet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionDetails {
    pub id: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(rename = "mainClass", default, skip_serializing_if = "Option::is_none")]
    pub main_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assets: Option<String>,
    #[serde(
        rename = "assetIndex",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub asset_index: Option<AssetIndexRef>,
    #[serde(default)]
    pub downloads: VersionDownloads,
    #[serde(default)]
    pub libraries: Vec<Library>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Arguments>,
    #[serde(
        rename = "minecraftArguments",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub minecraft_arguments: Option<String>,
    #[serde(
        rename = "javaVersion",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub java_version: Option<JavaVersion>,
    #[serde(
        rename = "inheritsFrom",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub inherits_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logging: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl VersionDetails {
    pub fn required_java(&self) -> Option<u32> {
        self.java_version
            .as_ref()
            .and_then(|java| java.major_version)
    }

    /// Merge a parent version into this child.
    ///
    /// Used for versions with `inheritsFrom` (Fabric, Forge, OptiFine, ...).
    /// Parent libraries and arguments come first, child values override scalar
    /// settings.
    pub fn merge(mut self, parent: VersionDetails) -> Self {
        self.kind = self.kind.or(parent.kind);
        self.main_class = self.main_class.or(parent.main_class);
        self.assets = self.assets.or(parent.assets);
        self.asset_index = self.asset_index.or(parent.asset_index);
        self.java_version = self.java_version.or(parent.java_version);
        self.logging = self.logging.or(parent.logging);

        self.downloads.client = self.downloads.client.or(parent.downloads.client);
        let mut downloads_extra = parent.downloads.extra;
        downloads_extra.extend(self.downloads.extra);
        self.downloads.extra = downloads_extra;

        let mut libraries = parent.libraries;
        libraries.extend(self.libraries);
        self.libraries = libraries;

        self.arguments = match (parent.arguments, self.arguments) {
            (Some(mut parent_args), Some(mut child_args)) => {
                parent_args.game.append(&mut child_args.game);
                parent_args.jvm.append(&mut child_args.jvm);
                Some(parent_args)
            }
            (Some(parent_args), None) => Some(parent_args),
            (None, child_args) => child_args,
        };

        self.minecraft_arguments = self.minecraft_arguments.or(parent.minecraft_arguments);
        self.inherits_from = None;

        let mut extra = parent.extra;
        extra.extend(self.extra);
        self.extra = extra;

        self
    }

    pub fn classpath_libraries(&self, features: &HashMap<String, bool>) -> Vec<&Library> {
        self.libraries
            .iter()
            .filter(|library| library.is_allowed(features))
            .filter(|library| library.downloads.artifact.is_some())
            .collect()
    }

    pub fn version_type(&self) -> &str {
        self.kind.as_deref().unwrap_or("release")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VersionDownloads {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<DownloadInfo>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DownloadInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl DownloadInfo {
    pub fn path(&self) -> Option<PathBuf> {
        self.path.as_ref().map(PathBuf::from)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetIndexRef {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(rename = "totalSize", default, skip_serializing_if = "Option::is_none")]
    pub total_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Library {
    pub name: String,
    #[serde(default)]
    pub downloads: LibraryDownloads,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub natives: Option<HashMap<String, String>>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extract: Option<Extract>,
}

impl Library {
    pub fn is_allowed(&self, features: &HashMap<String, bool>) -> bool {
        rules_allow(&self.rules, features)
    }

    pub fn artifact(&self) -> Option<&DownloadInfo> {
        self.downloads.artifact.as_ref()
    }

    pub fn artifact_path(&self) -> Option<PathBuf> {
        self.artifact()
            .and_then(|artifact| artifact.path())
            .or_else(|| maven_path(&self.name))
    }

    /// Resolve the native classifier for the current platform, if any.
    pub fn native_artifact(&self) -> Option<(String, DownloadInfo)> {
        let os = current_os()?;
        let template = self.natives.as_ref()?.get(os)?;
        let classifier = template.replace("${arch}", arch_bits());
        let mut artifact = self.downloads.classifiers.get(&classifier)?.clone();
        if artifact.path.is_none() {
            artifact.path = maven_path_with_classifier(&self.name, &classifier)
                .map(|path| path.to_string_lossy().into_owned());
        }
        Some((classifier, artifact))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LibraryDownloads {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<DownloadInfo>,
    #[serde(default)]
    pub classifiers: HashMap<String, DownloadInfo>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<OsRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Extract {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Argument>,
    #[serde(default)]
    pub jvm: Vec<Argument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Plain(String),
    Conditional {
        #[serde(default)]
        rules: Vec<Rule>,
        value: ArgumentValue,
    },
}

impl Argument {
    pub fn values(&self) -> Vec<String> {
        match self {
            Self::Plain(value) => vec![value.clone()],
            Self::Conditional { value, .. } => value.values(),
        }
    }

    pub fn is_allowed(&self, features: &HashMap<String, bool>) -> bool {
        match self {
            Self::Plain(_) => true,
            Self::Conditional { rules, .. } => rules_allow(rules, features),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ArgumentValue {
    One(String),
    Many(Vec<String>),
}

impl ArgumentValue {
    pub fn values(&self) -> Vec<String> {
        match self {
            Self::One(value) => vec![value.clone()],
            Self::Many(values) => values.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JavaVersion {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    #[serde(rename = "majorVersion", default)]
    pub major_version: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetIndex {
    #[serde(default)]
    pub objects: HashMap<String, AssetObject>,
    #[serde(rename = "virtual", default)]
    pub is_virtual: bool,
    #[serde(rename = "map_to_resources", default)]
    pub map_to_resources: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetObject {
    pub hash: String,
    #[serde(default)]
    pub size: u64,
}

pub fn current_os() -> Option<&'static str> {
    if cfg!(target_os = "windows") {
        Some("windows")
    } else if cfg!(target_os = "macos") {
        Some("osx")
    } else if cfg!(target_os = "linux") {
        Some("linux")
    } else {
        None
    }
}

fn current_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86" => "x86",
        "x86_64" => "x86_64",
        "aarch64" => "arm64",
        "arm" => "arm32",
        other => other,
    }
}

fn arch_bits() -> &'static str {
    if std::env::consts::ARCH.contains("64") {
        "64"
    } else {
        "32"
    }
}

/// Evaluate Minecraft's allow/disallow rule list for the current platform.
pub fn rules_allow(rules: &[Rule], features: &HashMap<String, bool>) -> bool {
    if rules.is_empty() {
        return true;
    }

    let mut allowed = false;
    for rule in rules {
        if rule_matches(rule, features) {
            allowed = rule.action == "allow";
        }
    }
    allowed
}

fn rule_matches(rule: &Rule, features: &HashMap<String, bool>) -> bool {
    if let Some(os) = &rule.os {
        if let Some(name) = &os.name
            && Some(name.as_str()) != current_os()
        {
            return false;
        }
        if let Some(arch) = &os.arch
            && arch != current_arch()
        {
            return false;
        }
        // OS version matchers are regular expressions in Mojang's format. They
        // are rare and platform specific; ignoring them is safer than treating
        // the pattern as a literal.
        let _ = &os.version;
    }

    if let Some(rule_features) = &rule.features {
        for (name, expected) in rule_features {
            if features.get(name).copied().unwrap_or(false) != *expected {
                return false;
            }
        }
    }

    true
}

/// Turn Maven coordinates (`group:artifact:version[:classifier][@ext]`) into a
/// repository-relative path.
pub(crate) fn maven_path(name: &str) -> Option<PathBuf> {
    let (coords, ext) = match name.split_once('@') {
        Some((coords, ext)) => (coords, ext),
        None => (name, "jar"),
    };
    let parts: Vec<&str> = coords.split(':').collect();
    let (group, artifact, version, classifier) = match parts.as_slice() {
        [group, artifact, version] => (*group, *artifact, *version, None),
        [group, artifact, version, classifier] => (*group, *artifact, *version, Some(*classifier)),
        _ => return None,
    };

    let mut file_name = format!("{artifact}-{version}");
    if let Some(classifier) = classifier {
        file_name.push('-');
        file_name.push_str(classifier);
    }
    file_name.push('.');
    file_name.push_str(ext);

    Some(
        PathBuf::from(group.replace('.', "/"))
            .join(artifact)
            .join(version)
            .join(file_name),
    )
}

fn maven_path_with_classifier(name: &str, classifier: &str) -> Option<PathBuf> {
    let (coords, ext) = match name.split_once('@') {
        Some((coords, ext)) => (coords, ext),
        None => (name, "jar"),
    };
    let parts: Vec<&str> = coords.split(':').collect();
    let (group, artifact, version) = match parts.as_slice() {
        [group, artifact, version] => (*group, *artifact, *version),
        [group, artifact, version, _] => (*group, *artifact, *version),
        _ => return None,
    };
    Some(
        PathBuf::from(group.replace('.', "/"))
            .join(artifact)
            .join(version)
            .join(format!("{artifact}-{version}-{classifier}.{ext}")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rules_default_allow_and_os_filter() {
        let features = HashMap::new();
        assert!(rules_allow(&[], &features));

        let rules: Vec<Rule> = serde_json::from_value(json!([
            { "action": "allow", "os": { "name": "linux" } },
            { "action": "disallow", "os": { "name": "osx" } }
        ]))
        .unwrap();
        assert_eq!(rules_allow(&rules, &features), cfg!(target_os = "linux"));
    }

    #[test]
    fn maven_coordinates_become_repository_paths() {
        assert_eq!(
            maven_path("org.lwjgl:lwjgl:3.3.3")
                .unwrap()
                .to_string_lossy(),
            "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3.jar"
        );
        assert_eq!(
            maven_path("org.lwjgl:lwjgl:3.3.3:natives-linux@zip")
                .unwrap()
                .to_string_lossy(),
            "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-linux.zip"
        );
    }

    #[test]
    fn version_inheritance_merges_in_order() {
        let parent: VersionDetails = serde_json::from_value(json!({
            "id": "parent",
            "mainClass": "example.Parent",
            "assets": "17",
            "libraries": [{ "name": "g:a:1" }],
            "arguments": { "game": ["--parent"], "jvm": ["-Dparent=true"] }
        }))
        .unwrap();
        let child: VersionDetails = serde_json::from_value(json!({
            "id": "child",
            "inheritsFrom": "parent",
            "libraries": [{ "name": "g:b:1" }],
            "arguments": { "game": ["--child"], "jvm": [] }
        }))
        .unwrap();

        let merged = child.merge(parent);
        assert_eq!(merged.main_class.as_deref(), Some("example.Parent"));
        assert_eq!(merged.assets.as_deref(), Some("17"));
        assert_eq!(merged.libraries.len(), 2);
        assert_eq!(merged.libraries[0].name, "g:a:1");
        assert_eq!(merged.libraries[1].name, "g:b:1");
        assert_eq!(
            merged.arguments.unwrap().game[0].values(),
            vec!["--parent".to_string()]
        );
        assert!(merged.inherits_from.is_none());
    }
}
