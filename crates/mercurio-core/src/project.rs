use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ir::{KirDocument, KirError};
use crate::library::{
    BaselineLibraryConfig, LibraryCacheMetadata, LibraryProviderConfig, LibrarySourceFingerprint,
    ResolvedLibraryArtifact,
};

pub const PROJECT_DESCRIPTOR_FILE_NAME: &str = ".mercurio-project.json";

fn is_model_source_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("sysml" | "kerml")
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct ProjectDescriptor {
    #[serde(default = "default_project_descriptor_version")]
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub libraries: Vec<ProjectLibraryConfig>,
    #[serde(default)]
    pub plugins: Vec<ProjectPluginConfig>,
}

#[derive(Debug)]
pub enum ProjectDescriptorError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Kir(KirError),
    Invalid(String),
}

#[derive(Debug, Clone)]
pub struct ResolvedProjectContext {
    pub workspace_root: PathBuf,
    pub descriptor_path: Option<PathBuf>,
    pub descriptor: Option<ProjectDescriptor>,
    pub resolved_libraries: Vec<ResolvedProjectLibrary>,
    pub library_context_document: KirDocument,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectLibraryRole {
    Baseline,
    Dependency,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectLibraryConfig {
    #[serde(default = "default_project_library_id")]
    pub id: String,
    #[serde(default = "default_project_library_role")]
    pub role: ProjectLibraryRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<LibraryProviderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectPluginConfig {
    pub id: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedProjectLibrary {
    pub id: String,
    pub role: ProjectLibraryRole,
    pub source_kind: String,
    pub source_path: Option<PathBuf>,
    pub cache_metadata: Option<LibraryCacheMetadata>,
    pub cache_path: Option<PathBuf>,
    pub cached_element_count: Option<usize>,
    pub document: KirDocument,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectLibraryCacheManifest {
    pub library_id: String,
    pub role: ProjectLibraryRole,
    pub source_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_identity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub importer_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_digest: Option<String>,
    pub element_count: usize,
}

impl std::fmt::Display for ProjectDescriptorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "failed to read project descriptor: {err}"),
            Self::Json(err) => write!(f, "failed to parse project descriptor: {err}"),
            Self::Kir(err) => write!(f, "failed to resolve project libraries: {err}"),
            Self::Invalid(message) => write!(f, "invalid project descriptor: {message}"),
        }
    }
}

impl std::error::Error for ProjectDescriptorError {}

impl From<std::io::Error> for ProjectDescriptorError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ProjectDescriptorError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<KirError> for ProjectDescriptorError {
    fn from(value: KirError) -> Self {
        Self::Kir(value)
    }
}

impl ProjectDescriptor {
    pub fn from_path(path: &Path) -> Result<Self, ProjectDescriptorError> {
        let input = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&input)?)
    }
}

pub fn resolve_project_context(
    open_path: &Path,
) -> Result<ResolvedProjectContext, ProjectDescriptorError> {
    let descriptor_path = discover_project_descriptor_path(open_path);
    let descriptor_root = descriptor_path
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf);
    let descriptor = descriptor_path
        .as_deref()
        .map(ProjectDescriptor::from_path)
        .transpose()?;
    let workspace_root = descriptor_root
        .clone()
        .unwrap_or_else(|| default_workspace_root_for_open_path(open_path));
    let cache_root = descriptor_root
        .as_ref()
        .map(|root| root.join(".mercurio").join("cache").join("libraries"));
    let (library_context_document, resolved_libraries) = resolve_library_context_document(
        descriptor.as_ref(),
        descriptor_root.as_deref(),
        cache_root.as_deref(),
    )?;

    Ok(ResolvedProjectContext {
        workspace_root,
        descriptor_path,
        descriptor,
        resolved_libraries,
        library_context_document,
    })
}

pub fn discover_project_descriptor_path(open_path: &Path) -> Option<PathBuf> {
    let start = if open_path.is_dir() {
        open_path
    } else {
        open_path.parent()?
    };

    for ancestor in start.ancestors() {
        let candidate = ancestor.join(PROJECT_DESCRIPTOR_FILE_NAME);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}

fn resolve_library_context_document(
    descriptor: Option<&ProjectDescriptor>,
    descriptor_root: Option<&Path>,
    cache_root: Option<&Path>,
) -> Result<(KirDocument, Vec<ResolvedProjectLibrary>), ProjectDescriptorError> {
    let project_libraries = descriptor
        .map(|descriptor| descriptor.libraries.as_slice())
        .unwrap_or(&[]);

    let mut resolved_libraries = Vec::new();
    let baseline_configs = project_libraries
        .iter()
        .filter(|library| library.role == ProjectLibraryRole::Baseline)
        .map(ProjectLibraryConfig::to_baseline_library_config)
        .collect::<Result<Vec<_>, _>>()?;
    let dependency_configs = project_libraries
        .iter()
        .filter(|library| library.role == ProjectLibraryRole::Dependency)
        .map(ProjectLibraryConfig::to_baseline_library_config)
        .collect::<Result<Vec<_>, _>>()?;
    let baseline_configs = if baseline_configs.is_empty() {
        vec![BaselineLibraryConfig::stdlib_locator()]
    } else {
        baseline_configs
    };
    for library in &baseline_configs {
        resolved_libraries.push(resolve_or_load_project_library(
            library,
            ProjectLibraryRole::Baseline,
            descriptor_root,
            cache_root,
            None,
        )?);
    }
    let baseline_documents = resolved_libraries
        .iter()
        .map(|library| library.document.clone())
        .collect::<Vec<_>>();

    let mut library_context = KirDocument::merge(baseline_documents)?;

    for library in &dependency_configs {
        let resolved_library = resolve_or_load_project_library(
            library,
            ProjectLibraryRole::Dependency,
            descriptor_root,
            cache_root,
            Some(&library_context),
        )?;
        library_context = KirDocument::merge([library_context, resolved_library.document.clone()])?;
        resolved_libraries.push(resolved_library);
    }

    Ok((library_context, resolved_libraries))
}

fn default_workspace_root_for_open_path(open_path: &Path) -> PathBuf {
    if is_model_source_file(open_path) {
        open_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    } else if open_path.is_dir() {
        open_path.to_path_buf()
    } else {
        crate::paths::repo_path("fixtures/l2")
    }
}

impl ResolvedProjectLibrary {
    fn from_artifact(
        role: ProjectLibraryRole,
        artifact: &ResolvedLibraryArtifact,
        cache_path: Option<PathBuf>,
    ) -> Self {
        Self {
            id: artifact.library_id.clone(),
            role,
            source_kind: artifact.source_kind.clone(),
            source_path: artifact.source_path.clone(),
            cache_metadata: artifact.cache_metadata.clone(),
            cache_path,
            cached_element_count: Some(artifact.document.elements.len()),
            document: artifact.document.clone(),
        }
    }
}

impl ProjectLibraryConfig {
    fn to_baseline_library_config(&self) -> Result<BaselineLibraryConfig, ProjectDescriptorError> {
        let provider = match (&self.locator, &self.provider) {
            (Some(locator), None) => LibraryProviderConfig::KparLocator {
                locator: locator.clone(),
            },
            (None, Some(LibraryProviderConfig::KparLocator { .. })) => {
                return Err(ProjectDescriptorError::Invalid(format!(
                    "library '{}' must use the top-level locator field instead of provider kind kpar_locator",
                    self.id
                )));
            }
            (None, Some(provider)) => provider.clone(),
            (Some(_), Some(_)) => {
                return Err(ProjectDescriptorError::Invalid(format!(
                    "library '{}' must use either locator or provider, not both",
                    self.id
                )));
            }
            (None, None) => {
                return Err(ProjectDescriptorError::Invalid(format!(
                    "library '{}' must declare locator or provider",
                    self.id
                )));
            }
        };

        Ok(BaselineLibraryConfig {
            id: self.id.clone(),
            provider,
        })
    }
}

fn resolve_or_load_project_library(
    library: &BaselineLibraryConfig,
    role: ProjectLibraryRole,
    descriptor_root: Option<&Path>,
    cache_root: Option<&Path>,
    library_context: Option<&KirDocument>,
) -> Result<ResolvedProjectLibrary, ProjectDescriptorError> {
    let context_digest = library_context.map(kir_document_digest).transpose()?;
    let fingerprint = cache_root
        .map(|_| {
            library
                .provider
                .source_fingerprint(&library.id, descriptor_root)
        })
        .transpose()?;

    if let (Some(cache_root), Some(fingerprint)) = (cache_root, fingerprint.as_ref()) {
        if let Some((artifact, cache_path)) =
            load_cached_library(cache_root, role, fingerprint, context_digest.as_deref())?
        {
            return Ok(ResolvedProjectLibrary::from_artifact(
                role,
                &artifact,
                Some(cache_path),
            ));
        }
    }

    let artifact =
        library
            .provider
            .resolve_with_context(&library.id, descriptor_root, library_context)?;
    let cache_path =
        cache_resolved_library(cache_root, role, &artifact, context_digest.as_deref())?;
    Ok(ResolvedProjectLibrary::from_artifact(
        role, &artifact, cache_path,
    ))
}

fn cache_resolved_library(
    cache_root: Option<&Path>,
    role: ProjectLibraryRole,
    artifact: &ResolvedLibraryArtifact,
    context_digest: Option<&str>,
) -> Result<Option<PathBuf>, ProjectDescriptorError> {
    let Some(cache_root) = cache_root else {
        return Ok(None);
    };

    let library_cache_dir = cache_root.join(safe_cache_segment(&artifact.library_id));
    let document_path = library_cache_dir.join("document.kir.json");
    artifact.document.write_pretty_to_path(&document_path)?;

    let manifest = ProjectLibraryCacheManifest {
        library_id: artifact.library_id.clone(),
        role,
        source_kind: artifact.source_kind.clone(),
        source_path: artifact
            .source_path
            .as_ref()
            .map(|path| path.display().to_string()),
        source_identity: artifact
            .cache_metadata
            .as_ref()
            .map(|metadata| metadata.source_identity.clone()),
        source_version: artifact
            .cache_metadata
            .as_ref()
            .and_then(|metadata| metadata.source_version.clone()),
        source_digest: artifact
            .cache_metadata
            .as_ref()
            .and_then(|metadata| metadata.source_digest.clone()),
        importer_version: artifact
            .cache_metadata
            .as_ref()
            .map(|metadata| metadata.importer_version.clone()),
        context_digest: context_digest.map(str::to_string),
        element_count: artifact.document.elements.len(),
    };
    std::fs::write(
        library_cache_dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)?,
    )?;

    Ok(Some(document_path))
}

fn load_cached_library(
    cache_root: &Path,
    role: ProjectLibraryRole,
    fingerprint: &LibrarySourceFingerprint,
    context_digest: Option<&str>,
) -> Result<Option<(ResolvedLibraryArtifact, PathBuf)>, ProjectDescriptorError> {
    let library_cache_dir = cache_root.join(safe_cache_segment(&fingerprint.library_id));
    let document_path = library_cache_dir.join("document.kir.json");
    let manifest_path = library_cache_dir.join("manifest.json");

    if !document_path.is_file() || !manifest_path.is_file() {
        return Ok(None);
    }

    let manifest = match std::fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|input| serde_json::from_str::<ProjectLibraryCacheManifest>(&input).ok())
    {
        Some(manifest) => manifest,
        None => return Ok(None),
    };

    if !cache_manifest_matches(&manifest, role, fingerprint, context_digest) {
        return Ok(None);
    }

    let document = match KirDocument::from_path(&document_path) {
        Ok(document) => document,
        Err(_) => return Ok(None),
    };

    Ok(Some((
        ResolvedLibraryArtifact {
            library_id: fingerprint.library_id.clone(),
            source_kind: fingerprint.source_kind.clone(),
            source_path: fingerprint.source_path.clone(),
            cache_metadata: Some(fingerprint.cache_metadata.clone()),
            document,
        },
        document_path,
    )))
}

fn cache_manifest_matches(
    manifest: &ProjectLibraryCacheManifest,
    role: ProjectLibraryRole,
    fingerprint: &LibrarySourceFingerprint,
    context_digest: Option<&str>,
) -> bool {
    manifest.library_id == fingerprint.library_id
        && manifest.role == role
        && manifest.source_kind == fingerprint.source_kind
        && manifest.source_identity == Some(fingerprint.cache_metadata.source_identity.clone())
        && manifest.source_version == fingerprint.cache_metadata.source_version
        && manifest.source_digest == fingerprint.cache_metadata.source_digest
        && manifest.importer_version == Some(fingerprint.cache_metadata.importer_version.clone())
        && manifest.context_digest.as_deref() == context_digest
}

fn kir_document_digest(document: &KirDocument) -> Result<String, ProjectDescriptorError> {
    let bytes = serde_json::to_vec(document)?;
    Ok(format!("fnv1a64:{:016x}", stable_digest_bytes(&bytes)))
}

fn stable_digest_bytes(bytes: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn safe_cache_segment(value: &str) -> String {
    let mut segment = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if segment.is_empty() || segment == "." || segment == ".." {
        segment = "library".to_string();
    }
    segment
}

fn default_project_descriptor_version() -> u32 {
    1
}

fn default_project_library_id() -> String {
    "stdlib".to_string()
}

fn default_project_library_role() -> ProjectLibraryRole {
    ProjectLibraryRole::Dependency
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Write;

    use serde_json::Value;

    use super::{
        PROJECT_DESCRIPTOR_FILE_NAME, ProjectDescriptor, ProjectLibraryRole,
        discover_project_descriptor_path, resolve_project_context,
    };
    use crate::ir::{KirDocument, KirElement};

    #[test]
    fn discovers_descriptor_from_ancestor_directory() {
        let root = temp_dir("discover_descriptor");
        let nested = root.join("models").join("subsystem");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join(PROJECT_DESCRIPTOR_FILE_NAME), "{\"version\":1}").unwrap();

        let found = discover_project_descriptor_path(&nested).unwrap();

        assert_eq!(found, root.join(PROJECT_DESCRIPTOR_FILE_NAME));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_project_context_with_local_baseline_library_override() {
        let root = temp_dir("project_context");
        let nested_file = root.join("models").join("demo.sysml");
        std::fs::create_dir_all(nested_file.parent().unwrap()).unwrap();
        std::fs::write(&nested_file, "package Demo {\n}\n").unwrap();

        let library_path = root.join("baseline.kir.json");
        let library = KirDocument {
            metadata: BTreeMap::new(),
            elements: vec![KirElement {
                id: "Demo::LibraryThing".to_string(),
                kind: "PartDefinition".to_string(),
                layer: 1,
                properties: BTreeMap::from([(
                    "declared_name".to_string(),
                    Value::String("LibraryThing".to_string()),
                )]),
            }],
        };
        library.write_pretty_to_path(&library_path).unwrap();

        let descriptor = serde_json::json!({
            "version": 1,
            "name": "Demo Project",
            "libraries": [
                {
                    "id": "custom",
                    "role": "baseline",
                    "provider": {
                        "kind": "precompiled_kir_artifact",
                        "path": library_path.display().to_string()
                    }
                }
            ]
        });
        std::fs::write(
            root.join(PROJECT_DESCRIPTOR_FILE_NAME),
            serde_json::to_string_pretty(&descriptor).unwrap(),
        )
        .unwrap();

        let resolved = resolve_project_context(&nested_file).unwrap();

        assert_eq!(resolved.workspace_root, root);
        assert_eq!(
            resolved.descriptor.unwrap().name.as_deref(),
            Some("Demo Project")
        );
        assert_eq!(resolved.resolved_libraries.len(), 1);
        assert_eq!(resolved.resolved_libraries[0].id, "custom");
        assert_eq!(
            resolved.resolved_libraries[0].role,
            ProjectLibraryRole::Baseline
        );
        assert_eq!(
            resolved.resolved_libraries[0].source_kind,
            "precompiled_kir_artifact"
        );
        assert_eq!(resolved.library_context_document.elements.len(), 1);
        assert_eq!(
            resolved.library_context_document.elements[0].id,
            "Demo::LibraryThing"
        );
        let cache_path = resolved.resolved_libraries[0].cache_path.as_ref().unwrap();
        assert_eq!(
            cache_path,
            &root
                .join(".mercurio")
                .join("cache")
                .join("libraries")
                .join("custom")
                .join("document.kir.json")
        );
        assert!(cache_path.is_file());
        assert!(cache_path.with_file_name("manifest.json").is_file());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_descriptor_defaults_version_to_one() {
        let descriptor: ProjectDescriptor = serde_json::from_str("{}").unwrap();

        assert_eq!(descriptor.version, 1);
        assert!(descriptor.libraries.is_empty());
        assert!(descriptor.plugins.is_empty());
    }

    #[test]
    fn project_descriptor_accepts_plugin_pins() {
        let descriptor: ProjectDescriptor = serde_json::from_str(
            r#"{
  "version": 1,
  "plugins": [
    {
      "id": "org.mercurio.samples.wasm-echo",
      "version": "0.1.0",
      "locator": "mpack:org.mercurio.samples.wasm-echo:0.1.0",
      "digest": "fnv1a64:sample"
    }
  ]
}"#,
        )
        .unwrap();

        assert_eq!(descriptor.plugins.len(), 1);
        assert_eq!(descriptor.plugins[0].id, "org.mercurio.samples.wasm-echo");
        assert_eq!(
            descriptor.plugins[0].locator.as_deref(),
            Some("mpack:org.mercurio.samples.wasm-echo:0.1.0")
        );
    }

    #[test]
    fn project_descriptor_rejects_legacy_baseline_libraries_field() {
        let err = serde_json::from_str::<ProjectDescriptor>(
            r#"{"version":1,"baseline_libraries":[],"libraries":[]}"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("baseline_libraries"));
    }

    #[test]
    fn resolves_project_context_with_additional_library_dependencies() {
        let root = temp_dir("project_dependency_context");
        let nested_file = root.join("models").join("demo.sysml");
        std::fs::create_dir_all(nested_file.parent().unwrap()).unwrap();
        std::fs::write(&nested_file, "package Demo {\n}\n").unwrap();

        let dependency_path = root.join("deps").join("library.kir.json");
        let dependency = KirDocument {
            metadata: BTreeMap::new(),
            elements: vec![KirElement {
                id: "Demo::DependencyThing".to_string(),
                kind: "PartDefinition".to_string(),
                layer: 2,
                properties: BTreeMap::new(),
            }],
        };
        dependency.write_pretty_to_path(&dependency_path).unwrap();

        let descriptor = serde_json::json!({
            "version": 1,
            "libraries": [
                {
                    "id": "dep",
                    "provider": {
                        "kind": "precompiled_kir_artifact",
                        "path": "deps/library.kir.json"
                    }
                }
            ]
        });
        std::fs::write(
            root.join(PROJECT_DESCRIPTOR_FILE_NAME),
            serde_json::to_string_pretty(&descriptor).unwrap(),
        )
        .unwrap();

        let resolved = resolve_project_context(&nested_file).unwrap();

        assert!(
            resolved
                .library_context_document
                .elements
                .iter()
                .any(|element| element.id == "Demo::DependencyThing")
        );
        assert!(resolved.library_context_document.elements.len() > 1);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_project_context_reuses_valid_cached_library_document() {
        let root = temp_dir("project_reuses_cached_library");
        let model_path = root.join("demo.sysml");
        std::fs::write(&model_path, "package Demo {\n}\n").unwrap();

        let library_path = root.join("baseline.kir.json");
        let original_library = KirDocument {
            metadata: BTreeMap::new(),
            elements: vec![KirElement {
                id: "Demo::OriginalLibraryThing".to_string(),
                kind: "PartDefinition".to_string(),
                layer: 1,
                properties: BTreeMap::new(),
            }],
        };
        original_library
            .write_pretty_to_path(&library_path)
            .unwrap();

        let descriptor = serde_json::json!({
            "version": 1,
            "libraries": [
                {
                    "id": "custom",
                    "role": "baseline",
                    "provider": {
                        "kind": "precompiled_kir_artifact",
                        "path": "baseline.kir.json"
                    }
                }
            ]
        });
        std::fs::write(
            root.join(PROJECT_DESCRIPTOR_FILE_NAME),
            serde_json::to_string_pretty(&descriptor).unwrap(),
        )
        .unwrap();

        let first = resolve_project_context(&model_path).unwrap();
        let cache_path = first.resolved_libraries[0].cache_path.as_ref().unwrap();
        let cached_library = KirDocument {
            metadata: BTreeMap::new(),
            elements: vec![KirElement {
                id: "Demo::CachedLibraryThing".to_string(),
                kind: "PartDefinition".to_string(),
                layer: 1,
                properties: BTreeMap::new(),
            }],
        };
        cached_library.write_pretty_to_path(cache_path).unwrap();

        let second = resolve_project_context(&model_path).unwrap();

        assert!(
            second
                .library_context_document
                .elements
                .iter()
                .any(|element| element.id == "Demo::CachedLibraryThing")
        );
        assert!(
            !second
                .library_context_document
                .elements
                .iter()
                .any(|element| element.id == "Demo::OriginalLibraryThing")
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_project_context_with_source_backed_library_dependency() {
        let root = temp_dir("project_source_dependency_context");
        let nested_file = root.join("models").join("demo.sysml");
        std::fs::create_dir_all(nested_file.parent().unwrap()).unwrap();
        std::fs::write(&nested_file, "package Demo {\n}\n").unwrap();

        let library_dir = root.join("libraries").join("domain-lib");
        std::fs::create_dir_all(&library_dir).unwrap();
        std::fs::write(
            library_dir.join("domain.sysml"),
            "package Domain {\n  part def Thing;\n}\n",
        )
        .unwrap();

        let descriptor = serde_json::json!({
            "version": 1,
            "libraries": [
                {
                    "id": "domain-lib",
                    "provider": {
                        "kind": "sysml_directory",
                        "path": "libraries/domain-lib"
                    }
                }
            ]
        });
        std::fs::write(
            root.join(PROJECT_DESCRIPTOR_FILE_NAME),
            serde_json::to_string_pretty(&descriptor).unwrap(),
        )
        .unwrap();

        let resolved = resolve_project_context(&nested_file).unwrap();

        assert!(
            resolved
                .library_context_document
                .elements
                .iter()
                .any(|element| element.id == "type.Domain.Thing")
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_project_context_with_kerml_library_dependency() {
        let root = temp_dir("project_kerml_dependency_context");
        let nested_file = root.join("models").join("demo.sysml");
        std::fs::create_dir_all(nested_file.parent().unwrap()).unwrap();
        std::fs::write(&nested_file, "package Demo {\n}\n").unwrap();

        let library_dir = root.join("libraries").join("kernel-lib");
        std::fs::create_dir_all(&library_dir).unwrap();
        std::fs::write(
            library_dir.join("kernel.kerml"),
            "package Kernel {\n  feature def SemanticThing;\n}\n",
        )
        .unwrap();

        let descriptor = serde_json::json!({
            "version": 1,
            "libraries": [
                {
                    "id": "kernel-lib",
                    "provider": {
                        "kind": "sysml_directory",
                        "path": "libraries/kernel-lib"
                    }
                }
            ]
        });
        std::fs::write(
            root.join(PROJECT_DESCRIPTOR_FILE_NAME),
            serde_json::to_string_pretty(&descriptor).unwrap(),
        )
        .unwrap();

        let resolved = resolve_project_context(&nested_file).unwrap();

        assert!(
            resolved
                .library_context_document
                .elements
                .iter()
                .any(|element| element.id == "type.Kernel.SemanticThing")
        );
        assert!(resolved.resolved_libraries[1].cache_path.is_some());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_project_context_with_kpar_library_dependency() {
        let root = temp_dir("project_kpar_dependency_context");
        let nested_file = root.join("models").join("demo.sysml");
        std::fs::create_dir_all(nested_file.parent().unwrap()).unwrap();
        std::fs::write(&nested_file, "package Demo {\n}\n").unwrap();

        let library_path = root.join("libraries").join("domain-lib.kpar");
        std::fs::create_dir_all(library_path.parent().unwrap()).unwrap();
        write_test_kpar(
            &library_path,
            "Domain Library",
            "1.0.0",
            &[("domain.sysml", "package Domain {\n  part def Thing;\n}\n")],
        );

        let descriptor = serde_json::json!({
            "version": 1,
            "libraries": [
                {
                    "id": "domain-lib",
                    "provider": {
                        "kind": "kpar_file",
                        "path": "libraries/domain-lib.kpar"
                    }
                }
            ]
        });
        std::fs::write(
            root.join(PROJECT_DESCRIPTOR_FILE_NAME),
            serde_json::to_string_pretty(&descriptor).unwrap(),
        )
        .unwrap();

        let resolved = resolve_project_context(&nested_file).unwrap();

        assert!(
            resolved
                .library_context_document
                .elements
                .iter()
                .any(|element| element.id == "type.Domain.Thing")
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_project_context_with_package_set_library_dependency() {
        let root = temp_dir("project_package_set_dependency_context");
        let nested_file = root.join("models").join("demo.sysml");
        std::fs::create_dir_all(nested_file.parent().unwrap()).unwrap();
        std::fs::write(&nested_file, "package Demo {\n}\n").unwrap();

        let package_set_dir = root.join("libraries").join("sysml.library.kpar");
        std::fs::create_dir_all(&package_set_dir).unwrap();
        write_test_kpar_with_usage(
            &package_set_dir.join("Kernel_Semantic_Library-1.0.0.kpar"),
            "Kernel Semantic Library",
            "1.0.0",
            &[],
            &[(
                "semantic.sysml",
                "package Kernel {\n  part def SemanticThing;\n}\n",
            )],
        );
        write_test_kpar_with_usage(
            &package_set_dir.join("SysML_Systems_Library-2.0.0.kpar"),
            "SysML Systems Library",
            "2.0.0",
            &[(
                "https://www.omg.org/spec/KerML/20250201/Semantic-Library.kpar",
                "1.0.0",
            )],
            &[(
                "systems.sysml",
                "package Systems {\n  part def SystemThing;\n}\n",
            )],
        );

        let descriptor = serde_json::json!({
            "version": 1,
            "libraries": [
                {
                    "id": "systems-lib",
                    "provider": {
                        "kind": "package_set_directory",
                        "path": "libraries/sysml.library.kpar",
                        "entry": "https://www.omg.org/spec/SysML/20250201/Systems-Library.kpar"
                    }
                }
            ]
        });
        std::fs::write(
            root.join(PROJECT_DESCRIPTOR_FILE_NAME),
            serde_json::to_string_pretty(&descriptor).unwrap(),
        )
        .unwrap();

        let resolved = resolve_project_context(&nested_file).unwrap();

        assert!(
            resolved
                .library_context_document
                .elements
                .iter()
                .any(|element| element.id == "type.Kernel.SemanticThing")
        );
        assert!(
            resolved
                .library_context_document
                .elements
                .iter()
                .any(|element| element.id == "type.Systems.SystemThing")
        );
        assert!(
            resolved
                .resolved_libraries
                .iter()
                .any(|library| library.role == ProjectLibraryRole::Baseline
                    && library.id == "stdlib")
        );
        assert!(
            resolved
                .resolved_libraries
                .iter()
                .any(|library| library.role == ProjectLibraryRole::Dependency
                    && library.id == "systems-lib"
                    && library.source_kind == "package_set_directory")
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "mercurio_project_{label}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_test_kpar(
        path: &std::path::Path,
        name: &str,
        version: &str,
        entries: &[(&str, &str)],
    ) {
        write_test_kpar_with_usage(path, name, version, &[], entries);
    }

    fn write_test_kpar_with_usage(
        path: &std::path::Path,
        name: &str,
        version: &str,
        usage: &[(&str, &str)],
        entries: &[(&str, &str)],
    ) {
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::FileOptions::default();

        writer.start_file(".project.json", options).unwrap();
        writer
            .write_all(
                serde_json::json!({
                    "name": name,
                    "version": version,
                    "usage": usage
                        .iter()
                        .map(|(resource, version_constraint)| serde_json::json!({
                            "resource": resource,
                            "versionConstraint": version_constraint
                        }))
                        .collect::<Vec<_>>()
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap();

        writer.start_file(".meta.json", options).unwrap();
        writer.write_all(br#"{"files":[]}"#).unwrap();

        for (entry_name, content) in entries {
            writer.start_file(*entry_name, options).unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }

        writer.finish().unwrap();
    }
}
