use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::api::{
    ApiError, AppState, CompiledWorkspaceFile, DEFAULT_KERML_TEMPLATE, DEFAULT_SYSML_TEMPLATE,
    EditorFileContentDto, EditorFileListDto, EditorFormatResponseDto, EditorLintResponseDto,
    EditorOutlineKey, EditorOutlineNodeDto, EditorParseResponseDto, EditorRefreshResponseDto,
    EditorSemanticCompileResponseDto, ElementDetailsDto, EvaluateExpressionRequestDto,
    EvaluateExpressionResponseDto, GraphDto, GraphScope, L2ExplorerGraphDto, L2ExplorerRequestDto,
    LibraryTreeNodeDto, MetatypeExplorerGraphDto, MetatypeExplorerRequestDto, ModelMetadataDto,
    MountedLibraryTreeDto, ProjectLintFileResultDto, ProjectLintResponseDto, SearchResultDto,
    SemanticCompileFileResultDto, SemanticProjectCompileResponseDto, SemanticWorkspaceFileDto,
    SemanticWorkspaceSessionDto, StagedEditorFileDto, WorkspaceProjectInfoDto,
    WorkspaceProjectLibraryDto, build_editor_outline, build_editor_outline_index_for_graph,
    build_library_tree_from_document, collect_editor_files, is_model_source_file,
    lint_diagnostic_to_dto, normalize_relative_path, resolve_workspace_file,
    resolve_workspace_scope, source_language_for_path,
    value_type_label,
};
use mercurio_core::diagrams::{DiagramRenderRequestDto, DiagramViewDto};
use mercurio_core::frontend::ast::{Declaration, PackageDecl, SysmlModule};
use mercurio_core::frontend::format::format_path_text;
use mercurio_core::frontend::lint::lint_text;
use mercurio_core::ir::{KirDocument, load_model_stack};
use mercurio_core::logging::{log_runtime_event, log_timed_event};
use mercurio_core::paths::default_stdlib_rulepack_path;
use mercurio_core::project::{ResolvedProjectContext, resolve_project_context};
use mercurio_core::runtime::{ExecutionContext, Runtime};
use mercurio_core::source_set::{
    SourceCompileContext, SourceDocument, collect_context_modules, parse_source_module,
};
use mercurio_core::views::RequirementTableViewDto;

const MAX_SEMANTIC_COMPILE_CACHE_ENTRIES: usize = 512;

#[derive(Debug, Clone)]
pub enum CompileProjectScopeOutcome {
    Completed(SemanticProjectCompileResponseDto),
    Cancelled {
        project_path: String,
        completed_files: usize,
        total_files: usize,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SemanticCompileCacheStats {
    pub entries: usize,
    pub capacity: usize,
    pub last_hits: usize,
    pub last_misses: usize,
}

#[derive(Debug)]
pub struct WorkspaceService {
    workspace_root: PathBuf,
    active_model_path: Option<PathBuf>,
    project_info: WorkspaceProjectInfoDto,
    mounted_library_trees: Vec<MountedLibraryTreeDto>,
    library_context_document: KirDocument,
    model_state: WorkspaceModelState,
    source_cache: RwLock<HashMap<String, CachedSourceFile>>,
    semantic_cache: RwLock<HashMap<SemanticCompileCacheKey, CompiledWorkspaceFile>>,
    semantic_cache_stats: RwLock<SemanticCompileCacheStats>,
}

#[derive(Debug, Clone)]
enum WorkspaceModelState {
    Shell,
    Fallback(AppState),
    Compiled(CompiledProject),
}

#[derive(Debug, Clone)]
struct CompiledProject {
    files: HashMap<String, CompiledWorkspaceFile>,
    app_state: AppState,
}

type WorkspaceSourceFile = SourceDocument;

#[derive(Debug, Clone)]
struct CachedSourceFile {
    content: String,
    document: SourceDocument,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SemanticCompileCacheKey {
    path: String,
    content_hash: u64,
    dependency_hash: u64,
    rulepack_hash: u64,
}

#[derive(Debug, Clone)]
struct SourceDependencyFingerprints {
    by_path: HashMap<String, u64>,
}

impl CompiledProject {
    fn build_app_state(
        files: &HashMap<String, CompiledWorkspaceFile>,
        library_context_document: &KirDocument,
    ) -> Result<AppState, ApiError> {
        let start = std::time::Instant::now();
        log_runtime_event(format!(
            "workspace.build_app_state start files={} semantic_files={}",
            files.len(),
            files
                .values()
                .filter(|file| file.document.is_some())
                .count()
        ));
        let merged_document = KirDocument::merge(
            std::iter::once(library_context_document.clone())
                .chain(files.values().filter_map(|file| file.document.clone())),
        )?;
        log_runtime_event(format!(
            "workspace.build_app_state merged_document_elements={}",
            merged_document.elements.len()
        ));
        let runtime = Runtime::from_document(merged_document)?;
        log_runtime_event(format!(
            "workspace.build_app_state runtime_graph nodes={} edges={}",
            runtime.graph().elements().len(),
            runtime.graph().edge_count()
        ));
        let app_state = AppState::from_runtime(&runtime, library_context_document);
        log_timed_event(
            "workspace.build_app_state",
            start,
            "ok",
            format!(
                "graph_nodes={} graph_edges={}",
                app_state.graph_ref().elements().len(),
                app_state.graph_ref().edge_count()
            ),
        );
        Ok(app_state)
    }

    fn semantic_workspace_file(&self, relative_path: &str) -> Option<SemanticWorkspaceFileDto> {
        self.files
            .get(relative_path)
            .map(|file| file.to_semantic_workspace_file_dto_with_outline(false))
    }

    fn workspace_file(&self, relative_path: &str) -> Option<&CompiledWorkspaceFile> {
        self.files.get(relative_path)
    }

    fn compile_response_for_scope(&self, project_path: &str) -> SemanticProjectCompileResponseDto {
        let scope_prefix = format!("{project_path}/");
        let mut results = self
            .files
            .iter()
            .filter(|(path, _)| {
                project_path == "."
                    || path.as_str() == project_path
                    || path.starts_with(&scope_prefix)
            })
            .map(|(_, file)| {
                let response = file.to_semantic_workspace_file_dto_with_outline(false);
                SemanticCompileFileResultDto {
                    path: response.path,
                    semantic_status: response.semantic_status,
                    ok: response.semantic_ok,
                    diagnostics: response.diagnostics,
                    element_count: file
                        .document
                        .as_ref()
                        .map(|document| document.elements.len()),
                    semantic_outline: Vec::new(),
                }
            })
            .collect::<Vec<_>>();
        results.sort_by(|left, right| left.path.cmp(&right.path));

        let file_count = results.len();
        let success_count = results.iter().filter(|result| result.ok).count();
        let failure_count = file_count.saturating_sub(success_count);
        SemanticProjectCompileResponseDto {
            ok: failure_count == 0,
            project_path: project_path.to_string(),
            file_count,
            success_count,
            failure_count,
            results,
        }
    }
}

impl WorkspaceService {
    fn new_semantic_cache_stats() -> RwLock<SemanticCompileCacheStats> {
        RwLock::new(SemanticCompileCacheStats {
            capacity: MAX_SEMANTIC_COMPILE_CACHE_ENTRIES,
            ..SemanticCompileCacheStats::default()
        })
    }

    fn new_source_cache() -> RwLock<HashMap<String, CachedSourceFile>> {
        RwLock::new(HashMap::new())
    }

    fn new_semantic_cache() -> RwLock<HashMap<SemanticCompileCacheKey, CompiledWorkspaceFile>> {
        RwLock::new(HashMap::new())
    }

    fn clear_source_cache(&self) {
        self.source_cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.semantic_cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.set_semantic_cache_stats(0, 0);
    }

    fn load_workspace_source_files_cached(
        &self,
        scope_root: &Path,
        overrides: &HashMap<String, String>,
    ) -> Result<Vec<WorkspaceSourceFile>, ApiError> {
        let start = std::time::Instant::now();
        log_runtime_event(format!(
            "workspace.load_workspace_source_files_cached start root={} scope_root={} overrides={}",
            self.workspace_root.display(),
            scope_root.display(),
            overrides.len()
        ));
        let mut discovered_files = Vec::new();
        collect_editor_files(&self.workspace_root, scope_root, &mut discovered_files)?;
        discovered_files.retain(|file| is_model_source_file(Path::new(&file.path)));
        discovered_files.sort_by(|left, right| left.path.cmp(&right.path));
        let discovered_count = discovered_files.len();

        let mut source_files = Vec::with_capacity(discovered_files.len() + overrides.len());
        let mut seen_paths = HashSet::new();
        let mut cache_hits = 0usize;
        let mut cache_misses = 0usize;

        for file in discovered_files {
            let absolute_path = resolve_workspace_file(&self.workspace_root, &file.path)?;
            let content = overrides
                .get(&file.path)
                .cloned()
                .unwrap_or(std::fs::read_to_string(absolute_path)?);
            seen_paths.insert(file.path.clone());

            let (document, hit) = self.source_document_from_cache(file.path, content);
            if hit {
                cache_hits += 1;
            } else {
                cache_misses += 1;
            }
            source_files.push(document);
        }

        for (relative_path, content) in overrides {
            if !is_model_source_file(Path::new(relative_path)) {
                continue;
            }
            if seen_paths.contains(relative_path) {
                continue;
            }

            let absolute_path = resolve_workspace_file(&self.workspace_root, relative_path)?;
            if !absolute_path.starts_with(scope_root) {
                continue;
            }

            let normalized_path = normalize_relative_path(
                absolute_path
                    .strip_prefix(&self.workspace_root)
                    .expect("override path should remain under workspace root"),
            );
            let (document, hit) = self.source_document_from_cache(normalized_path, content.clone());
            if hit {
                cache_hits += 1;
            } else {
                cache_misses += 1;
            }
            source_files.push(document);
        }

        source_files.sort_by(|left, right| left.path.cmp(&right.path));
        log_timed_event(
            "workspace.load_workspace_source_files_cached",
            start,
            "ok",
            format!(
                "scope_root={} discovered={} returned={} cache_hits={} cache_misses={}",
                scope_root.display(),
                discovered_count,
                source_files.len(),
                cache_hits,
                cache_misses
            ),
        );
        Ok(source_files)
    }

    fn source_document_from_cache(
        &self,
        path: String,
        content: String,
    ) -> (WorkspaceSourceFile, bool) {
        {
            let cache = self
                .source_cache
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(cached) = cache.get(&path) {
                if cached.content == content {
                    return (cached.document.clone(), true);
                }
            }
        }

        let document = WorkspaceSourceFile::new(path.clone(), content.clone());
        let mut cache = self
            .source_cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.insert(
            path,
            CachedSourceFile {
                content,
                document: document.clone(),
            },
        );
        (document, false)
    }

    fn compile_project_from_workspace(&self) -> Result<CompiledProject, ApiError> {
        let start = std::time::Instant::now();
        log_runtime_event(format!(
            "workspace.compile_project start root={}",
            self.workspace_root.display()
        ));
        let source_files =
            self.load_workspace_source_files_cached(&self.workspace_root, &HashMap::new())?;
        let compile_context = SourceCompileContext::from_source_documents(
            &source_files,
            &self.library_context_document,
        )?;
        log_runtime_event(format!(
            "workspace.compile_project discovered_files={} context_modules={} root={}",
            source_files.len(),
            compile_context.context_modules.len(),
            self.workspace_root.display()
        ));

        let dependency_fingerprints =
            SourceDependencyFingerprints::from_source_files(&source_files);
        let mut files = HashMap::new();
        let mut semantic_cache_hits = 0usize;
        let mut semantic_cache_misses = 0usize;
        for file in &source_files {
            let file_start = std::time::Instant::now();
            log_runtime_event(format!(
                "workspace.compile_file start path={} bytes={}",
                file.path,
                file.content.len()
            ));
            let (compiled, cache_hit) =
                self.compile_source_file_cached(file, &compile_context, &dependency_fingerprints);
            if cache_hit {
                semantic_cache_hits += 1;
            } else {
                semantic_cache_misses += 1;
            }
            log_timed_event(
                "workspace.compile_file",
                file_start,
                if cache_hit {
                    "cache_hit"
                } else if compiled.semantic_ok() {
                    "ok"
                } else {
                    "error"
                },
                format!(
                    "path={} diagnostics={} has_document={} semantic_cache_hit={}",
                    file.path,
                    compiled.diagnostic_count(),
                    compiled.document.is_some(),
                    cache_hit
                ),
            );
            files.insert(file.path.clone(), compiled);
        }

        let app_state = CompiledProject::build_app_state(&files, &self.library_context_document)?;
        log_timed_event(
            "workspace.compile_project",
            start,
            "ok",
            format!(
                "root={} files={} app_graph_nodes={} semantic_cache_hits={} semantic_cache_misses={}",
                self.workspace_root.display(),
                files.len(),
                app_state.graph_ref().elements().len(),
                semantic_cache_hits,
                semantic_cache_misses
            ),
        );
        self.set_semantic_cache_stats(semantic_cache_hits, semantic_cache_misses);
        Ok(CompiledProject { files, app_state })
    }

    fn compile_source_file_cached(
        &self,
        file: &SourceDocument,
        compile_context: &SourceCompileContext,
        dependency_fingerprints: &SourceDependencyFingerprints,
    ) -> (CompiledWorkspaceFile, bool) {
        let key = semantic_compile_cache_key(file, dependency_fingerprints.for_source_file(file));
        {
            let cache = self
                .semantic_cache
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(compiled) = cache.get(&key) {
                return (compiled.clone(), true);
            }
        }

        let compiled = CompiledWorkspaceFile::from_source_with_context(
            file,
            compile_context,
            &self.library_context_document,
        );
        let mut cache = self
            .semantic_cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if cache.len() >= MAX_SEMANTIC_COMPILE_CACHE_ENTRIES {
            if let Some(key) = cache.keys().next().cloned() {
                cache.remove(&key);
            }
        }
        cache.insert(key, compiled.clone());
        (compiled, false)
    }

    fn set_semantic_cache_stats(&self, last_hits: usize, last_misses: usize) {
        let entries = self
            .semantic_cache
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len();
        *self
            .semantic_cache_stats
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = SemanticCompileCacheStats {
            entries,
            capacity: MAX_SEMANTIC_COMPILE_CACHE_ENTRIES,
            last_hits,
            last_misses,
        };
    }

    fn compiled_project(&self) -> Option<&CompiledProject> {
        match &self.model_state {
            WorkspaceModelState::Compiled(project) => Some(project),
            WorkspaceModelState::Shell | WorkspaceModelState::Fallback(_) => None,
        }
    }

    fn has_compiled_project(&self) -> bool {
        self.compiled_project().is_some()
    }

    fn app_state_opt(&self) -> Option<&AppState> {
        match &self.model_state {
            WorkspaceModelState::Compiled(project) => Some(&project.app_state),
            WorkspaceModelState::Fallback(app_state) => Some(app_state),
            WorkspaceModelState::Shell => None,
        }
    }
}

impl WorkspaceService {
    pub fn from_open_path(path: &Path) -> Result<Self, ApiError> {
        let start = std::time::Instant::now();
        log_runtime_event(format!(
            "workspace.from_open_path start path={} kind={}",
            path.display(),
            if path.is_dir() { "dir" } else { "file" }
        ));
        if path.is_dir() {
            let result = Self::from_workspace_root(path);
            log_timed_event(
                "workspace.from_open_path",
                start,
                if result.is_ok() { "ok" } else { "error" },
                format!("path={} branch=workspace_root", path.display()),
            );
            return result;
        }

        let result = Self::from_model_path(path);
        log_timed_event(
            "workspace.from_open_path",
            start,
            if result.is_ok() { "ok" } else { "error" },
            format!("path={} branch=model_path", path.display()),
        );
        result
    }

    pub fn from_open_path_lazy_model_sources(path: &Path) -> Result<Self, ApiError> {
        let start = std::time::Instant::now();
        log_runtime_event(format!(
            "workspace.from_open_path_lazy_model_sources start path={} kind={}",
            path.display(),
            if path.is_dir() { "dir" } else { "file" }
        ));
        if path.is_dir() {
            let result = Self::from_workspace_root_shell(path);
            log_timed_event(
                "workspace.from_open_path_lazy_model_sources",
                start,
                if result.is_ok() { "ok" } else { "error" },
                format!("path={} branch=workspace_root", path.display()),
            );
            return result;
        }

        if is_model_source_file(path) {
            let result = Self::from_model_source_path_lazy(path);
            log_timed_event(
                "workspace.from_open_path_lazy_model_sources",
                start,
                if result.is_ok() { "ok" } else { "error" },
                format!("path={} branch=model_source_lazy", path.display()),
            );
            return result;
        }

        let result = Self::from_model_path(path);
        log_timed_event(
            "workspace.from_open_path_lazy_model_sources",
            start,
            if result.is_ok() { "ok" } else { "error" },
            format!("path={} branch=model_path", path.display()),
        );
        result
    }

    pub fn from_model_path(model_path: &Path) -> Result<Self, ApiError> {
        let start = std::time::Instant::now();
        log_runtime_event(format!(
            "workspace.from_model_path start path={}",
            model_path.display()
        ));
        let project = resolve_project_context(model_path)?;
        let project_info = build_project_info(&project);
        let mounted_library_trees = build_mounted_library_trees(&project)?;
        let library_context_document = project.library_context_document;
        let workspace_root = project.workspace_root;
        std::fs::create_dir_all(&workspace_root)?;

        let active_model_path = if is_model_source_file(model_path) {
            Some(model_path.to_path_buf())
        } else {
            None
        };

        let should_compile_project = is_model_source_file(model_path);
        let model_state = if !should_compile_project {
            let document = load_model_stack(model_path)?;
            log_runtime_event(format!(
                "workspace.from_model_path loaded_stack elements={}",
                document.elements.len()
            ));
            let runtime = Runtime::from_document(document)?;
            WorkspaceModelState::Fallback(AppState::from_runtime(
                &runtime,
                &library_context_document,
            ))
        } else {
            WorkspaceModelState::Shell
        };

        let mut service = Self {
            workspace_root,
            active_model_path,
            project_info,
            mounted_library_trees,
            library_context_document,
            model_state,
            source_cache: Self::new_source_cache(),
            semantic_cache: Self::new_semantic_cache(),
            semantic_cache_stats: Self::new_semantic_cache_stats(),
        };
        if should_compile_project {
            service.model_state =
                WorkspaceModelState::Compiled(service.compile_project_from_workspace()?);
        }
        log_timed_event(
            "workspace.from_model_path",
            start,
            "ok",
            format!(
                "path={} workspace_root={} compiled_project={} active_path={}",
                model_path.display(),
                service.workspace_root.display(),
                service.has_compiled_project(),
                service.active_editor_path().unwrap_or_default()
            ),
        );
        Ok(service)
    }

    pub fn from_model_source_path_lazy(model_path: &Path) -> Result<Self, ApiError> {
        let start = std::time::Instant::now();
        log_runtime_event(format!(
            "workspace.from_model_source_path_lazy start path={}",
            model_path.display()
        ));
        let project = resolve_project_context(model_path)?;
        let project_info = build_project_info(&project);
        let library_context_document = project.library_context_document;
        let workspace_root = project.workspace_root;
        std::fs::create_dir_all(&workspace_root)?;

        let service = Self {
            workspace_root,
            active_model_path: Some(model_path.to_path_buf()),
            project_info,
            mounted_library_trees: Vec::new(),
            library_context_document,
            model_state: WorkspaceModelState::Shell,
            source_cache: Self::new_source_cache(),
            semantic_cache: Self::new_semantic_cache(),
            semantic_cache_stats: Self::new_semantic_cache_stats(),
        };
        log_timed_event(
            "workspace.from_model_source_path_lazy",
            start,
            "ok",
            format!(
                "path={} workspace_root={} active_path={}",
                model_path.display(),
                service.workspace_root.display(),
                service.active_editor_path().unwrap_or_default()
            ),
        );
        Ok(service)
    }

    pub fn from_workspace_root(workspace_root: &Path) -> Result<Self, ApiError> {
        let start = std::time::Instant::now();
        log_runtime_event(format!(
            "workspace.from_workspace_root start root={}",
            workspace_root.display()
        ));
        let project = resolve_project_context(workspace_root)?;
        std::fs::create_dir_all(&project.workspace_root)?;
        let project_info = build_project_info(&project);
        let mounted_library_trees = build_mounted_library_trees(&project)?;
        let workspace_root = project.workspace_root;
        let library_context_document = project.library_context_document;
        let fallback_app_state = Self::build_library_fallback_app_state(&library_context_document)?;

        let service = Self {
            workspace_root,
            active_model_path: None,
            project_info,
            mounted_library_trees,
            library_context_document,
            model_state: WorkspaceModelState::Fallback(fallback_app_state),
            source_cache: Self::new_source_cache(),
            semantic_cache: Self::new_semantic_cache(),
            semantic_cache_stats: Self::new_semantic_cache_stats(),
        };
        log_timed_event(
            "workspace.from_workspace_root",
            start,
            "ok",
            format!(
                "root={} files={} mode=lazy",
                service.workspace_root.display(),
                service
                    .list_editor_files()
                    .map(|files| files.files.len())
                    .unwrap_or_default()
            ),
        );
        Ok(service)
    }

    pub fn from_workspace_root_shell(workspace_root: &Path) -> Result<Self, ApiError> {
        let start = std::time::Instant::now();
        log_runtime_event(format!(
            "workspace.from_workspace_root_shell start root={}",
            workspace_root.display()
        ));
        let project = resolve_project_context(workspace_root)?;
        std::fs::create_dir_all(&project.workspace_root)?;
        let project_info = build_project_info(&project);

        let service = Self {
            workspace_root: project.workspace_root,
            active_model_path: None,
            project_info,
            mounted_library_trees: Vec::new(),
            library_context_document: project.library_context_document,
            model_state: WorkspaceModelState::Shell,
            source_cache: Self::new_source_cache(),
            semantic_cache: Self::new_semantic_cache(),
            semantic_cache_stats: Self::new_semantic_cache_stats(),
        };
        log_timed_event(
            "workspace.from_workspace_root_shell",
            start,
            "ok",
            format!("root={} mode=shell", service.workspace_root.display()),
        );
        Ok(service)
    }

    pub fn from_workspace_root_diagnostics_only(workspace_root: &Path) -> Result<Self, ApiError> {
        let start = std::time::Instant::now();
        log_runtime_event(format!(
            "workspace.from_workspace_root_diagnostics_only start root={}",
            workspace_root.display()
        ));
        let project = resolve_project_context(workspace_root)?;
        std::fs::create_dir_all(&project.workspace_root)?;
        let project_info = build_project_info(&project);
        let mounted_library_trees = build_mounted_library_trees(&project)?;

        let service = Self {
            workspace_root: project.workspace_root,
            active_model_path: None,
            project_info,
            mounted_library_trees,
            library_context_document: project.library_context_document,
            model_state: WorkspaceModelState::Shell,
            source_cache: Self::new_source_cache(),
            semantic_cache: Self::new_semantic_cache(),
            semantic_cache_stats: Self::new_semantic_cache_stats(),
        };
        log_timed_event(
            "workspace.from_workspace_root_diagnostics_only",
            start,
            "ok",
            format!("root={}", service.workspace_root.display()),
        );
        Ok(service)
    }

    pub fn from_workspace_root_compiled(workspace_root: &Path) -> Result<Self, ApiError> {
        let start = std::time::Instant::now();
        log_runtime_event(format!(
            "workspace.from_workspace_root_compiled start root={}",
            workspace_root.display()
        ));
        let project = resolve_project_context(workspace_root)?;
        std::fs::create_dir_all(&project.workspace_root)?;
        let project_info = build_project_info(&project);
        let mounted_library_trees = build_mounted_library_trees(&project)?;
        let workspace_root = project.workspace_root;
        let library_context_document = project.library_context_document;
        let mut service = Self {
            workspace_root,
            active_model_path: None,
            project_info,
            mounted_library_trees,
            library_context_document,
            model_state: WorkspaceModelState::Shell,
            source_cache: Self::new_source_cache(),
            semantic_cache: Self::new_semantic_cache(),
            semantic_cache_stats: Self::new_semantic_cache_stats(),
        };
        service.model_state =
            WorkspaceModelState::Compiled(service.compile_project_from_workspace()?);
        log_timed_event(
            "workspace.from_workspace_root_compiled",
            start,
            "ok",
            format!(
                "root={} files={}",
                service.workspace_root.display(),
                service
                    .compiled_project()
                    .map(|project| project.files.len())
                    .unwrap_or_default()
            ),
        );
        Ok(service)
    }

    fn build_library_fallback_app_state(
        library_context_document: &KirDocument,
    ) -> Result<AppState, ApiError> {
        let runtime = Runtime::from_document(library_context_document.clone())?;
        Ok(AppState::from_runtime(&runtime, library_context_document))
    }

    pub fn model_metadata(&self) -> ModelMetadataDto {
        self.app_state_opt()
            .map(|app_state| app_state.metadata().clone())
            .unwrap_or_else(|| shell_model_metadata(&self.library_context_document))
    }

    pub fn semantic_compile_cache_stats(&self) -> SemanticCompileCacheStats {
        let mut stats = *self
            .semantic_cache_stats
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        stats.entries = self
            .semantic_cache
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len();
        stats
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn active_path(&self) -> Option<String> {
        self.active_editor_path()
    }

    pub fn project_info(&self) -> WorkspaceProjectInfoDto {
        self.project_info.clone()
    }

    pub fn graph(&self, scope: &str) -> GraphDto {
        self.app_state().graph(GraphScope::from_query(Some(scope)))
    }

    pub fn library_tree(&self) -> Vec<LibraryTreeNodeDto> {
        self.app_state().library_tree()
    }

    pub fn mounted_library_trees(&self) -> Vec<MountedLibraryTreeDto> {
        self.mounted_library_trees.clone()
    }

    pub fn load_mounted_library_trees(&mut self) -> Result<Vec<MountedLibraryTreeDto>, ApiError> {
        if self.mounted_library_trees.is_empty() {
            let project = resolve_project_context(&self.workspace_root)?;
            self.mounted_library_trees = build_mounted_library_trees(&project)?;
        }
        Ok(self.mounted_library_trees.clone())
    }

    pub fn metatype_explorer(
        &self,
        request: &MetatypeExplorerRequestDto,
    ) -> Result<MetatypeExplorerGraphDto, ApiError> {
        self.app_state().metatype_explorer(request)
    }

    pub fn l2_explorer(
        &self,
        request: &L2ExplorerRequestDto,
    ) -> Result<L2ExplorerGraphDto, ApiError> {
        self.app_state().l2_explorer(request)
    }

    pub fn render_diagram(
        &self,
        request: DiagramRenderRequestDto,
    ) -> Result<DiagramViewDto, ApiError> {
        self.app_state().render_diagram(request)
    }

    pub fn requirements_table(&self) -> RequirementTableViewDto {
        self.app_state().requirements_table()
    }

    pub fn element(&self, id: &str) -> Result<ElementDetailsDto, ApiError> {
        self.app_state()
            .element(id)
            .ok_or_else(|| ApiError::MissingElement(id.to_string()))
    }

    pub fn search(&self, query: &str) -> Vec<SearchResultDto> {
        self.app_state().search(query)
    }

    pub fn evaluate_expression(
        &self,
        request: &EvaluateExpressionRequestDto,
    ) -> EvaluateExpressionResponseDto {
        self.app_state().evaluate_expression(request)
    }

    pub fn list_editor_files(&self) -> Result<EditorFileListDto, ApiError> {
        let mut files = Vec::new();
        collect_editor_files(&self.workspace_root, &self.workspace_root, &mut files)?;
        files.sort_by(|left, right| left.path.cmp(&right.path));

        Ok(EditorFileListDto {
            workspace_root: normalize_relative_path(&self.workspace_root),
            active_path: self.active_editor_path(),
            files,
        })
    }

    pub fn semantic_workspace_session(&self) -> Result<SemanticWorkspaceSessionDto, ApiError> {
        let start = std::time::Instant::now();
        log_runtime_event(format!(
            "workspace.semantic_workspace_session start root={} compiled_project={}",
            self.workspace_root.display(),
            self.has_compiled_project()
        ));
        if let Some(project) = self.compiled_project() {
            let mut files = project
                .files
                .values()
                .map(|file| file.to_semantic_workspace_file_dto_with_outline(true))
                .collect::<Vec<_>>();
            files.sort_by(|left, right| left.path.cmp(&right.path));

            let session = SemanticWorkspaceSessionDto {
                workspace_root: normalize_relative_path(&self.workspace_root),
                active_path: self.active_editor_path(),
                project: self.project_info(),
                files,
            };
            log_timed_event(
                "workspace.semantic_workspace_session",
                start,
                "ok",
                format!(
                    "root={} files={} mode=compiled_project",
                    self.workspace_root.display(),
                    session.files.len()
                ),
            );
            return Ok(session);
        }

        let mut files = Vec::new();
        collect_editor_files(&self.workspace_root, &self.workspace_root, &mut files)?;
        files.sort_by(|left, right| left.path.cmp(&right.path));

        let files = files
            .into_iter()
            .filter(|file| is_model_source_file(Path::new(&file.path)))
            .map(|file| self.semantic_workspace_file(&file.path))
            .collect::<Result<Vec<_>, _>>()?;

        let session = SemanticWorkspaceSessionDto {
            workspace_root: normalize_relative_path(&self.workspace_root),
            active_path: self.active_editor_path(),
            project: self.project_info(),
            files,
        };
        log_timed_event(
            "workspace.semantic_workspace_session",
            start,
            "ok",
            format!(
                "root={} files={} mode=ad_hoc",
                self.workspace_root.display(),
                session.files.len()
            ),
        );
        Ok(session)
    }

    pub fn read_editor_file(&self, relative_path: &str) -> Result<EditorFileContentDto, ApiError> {
        let path = resolve_workspace_file(&self.workspace_root, relative_path)?;
        if !path.exists() {
            return Err(ApiError::MissingEditorFile(relative_path.to_string()));
        }
        Ok(EditorFileContentDto {
            path: normalize_relative_path(
                path.strip_prefix(&self.workspace_root)
                    .expect("editor path should remain under root"),
            ),
            content: std::fs::read_to_string(path)?,
        })
    }

    pub fn create_editor_file(
        &mut self,
        relative_path: &str,
        template: Option<&str>,
    ) -> Result<EditorFileContentDto, ApiError> {
        let path = resolve_workspace_file(&self.workspace_root, relative_path)?;
        if path.exists() {
            return Err(ApiError::AlreadyExists(relative_path.to_string()));
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = template
            .unwrap_or_else(|| default_template_for_path(&path))
            .to_string();
        std::fs::write(&path, &content)?;
        let source_name = normalize_relative_path(
            path.strip_prefix(&self.workspace_root)
                .expect("editor path should remain under root"),
        );
        if is_model_source_file(&path) && self.has_compiled_project() {
            self.clear_source_cache();
            self.rebuild_compiled_project()?;
        }

        Ok(EditorFileContentDto {
            path: source_name,
            content,
        })
    }

    pub fn write_editor_file(
        &mut self,
        relative_path: &str,
        content: &str,
    ) -> Result<(), ApiError> {
        let path = resolve_workspace_file(&self.workspace_root, relative_path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        if is_model_source_file(&path) && self.has_compiled_project() {
            self.clear_source_cache();
            self.rebuild_compiled_project()?;
        }
        Ok(())
    }

    pub fn parse_editor_content(
        &self,
        relative_path: &str,
        content: &str,
    ) -> Result<EditorParseResponseDto, ApiError> {
        let path = resolve_workspace_file(&self.workspace_root, relative_path)?;
        let source_name = normalize_relative_path(
            path.strip_prefix(&self.workspace_root)
                .expect("editor path should remain under root"),
        );

        let compiled =
            self.compile_workspace_file(&source_name, Some(content), &self.workspace_root)?;
        let semantic = compiled.to_semantic_workspace_file_dto_with_outline(false);
        Ok(EditorParseResponseDto {
            ok: semantic.semantic_ok,
            diagnostics: semantic.diagnostics,
            element_count: compiled
                .document
                .as_ref()
                .map(|document| document.elements.len()),
        })
    }

    pub fn format_editor_content(
        &self,
        relative_path: &str,
        content: &str,
    ) -> Result<EditorFormatResponseDto, ApiError> {
        let path = resolve_workspace_file(&self.workspace_root, relative_path)?;
        let source_name = normalize_relative_path(
            path.strip_prefix(&self.workspace_root)
                .expect("editor path should remain under root"),
        );
        let formatted_content = format_path_text(Path::new(&source_name), content)
            .map_err(|err| ApiError::InvalidPath(format!("{relative_path}: {err}")))?;

        Ok(EditorFormatResponseDto {
            path: source_name,
            formatted_content,
        })
    }

    pub fn compile_editor_semantic_content(
        &self,
        relative_path: &str,
        content: &str,
    ) -> Result<EditorSemanticCompileResponseDto, ApiError> {
        let path = resolve_workspace_file(&self.workspace_root, relative_path)?;
        let source_name = normalize_relative_path(
            path.strip_prefix(&self.workspace_root)
                .expect("editor path should remain under root"),
        );

        let compiled =
            self.compile_workspace_file(&source_name, Some(content), &self.workspace_root)?;
        Ok(EditorSemanticCompileResponseDto {
            path: source_name,
            semantic_status: compiled.semantic_status_name().to_string(),
            ok: compiled.semantic_ok(),
            diagnostics: compiled
                .to_semantic_workspace_file_dto_with_outline(false)
                .diagnostics,
            element_count: compiled
                .document
                .as_ref()
                .map(|document| document.elements.len()),
            semantic_outline: compiled.semantic_outline(),
        })
    }

    pub(crate) fn compile_editor_semantic_file(
        &self,
        relative_path: &str,
        content: &str,
    ) -> Result<CompiledWorkspaceFile, ApiError> {
        let path = resolve_workspace_file(&self.workspace_root, relative_path)?;
        let source_name = normalize_relative_path(
            path.strip_prefix(&self.workspace_root)
                .expect("resolved editor path must be inside workspace"),
        );
        self.compile_workspace_file(&source_name, Some(content), &self.workspace_root)
    }

    pub(crate) fn evaluate_compiled_expression(
        &self,
        compiled: &CompiledWorkspaceFile,
        request: &EvaluateExpressionRequestDto,
    ) -> EvaluateExpressionResponseDto {
        let document = match compiled.document.as_ref() {
            Some(document) => document,
            None => {
                return EvaluateExpressionResponseDto {
                    ok: false,
                    value: None,
                    value_type: None,
                    explanation: Vec::new(),
                    error: Some("snippet did not produce an evaluatable semantic document".to_string()),
                };
            }
        };
        let merged_document = match KirDocument::merge([
            self.library_context_document.clone(),
            document.clone(),
        ]) {
            Ok(document) => document,
            Err(err) => {
                return EvaluateExpressionResponseDto {
                    ok: false,
                    value: None,
                    value_type: None,
                    explanation: Vec::new(),
                    error: Some(err.to_string()),
                };
            }
        };
        let runtime = match Runtime::from_document(merged_document) {
            Ok(runtime) => runtime,
            Err(err) => {
                return EvaluateExpressionResponseDto {
                    ok: false,
                    value: None,
                    value_type: None,
                    explanation: Vec::new(),
                    error: Some(err.to_string()),
                };
            }
        };
        let mut context = ExecutionContext::default();
        for (owner_id, values) in &request.context_values {
            for (feature, value) in values {
                context
                    .values
                    .insert((owner_id.clone(), feature.clone()), value.clone());
            }
        }

        match runtime.evaluate(&request.feature_id, &request.owner_id, &context) {
            Ok(result) => EvaluateExpressionResponseDto {
                ok: true,
                value_type: Some(value_type_label(&result.value).to_string()),
                value: Some(result.value),
                explanation: result.explanation,
                error: None,
            },
            Err(err) => EvaluateExpressionResponseDto {
                ok: false,
                value: None,
                value_type: None,
                explanation: Vec::new(),
                error: Some(err.to_string()),
            },
        }
    }

    pub fn lint_editor_content(
        &self,
        relative_path: &str,
        content: &str,
    ) -> Result<EditorLintResponseDto, ApiError> {
        let path = resolve_workspace_file(&self.workspace_root, relative_path)?;
        let source_name = normalize_relative_path(
            path.strip_prefix(&self.workspace_root)
                .expect("editor path should remain under root"),
        );

        let mut overrides = HashMap::new();
        overrides.insert(source_name.clone(), content.to_string());
        let source_files =
            self.load_workspace_source_files_cached(&self.workspace_root, &overrides)?;
        let context_modules = collect_context_modules(&source_files);
        let file = source_files
            .iter()
            .find(|file| file.path == source_name)
            .ok_or_else(|| ApiError::MissingEditorFile(relative_path.to_string()))?;
        let language = source_language_for_path(&file.path)
            .ok_or_else(|| ApiError::InvalidPath(relative_path.to_string()))?;
        let report = lint_text(
            &file.content,
            &file.path,
            language,
            &context_modules,
            &self.library_context_document,
        );

        Ok(EditorLintResponseDto {
            path: source_name,
            ok: !report.has_errors(),
            diagnostics: report
                .diagnostics
                .iter()
                .map(lint_diagnostic_to_dto)
                .collect(),
        })
    }

    pub fn editor_outline(
        &self,
        relative_path: &str,
    ) -> Result<Vec<EditorOutlineNodeDto>, ApiError> {
        let path = resolve_workspace_file(&self.workspace_root, relative_path)?;
        if !path.exists() {
            return Err(ApiError::MissingEditorFile(relative_path.to_string()));
        }
        let source_name = normalize_relative_path(
            path.strip_prefix(&self.workspace_root)
                .expect("editor path should remain under root"),
        );
        let content = std::fs::read_to_string(path)?;
        let module = parse_source_module(&source_name, &content)?;
        let element_index = self.editor_outline_element_index();
        Ok(build_editor_outline(&source_name, &module, &element_index))
    }

    pub fn editor_semantic_outline(
        &self,
        relative_path: &str,
    ) -> Result<Vec<EditorOutlineNodeDto>, ApiError> {
        let path = resolve_workspace_file(&self.workspace_root, relative_path)?;
        if !path.exists() {
            return Err(ApiError::MissingEditorFile(relative_path.to_string()));
        }
        let source_name = normalize_relative_path(
            path.strip_prefix(&self.workspace_root)
                .expect("editor path should remain under root"),
        );
        if let Some(project) = self.compiled_project() {
            return project
                .workspace_file(&source_name)
                .map(CompiledWorkspaceFile::semantic_outline)
                .ok_or_else(|| ApiError::MissingEditorFile(relative_path.to_string()));
        }
        Ok(self
            .compile_workspace_file(&source_name, None, &self.workspace_root)?
            .semantic_outline())
    }

    pub fn semantic_workspace_file(
        &self,
        relative_path: &str,
    ) -> Result<SemanticWorkspaceFileDto, ApiError> {
        let path = resolve_workspace_file(&self.workspace_root, relative_path)?;
        if !path.exists() {
            return Err(ApiError::MissingEditorFile(relative_path.to_string()));
        }
        let source_name = normalize_relative_path(
            path.strip_prefix(&self.workspace_root)
                .expect("editor path should remain under root"),
        );
        if let Some(project) = self.compiled_project() {
            return project
                .semantic_workspace_file(&source_name)
                .ok_or_else(|| ApiError::MissingEditorFile(relative_path.to_string()));
        }
        Ok(self
            .compile_workspace_file(&source_name, None, &self.workspace_root)?
            .to_semantic_workspace_file_dto())
    }

    pub fn semantic_element(
        &self,
        relative_path: &str,
        id: &str,
    ) -> Result<ElementDetailsDto, ApiError> {
        let path = resolve_workspace_file(&self.workspace_root, relative_path)?;
        if !path.exists() {
            return Err(ApiError::MissingEditorFile(relative_path.to_string()));
        }
        let source_name = normalize_relative_path(
            path.strip_prefix(&self.workspace_root)
                .expect("editor path should remain under root"),
        );
        if let Some(project) = self.compiled_project() {
            return project
                .workspace_file(&source_name)
                .ok_or_else(|| ApiError::MissingEditorFile(relative_path.to_string()))
                .and_then(|_| {
                    project
                        .app_state
                        .element(id)
                        .ok_or_else(|| ApiError::MissingElement(id.to_string()))
                });
        }
        let project = self.compile_project_from_workspace()?;

        project
            .app_state
            .element(id)
            .ok_or_else(|| ApiError::MissingElement(id.to_string()))
    }

    pub fn refresh_from_editor_path(
        &mut self,
        relative_path: &str,
    ) -> Result<EditorRefreshResponseDto, ApiError> {
        let path = resolve_workspace_file(&self.workspace_root, relative_path)?;
        if !path.exists() {
            return Err(ApiError::MissingEditorFile(relative_path.to_string()));
        }
        let source_name = normalize_relative_path(
            path.strip_prefix(&self.workspace_root)
                .expect("editor path should remain under root"),
        );
        if self.has_compiled_project() {
            self.rebuild_compiled_project()?;
        }
        self.active_model_path = Some(path);

        Ok(EditorRefreshResponseDto {
            ok: true,
            active_path: source_name,
        })
    }

    pub fn compile_project_scope(
        &self,
        project_path: &str,
        staged_files: &[StagedEditorFileDto],
    ) -> Result<SemanticProjectCompileResponseDto, ApiError> {
        match self.compile_project_scope_cancelable(
            project_path,
            staged_files,
            || false,
            |_, _| {},
        )? {
            CompileProjectScopeOutcome::Completed(response) => Ok(response),
            CompileProjectScopeOutcome::Cancelled { .. } => unreachable!(
                "compile_project_scope cannot be cancelled without a cancellation predicate"
            ),
        }
    }

    pub fn compile_project_scope_diagnostics_only(
        &self,
        project_path: &str,
        staged_files: &[StagedEditorFileDto],
    ) -> Result<SemanticProjectCompileResponseDto, ApiError> {
        let start = std::time::Instant::now();
        log_runtime_event(format!(
            "workspace.compile_project_scope_diagnostics start root={} project_path={} staged_files={}",
            self.workspace_root.display(),
            project_path,
            staged_files.len()
        ));
        let scope_root = resolve_workspace_scope(&self.workspace_root, project_path)?;
        let staged_files_by_path = staged_files
            .iter()
            .map(|file| (file.path.clone(), file.content.clone()))
            .collect::<HashMap<_, _>>();
        let source_files =
            self.load_workspace_source_files_cached(&scope_root, &staged_files_by_path)?;
        let compile_context = SourceCompileContext::from_source_documents(
            &source_files,
            &self.library_context_document,
        )?;
        let dependency_fingerprints =
            SourceDependencyFingerprints::from_source_files(&source_files);

        let total_files = source_files.len();
        let mut results = Vec::with_capacity(total_files);
        let mut success_count = 0;
        let mut semantic_cache_hits = 0usize;
        let mut semantic_cache_misses = 0usize;

        for file in &source_files {
            let file_start = std::time::Instant::now();
            let (compiled, cache_hit) =
                self.compile_source_file_cached(file, &compile_context, &dependency_fingerprints);
            if cache_hit {
                semantic_cache_hits += 1;
            } else {
                semantic_cache_misses += 1;
            }
            log_timed_event(
                "workspace.compile_project_scope_diagnostics.file",
                file_start,
                if cache_hit {
                    "cache_hit"
                } else if compiled.semantic_ok() {
                    "ok"
                } else {
                    "error"
                },
                format!(
                    "path={} diagnostics={} has_document={} semantic_cache_hit={}",
                    file.path,
                    compiled.diagnostic_count(),
                    compiled.document.is_some(),
                    cache_hit
                ),
            );

            let response = compiled.to_semantic_workspace_file_dto_with_outline(false);
            if response.semantic_ok {
                success_count += 1;
            }
            results.push(SemanticCompileFileResultDto {
                path: response.path,
                semantic_status: response.semantic_status,
                ok: response.semantic_ok,
                diagnostics: response.diagnostics,
                element_count: compiled
                    .document
                    .as_ref()
                    .map(|document| document.elements.len()),
                semantic_outline: Vec::new(),
            });
        }

        let file_count = results.len();
        let failure_count = file_count.saturating_sub(success_count);
        let response = SemanticProjectCompileResponseDto {
            ok: failure_count == 0,
            project_path: project_path.to_string(),
            file_count,
            success_count,
            failure_count,
            results,
        };
        log_timed_event(
            "workspace.compile_project_scope_diagnostics",
            start,
            if response.ok { "ok" } else { "error" },
            format!(
                "project_path={} files={} success={} failure={} semantic_cache_hits={} semantic_cache_misses={}",
                project_path,
                file_count,
                success_count,
                failure_count,
                semantic_cache_hits,
                semantic_cache_misses
            ),
        );
        self.set_semantic_cache_stats(semantic_cache_hits, semantic_cache_misses);
        Ok(response)
    }

    pub fn lint_project_scope(
        &self,
        project_path: &str,
        staged_files: &[StagedEditorFileDto],
    ) -> Result<ProjectLintResponseDto, ApiError> {
        let scope_root = resolve_workspace_scope(&self.workspace_root, project_path)?;
        let staged_files_by_path = staged_files
            .iter()
            .map(|file| (file.path.clone(), file.content.clone()))
            .collect::<HashMap<_, _>>();
        let source_files =
            self.load_workspace_source_files_cached(&scope_root, &staged_files_by_path)?;
        let context_modules = collect_context_modules(&source_files);

        let mut results = Vec::with_capacity(source_files.len());
        let mut success_count = 0;

        for file in source_files {
            let Some(language) = source_language_for_path(&file.path) else {
                continue;
            };
            let report = lint_text(
                &file.content,
                &file.path,
                language,
                &context_modules,
                &self.library_context_document,
            );
            let ok = !report.has_errors();
            if ok {
                success_count += 1;
            }
            results.push(ProjectLintFileResultDto {
                path: file.path,
                ok,
                diagnostics: report
                    .diagnostics
                    .iter()
                    .map(lint_diagnostic_to_dto)
                    .collect(),
            });
        }

        let file_count = results.len();
        let failure_count = file_count.saturating_sub(success_count);
        Ok(ProjectLintResponseDto {
            ok: failure_count == 0,
            project_path: project_path.to_string(),
            file_count,
            success_count,
            failure_count,
            results,
        })
    }

    pub fn compile_project_scope_cancelable<F, G>(
        &self,
        project_path: &str,
        staged_files: &[StagedEditorFileDto],
        should_cancel: F,
        on_progress: G,
    ) -> Result<CompileProjectScopeOutcome, ApiError>
    where
        F: Fn() -> bool,
        G: Fn(usize, usize),
    {
        let start = std::time::Instant::now();
        log_runtime_event(format!(
            "workspace.compile_project_scope start root={} project_path={} staged_files={}",
            self.workspace_root.display(),
            project_path,
            staged_files.len()
        ));
        let scope_root = resolve_workspace_scope(&self.workspace_root, project_path)?;
        let mut files = Vec::new();
        collect_editor_files(&self.workspace_root, &scope_root, &mut files)?;
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let staged_files_by_path = staged_files
            .iter()
            .map(|file| (file.path.clone(), file.content.clone()))
            .collect::<HashMap<_, _>>();
        let staged_file_paths = staged_files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<HashSet<_>>();
        let source_files =
            self.load_workspace_source_files_cached(&scope_root, &staged_files_by_path)?;
        let compile_context = SourceCompileContext::from_source_documents(
            &source_files,
            &self.library_context_document,
        )?;
        let dependency_fingerprints =
            SourceDependencyFingerprints::from_source_files(&source_files);

        let total_files = source_files.len();
        let mut results = Vec::with_capacity(total_files);
        let mut success_count = 0;
        let mut completed_files = 0;
        let mut semantic_cache_hits = 0usize;
        let mut semantic_cache_misses = 0usize;

        for file in &source_files {
            if should_cancel() {
                log_timed_event(
                    "workspace.compile_project_scope",
                    start,
                    "cancelled",
                    format!(
                        "project_path={} completed={} total={}",
                        project_path, completed_files, total_files
                    ),
                );
                return Ok(CompileProjectScopeOutcome::Cancelled {
                    project_path: project_path.to_string(),
                    completed_files,
                    total_files,
                });
            }

            let file_start = std::time::Instant::now();
            let (compiled, cache_hit) =
                self.compile_source_file_cached(file, &compile_context, &dependency_fingerprints);
            if cache_hit {
                semantic_cache_hits += 1;
            } else {
                semantic_cache_misses += 1;
            }
            log_timed_event(
                "workspace.compile_project_scope.file",
                file_start,
                if cache_hit {
                    "cache_hit"
                } else if compiled.semantic_ok() {
                    "ok"
                } else {
                    "error"
                },
                format!(
                    "path={} diagnostics={} has_document={} semantic_cache_hit={}",
                    file.path,
                    compiled.diagnostic_count(),
                    compiled.document.is_some(),
                    cache_hit
                ),
            );

            let response = compiled.to_semantic_workspace_file_dto_with_outline(false);
            if response.semantic_ok {
                success_count += 1;
            }
            results.push(SemanticCompileFileResultDto {
                path: response.path,
                semantic_status: response.semantic_status,
                ok: response.semantic_ok,
                diagnostics: response.diagnostics,
                element_count: compiled
                    .document
                    .as_ref()
                    .map(|document| document.elements.len()),
                semantic_outline: if staged_file_paths.contains(file.path.as_str()) {
                    compiled.semantic_outline()
                } else {
                    Vec::new()
                },
            });
            completed_files += 1;
            on_progress(completed_files, total_files);
        }

        let file_count = results.len();
        let failure_count = file_count.saturating_sub(success_count);
        let response = SemanticProjectCompileResponseDto {
            ok: failure_count == 0,
            project_path: project_path.to_string(),
            file_count,
            success_count,
            failure_count,
            results,
        };
        log_timed_event(
            "workspace.compile_project_scope",
            start,
            if response.ok { "ok" } else { "error" },
            format!(
                "project_path={} files={} success={} failure={} semantic_cache_hits={} semantic_cache_misses={}",
                project_path,
                response.file_count,
                response.success_count,
                response.failure_count,
                semantic_cache_hits,
                semantic_cache_misses
            ),
        );
        self.set_semantic_cache_stats(semantic_cache_hits, semantic_cache_misses);
        Ok(CompileProjectScopeOutcome::Completed(response))
    }

    pub fn compiled_project_scope_response(
        &self,
        project_path: &str,
    ) -> Result<SemanticProjectCompileResponseDto, ApiError> {
        let scope_root = resolve_workspace_scope(&self.workspace_root, project_path)?;
        let Some(project) = self.compiled_project() else {
            return Ok(SemanticProjectCompileResponseDto {
                ok: true,
                project_path: project_path.to_string(),
                file_count: 0,
                success_count: 0,
                failure_count: 0,
                results: Vec::new(),
            });
        };
        let response = project.compile_response_for_scope(project_path);
        log_runtime_event(format!(
            "workspace.compiled_project_scope_response project_path={} scope_root={} files={} success={} failure={}",
            project_path,
            scope_root.display(),
            response.file_count,
            response.success_count,
            response.failure_count
        ));
        Ok(response)
    }

    pub(crate) fn app_state(&self) -> &AppState {
        self.app_state_opt()
            .expect("workspace service should always expose an app state")
    }

    pub fn replace_compiled_state_from(&mut self, compiled_workspace: Self) {
        if self.workspace_root != compiled_workspace.workspace_root {
            return;
        }

        self.project_info = compiled_workspace.project_info;
        self.mounted_library_trees = compiled_workspace.mounted_library_trees;
        self.library_context_document = compiled_workspace.library_context_document;
        self.model_state = compiled_workspace.model_state;
    }

    fn active_editor_path(&self) -> Option<String> {
        self.active_model_path
            .as_ref()
            .and_then(|path| path.strip_prefix(&self.workspace_root).ok())
            .map(normalize_relative_path)
    }

    fn rebuild_compiled_project(&mut self) -> Result<(), ApiError> {
        let start = std::time::Instant::now();
        log_runtime_event(format!(
            "workspace.rebuild_compiled_project start root={}",
            self.workspace_root.display()
        ));
        self.model_state = WorkspaceModelState::Compiled(self.compile_project_from_workspace()?);
        log_timed_event(
            "workspace.rebuild_compiled_project",
            start,
            "ok",
            format!(
                "root={} files={}",
                self.workspace_root.display(),
                self.compiled_project()
                    .map(|project| project.files.len())
                    .unwrap_or_default()
            ),
        );
        Ok(())
    }

    fn editor_outline_element_index(&self) -> HashMap<EditorOutlineKey, String> {
        if let Some(project) = self.compiled_project() {
            return build_editor_outline_index_for_graph(project.app_state.graph_ref());
        }
        if let Some(app_state) = self.app_state_opt() {
            return build_editor_outline_index_for_graph(app_state.graph_ref());
        }
        HashMap::new()
    }

    fn compile_workspace_file(
        &self,
        relative_path: &str,
        override_content: Option<&str>,
        scope_root: &Path,
    ) -> Result<CompiledWorkspaceFile, ApiError> {
        let mut overrides = HashMap::new();
        if let Some(content) = override_content {
            overrides.insert(relative_path.to_string(), content.to_string());
        }

        let source_files = self.load_workspace_source_files_cached(scope_root, &overrides)?;
        let compile_context = SourceCompileContext::from_source_documents(
            &source_files,
            &self.library_context_document,
        )?;
        let file = source_files
            .iter()
            .find(|file| file.path == relative_path)
            .ok_or_else(|| ApiError::MissingEditorFile(relative_path.to_string()))?;

        Ok(CompiledWorkspaceFile::from_source_with_context(
            file,
            &compile_context,
            &self.library_context_document,
        ))
    }
}

pub fn load_workspace_service(model_path: &Path) -> Result<WorkspaceService, ApiError> {
    WorkspaceService::from_model_path(model_path)
}

fn build_project_info(project: &ResolvedProjectContext) -> WorkspaceProjectInfoDto {
    let mut libraries = project
        .resolved_libraries
        .iter()
        .map(|library| WorkspaceProjectLibraryDto {
            id: library.id.clone(),
            role: library.role,
            source_kind: library.source_kind.clone(),
            source_identity: library
                .cache_metadata
                .as_ref()
                .map(|metadata| metadata.source_identity.clone()),
            source_path: library
                .source_path
                .as_ref()
                .map(|path| path.display().to_string()),
            source_version: library
                .cache_metadata
                .as_ref()
                .and_then(|metadata| metadata.source_version.clone()),
            source_digest: library
                .cache_metadata
                .as_ref()
                .and_then(|metadata| metadata.source_digest.clone()),
            cache_path: library
                .cache_path
                .as_ref()
                .map(|path| path.display().to_string()),
            cached_element_count: library.cached_element_count,
        })
        .collect::<Vec<_>>();
    libraries.sort_by(|left, right| {
        left.id
            .cmp(&right.id)
            .then(left.source_kind.cmp(&right.source_kind))
    });

    WorkspaceProjectInfoDto {
        name: project
            .descriptor
            .as_ref()
            .and_then(|descriptor| descriptor.name.clone()),
        descriptor_path: project
            .descriptor_path
            .as_ref()
            .map(|path| path.display().to_string()),
        descriptor: project.descriptor.clone(),
        libraries,
    }
}

fn build_mounted_library_trees(
    project: &ResolvedProjectContext,
) -> Result<Vec<MountedLibraryTreeDto>, ApiError> {
    let mut mounted_library_trees = project
        .resolved_libraries
        .iter()
        .map(|library| {
            Ok(MountedLibraryTreeDto {
                library_id: library.id.clone(),
                nodes: build_library_tree_from_document(&library.document)?,
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    mounted_library_trees.sort_by(|left, right| left.library_id.cmp(&right.library_id));
    Ok(mounted_library_trees)
}

impl SourceDependencyFingerprints {
    fn from_source_files(source_files: &[SourceDocument]) -> Self {
        let mut package_providers = Vec::new();
        for file in source_files {
            let content_hash = stable_hash(&file.content);
            if let Some(module) = file.module.as_ref() {
                for package_name in module_package_names(module) {
                    package_providers.push((package_name, file.path.clone(), content_hash));
                }
            }
        }
        package_providers.sort_by(|left, right| {
            right
                .0
                .len()
                .cmp(&left.0.len())
                .then(left.0.cmp(&right.0))
                .then(left.1.cmp(&right.1))
        });

        let mut by_path = HashMap::new();
        for file in source_files {
            let mut hasher = DefaultHasher::new();
            file.path.hash(&mut hasher);

            let mut dependency_paths = HashSet::new();
            if let Some(module) = file.module.as_ref() {
                for import_name in module_import_names(module) {
                    for (package_name, provider_path, provider_hash) in &package_providers {
                        if provider_path == &file.path {
                            continue;
                        }
                        if import_depends_on_package(&import_name, package_name) {
                            dependency_paths.insert(provider_path.clone());
                            package_name.hash(&mut hasher);
                            provider_path.hash(&mut hasher);
                            provider_hash.hash(&mut hasher);
                        }
                    }
                }
            }

            let dependency_hash = hasher.finish();
            by_path.insert(file.path.clone(), dependency_hash);
        }

        Self { by_path }
    }

    fn for_source_file(&self, file: &SourceDocument) -> u64 {
        self.by_path
            .get(&file.path)
            .copied()
            .unwrap_or_else(|| stable_hash(&file.path))
    }
}

fn semantic_compile_cache_key(
    file: &SourceDocument,
    dependency_hash: u64,
) -> SemanticCompileCacheKey {
    SemanticCompileCacheKey {
        path: file.path.clone(),
        content_hash: stable_hash(&file.content),
        dependency_hash,
        rulepack_hash: default_rulepack_hash(),
    }
}

fn module_package_names(module: &SysmlModule) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(package) = module.package.as_ref() {
        collect_package_names(package, &mut names);
    }
    for member in &module.members {
        collect_declaration_package_names(member, &mut names);
    }
    names.sort();
    names.dedup();
    names
}

fn collect_declaration_package_names(declaration: &Declaration, names: &mut Vec<String>) {
    match declaration {
        Declaration::Package(package) => collect_package_names(package, names),
        Declaration::PartDefinition(definition) => {
            for member in &definition.members {
                collect_declaration_package_names(member, names);
            }
        }
        Declaration::GenericDefinition(definition) => {
            for member in &definition.members {
                collect_declaration_package_names(member, names);
            }
        }
        Declaration::PartUsage(usage) => {
            for member in &usage.body_members {
                collect_declaration_package_names(member, names);
            }
        }
        Declaration::GenericUsage(usage) => {
            for member in &usage.body_members {
                collect_declaration_package_names(member, names);
            }
        }
        Declaration::Import(_) | Declaration::Alias(_) => {}
    }
}

fn collect_package_names(package: &PackageDecl, names: &mut Vec<String>) {
    names.push(package.name.as_colon_string());
    for member in &package.members {
        collect_declaration_package_names(member, names);
    }
}

fn module_import_names(module: &SysmlModule) -> Vec<String> {
    let mut names = Vec::new();
    for import in &module.imports {
        names.push(import.path.as_colon_string());
    }
    if let Some(package) = module.package.as_ref() {
        collect_package_import_names(package, &mut names);
    }
    for member in &module.members {
        collect_declaration_import_names(member, &mut names);
    }
    names.sort();
    names.dedup();
    names
}

fn collect_declaration_import_names(declaration: &Declaration, names: &mut Vec<String>) {
    match declaration {
        Declaration::Package(package) => collect_package_import_names(package, names),
        Declaration::Import(import) => names.push(import.path.as_colon_string()),
        Declaration::PartDefinition(definition) => {
            for member in &definition.members {
                collect_declaration_import_names(member, names);
            }
        }
        Declaration::GenericDefinition(definition) => {
            for member in &definition.members {
                collect_declaration_import_names(member, names);
            }
        }
        Declaration::PartUsage(usage) => {
            for member in &usage.body_members {
                collect_declaration_import_names(member, names);
            }
        }
        Declaration::GenericUsage(usage) => {
            for member in &usage.body_members {
                collect_declaration_import_names(member, names);
            }
        }
        Declaration::Alias(_) => {}
    }
}

fn collect_package_import_names(package: &PackageDecl, names: &mut Vec<String>) {
    for import in &package.imports {
        names.push(import.path.as_colon_string());
    }
    for member in &package.members {
        collect_declaration_import_names(member, names);
    }
}

fn import_depends_on_package(import_name: &str, package_name: &str) -> bool {
    import_name == package_name
        || import_name
            .strip_prefix(package_name)
            .is_some_and(|suffix| suffix.starts_with("::"))
        || package_name
            .strip_prefix(import_name)
            .is_some_and(|suffix| suffix.starts_with("::"))
}

fn shell_model_metadata(library_context_document: &KirDocument) -> ModelMetadataDto {
    let layers = library_context_document
        .elements
        .iter()
        .map(|element| element.layer)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    ModelMetadataDto {
        element_count: library_context_document.elements.len(),
        edge_count: 0,
        library_element_count: library_context_document.elements.len(),
        user_element_count: 0,
        library_version: library_context_document
            .metadata
            .get("stdlib_version")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        layers,
        relations: Vec::new(),
        graph_scopes: vec![
            GraphScope::L2.as_str().to_string(),
            GraphScope::L2PlusContext.as_str().to_string(),
            GraphScope::Full.as_str().to_string(),
        ],
        default_graph_scope: GraphScope::L2.as_str().to_string(),
    }
}

fn stable_hash<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn default_rulepack_hash() -> u64 {
    let path = default_stdlib_rulepack_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => stable_hash(&content),
        Err(_) => stable_hash(&path.display().to_string()),
    }
}

fn default_template_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|value| value.to_str()) {
        Some("kerml") => DEFAULT_KERML_TEMPLATE,
        Some("py") => "from pathlib import Path\n\nprint('Workspace files:')\nfor path in sorted(Path('.').glob('**/*')):\n    if path.is_file() and '.mercurio' not in path.parts:\n        print(path)\n",
        _ => DEFAULT_SYSML_TEMPLATE,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{SourceDependencyFingerprints, WorkspaceService};
    use mercurio_core::source_set::SourceDocument;

    #[test]
    fn workspace_service_exposes_mounted_library_trees_per_library() {
        let root = temp_workspace_root("mounted_library_trees");
        let model_path = root.join("model.sysml");
        let library_dir = temp_workspace_root("mounted_library_dependency").join("domain-lib");
        std::fs::create_dir_all(&library_dir).unwrap();
        std::fs::write(&model_path, "package Demo {\n}\n").unwrap();
        std::fs::write(
            library_dir.join("domain.sysml"),
            "package Domain {\n  part def Thing;\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join(mercurio_core::project::PROJECT_DESCRIPTOR_FILE_NAME),
            serde_json::to_string_pretty(&serde_json::json!({
                "version": 1,
                "libraries": [
                    {
                        "id": "domain-lib",
                        "provider": {
                            "kind": "sysml_directory",
                            "path": library_dir.display().to_string()
                        }
                    }
                ]
            }))
            .unwrap(),
        )
        .unwrap();

        let workspace = WorkspaceService::from_workspace_root(&root).unwrap();
        let mounted_library_trees = workspace.mounted_library_trees();

        assert!(
            mounted_library_trees
                .iter()
                .any(|tree| tree.library_id == "stdlib" && !tree.nodes.is_empty())
        );
        assert!(mounted_library_trees.iter().any(|tree| {
            tree.library_id == "domain-lib"
                && tree
                    .nodes
                    .iter()
                    .any(|node| contains_element_id(node, "type.Domain.Thing"))
        }));

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(library_dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn create_editor_file_uses_kerml_template_for_kerml_paths() {
        let root = temp_workspace_root("create_kerml_template");
        let mut workspace = WorkspaceService::from_workspace_root(&root).unwrap();

        let created = workspace
            .create_editor_file("models/new_model.kerml", None)
            .unwrap();

        assert_eq!(created.path, "models/new_model.kerml");
        assert!(created.content.contains("classifier Vehicle"));
        assert!(created.content.contains("feature engine : Engine"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dependency_fingerprint_tracks_direct_import_providers() {
        let provider = SourceDocument::new(
            "provider.sysml",
            "package Provider {
                part def Thing;
            }",
        );
        let consumer = SourceDocument::new(
            "consumer.sysml",
            "package Consumer {
                import Provider::*;
                part p : Thing;
            }",
        );
        let unrelated = SourceDocument::new(
            "unrelated.sysml",
            "package Unrelated {
                part def OtherThing;
            }",
        );
        let baseline = SourceDependencyFingerprints::from_source_files(&[
            provider.clone(),
            consumer.clone(),
            unrelated,
        ]);
        let consumer_baseline = baseline.for_source_file(&consumer);

        let unrelated_edit = SourceDocument::new(
            "unrelated.sysml",
            "package Unrelated {
                part def OtherThing;
                part def AddedThing;
            }",
        );
        let unrelated_fingerprints = SourceDependencyFingerprints::from_source_files(&[
            provider.clone(),
            consumer.clone(),
            unrelated_edit,
        ]);
        assert_eq!(
            consumer_baseline,
            unrelated_fingerprints.for_source_file(&consumer)
        );

        let provider_edit = SourceDocument::new(
            "provider.sysml",
            "package Provider {
                part def Thing;
                part def AddedThing;
            }",
        );
        let provider_fingerprints =
            SourceDependencyFingerprints::from_source_files(&[provider_edit, consumer.clone()]);
        assert_ne!(
            consumer_baseline,
            provider_fingerprints.for_source_file(&consumer)
        );
    }

    #[test]
    fn lazy_model_source_open_sets_active_path_without_compile() {
        let root = temp_workspace_root("lazy_model_source_open");
        let model_path = root.join("model.sysml");
        std::fs::write(&model_path, "package Demo {\n  part def Thing;\n}\n").unwrap();

        let workspace = WorkspaceService::from_open_path_lazy_model_sources(&model_path).unwrap();

        assert_eq!(workspace.active_path().as_deref(), Some("model.sysml"));
        assert_eq!(workspace.semantic_compile_cache_stats().entries, 0);
        let metadata = workspace.model_metadata();
        assert_eq!(metadata.user_element_count, 0);
        assert!(metadata.library_element_count > 0);
        let outline = workspace.editor_outline("model.sysml").unwrap();
        assert_eq!(outline.len(), 1);
        assert_eq!(outline[0].label, "Demo");
        assert_eq!(outline[0].element_id, None);

        std::fs::remove_dir_all(root).unwrap();
    }

    fn temp_workspace_root(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "mercurio_workspace_{label}_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn contains_element_id(
        node: &crate::api::LibraryTreeNodeDto,
        expected_element_id: &str,
    ) -> bool {
        node.element_id.as_deref() == Some(expected_element_id)
            || node
                .children
                .iter()
                .any(|child| contains_element_id(child, expected_element_id))
    }
}
