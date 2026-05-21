use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::ir::{KIR_SCHEMA_VERSION, KirDocument, KirError};
use crate::runtime::{Runtime, RuntimeArtifact};
use crate::source_set::{SourceDocument, compile_source_documents};

const CACHE_SCHEMA_VERSION: u32 = 2;
const ARTIFACT_FAMILY_COMPILE: &str = "compile";
const DOCUMENT_FILE_NAME: &str = "document.kir.json";
const MANIFEST_FILE_NAME: &str = "manifest.json";
const RUNTIME_ARTIFACT_FILE_NAME: &str = "runtime-artifact.json";

#[derive(Debug, Clone)]
pub struct PersistentProjectCache {
    root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PersistentCacheStatus {
    FreshCompile,
    PersistentHit,
    PersistentMiss,
    PersistentRejected { reason: String },
}

#[derive(Debug, Clone)]
pub struct PersistentCompileResult {
    pub document: KirDocument,
    pub runtime_artifact: RuntimeArtifact,
    pub cache_status: PersistentCacheStatus,
    pub artifact_key: String,
    pub cache_write_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSourceFileFingerprint {
    pub path: String,
    pub size_bytes: usize,
    pub content_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectCompileArtifactKey {
    pub source_authority: String,
    pub source_tree_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_descriptor_digest: Option<String>,
    pub compiler_digest: String,
    pub kir_schema_version: String,
    pub library_context_digest: String,
    pub mapping_rules_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectCompileCacheManifest {
    pub cache_schema_version: u32,
    pub artifact_family: String,
    pub artifact_key: String,
    pub key: ProjectCompileArtifactKey,
    pub files: Vec<ProjectSourceFileFingerprint>,
    pub outputs: ProjectCompileCacheOutputs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectCompileCacheOutputs {
    pub kir: String,
    pub runtime_artifact: String,
}

enum CacheLookup {
    Hit {
        document: KirDocument,
        runtime_artifact: RuntimeArtifact,
    },
    Miss,
    Rejected(String),
}

impl PersistentProjectCache {
    pub fn for_workspace_root(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            root: workspace_root
                .into()
                .join(".mercurio")
                .join("cache")
                .join("project"),
        }
    }

    pub fn from_cache_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn compile_source_documents(
        &self,
        source_documents: Vec<SourceDocument>,
        library_context: &KirDocument,
        project_descriptor_path: Option<&Path>,
    ) -> Result<PersistentCompileResult, KirError> {
        let (key, files) = project_compile_artifact_key(
            &source_documents,
            library_context,
            project_descriptor_path,
        )?;
        let artifact_key = artifact_key_digest(&key)?;

        match self.load_compile_artifact(&artifact_key, &key, &files)? {
            CacheLookup::Hit {
                document,
                runtime_artifact,
            } => {
                return Ok(PersistentCompileResult {
                    document,
                    runtime_artifact,
                    cache_status: PersistentCacheStatus::PersistentHit,
                    artifact_key,
                    cache_write_error: None,
                });
            }
            CacheLookup::Miss => {
                let document = compile_source_documents(source_documents, library_context)?;
                let runtime_artifact = runtime_artifact_for_document(&document, library_context)?;
                let cache_write_error = self
                    .write_compile_artifact(
                        &artifact_key,
                        &key,
                        &files,
                        &document,
                        &runtime_artifact,
                    )
                    .err()
                    .map(|err| err.to_string());
                return Ok(PersistentCompileResult {
                    document,
                    runtime_artifact,
                    cache_status: PersistentCacheStatus::PersistentMiss,
                    artifact_key,
                    cache_write_error,
                });
            }
            CacheLookup::Rejected(reason) => {
                let document = compile_source_documents(source_documents, library_context)?;
                let runtime_artifact = runtime_artifact_for_document(&document, library_context)?;
                let cache_write_error = self
                    .write_compile_artifact(
                        &artifact_key,
                        &key,
                        &files,
                        &document,
                        &runtime_artifact,
                    )
                    .err()
                    .map(|err| err.to_string());
                return Ok(PersistentCompileResult {
                    document,
                    runtime_artifact,
                    cache_status: PersistentCacheStatus::PersistentRejected { reason },
                    artifact_key,
                    cache_write_error,
                });
            }
        }
    }

    fn artifact_dir(&self, artifact_key: &str) -> PathBuf {
        self.root
            .join("artifacts")
            .join(safe_cache_segment(artifact_key))
    }

    fn load_compile_artifact(
        &self,
        artifact_key: &str,
        key: &ProjectCompileArtifactKey,
        files: &[ProjectSourceFileFingerprint],
    ) -> Result<CacheLookup, KirError> {
        let artifact_dir = self.artifact_dir(artifact_key);
        let manifest_path = artifact_dir.join(MANIFEST_FILE_NAME);
        let document_path = artifact_dir.join(DOCUMENT_FILE_NAME);
        let runtime_artifact_path = artifact_dir.join(RUNTIME_ARTIFACT_FILE_NAME);

        if !manifest_path.is_file() || !document_path.is_file() || !runtime_artifact_path.is_file()
        {
            return Ok(CacheLookup::Miss);
        }

        let manifest = match std::fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|input| serde_json::from_str::<ProjectCompileCacheManifest>(&input).ok())
        {
            Some(manifest) => manifest,
            None => return Ok(CacheLookup::Rejected("manifest is unreadable".to_string())),
        };

        if let Some(reason) = manifest_rejection_reason(&manifest, artifact_key, key, files) {
            return Ok(CacheLookup::Rejected(reason));
        }

        let document = match KirDocument::from_path(&document_path) {
            Ok(document) => document,
            Err(err) => {
                return Ok(CacheLookup::Rejected(format!(
                    "cached KIR is invalid: {err}"
                )));
            }
        };
        let runtime_artifact =
            match serde_json::from_str(&std::fs::read_to_string(&runtime_artifact_path)?) {
                Ok(artifact) => artifact,
                Err(err) => {
                    return Ok(CacheLookup::Rejected(format!(
                        "cached runtime artifact is invalid: {err}"
                    )));
                }
            };
        Ok(CacheLookup::Hit {
            document,
            runtime_artifact,
        })
    }

    fn write_compile_artifact(
        &self,
        artifact_key: &str,
        key: &ProjectCompileArtifactKey,
        files: &[ProjectSourceFileFingerprint],
        document: &KirDocument,
        runtime_artifact: &RuntimeArtifact,
    ) -> Result<(), KirError> {
        let final_dir = self.artifact_dir(artifact_key);
        if final_dir.is_dir() {
            return self.write_compile_artifact_files(
                &final_dir,
                artifact_key,
                key,
                files,
                document,
                runtime_artifact,
            );
        }

        let tmp_dir = self.root.join("tmp").join(format!(
            "{}-{}",
            process_id_segment(),
            unique_cache_nonce()
        ));
        std::fs::create_dir_all(&tmp_dir)?;

        self.write_compile_artifact_files(
            &tmp_dir,
            artifact_key,
            key,
            files,
            document,
            runtime_artifact,
        )?;

        if let Some(parent) = final_dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::rename(&tmp_dir, &final_dir) {
            Ok(()) => Ok(()),
            Err(err) if final_dir.is_dir() => {
                let _ = std::fs::remove_dir_all(&tmp_dir);
                if err.kind() == std::io::ErrorKind::AlreadyExists {
                    Ok(())
                } else {
                    Ok(())
                }
            }
            Err(err) => Err(KirError::Io(err)),
        }
    }

    fn write_compile_artifact_files(
        &self,
        dir: &Path,
        artifact_key: &str,
        key: &ProjectCompileArtifactKey,
        files: &[ProjectSourceFileFingerprint],
        document: &KirDocument,
        runtime_artifact: &RuntimeArtifact,
    ) -> Result<(), KirError> {
        std::fs::create_dir_all(dir)?;
        let manifest = ProjectCompileCacheManifest {
            cache_schema_version: CACHE_SCHEMA_VERSION,
            artifact_family: ARTIFACT_FAMILY_COMPILE.to_string(),
            artifact_key: artifact_key.to_string(),
            key: key.clone(),
            files: files.to_vec(),
            outputs: ProjectCompileCacheOutputs {
                kir: DOCUMENT_FILE_NAME.to_string(),
                runtime_artifact: RUNTIME_ARTIFACT_FILE_NAME.to_string(),
            },
        };
        document.write_pretty_to_path(&dir.join(DOCUMENT_FILE_NAME))?;
        std::fs::write(
            dir.join(RUNTIME_ARTIFACT_FILE_NAME),
            serde_json::to_string_pretty(runtime_artifact)?,
        )?;
        std::fs::write(
            dir.join(MANIFEST_FILE_NAME),
            serde_json::to_string_pretty(&manifest)?,
        )?;

        let roundtrip_manifest: ProjectCompileCacheManifest =
            serde_json::from_str(&std::fs::read_to_string(dir.join(MANIFEST_FILE_NAME))?)?;
        if roundtrip_manifest != manifest {
            return Err(KirError::Sysml(
                "persistent cache manifest failed roundtrip validation".to_string(),
            ));
        }
        KirDocument::from_path(&dir.join(DOCUMENT_FILE_NAME))?;
        let _: RuntimeArtifact = serde_json::from_str(&std::fs::read_to_string(
            dir.join(RUNTIME_ARTIFACT_FILE_NAME),
        )?)?;
        Ok(())
    }
}

pub fn project_compile_artifact_key(
    source_documents: &[SourceDocument],
    library_context: &KirDocument,
    project_descriptor_path: Option<&Path>,
) -> Result<(ProjectCompileArtifactKey, Vec<ProjectSourceFileFingerprint>), KirError> {
    let files = source_file_fingerprints(source_documents);
    let source_tree_digest = digest_source_file_fingerprints(&files);
    let project_descriptor_digest = project_descriptor_path
        .filter(|path| path.is_file())
        .map(digest_file)
        .transpose()?;
    let library_context_digest = digest_json(library_context)?;
    let mapping_rules_digest = mapping_rules_digest()?;

    Ok((
        ProjectCompileArtifactKey {
            source_authority: "local_files".to_string(),
            source_tree_digest,
            project_descriptor_digest,
            compiler_digest: compiler_digest(),
            kir_schema_version: KIR_SCHEMA_VERSION.to_string(),
            library_context_digest,
            mapping_rules_digest,
        },
        files,
    ))
}

pub fn source_file_fingerprints(
    source_documents: &[SourceDocument],
) -> Vec<ProjectSourceFileFingerprint> {
    let mut files = source_documents
        .iter()
        .map(|source| ProjectSourceFileFingerprint {
            path: normalized_source_path(&source.path),
            size_bytes: source.content.len(),
            content_digest: digest_labeled_chunks([(
                "content".as_bytes(),
                source.content.as_bytes(),
            )]),
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files
}

fn manifest_rejection_reason(
    manifest: &ProjectCompileCacheManifest,
    artifact_key: &str,
    key: &ProjectCompileArtifactKey,
    files: &[ProjectSourceFileFingerprint],
) -> Option<String> {
    if manifest.cache_schema_version != CACHE_SCHEMA_VERSION {
        return Some("cache schema version changed".to_string());
    }
    if manifest.artifact_family != ARTIFACT_FAMILY_COMPILE {
        return Some("artifact family does not match compile cache".to_string());
    }
    if manifest.artifact_key != artifact_key {
        return Some("artifact key does not match manifest location".to_string());
    }
    if &manifest.key != key {
        return Some("artifact key inputs changed".to_string());
    }
    if manifest.files != files {
        return Some("source file fingerprints changed".to_string());
    }
    if manifest.outputs.kir != DOCUMENT_FILE_NAME {
        return Some("manifest output path is not recognized".to_string());
    }
    if manifest.outputs.runtime_artifact != RUNTIME_ARTIFACT_FILE_NAME {
        return Some("manifest runtime artifact path is not recognized".to_string());
    }
    None
}

fn runtime_artifact_for_document(
    document: &KirDocument,
    library_context: &KirDocument,
) -> Result<RuntimeArtifact, KirError> {
    let merged_document = KirDocument::merge([library_context.clone(), document.clone()])?;
    Runtime::from_document(merged_document)
        .map(Runtime::into_artifact)
        .map_err(|err| KirError::Sysml(format!("failed to build runtime artifact: {err}")))
}

fn artifact_key_digest(key: &ProjectCompileArtifactKey) -> Result<String, KirError> {
    digest_json(key)
}

fn digest_json<T: Serialize>(value: &T) -> Result<String, KirError> {
    Ok(digest_labeled_chunks([(
        "json".as_bytes(),
        serde_json::to_vec(value)?.as_slice(),
    )]))
}

fn digest_file(path: &Path) -> Result<String, KirError> {
    let bytes = std::fs::read(path)?;
    Ok(digest_labeled_chunks([(
        "file".as_bytes(),
        bytes.as_slice(),
    )]))
}

fn digest_source_file_fingerprints(files: &[ProjectSourceFileFingerprint]) -> String {
    digest_labeled_chunks(files.iter().flat_map(|file| {
        [
            ("path".as_bytes(), file.path.as_bytes()),
            ("content_digest".as_bytes(), file.content_digest.as_bytes()),
        ]
    }))
}

fn mapping_rules_digest() -> Result<String, KirError> {
    #[cfg(target_arch = "wasm32")]
    {
        return Ok(digest_labeled_chunks([
            (
                "mapping".as_bytes(),
                include_bytes!("../../../mappings/l2/pilot_constructs.seed.json").as_slice(),
            ),
            (
                "mapping".as_bytes(),
                include_bytes!("../../../mappings/l2/kir_emission.seed.json").as_slice(),
            ),
        ]));
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut chunks = Vec::new();
        for path in [
            crate::paths::repo_path("mappings/l2/pilot_constructs.seed.json"),
            crate::paths::repo_path("mappings/l2/kir_emission.seed.json"),
        ] {
            let bytes = std::fs::read(&path)?;
            chunks.push(("mapping".as_bytes().to_vec(), bytes));
        }
        Ok(digest_labeled_chunks(chunks.iter().map(
            |(label, bytes)| (label.as_slice(), bytes.as_slice()),
        )))
    }
}

fn compiler_digest() -> String {
    digest_labeled_chunks([
        ("crate".as_bytes(), "mercurio-core".as_bytes()),
        ("version".as_bytes(), env!("CARGO_PKG_VERSION").as_bytes()),
        ("kir_schema".as_bytes(), KIR_SCHEMA_VERSION.as_bytes()),
    ])
}

fn digest_labeled_chunks<'a, I>(chunks: I) -> String
where
    I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
{
    let mut hash = FNV_OFFSET;
    for (label, bytes) in chunks {
        hash = digest_bytes(hash, &(label.len() as u64).to_le_bytes());
        hash = digest_bytes(hash, label);
        hash = digest_bytes(hash, &(bytes.len() as u64).to_le_bytes());
        hash = digest_bytes(hash, bytes);
    }
    format!("fnv1a64:{hash:016x}")
}

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

fn digest_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn normalized_source_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn safe_cache_segment(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(not(target_arch = "wasm32"))]
fn process_id_segment() -> String {
    std::process::id().to_string()
}

#[cfg(target_arch = "wasm32")]
fn process_id_segment() -> String {
    "wasm".to_string()
}

fn unique_cache_nonce() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ counter.rotate_left(17)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::Value;

    use super::{
        PersistentCacheStatus, PersistentProjectCache, ProjectCompileCacheManifest,
        source_file_fingerprints,
    };
    use crate::ir::{KirDocument, KirElement};
    use crate::runtime::Runtime;
    use crate::source_set::SourceDocument;

    #[test]
    fn persistent_compile_cache_reuses_unchanged_artifact() {
        let root = temp_dir("persistent_hit");
        let cache = PersistentProjectCache::for_workspace_root(&root);
        let library_context = test_library_context();
        let sources = vec![SourceDocument::new(
            "demo.sysml",
            "package Demo { part def Thing; }",
        )];

        let first = cache
            .compile_source_documents(sources.clone(), &library_context, None)
            .unwrap();
        let second = cache
            .compile_source_documents(sources, &library_context, None)
            .unwrap();

        assert_eq!(first.cache_status, PersistentCacheStatus::PersistentMiss);
        assert_eq!(second.cache_status, PersistentCacheStatus::PersistentHit);
        assert_eq!(first.document, second.document);
        assert!(second.cache_write_error.is_none());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persistent_compile_cache_hit_preserves_runtime_end_state() {
        let root = temp_dir("persistent_runtime_equivalence");
        let cache = PersistentProjectCache::for_workspace_root(&root);
        let library_context = test_library_context();
        let sources = vec![
            SourceDocument::new("domain.sysml", "package Domain { part def Camera; }"),
            SourceDocument::new(
                "usage.sysml",
                "package Usage {
                  import Domain::*;
                  part camera : Camera;
                }",
            ),
        ];

        let miss = cache
            .compile_source_documents(sources.clone(), &library_context, None)
            .unwrap();
        let hit = cache
            .compile_source_documents(sources, &library_context, None)
            .unwrap();

        assert_eq!(miss.cache_status, PersistentCacheStatus::PersistentMiss);
        assert_eq!(hit.cache_status, PersistentCacheStatus::PersistentHit);
        assert_eq!(miss.document, hit.document);

        let miss_runtime = Runtime::from_artifact(miss.runtime_artifact).unwrap();
        let hit_runtime = Runtime::from_artifact(hit.runtime_artifact).unwrap();

        assert_eq!(
            miss_runtime.graph().elements(),
            hit_runtime.graph().elements()
        );
        assert_eq!(miss_runtime.graph().edges(), hit_runtime.graph().edges());
        assert_eq!(miss_runtime.derived(), hit_runtime.derived());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persistent_compile_cache_invalidates_changed_source() {
        let root = temp_dir("persistent_changed_source");
        let cache = PersistentProjectCache::for_workspace_root(&root);
        let library_context = test_library_context();

        let first_sources = vec![SourceDocument::new(
            "demo.sysml",
            "package Demo { part def Thing; }",
        )];
        let second_sources = vec![SourceDocument::new(
            "demo.sysml",
            "package Demo { part def OtherThing; }",
        )];

        let first = cache
            .compile_source_documents(first_sources, &library_context, None)
            .unwrap();
        let second = cache
            .compile_source_documents(second_sources, &library_context, None)
            .unwrap();

        assert_eq!(first.cache_status, PersistentCacheStatus::PersistentMiss);
        assert_eq!(second.cache_status, PersistentCacheStatus::PersistentMiss);
        assert_ne!(first.artifact_key, second.artifact_key);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persistent_compile_cache_invalidates_changed_descriptor() {
        let root = temp_dir("persistent_changed_descriptor");
        let cache = PersistentProjectCache::for_workspace_root(&root);
        let library_context = test_library_context();
        let descriptor_path = root.join("mercurio-project.json");
        let sources = vec![SourceDocument::new(
            "demo.sysml",
            "package Demo { part def Thing; }",
        )];

        std::fs::write(&descriptor_path, r#"{"version":1,"name":"A"}"#).unwrap();
        let first = cache
            .compile_source_documents(sources.clone(), &library_context, Some(&descriptor_path))
            .unwrap();
        std::fs::write(&descriptor_path, r#"{"version":1,"name":"B"}"#).unwrap();
        let second = cache
            .compile_source_documents(sources, &library_context, Some(&descriptor_path))
            .unwrap();

        assert_eq!(first.cache_status, PersistentCacheStatus::PersistentMiss);
        assert_eq!(second.cache_status, PersistentCacheStatus::PersistentMiss);
        assert_ne!(first.artifact_key, second.artifact_key);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persistent_compile_cache_rejects_corrupt_manifest_and_falls_back() {
        let root = temp_dir("persistent_corrupt_manifest");
        let cache = PersistentProjectCache::for_workspace_root(&root);
        let library_context = test_library_context();
        let sources = vec![SourceDocument::new(
            "demo.sysml",
            "package Demo { part def Thing; }",
        )];

        let first = cache
            .compile_source_documents(sources.clone(), &library_context, None)
            .unwrap();
        let manifest_path = cache
            .root()
            .join("artifacts")
            .join(first.artifact_key.replace(':', "_"))
            .join("manifest.json");
        std::fs::write(&manifest_path, "{ not-json").unwrap();

        let second = cache
            .compile_source_documents(sources, &library_context, None)
            .unwrap();

        assert!(matches!(
            second.cache_status,
            PersistentCacheStatus::PersistentRejected { .. }
        ));
        assert_eq!(first.document, second.document);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persistent_compile_cache_rejects_corrupt_kir_and_falls_back() {
        let root = temp_dir("persistent_corrupt_kir");
        let cache = PersistentProjectCache::for_workspace_root(&root);
        let library_context = test_library_context();
        let sources = vec![SourceDocument::new(
            "demo.sysml",
            "package Demo { part def Thing; }",
        )];

        let first = cache
            .compile_source_documents(sources.clone(), &library_context, None)
            .unwrap();
        let document_path = cache
            .root()
            .join("artifacts")
            .join(first.artifact_key.replace(':', "_"))
            .join("document.kir.json");
        std::fs::write(&document_path, "{ not-json").unwrap();

        let second = cache
            .compile_source_documents(sources, &library_context, None)
            .unwrap();

        assert!(matches!(
            second.cache_status,
            PersistentCacheStatus::PersistentRejected { .. }
        ));
        assert_eq!(first.document, second.document);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_file_fingerprints_are_path_order_independent() {
        let left = vec![
            SourceDocument::new("b.sysml", "package B {}"),
            SourceDocument::new("a.sysml", "package A {}"),
        ];
        let right = vec![
            SourceDocument::new("a.sysml", "package A {}"),
            SourceDocument::new("b.sysml", "package B {}"),
        ];

        assert_eq!(
            source_file_fingerprints(&left),
            source_file_fingerprints(&right)
        );
    }

    #[test]
    fn manifest_roundtrip_keeps_required_key_fields() {
        let manifest = ProjectCompileCacheManifest {
            cache_schema_version: super::CACHE_SCHEMA_VERSION,
            artifact_family: "compile".to_string(),
            artifact_key: "fnv1a64:test".to_string(),
            key: super::ProjectCompileArtifactKey {
                source_authority: "local_files".to_string(),
                source_tree_digest: "fnv1a64:source".to_string(),
                project_descriptor_digest: Some("fnv1a64:descriptor".to_string()),
                compiler_digest: "fnv1a64:compiler".to_string(),
                kir_schema_version: "0.2".to_string(),
                library_context_digest: "fnv1a64:library".to_string(),
                mapping_rules_digest: "fnv1a64:mapping".to_string(),
            },
            files: Vec::new(),
            outputs: super::ProjectCompileCacheOutputs {
                kir: "document.kir.json".to_string(),
                runtime_artifact: "runtime-artifact.json".to_string(),
            },
        };

        let encoded = serde_json::to_string(&manifest).unwrap();
        let decoded: ProjectCompileCacheManifest = serde_json::from_str(&encoded).unwrap();

        assert_eq!(decoded, manifest);
    }

    fn test_library_context() -> KirDocument {
        KirDocument {
            metadata: BTreeMap::from([(
                "source".to_string(),
                Value::String("test-stdlib".to_string()),
            )]),
            elements: vec![
                KirElement {
                    id: "Parts::Part".to_string(),
                    kind: "SysML::PartDefinition".to_string(),
                    layer: 1,
                    properties: BTreeMap::new(),
                },
                KirElement {
                    id: "Items::Item::subparts".to_string(),
                    kind: "SysML::PartUsage".to_string(),
                    layer: 1,
                    properties: BTreeMap::new(),
                },
            ],
        }
    }

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "mercurio_project_cache_{label}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }
}
