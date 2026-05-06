#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::{Deref, DerefMut};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::{
    Arc, Mutex, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard,
    atomic::{AtomicBool, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::workspace::WorkspaceService;
use mercurio_core::ai::{
    AskMercurioProjectContext, AskMercurioRequest, AskMercurioResponse, ChatCompletionRequest,
    ReasoningProvider, ReasoningProviderConfigOverrides, ReasoningProviderKind,
    ReasoningProviderSecretOverrides, ReasoningProviderStatus, SemanticSummaryRequest,
    SemanticSummaryResponse, ask_mercurio_with_config, complete_configured_chat,
    configured_reasoning_provider, summarize_semantic_changes,
    test_configured_reasoning_provider_connection, test_default_reasoning_provider_connection,
};
use mercurio_core::derived::derived_properties;
use mercurio_core::diagrams::{
    DiagramKindDto, DiagramRenderRequestDto, DiagramViewDto, list_diagram_kinds, render_diagram,
};
use mercurio_core::frontend::ast::{
    AliasDecl, Declaration, GenericDefinitionDecl, GenericUsageDecl, ImportDecl, PackageDecl,
    PartDefinitionDecl, PartUsageDecl, SourceSpan, SysmlModule,
};
use mercurio_core::frontend::diagnostics::Diagnostic;
use mercurio_core::frontend::kerml::compile_kerml_module_with_resolver_context;
use mercurio_core::frontend::lint::{LintDiagnostic, LintSeverity, SourceLanguage};
use mercurio_core::frontend::sysml::{
    SemanticCompileStatus, compile_sysml_module_with_resolver_context_report_with_limit,
    compile_sysml_text_with_context_report, partial_compile_attempt_limit,
};
use mercurio_core::graph::{Edge, Graph};
use mercurio_core::ir::{KirDocument, KirElement, KirError};
use mercurio_core::metamodel::{
    AttributeRow, AttributeValueSource, MetamodelAttributeRegistry,
    collect_specialization_ancestors, query_element_attributes,
};
use mercurio_core::project::{ProjectDescriptor, ProjectDescriptorError, ProjectLibraryRole};
use mercurio_core::runtime::{ExecutionContext, Runtime};
use mercurio_core::source_set::{
    SourceCompileContext, SourceDocument, compile_source_document_with_context,
};
use mercurio_core::views::{RequirementTableViewDto, requirements_table_view};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ModelMetadataDto {
    pub element_count: usize,
    pub edge_count: usize,
    pub library_element_count: usize,
    pub user_element_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_version: Option<String>,
    pub layers: Vec<u8>,
    pub relations: Vec<String>,
    pub graph_scopes: Vec<String>,
    pub default_graph_scope: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GraphDto {
    pub nodes: Vec<GraphNodeDto>,
    pub edges: Vec<GraphEdgeDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GraphNodeDto {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub layer: u8,
    pub property_count: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GraphEdgeDto {
    pub id: String,
    pub source: String,
    pub target: String,
    pub relation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetatypeExplorerRequestDto {
    pub seed_id: String,
    #[serde(default)]
    pub expanded_parents: Vec<String>,
    #[serde(default)]
    pub expanded_children: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct L2ExplorerRequestDto {
    pub seed_id: String,
    #[serde(default)]
    pub expanded_parents: Vec<String>,
    #[serde(default)]
    pub expanded_children: Vec<String>,
    #[serde(default = "default_true")]
    pub include_reference_edges: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ExplorerAttributeDto {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MetatypeExplorerGraphDto {
    pub seed_id: String,
    pub nodes: Vec<MetatypeExplorerNodeDto>,
    pub edges: Vec<MetatypeExplorerEdgeDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MetatypeExplorerNodeDto {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub layer: u8,
    pub attributes: Vec<ExplorerAttributeDto>,
    pub specializes_count: usize,
    pub specialized_by_count: usize,
    pub is_seed: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MetatypeExplorerEdgeDto {
    pub id: String,
    pub source: String,
    pub target: String,
    pub relation: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct L2ExplorerGraphDto {
    pub seed_id: String,
    pub nodes: Vec<L2ExplorerNodeDto>,
    pub edges: Vec<L2ExplorerEdgeDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct L2ExplorerNodeDto {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub layer: u8,
    pub attributes: Vec<ExplorerAttributeDto>,
    pub specializes_count: usize,
    pub specialized_by_count: usize,
    pub is_seed: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct L2ExplorerEdgeDto {
    pub id: String,
    pub source: String,
    pub target: String,
    pub relation: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ElementDetailsDto {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub layer: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metatype: Option<ElementSummaryDto>,
    pub metatype_specialization_chain: Vec<ElementSummaryDto>,
    pub direct_properties: BTreeMap<String, Value>,
    pub inherited_properties: Vec<InheritedPropertiesDto>,
    pub effective_properties: BTreeMap<String, Value>,
    pub property_table: ElementPropertyTableDto,
    pub specialization_chain: Vec<ElementSummaryDto>,
    pub inbound: Vec<GraphEdgeDto>,
    pub outbound: Vec<GraphEdgeDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ElementSummaryDto {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub layer: u8,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct InheritedPropertiesDto {
    pub element: ElementSummaryDto,
    pub properties: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ElementPropertyTableDto {
    pub rows: Vec<ElementPropertyRowDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ElementPropertyRowDto {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_by: Option<ElementSummaryDto>,
    pub origin_kind: String,
    pub has_direct_value: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direct_value: Option<Value>,
    pub has_effective_value: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_value: Option<Value>,
    pub inherited_values: Vec<InheritedPropertyValueDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct InheritedPropertyValueDto {
    pub element: ElementSummaryDto,
    pub value: Value,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchResultDto {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub layer: u8,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LibraryTreeNodeDto {
    pub id: String,
    pub label: String,
    pub node_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element_id: Option<String>,
    pub child_count: usize,
    pub children: Vec<LibraryTreeNodeDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MountedLibraryTreeDto {
    pub library_id: String,
    pub nodes: Vec<LibraryTreeNodeDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EditorFileListDto {
    pub workspace_root: String,
    pub active_path: Option<String>,
    pub files: Vec<EditorFileSummaryDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SemanticWorkspaceSessionDto {
    pub workspace_root: String,
    pub active_path: Option<String>,
    pub project: WorkspaceProjectInfoDto,
    pub files: Vec<SemanticWorkspaceFileDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WorkspaceProjectInfoDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descriptor_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descriptor: Option<ProjectDescriptor>,
    pub libraries: Vec<WorkspaceProjectLibraryDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WorkspaceProjectLibraryDto {
    pub id: String,
    pub role: ProjectLibraryRole,
    pub source_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_identity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_element_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SemanticWorkspaceFileDto {
    pub path: String,
    pub semantic_status: String,
    pub semantic_ok: bool,
    pub diagnostics: Vec<EditorDiagnosticDto>,
    pub semantic_outline: Vec<EditorOutlineNodeDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EditorFileSummaryDto {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EditorFileContentDto {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EditorDiagnosticDto {
    pub message: String,
    pub severity: String,
    pub start_line_number: usize,
    pub start_column: usize,
    pub end_line_number: usize,
    pub end_column: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EditorParseResponseDto {
    pub ok: bool,
    pub diagnostics: Vec<EditorDiagnosticDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EditorSemanticCompileResponseDto {
    pub path: String,
    pub semantic_status: String,
    pub ok: bool,
    pub diagnostics: Vec<EditorDiagnosticDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element_count: Option<usize>,
    #[serde(default)]
    pub semantic_outline: Vec<EditorOutlineNodeDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EditorLintResponseDto {
    pub path: String,
    pub ok: bool,
    pub diagnostics: Vec<EditorDiagnosticDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EditorFormatResponseDto {
    pub path: String,
    pub formatted_content: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EditorRefreshResponseDto {
    pub ok: bool,
    pub active_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SemanticProjectCompileRequestDto {
    pub project_path: String,
    #[serde(default)]
    pub staged_files: Vec<StagedEditorFileDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StagedEditorFileDto {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SemanticProjectCompileResponseDto {
    pub ok: bool,
    pub project_path: String,
    pub file_count: usize,
    pub success_count: usize,
    pub failure_count: usize,
    pub results: Vec<SemanticCompileFileResultDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProjectLintResponseDto {
    pub ok: bool,
    pub project_path: String,
    pub file_count: usize,
    pub success_count: usize,
    pub failure_count: usize,
    pub results: Vec<ProjectLintFileResultDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProjectLintFileResultDto {
    pub path: String,
    pub ok: bool,
    pub diagnostics: Vec<EditorDiagnosticDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SemanticCompileFileResultDto {
    pub path: String,
    pub semantic_status: String,
    pub ok: bool,
    pub diagnostics: Vec<EditorDiagnosticDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element_count: Option<usize>,
    #[serde(default)]
    pub semantic_outline: Vec<EditorOutlineNodeDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EditorOutlineNodeDto {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element_id: Option<String>,
    pub label: String,
    pub kind: String,
    pub start_line_number: usize,
    pub start_column: usize,
    pub end_line_number: usize,
    pub end_column: usize,
    pub properties: BTreeMap<String, Value>,
    pub children: Vec<EditorOutlineNodeDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvaluateExpressionRequestDto {
    pub feature_id: String,
    pub owner_id: String,
    #[serde(default)]
    pub context_values: BTreeMap<String, BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct EvaluateExpressionResponseDto {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_type: Option<String>,
    pub explanation: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AppState {
    metadata: ModelMetadataDto,
    graph: Graph,
    metamodel_registry: MetamodelAttributeRegistry,
}

#[derive(Debug, Clone)]
pub(crate) struct CompiledWorkspaceFile {
    path: String,
    semantic_status: SemanticCompileStatus,
    diagnostics: Vec<EditorDiagnosticDto>,
    pub(crate) document: Option<KirDocument>,
}

#[derive(Debug)]
pub struct ServerState {
    default_workspace: WorkspaceService,
    workspaces: HashMap<String, Arc<RwLock<WorkspaceService>>>,
    next_workspace_number: u64,
}

impl ServerState {
    pub fn new(default_workspace: WorkspaceService) -> Self {
        Self {
            default_workspace,
            workspaces: HashMap::new(),
            next_workspace_number: 1,
        }
    }

    fn replace_default_workspace(&mut self, workspace: WorkspaceService) {
        self.default_workspace = workspace;
    }

    fn open_workspace(
        &mut self,
        path: &Path,
        mode: WorkspaceOpenMode,
    ) -> Result<WorkspaceOpenResponse, ApiError> {
        let workspace = workspace_from_open_mode(path, mode)?;
        let workspace_id = self.next_workspace_id();
        let status = workspace_status_for(&workspace);
        self.workspaces
            .insert(workspace_id.clone(), Arc::new(RwLock::new(workspace)));
        Ok(WorkspaceOpenResponse {
            workspace_id,
            workspace_root: status.workspace_root,
            active_path: status.active_path,
            project: status.project,
        })
    }

    fn list_workspaces(&self) -> Vec<WorkspaceSummaryDto> {
        let mut workspaces = self
            .workspaces
            .iter()
            .map(|(workspace_id, workspace)| {
                let workspace = read_workspace_state(workspace);
                let status = workspace_status_for(&workspace);
                WorkspaceSummaryDto {
                    workspace_id: workspace_id.clone(),
                    workspace_root: status.workspace_root.unwrap_or_default(),
                    active_path: status.active_path,
                    project: status.project,
                }
            })
            .collect::<Vec<_>>();
        workspaces.sort_by(|left, right| left.workspace_id.cmp(&right.workspace_id));
        workspaces
    }

    fn workspace(&self, workspace_id: &str) -> Result<Arc<RwLock<WorkspaceService>>, ApiError> {
        self.workspaces
            .get(workspace_id)
            .cloned()
            .ok_or_else(|| ApiError::MissingWorkspace(workspace_id.to_string()))
    }

    fn delete_workspace(&mut self, workspace_id: &str) -> Result<(), ApiError> {
        self.workspaces
            .remove(workspace_id)
            .map(|_| ())
            .ok_or_else(|| ApiError::MissingWorkspace(workspace_id.to_string()))
    }

    fn next_workspace_id(&mut self) -> String {
        let id = format!("ws_{:016x}", self.next_workspace_number);
        self.next_workspace_number += 1;
        id
    }
}

impl From<WorkspaceService> for ServerState {
    fn from(value: WorkspaceService) -> Self {
        Self::new(value)
    }
}

impl Deref for ServerState {
    type Target = WorkspaceService;

    fn deref(&self) -> &Self::Target {
        &self.default_workspace
    }
}

impl DerefMut for ServerState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.default_workspace
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum GraphScope {
    L2,
    L2PlusContext,
    Full,
}

impl GraphScope {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::L2 => "l2",
            Self::L2PlusContext => "l2_plus_context",
            Self::Full => "full",
        }
    }

    pub(crate) fn from_query(value: Option<&str>) -> Self {
        match value {
            Some("full") => Self::Full,
            Some("l2_plus_context") => Self::L2PlusContext,
            Some("l2") | None | Some(_) => Self::L2,
        }
    }

    fn all() -> Vec<String> {
        [Self::L2, Self::L2PlusContext, Self::Full]
            .into_iter()
            .map(|scope| scope.as_str().to_string())
            .collect()
    }
}

#[derive(Debug)]
pub enum ApiError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Kir(KirError),
    Project(ProjectDescriptorError),
    Runtime(mercurio_core::runtime::RuntimeError),
    Diagram(mercurio_core::diagrams::DiagramError),
    Diagnostic(Diagnostic),
    Ai(String),
    MissingElement(String),
    MissingEditorFile(String),
    MissingWorkspace(String),
    InvalidPath(String),
    AlreadyExists(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingElement(id) => write!(f, "element not found: {id}"),
            Self::MissingEditorFile(path) => write!(f, "editor file not found: {path}"),
            Self::MissingWorkspace(id) => write!(f, "workspace not found: {id}"),
            Self::InvalidPath(path) => write!(f, "invalid editor path: {path}"),
            Self::AlreadyExists(path) => write!(f, "editor file already exists: {path}"),
            Self::Diagnostic(err) => write!(f, "{err}"),
            Self::Ai(err) => write!(f, "{err}"),
            Self::Io(err) => write!(f, "failed to read model: {err}"),
            Self::Json(err) => write!(f, "failed to parse model json: {err}"),
            Self::Kir(err) => write!(f, "{err}"),
            Self::Project(err) => write!(f, "{err}"),
            Self::Runtime(err) => write!(f, "failed to initialize runtime: {err}"),
            Self::Diagram(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<std::io::Error> for ApiError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<mercurio_core::runtime::RuntimeError> for ApiError {
    fn from(value: mercurio_core::runtime::RuntimeError) -> Self {
        Self::Runtime(value)
    }
}

impl From<KirError> for ApiError {
    fn from(value: KirError) -> Self {
        Self::Kir(value)
    }
}

impl From<ProjectDescriptorError> for ApiError {
    fn from(value: ProjectDescriptorError) -> Self {
        Self::Project(value)
    }
}

impl From<Diagnostic> for ApiError {
    fn from(value: Diagnostic) -> Self {
        Self::Diagnostic(value)
    }
}

impl From<mercurio_core::diagrams::DiagramError> for ApiError {
    fn from(value: mercurio_core::diagrams::DiagramError) -> Self {
        Self::Diagram(value)
    }
}

impl AppState {
    pub fn from_runtime(runtime: &Runtime, stdlib_document: &KirDocument) -> Self {
        let graph = runtime.graph().clone();
        let metamodel_registry = MetamodelAttributeRegistry::build(&graph);
        let metadata = build_metadata(&graph, stdlib_document);

        Self {
            metadata,
            graph,
            metamodel_registry,
        }
    }

    pub fn metadata(&self) -> &ModelMetadataDto {
        &self.metadata
    }

    pub(crate) fn graph(&self, scope: GraphScope) -> GraphDto {
        build_graph_dto(&self.graph, scope)
    }

    pub fn metatype_explorer(
        &self,
        request: &MetatypeExplorerRequestDto,
    ) -> Result<MetatypeExplorerGraphDto, ApiError> {
        build_metatype_explorer_dto(&self.graph, &self.metamodel_registry, request)
    }

    pub fn l2_explorer(
        &self,
        request: &L2ExplorerRequestDto,
    ) -> Result<L2ExplorerGraphDto, ApiError> {
        build_l2_explorer_dto(&self.graph, request)
    }

    pub fn render_diagram(
        &self,
        request: DiagramRenderRequestDto,
    ) -> Result<DiagramViewDto, ApiError> {
        render_diagram(&self.graph, &self.metamodel_registry, request.spec).map_err(ApiError::from)
    }

    pub fn requirements_table(&self) -> RequirementTableViewDto {
        requirements_table_view(&self.graph)
    }

    pub(crate) fn graph_ref(&self) -> &Graph {
        &self.graph
    }

    pub fn element(&self, id: &str) -> Option<ElementDetailsDto> {
        let element = self.graph.element_by_element_id(id)?;

        let mut inbound = self
            .graph
            .incoming_edges(element.id)
            .map(|edge| to_edge_dto(&self.graph, edge))
            .collect::<Vec<_>>();
        inbound.sort_by(|left, right| left.id.cmp(&right.id));

        let mut outbound = self
            .graph
            .outgoing_edges(element.id)
            .map(|edge| to_edge_dto(&self.graph, edge))
            .collect::<Vec<_>>();
        outbound.sort_by(|left, right| left.id.cmp(&right.id));

        Some(build_element_details(
            &self.graph,
            &self.metamodel_registry,
            element,
            inbound,
            outbound,
        ))
    }

    pub fn search(&self, query: &str) -> Vec<SearchResultDto> {
        let query = query.trim().to_ascii_lowercase();
        let mut results = self
            .graph
            .elements()
            .iter()
            .map(|element| SearchResultDto {
                id: element.element_id.clone(),
                label: label_for_id(&element.element_id),
                kind: element.kind.clone(),
                layer: element.layer,
            })
            .filter(|entry| {
                query.is_empty()
                    || entry.id.to_ascii_lowercase().contains(&query)
                    || entry.kind.to_ascii_lowercase().contains(&query)
                    || entry.label.to_ascii_lowercase().contains(&query)
            })
            .collect::<Vec<_>>();
        results.sort_by(|left, right| left.id.cmp(&right.id));
        results
    }

    pub fn library_tree(&self) -> Vec<LibraryTreeNodeDto> {
        build_library_tree(&self.graph)
    }

    pub fn evaluate_expression(
        &self,
        request: &EvaluateExpressionRequestDto,
    ) -> EvaluateExpressionResponseDto {
        let runtime = Runtime::from_graph(self.graph.clone());
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
}

pub(crate) fn build_library_tree_from_document(
    document: &KirDocument,
) -> Result<Vec<LibraryTreeNodeDto>, ApiError> {
    let runtime = Runtime::from_document(document.clone())?;
    Ok(build_tree_from_graph(runtime.graph(), |_| true))
}

impl CompiledWorkspaceFile {
    pub(crate) fn from_source_with_context(
        file: &SourceDocument,
        compile_context: &SourceCompileContext,
        stdlib_document: &KirDocument,
    ) -> Self {
        if is_sysml_source_path(&file.path) {
            let report = match file.module.as_ref() {
                Some(module) => compile_sysml_module_with_resolver_context_report_with_limit(
                    module,
                    &file.path,
                    &compile_context.context_modules,
                    stdlib_document,
                    &compile_context.resolver_context,
                    compile_context.mappings,
                    partial_compile_attempt_limit(file.content.len()),
                ),
                None => compile_sysml_text_with_context_report(
                    &file.content,
                    &file.path,
                    &compile_context.context_modules,
                    stdlib_document,
                ),
            };
            return Self {
                path: file.path.clone(),
                semantic_status: report.status,
                diagnostics: report.diagnostics.iter().map(diagnostic_to_dto).collect(),
                document: report.document,
            };
        }

        let compile_result = match file.module.as_ref() {
            Some(module) => compile_kerml_module_with_resolver_context(
                module,
                &file.path,
                &compile_context.resolver_context,
                compile_context.mappings,
            ),
            None => compile_source_document_with_context(file, compile_context, stdlib_document),
        };

        match compile_result {
            Ok(document) => Self {
                path: file.path.clone(),
                semantic_status: SemanticCompileStatus::Ok,
                diagnostics: Vec::new(),
                document: Some(document),
            },
            Err(diagnostic) => Self {
                path: file.path.clone(),
                semantic_status: SemanticCompileStatus::Failed,
                diagnostics: vec![diagnostic_to_dto(&diagnostic)],
                document: None,
            },
        }
    }

    pub(crate) fn semantic_outline(&self) -> Vec<EditorOutlineNodeDto> {
        self.document
            .as_ref()
            .map(|document| build_semantic_editor_outline_from_document(&self.path, document))
            .unwrap_or_default()
    }

    pub(crate) fn to_semantic_workspace_file_dto_with_outline(
        &self,
        include_outline: bool,
    ) -> SemanticWorkspaceFileDto {
        SemanticWorkspaceFileDto {
            path: self.path.clone(),
            semantic_status: semantic_status_name(self.semantic_status).to_string(),
            semantic_ok: self.semantic_ok(),
            diagnostics: self.diagnostics.clone(),
            semantic_outline: if include_outline {
                self.semantic_outline()
            } else {
                Vec::new()
            },
        }
    }

    pub(crate) fn to_semantic_workspace_file_dto(&self) -> SemanticWorkspaceFileDto {
        self.to_semantic_workspace_file_dto_with_outline(true)
    }

    pub(crate) fn semantic_ok(&self) -> bool {
        self.semantic_status == SemanticCompileStatus::Ok
    }

    pub(crate) fn semantic_status_name(&self) -> &'static str {
        semantic_status_name(self.semantic_status)
    }

    pub(crate) fn diagnostic_count(&self) -> usize {
        self.diagnostics.len()
    }
}

fn semantic_status_name(status: SemanticCompileStatus) -> &'static str {
    match status {
        SemanticCompileStatus::Ok => "ok",
        SemanticCompileStatus::Partial => "partial",
        SemanticCompileStatus::Failed => "failed",
    }
}

fn is_sysml_source_path(path: &str) -> bool {
    Path::new(path).extension().and_then(|value| value.to_str()) == Some("sysml")
}

pub fn load_app_state(model_path: &Path) -> Result<AppState, ApiError> {
    Ok(load_server_state(model_path)?.app_state().clone())
}

pub fn load_server_state(model_path: &Path) -> Result<ServerState, ApiError> {
    crate::workspace::load_workspace_service(model_path).map(ServerState::new)
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GraphQuery {
    scope: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MetatypeExplorerQuery {
    seed_id: String,
    #[serde(default)]
    expanded_parents: Vec<String>,
    #[serde(default)]
    expanded_children: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct L2ExplorerQuery {
    seed_id: String,
    #[serde(default)]
    expanded_parents: Vec<String>,
    #[serde(default)]
    expanded_children: Vec<String>,
    include_reference_edges: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct EditorPathQuery {
    path: String,
}

#[derive(Debug, Deserialize)]
struct SemanticElementQuery {
    path: String,
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WatchedProjectEditorDeltaQuery {
    branch: Option<String>,
    base_branch: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateEditorFileRequest {
    path: String,
    template: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateEditorFileRequest {
    content: String,
}

#[derive(Debug, Deserialize)]
struct EditorParseRequest {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct EditorRefreshRequest {
    path: String,
}

#[derive(Debug, Deserialize)]
struct OpenWorkspaceRequest {
    path: String,
    mode: Option<WorkspaceOpenMode>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceStatus {
    is_open: bool,
    workspace_root: Option<String>,
    active_path: Option<String>,
    project: Option<WorkspaceProjectInfoDto>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WorkspaceOpenMode {
    Shell,
    Lazy,
    Compiled,
}

impl Default for WorkspaceOpenMode {
    fn default() -> Self {
        Self::Lazy
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceOpenResponse {
    workspace_id: String,
    workspace_root: Option<String>,
    active_path: Option<String>,
    project: Option<WorkspaceProjectInfoDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceSummaryDto {
    workspace_id: String,
    workspace_root: String,
    active_path: Option<String>,
    project: Option<WorkspaceProjectInfoDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthStatus {
    service: String,
    version: String,
    status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionStatus {
    service: String,
    version: String,
    api_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PackageDependencyDto {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PackageProvenanceDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_git_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_git_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_repository_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_revision_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PackageMetadataDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PackagePublishRequestDto {
    #[serde(default = "default_package_artifact_name")]
    pub artifact_name: String,
    #[serde(default)]
    pub artifact_content_base64: Option<String>,
    #[serde(default)]
    pub artifact_content: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<PackageDependencyDto>,
    #[serde(default)]
    pub provenance: PackageProvenanceDto,
    #[serde(default = "default_package_kind")]
    pub package_kind: String,
    #[serde(default = "default_package_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub metadata: PackageMetadataDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PackageManifestDto {
    pub name: String,
    pub version: String,
    #[serde(default = "default_package_kind")]
    pub package_kind: String,
    #[serde(default = "default_package_schema_version")]
    pub schema_version: u32,
    pub artifact_name: String,
    pub artifact_path: String,
    pub byte_count: usize,
    pub checksum: String,
    pub published_at_unix_ms: u128,
    #[serde(default)]
    pub dependencies: Vec<PackageDependencyDto>,
    #[serde(default)]
    pub provenance: PackageProvenanceDto,
    #[serde(default)]
    pub metadata: PackageMetadataDto,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelPackagePublishRequestDto {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub artifact_id: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<PackageDependencyDto>,
    #[serde(default)]
    pub metadata: PackageMetadataDto,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerProjectCreateRequestDto {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub starter_file_name: Option<String>,
    #[serde(default)]
    pub starter_file_content: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerProjectUpdateRequestDto {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerUserDto {
    pub id: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerLoginRequestDto {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerLoginResponseDto {
    pub user: ServerUserDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerProjectMetadataDto {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_at_unix_ms: u128,
    pub updated_at_unix_ms: u128,
    pub revision_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<ServerUserDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<ServerUserDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerProjectSummaryDto {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub updated_at_unix_ms: u128,
    pub revision_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<ServerUserDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerProjectListDto {
    pub projects: Vec<ServerProjectSummaryDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerProjectFileEntryDto {
    pub path: String,
    pub byte_count: usize,
    pub updated_at_unix_ms: u128,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerProjectFileListDto {
    pub project_id: String,
    pub revision_id: String,
    pub files: Vec<ServerProjectFileEntryDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerProjectFileContentsDto {
    pub project_id: String,
    pub path: String,
    pub content: String,
    pub revision_id: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerProjectFileWriteRequestDto {
    pub content: String,
    #[serde(default)]
    pub base_revision_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerProjectFileRenameRequestDto {
    pub new_path: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerProjectCommitRequestDto {
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerProjectValidationResponseDto {
    pub project_id: String,
    pub revision_id: String,
    pub ok: bool,
    pub semantic: SemanticProjectCompileResponseDto,
    pub lint: ProjectLintResponseDto,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerProjectCompileResponseDto {
    pub project_id: String,
    pub revision_id: String,
    pub ok: bool,
    pub semantic: SemanticProjectCompileResponseDto,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2StatusDto {
    pub service: String,
    pub version: String,
    pub store_root: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2GitStatusDto {
    pub repository_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    pub dirty_files: Vec<String>,
    pub recent_commits: Vec<ServerProjectHistoryEntryDto>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2ProposalCreateRequestDto {
    pub title: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub item_type: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub linked_semantic_elements: Vec<String>,
    #[serde(default)]
    pub external_bindings: Vec<V2ExternalBindingDto>,
    #[serde(default)]
    pub related_work_items: Vec<String>,
    #[serde(default)]
    pub pull_request: Option<V2ProposalPullRequestDto>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2ProposalUpdateRequestDto {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub item_type: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    #[serde(default)]
    pub linked_semantic_elements: Option<Vec<String>>,
    #[serde(default)]
    pub external_bindings: Option<Vec<V2ExternalBindingDto>>,
    #[serde(default)]
    pub related_work_items: Option<Vec<String>>,
    #[serde(default)]
    pub pull_request: Option<Option<V2ProposalPullRequestDto>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2ProposalPullRequestDto {
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository_external_id: Option<String>,
    pub branch: String,
    pub base_branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull_request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull_request_url: Option<String>,
    pub pull_request_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mergeable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_synced_at_unix_ms: Option<u128>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2ProposalPullRequestUpdateDto {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub repository_id: Option<String>,
    #[serde(default)]
    pub repository_external_id: Option<String>,
    pub branch: String,
    #[serde(default)]
    pub base_branch: Option<String>,
    #[serde(default)]
    pub pull_request_id: Option<String>,
    #[serde(default)]
    pub pull_request_url: Option<String>,
    #[serde(default)]
    pub pull_request_state: Option<String>,
    #[serde(default)]
    pub mergeable: Option<bool>,
    #[serde(default)]
    pub review_state: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2ExternalBindingDto {
    pub provider: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository_external_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_synced_at_unix_ms: Option<u128>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2ProblemReportCreateRequestDto {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub repository_id: Option<String>,
    #[serde(default)]
    pub repository_external_id: Option<String>,
    #[serde(default)]
    pub linked_semantic_elements: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2ProblemReportCreateResponseDto {
    pub discussion: V2ProposalDto,
    pub problem_report: V2ProposalDto,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2DemoCycleAdvanceRequestDto {
    pub action: String,
    #[serde(default)]
    pub repository_id: Option<String>,
    #[serde(default)]
    pub repository_external_id: Option<String>,
    #[serde(default)]
    pub base_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedArtifactElementSummaryDto {
    pub id: String,
    pub label: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedArtifactElementSummaryListDto {
    pub artifact_id: String,
    pub elements: Vec<WatchedArtifactElementSummaryDto>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedArtifactElementQueryDto {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2ProposalDto {
    pub key: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub item_type: String,
    pub status: String,
    pub priority: String,
    #[serde(default)]
    pub labels: Vec<String>,
    pub created_at_unix_ms: u128,
    pub updated_at_unix_ms: u128,
    #[serde(default)]
    pub linked_commits: Vec<String>,
    #[serde(default)]
    pub linked_semantic_elements: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_bindings: Vec<V2ExternalBindingDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_work_items: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<V2ProposalPullRequestDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2ProposalListDto {
    pub items: Vec<V2ProposalDto>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2ChangedFileDto {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2ProposalCommitRequestDto {
    #[serde(default)]
    pub project_id: Option<String>,
    pub message: String,
    #[serde(default)]
    pub base_revision_id: Option<String>,
    #[serde(default)]
    pub changed_files: Vec<V2ChangedFileDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct V2ProposalCommitResponseDto {
    pub proposal: V2ProposalDto,
    pub project: ServerProjectMetadataDto,
    pub commit: ServerProjectHistoryEntryDto,
    pub semantic: ServerProjectCompileResponseDto,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedRepositoryCreateRequestDto {
    pub url: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub default_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedRepositoryMetadataDto {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
    pub created_at_unix_ms: u128,
    pub updated_at_unix_ms: u128,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedRepositoryListDto {
    pub repositories: Vec<WatchedRepositoryMetadataDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedRepositoryBranchListDto {
    pub repository_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
    pub branches: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedRepositoryCloneInfoDto {
    pub repository_id: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
    pub suggested_directory_name: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedProjectCreateRequestDto {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub root_path: Option<String>,
    #[serde(default)]
    pub descriptor_path: Option<String>,
    #[serde(default)]
    pub source_globs: Vec<String>,
    #[serde(default)]
    pub stdlib_profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedProjectMetadataDto {
    pub id: String,
    pub repository_id: String,
    pub name: String,
    pub root_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptor_path: Option<String>,
    #[serde(default)]
    pub source_globs: Vec<String>,
    pub stdlib_profile: String,
    pub created_at_unix_ms: u128,
    pub updated_at_unix_ms: u128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_artifact_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedProjectListDto {
    pub watched_projects: Vec<WatchedProjectMetadataDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListDto {
    pub projects: Vec<WatchedProjectMetadataDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedProjectEditorFileDto {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedProjectEditorSnapshotDto {
    pub project: WatchedProjectMetadataDto,
    pub repository: WatchedRepositoryMetadataDto,
    pub base_commit: String,
    pub branch: String,
    pub files: Vec<WatchedProjectEditorFileDto>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedProjectEditorCommitRequestDto {
    pub base_commit: String,
    pub message: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub base_branch: Option<String>,
    #[serde(default)]
    pub files: Vec<WatchedProjectEditorFileDto>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedProjectEditorCompileRequestDto {
    pub base_commit: String,
    #[serde(default)]
    pub project_path: Option<String>,
    #[serde(default)]
    pub staged_files: Vec<WatchedProjectEditorFileDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedProjectEditorCommitResponseDto {
    pub project: WatchedProjectMetadataDto,
    pub branch: String,
    pub base_commit: String,
    pub commit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    pub pushed: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedProjectArtifactBuildRequestDto {
    pub checkout_path: String,
    pub commit: String,
    #[serde(default)]
    pub ref_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedProjectRefreshRequestDto {
    #[serde(default)]
    pub ref_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedProjectIndexingStatusDto {
    pub watched_project_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at_unix_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at_unix_ms: Option<u128>,
    pub cancel_requested: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedProjectIndexingStatusListDto {
    pub statuses: Vec<WatchedProjectIndexingStatusDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiSettingsDto {
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openai_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openai_base_url: Option<String>,
    pub openai_api_key_configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub azure_openai_deployment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub azure_openai_base_url: Option<String>,
    pub azure_openai_api_key_configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daily_token_limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_request_token_limit: Option<u64>,
    pub usage: AiUsageDto,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiSettingsUpdateDto {
    pub provider: String,
    #[serde(default)]
    pub openai_model: Option<String>,
    #[serde(default)]
    pub openai_base_url: Option<String>,
    #[serde(default)]
    pub openai_api_key: Option<String>,
    #[serde(default)]
    pub clear_openai_api_key: bool,
    #[serde(default)]
    pub azure_openai_deployment: Option<String>,
    #[serde(default)]
    pub azure_openai_base_url: Option<String>,
    #[serde(default)]
    pub azure_openai_api_key: Option<String>,
    #[serde(default)]
    pub clear_azure_openai_api_key: bool,
    #[serde(default)]
    pub daily_token_limit: Option<u64>,
    #[serde(default)]
    pub per_request_token_limit: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct StoredAiSettings {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    openai_model: Option<String>,
    #[serde(default)]
    openai_base_url: Option<String>,
    #[serde(default)]
    azure_openai_deployment: Option<String>,
    #[serde(default)]
    azure_openai_base_url: Option<String>,
    #[serde(default)]
    daily_token_limit: Option<u64>,
    #[serde(default)]
    per_request_token_limit: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct StoredAiSecrets {
    #[serde(default)]
    openai_api_key: Option<String>,
    #[serde(default)]
    azure_openai_api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AiUsageDto {
    pub day: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub requests: u64,
    pub rejected_requests: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedProjectArtifactManifestDto {
    pub id: String,
    pub watched_project_id: String,
    pub repository_id: String,
    pub repository_url: String,
    pub commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub branches: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parent_commits: Vec<String>,
    pub root_path: String,
    pub descriptor_path: Option<String>,
    pub source_globs: Vec<String>,
    pub stdlib_profile: String,
    pub source_digest: String,
    pub compiler: String,
    pub kir_schema: String,
    pub built_at_unix_ms: u128,
    pub ok: bool,
    pub file_count: usize,
    pub success_count: usize,
    pub failure_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedProjectArtifactDto {
    pub manifest: WatchedProjectArtifactManifestDto,
    pub semantic: Value,
    #[serde(default)]
    pub model: Value,
    #[serde(default)]
    pub graph: Value,
    #[serde(default)]
    pub elements: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedProjectArtifactListDto {
    pub watched_project_id: String,
    pub artifacts: Vec<WatchedProjectArtifactManifestDto>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedArtifactDiffRequestDto {
    pub base_artifact_id: String,
    pub target_artifact_id: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedArtifactDiffDto {
    pub base_artifact_id: String,
    pub target_artifact_id: String,
    pub base_commit: String,
    pub target_commit: String,
    pub added_count: usize,
    pub removed_count: usize,
    pub changed_count: usize,
    pub added: Vec<WatchedArtifactDiffElementDto>,
    pub removed: Vec<WatchedArtifactDiffElementDto>,
    pub changed: Vec<WatchedArtifactChangedElementDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedArtifactDiffElementDto {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchedArtifactChangedElementDto {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub before: Value,
    pub after: Value,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCommitDagQueryDto {
    #[serde(default)]
    pub ref_name: Option<String>,
    #[serde(default)]
    pub base_ref: Option<String>,
    #[serde(default)]
    pub max_commits: Option<usize>,
    #[serde(default)]
    pub include_diffs: Option<bool>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCommitDagDto {
    pub project_id: String,
    pub repository_id: String,
    pub repository_url: String,
    pub ref_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    pub max_commits: usize,
    pub nodes: Vec<ProjectCommitDagNodeDto>,
    pub edges: Vec<ProjectCommitDagEdgeDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCommitDagNodeDto {
    pub commit: String,
    pub parents: Vec<String>,
    pub summary: String,
    pub author: String,
    pub timestamp: String,
    pub refs: Vec<String>,
    pub branches: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_artifact: Option<WatchedProjectArtifactManifestDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCommitDagEdgeDto {
    pub parent: String,
    pub child: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_diff: Option<WatchedArtifactDiffDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerProjectHistoryEntryDto {
    pub id: String,
    pub summary: String,
    pub author: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerProjectHistoryDto {
    pub project_id: String,
    pub revision_id: String,
    pub entries: Vec<ServerProjectHistoryEntryDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PackageSummaryDto {
    name: String,
    versions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PackageListDto {
    packages: Vec<PackageSummaryDto>,
}

#[derive(Debug, Clone)]
struct PackageApiError {
    status: StatusCode,
    message: String,
}

#[derive(Clone)]
struct WatchedProjectIndexingJob {
    watched_project_id: String,
    status: String,
    ref_name: Option<String>,
    message: Option<String>,
    artifact_id: Option<String>,
    started_at_unix_ms: Option<u128>,
    finished_at_unix_ms: Option<u128>,
    cancel_requested: Arc<AtomicBool>,
}

struct HttpApiError(ApiError);

impl From<ApiError> for HttpApiError {
    fn from(value: ApiError) -> Self {
        Self(value)
    }
}

impl IntoResponse for HttpApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self.0 {
            ApiError::MissingElement(id) => {
                (StatusCode::NOT_FOUND, format!("element not found: {id}"))
            }
            ApiError::MissingEditorFile(path) => (
                StatusCode::NOT_FOUND,
                format!("editor file not found: {path}"),
            ),
            ApiError::MissingWorkspace(id) => {
                (StatusCode::NOT_FOUND, format!("workspace not found: {id}"))
            }
            ApiError::InvalidPath(path) => (
                StatusCode::BAD_REQUEST,
                format!("invalid editor path: {path}"),
            ),
            ApiError::AlreadyExists(path) => (
                StatusCode::CONFLICT,
                format!("editor file already exists: {path}"),
            ),
            ApiError::Diagnostic(err) => (StatusCode::BAD_REQUEST, err.to_string()),
            ApiError::Ai(err) => (StatusCode::BAD_GATEWAY, err),
            ApiError::Io(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to read model: {err}"),
            ),
            ApiError::Json(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to parse model json: {err}"),
            ),
            ApiError::Kir(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
            ApiError::Project(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
            ApiError::Runtime(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to initialize runtime: {err}"),
            ),
            ApiError::Diagram(err) => (StatusCode::BAD_REQUEST, err.to_string()),
        };

        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

impl PackageApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
        }
    }

    fn too_many_requests(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }
}

impl IntoResponse for PackageApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

fn package_error_from_api_error(error: ApiError) -> PackageApiError {
    match error {
        ApiError::MissingElement(id) => {
            PackageApiError::not_found(format!("element not found: {id}"))
        }
        ApiError::MissingEditorFile(path) => {
            PackageApiError::not_found(format!("server project file not found: {path}"))
        }
        ApiError::MissingWorkspace(id) => {
            PackageApiError::not_found(format!("workspace not found: {id}"))
        }
        ApiError::InvalidPath(path) => {
            PackageApiError::bad_request(format!("invalid server project path: {path}"))
        }
        ApiError::AlreadyExists(path) => {
            PackageApiError::conflict(format!("server project file already exists: {path}"))
        }
        other => PackageApiError::internal(other.to_string()),
    }
}

pub fn build_router(state: impl Into<ServerState>) -> Router {
    let state = state.into();
    Router::new()
        .route("/api/health", get(get_health))
        .route("/api/version", get(get_version))
        .route("/api/session/current-user", get(get_current_user))
        .route("/api/session/login", post(post_session_login))
        .route(
            "/api/ai/settings",
            get(get_ai_settings).put(put_ai_settings),
        )
        .route("/api/ai/settings/test", post(test_ai_settings))
        .route("/api/ai/status", get(get_ai_status))
        .route("/api/ai/test", post(test_ai_provider))
        .route("/api/ai/ask", post(post_ai_ask))
        .route("/api/ai/chat", post(post_ai_chat))
        .route("/api/ai/semantic-summary", post(post_ai_semantic_summary))
        .route("/api/workspace/status", get(get_workspace_status))
        .route(
            "/api/workspace/default-path",
            get(get_default_workspace_path),
        )
        .route("/api/workspace/open", post(open_workspace))
        .route(
            "/api/workspaces",
            get(list_workspaces).post(open_scoped_workspace),
        )
        .route(
            "/api/workspaces/{workspace_id}",
            get(get_scoped_workspace).delete(delete_scoped_workspace),
        )
        .route(
            "/api/workspaces/{workspace_id}/model",
            get(get_scoped_model),
        )
        .route(
            "/api/workspaces/{workspace_id}/graph",
            get(get_scoped_graph),
        )
        .route(
            "/api/workspaces/{workspace_id}/elements/{id}",
            get(get_scoped_element),
        )
        .route(
            "/api/workspaces/{workspace_id}/search",
            get(search_scoped_elements),
        )
        .route(
            "/api/workspaces/{workspace_id}/editor/files",
            get(get_scoped_editor_files).post(create_scoped_editor_file),
        )
        .route(
            "/api/workspaces/{workspace_id}/editor/file",
            get(get_scoped_editor_file).put(put_scoped_editor_file),
        )
        .route(
            "/api/workspaces/{workspace_id}/editor/parse",
            post(parse_scoped_editor_content),
        )
        .route(
            "/api/workspaces/{workspace_id}/editor/format",
            post(format_scoped_editor_content),
        )
        .route(
            "/api/workspaces/{workspace_id}/editor/semantic-compile",
            post(compile_scoped_editor_semantic_content),
        )
        .route(
            "/api/workspaces/{workspace_id}/editor/lint",
            post(lint_scoped_editor_content),
        )
        .route(
            "/api/workspaces/{workspace_id}/editor/refresh",
            post(refresh_scoped_editor_model),
        )
        .route(
            "/api/workspaces/{workspace_id}/semantic/workspace-session",
            get(get_scoped_semantic_workspace_session),
        )
        .route(
            "/api/workspaces/{workspace_id}/semantic/project/compile",
            post(compile_scoped_semantic_project),
        )
        .route(
            "/api/workspaces/{workspace_id}/semantic/project/lint",
            post(lint_scoped_semantic_project),
        )
        .route("/api/model", get(get_model))
        .route("/api/graph", get(get_graph))
        .route("/api/diagrams/kinds", get(get_diagram_kinds))
        .route("/api/diagrams/render", post(post_render_diagram))
        .route(
            "/api/views/requirements-table",
            get(get_requirements_table_view),
        )
        .route("/api/metatype-explorer", get(get_metatype_explorer))
        .route("/api/l2-explorer", get(get_l2_explorer))
        .route("/api/library/tree", get(get_library_tree))
        .route("/api/library/mounted-trees", get(get_mounted_library_trees))
        .route("/api/elements/{id}", get(get_element))
        .route("/api/search", get(search_elements))
        .route("/api/runtime/evaluate", post(evaluate_expression))
        .route(
            "/api/editor/files",
            get(get_editor_files).post(create_editor_file),
        )
        .route(
            "/api/editor/file",
            get(get_editor_file).put(put_editor_file),
        )
        .route("/api/editor/outline", get(get_editor_outline))
        .route(
            "/api/editor/semantic-outline",
            get(get_editor_semantic_outline),
        )
        .route(
            "/api/editor/semantic-element",
            get(get_editor_semantic_element),
        )
        .route("/api/editor/parse", post(parse_editor_content))
        .route("/api/editor/format", post(format_editor_content))
        .route(
            "/api/editor/semantic-compile",
            post(compile_editor_semantic_content),
        )
        .route("/api/editor/lint", post(lint_editor_content))
        .route("/api/editor/refresh", post(refresh_editor_model))
        .route(
            "/api/semantic/workspace-session",
            get(get_semantic_workspace_session),
        )
        .route(
            "/api/semantic/project/compile",
            post(compile_semantic_project),
        )
        .route("/api/semantic/project/lint", post(lint_semantic_project))
        .route("/api/v2/status", get(get_v2_status))
        .route("/api/v2/git/status", get(get_v2_git_status))
        .route(
            "/api/v2/proposals",
            get(list_v2_proposals).post(create_v2_proposal),
        )
        .route(
            "/api/v2/proposals/{key}",
            get(get_v2_proposal).patch(update_v2_proposal),
        )
        .route(
            "/api/v2/proposals/{key}/pull-request",
            post(update_v2_proposal_pull_request),
        )
        .route(
            "/api/v2/proposals/{key}/problem-reports",
            post(create_v2_problem_report),
        )
        .route(
            "/api/v2/proposals/{key}/demo-cycle",
            post(advance_v2_demo_cycle),
        )
        .route(
            "/api/repositories",
            get(list_watched_repositories).post(create_watched_repository),
        )
        .route(
            "/api/repositories/{repository_id}",
            get(get_watched_repository),
        )
        .route(
            "/api/repositories/{repository_id}/clone-info",
            get(get_watched_repository_clone_info),
        )
        .route(
            "/api/repositories/{repository_id}/branches",
            get(list_watched_repository_branches),
        )
        .route(
            "/api/repositories/{repository_id}/projects",
            get(list_watched_projects_for_repository).post(create_watched_project),
        )
        .route("/api/projects", get(list_projects))
        .route(
            "/api/projects/{watched_project_id}",
            get(get_watched_project).delete(delete_watched_project),
        )
        .route(
            "/api/projects/{watched_project_id}/refresh",
            post(refresh_watched_project),
        )
        .route(
            "/api/projects/indexing",
            get(list_watched_project_indexing_statuses),
        )
        .route(
            "/api/projects/{watched_project_id}/indexing",
            get(get_watched_project_indexing_status)
                .post(start_watched_project_indexing)
                .delete(cancel_watched_project_indexing),
        )
        .route(
            "/api/projects/{watched_project_id}/artifacts/build",
            post(build_watched_project_artifact),
        )
        .route(
            "/api/projects/{watched_project_id}/artifacts",
            get(list_watched_project_artifacts),
        )
        .route(
            "/api/projects/{watched_project_id}/artifacts/latest",
            get(get_latest_watched_project_artifact),
        )
        .route(
            "/api/projects/{watched_project_id}/commit-dag",
            get(get_project_commit_dag),
        )
        .route(
            "/api/editor/projects/{watched_project_id}/delta",
            get(get_watched_project_editor_delta),
        )
        .route(
            "/api/editor/projects/{watched_project_id}/delta/compile",
            post(compile_watched_project_editor_delta),
        )
        .route(
            "/api/editor/projects/{watched_project_id}/delta/commit",
            post(commit_watched_project_editor_delta),
        )
        .route(
            "/api/artifacts/{artifact_id}",
            get(get_watched_project_artifact),
        )
        .route(
            "/api/artifacts/{artifact_id}/model",
            get(get_watched_project_artifact_model),
        )
        .route(
            "/api/artifacts/{artifact_id}/graph",
            get(get_watched_project_artifact_graph),
        )
        .route(
            "/api/artifacts/{artifact_id}/elements/{element_id}",
            get(get_watched_project_artifact_element),
        )
        .route(
            "/api/artifacts/{artifact_id}/elements",
            get(list_watched_project_artifact_elements),
        )
        .route("/api/artifacts/diff", post(diff_watched_project_artifacts))
        .route("/api/packages", get(list_packages))
        .route("/api/packages/{name}", get(get_package))
        .route(
            "/api/packages/{name}/versions/{version}",
            get(get_package_version),
        )
        .route(
            "/api/packages/{name}/versions/{version}/download",
            get(download_package_version),
        )
        .route(
            "/api/packages/{name}/versions/{version}/publish",
            post(publish_package_version),
        )
        .route("/api/model-packages/publish", post(publish_model_package))
        .with_state(Arc::new(RwLock::new(state)))
}

async fn get_health() -> Json<HealthStatus> {
    Json(HealthStatus {
        service: "mercurio-core".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        status: "ok".to_string(),
    })
}

async fn get_version() -> Json<VersionStatus> {
    Json(VersionStatus {
        service: "mercurio-core".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        api_version: 1,
    })
}

async fn get_current_user() -> Json<ServerUserDto> {
    Json(default_server_user())
}

async fn post_session_login(
    Json(request): Json<ServerLoginRequestDto>,
) -> Result<Json<ServerLoginResponseDto>, PackageApiError> {
    if request.username == "Mark" && request.password == "mark" {
        return Ok(Json(ServerLoginResponseDto {
            user: default_server_user(),
        }));
    }

    Err(PackageApiError::unauthorized(
        "invalid username or password",
    ))
}

async fn get_ai_settings() -> Result<Json<AiSettingsDto>, PackageApiError> {
    Ok(Json(read_ai_settings_dto()?))
}

async fn put_ai_settings(
    Json(request): Json<AiSettingsUpdateDto>,
) -> Result<Json<AiSettingsDto>, PackageApiError> {
    let mut secrets = read_stored_ai_secrets()?;
    let settings = StoredAiSettings {
        provider: Some(normalize_ai_provider(&request.provider)?),
        openai_model: non_empty_string_option(request.openai_model),
        openai_base_url: non_empty_string_option(request.openai_base_url),
        azure_openai_deployment: non_empty_string_option(request.azure_openai_deployment),
        azure_openai_base_url: non_empty_string_option(request.azure_openai_base_url),
        daily_token_limit: normalize_optional_token_limit(request.daily_token_limit),
        per_request_token_limit: normalize_optional_token_limit(request.per_request_token_limit),
    };

    if request.clear_openai_api_key {
        secrets.openai_api_key = None;
    } else if let Some(api_key) = non_empty_string_option(request.openai_api_key) {
        secrets.openai_api_key = Some(api_key);
    }

    if request.clear_azure_openai_api_key {
        secrets.azure_openai_api_key = None;
    } else if let Some(api_key) = non_empty_string_option(request.azure_openai_api_key) {
        secrets.azure_openai_api_key = Some(api_key);
    }

    write_stored_ai_settings(&settings)?;
    write_stored_ai_secrets(&secrets)?;
    Ok(Json(ai_settings_dto(settings, secrets)))
}

async fn test_ai_settings() -> Result<Json<ReasoningProviderStatus>, HttpApiError> {
    let (config, secrets) = stored_reasoning_provider_config()
        .map_err(|err| HttpApiError(ApiError::Ai(err.message)))?;
    tokio::task::spawn_blocking(move || {
        test_configured_reasoning_provider_connection(config, secrets)
    })
    .await
    .map_err(|err| HttpApiError(ApiError::Ai(format!("AI worker failed: {err}"))))?
    .map(Json)
    .map_err(ApiError::Ai)
    .map_err(HttpApiError)
}

async fn get_ai_status() -> Result<Json<ReasoningProviderStatus>, PackageApiError> {
    let (config, secrets) = stored_reasoning_provider_config()?;
    tokio::task::spawn_blocking(move || {
        configured_reasoning_provider(config, secrets).provider_status()
    })
    .await
    .map(Json)
    .map_err(|err| PackageApiError::internal(format!("AI status worker failed: {err}")))
}

async fn test_ai_provider() -> Result<Json<ReasoningProviderStatus>, HttpApiError> {
    test_default_reasoning_provider_connection()
        .map(Json)
        .map_err(ApiError::Ai)
        .map_err(HttpApiError)
}

async fn post_ai_semantic_summary(
    Json(request): Json<SemanticSummaryRequest>,
) -> Result<Json<SemanticSummaryResponse>, HttpApiError> {
    Ok(Json(summarize_semantic_changes(&request)))
}

async fn post_ai_chat(
    Json(request): Json<ChatCompletionRequest>,
) -> Result<Json<mercurio_core::ai::ChatCompletionResponse>, PackageApiError> {
    let settings = read_stored_ai_settings()?;
    let (config, secrets) = stored_reasoning_provider_config()?;
    let usage_scope = ai_usage_scope(&config, &secrets);
    let estimated_input_tokens = estimate_chat_request_tokens(&request);
    reserve_ai_tokens(&settings, usage_scope, estimated_input_tokens)?;
    tokio::task::spawn_blocking(move || complete_configured_chat(config, secrets, &request))
        .await
        .map_err(|err| PackageApiError::internal(format!("AI worker failed: {err}")))?
        .map(|response| {
            let output_tokens = estimate_text_tokens(&response.message);
            if usage_scope {
                let _ = record_ai_usage(estimated_input_tokens, output_tokens);
            }
            response
        })
        .map(Json)
        .map_err(|err| PackageApiError::internal(format!("AI chat failed: {err}")))
}

async fn post_ai_ask(
    Json(request): Json<AskMercurioRequest>,
) -> Result<Json<AskMercurioResponse>, PackageApiError> {
    let (project, context) = build_ask_mercurio_project_context(&request)?;
    let settings = read_stored_ai_settings()?;
    let (config, secrets) = stored_reasoning_provider_config()?;
    let usage_scope = ai_usage_scope(&config, &secrets);
    let estimated_input_tokens = estimate_ask_request_tokens(&request, &context);
    reserve_ai_tokens(&settings, usage_scope, estimated_input_tokens)?;
    tokio::task::spawn_blocking(move || {
        ask_mercurio_with_config(config, secrets, &request, project, context)
    })
    .await
    .map_err(|err| PackageApiError::internal(format!("Ask Mercurio worker failed: {err}")))?
    .map(|response| {
        let artifact_text = serde_json::to_string(&response.artifacts).unwrap_or_default();
        let output_tokens =
            estimate_text_tokens(&response.message) + estimate_text_tokens(&artifact_text);
        if usage_scope {
            let _ = record_ai_usage(estimated_input_tokens, output_tokens);
        }
        response
    })
    .map(Json)
    .map_err(|err| PackageApiError::internal(format!("Ask Mercurio failed: {err}")))
}

fn build_ask_mercurio_project_context(
    request: &AskMercurioRequest,
) -> Result<(Option<AskMercurioProjectContext>, Vec<String>), PackageApiError> {
    let Some(project_id) = request
        .project_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok((
            None,
            vec!["Ask Mercurio context: no project selected.".to_string()],
        ));
    };
    let watched_project = read_watched_project_metadata_by_id(project_id)?;
    let artifact_id = request
        .artifact_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| watched_project.latest_artifact_id.clone())
        .ok_or_else(|| {
            PackageApiError::not_found(format!(
                "watched project has no cached artifacts: {project_id}"
            ))
        })?;
    let artifact = read_watched_project_artifact(&artifact_id)?;
    if artifact.manifest.watched_project_id != watched_project.id {
        return Err(PackageApiError::bad_request(format!(
            "artifact {artifact_id} does not belong to project {project_id}"
        )));
    }

    let latest_prompt = request
        .messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, mercurio_core::ai::ChatMessageRole::User))
        .map(|message| message.content.as_str())
        .unwrap_or_default();
    let evidence = ask_mercurio_artifact_evidence(&artifact, latest_prompt);
    let diagram_root = ask_mercurio_diagram_root(&artifact.elements, latest_prompt);
    let project = AskMercurioProjectContext {
        project_id: watched_project.id,
        project_name: Some(watched_project.name),
        artifact_id: Some(artifact.manifest.id.clone()),
        repository_url: Some(artifact.manifest.repository_url.clone()),
        commit: Some(artifact.manifest.commit.clone()),
        diagram_root_id: diagram_root.as_ref().map(|root| root.0.clone()),
        diagram_root_label: diagram_root.map(|root| root.1),
    };
    Ok((Some(project), evidence))
}

fn ask_mercurio_artifact_evidence(
    artifact: &WatchedProjectArtifactDto,
    prompt: &str,
) -> Vec<String> {
    let mut context = vec![
        format!(
            "Project artifact: id={} commit={} files={} ok={}",
            artifact.manifest.id,
            artifact.manifest.commit,
            artifact.manifest.file_count,
            artifact.manifest.ok
        ),
        format!(
            "Semantic build: success_count={} failure_count={} source_digest={}",
            artifact.manifest.success_count,
            artifact.manifest.failure_count,
            artifact.manifest.source_digest
        ),
        format!("Model metadata: {}", compact_json(&artifact.model, 1600)),
        format!("Graph summary: {}", compact_json(&artifact.graph, 1600)),
    ];
    let matched = ask_mercurio_matching_elements(&artifact.elements, prompt);
    if matched.is_empty() {
        context.push(
            "Relevant elements: no direct prompt matches found in artifact element ids or labels."
                .to_string(),
        );
    } else {
        context.push(format!("Relevant elements: {}", matched.join("\n")));
    }
    context
}

fn ask_mercurio_matching_elements(elements: &BTreeMap<String, Value>, prompt: &str) -> Vec<String> {
    let terms = prompt
        .split(|ch: char| !ch.is_alphanumeric() && ch != '.')
        .map(str::trim)
        .filter(|term| term.len() >= 3)
        .map(str::to_ascii_lowercase)
        .take(16)
        .collect::<Vec<_>>();

    elements
        .iter()
        .filter_map(|(id, value)| {
            let label = value.get("label").and_then(Value::as_str).unwrap_or(id);
            let kind = value
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("element");
            let haystack = format!("{id} {label} {kind}").to_ascii_lowercase();
            if terms.is_empty() || terms.iter().any(|term| haystack.contains(term)) {
                Some(format!(
                    "- id={} label={} kind={} evidence={}",
                    id,
                    label,
                    kind,
                    compact_json(value, 700)
                ))
            } else {
                None
            }
        })
        .take(8)
        .collect()
}

fn ask_mercurio_diagram_root(
    elements: &BTreeMap<String, Value>,
    prompt: &str,
) -> Option<(String, String)> {
    ask_mercurio_ranked_elements(elements, prompt)
        .into_iter()
        .find(|(_, _, kind)| {
            !kind.to_ascii_lowercase().contains("feature")
                && !kind.to_ascii_lowercase().contains("attribute")
        })
        .or_else(|| {
            ask_mercurio_ranked_elements(elements, prompt)
                .into_iter()
                .next()
        })
        .map(|(id, label, _)| (id, label))
}

fn ask_mercurio_ranked_elements(
    elements: &BTreeMap<String, Value>,
    prompt: &str,
) -> Vec<(String, String, String)> {
    let terms = prompt
        .split(|ch: char| !ch.is_alphanumeric() && ch != '.')
        .map(str::trim)
        .filter(|term| term.len() >= 3)
        .map(str::to_ascii_lowercase)
        .take(16)
        .collect::<Vec<_>>();
    let mut ranked = elements
        .iter()
        .filter_map(|(id, value)| {
            let label = value
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or(id)
                .to_string();
            let kind = value
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("element")
                .to_string();
            let haystack = format!("{id} {label} {kind}").to_ascii_lowercase();
            let score = if terms.is_empty() {
                1
            } else {
                terms
                    .iter()
                    .filter(|term| haystack.contains(term.as_str()))
                    .count()
            };
            if score == 0 {
                None
            } else {
                Some((score, id.clone(), label, kind))
            }
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.len().cmp(&right.1.len()))
            .then_with(|| left.1.cmp(&right.1))
    });
    ranked
        .into_iter()
        .map(|(_, id, label, kind)| (id, label, kind))
        .collect()
}

fn artifact_element_label(id: &str, value: &Value) -> String {
    value
        .get("label")
        .and_then(Value::as_str)
        .or_else(|| value.get("name").and_then(Value::as_str))
        .unwrap_or(id)
        .to_string()
}

fn artifact_element_kind(value: &Value) -> String {
    value
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("element")
        .to_string()
}

fn compact_json(value: &Value, max_chars: usize) -> String {
    let mut rendered = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    if rendered.chars().count() > max_chars {
        rendered = rendered.chars().take(max_chars).collect::<String>();
        rendered.push_str("...[truncated]");
    }
    rendered
}

async fn list_watched_repositories() -> Result<Json<WatchedRepositoryListDto>, PackageApiError> {
    Ok(Json(WatchedRepositoryListDto {
        repositories: read_all_watched_repositories()?,
    }))
}

async fn create_watched_repository(
    Json(request): Json<WatchedRepositoryCreateRequestDto>,
) -> Result<Json<WatchedRepositoryMetadataDto>, PackageApiError> {
    let url = request.url.trim();
    if url.is_empty() {
        return Err(PackageApiError::bad_request(
            "repository url cannot be empty",
        ));
    }

    let name = request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| repository_name_from_url(url));
    let repository_id = unique_project_id(&name);
    let repository_dir = watched_repository_dir(&repository_id)?;
    if repository_dir.exists() {
        return Err(PackageApiError::conflict(format!(
            "repository already exists: {repository_id}"
        )));
    }

    std::fs::create_dir_all(watched_repository_projects_root(&repository_id)?).map_err(|err| {
        PackageApiError::internal(format!("failed to create watched repository: {err}"))
    })?;
    let now = unix_time_ms();
    let metadata = WatchedRepositoryMetadataDto {
        id: repository_id,
        name,
        url: url.to_string(),
        description: request.description,
        provider: request.provider,
        default_branch: request.default_branch,
        created_at_unix_ms: now,
        updated_at_unix_ms: now,
    };
    write_watched_repository_metadata(&repository_dir, &metadata)?;
    Ok(Json(metadata))
}

async fn get_watched_repository(
    AxumPath(repository_id): AxumPath<String>,
) -> Result<Json<WatchedRepositoryMetadataDto>, PackageApiError> {
    Ok(Json(read_watched_repository_metadata(&repository_id)?))
}

async fn get_watched_repository_clone_info(
    AxumPath(repository_id): AxumPath<String>,
) -> Result<Json<WatchedRepositoryCloneInfoDto>, PackageApiError> {
    let repository = read_watched_repository_metadata(&repository_id)?;
    Ok(Json(WatchedRepositoryCloneInfoDto {
        repository_id: repository.id,
        url: repository.url,
        default_branch: repository.default_branch,
        suggested_directory_name: slugify_identifier(&repository.name),
    }))
}

async fn list_watched_repository_branches(
    AxumPath(repository_id): AxumPath<String>,
) -> Result<Json<WatchedRepositoryBranchListDto>, PackageApiError> {
    let repository = read_watched_repository_metadata(&repository_id)?;
    let mirror_dir = watched_repo_cache_dir(&repository.id)?;
    ensure_watched_repository_mirror(&repository, &mirror_dir)?;
    fetch_watched_repository_mirror(&mirror_dir)?;
    Ok(Json(WatchedRepositoryBranchListDto {
        repository_id: repository.id.clone(),
        default_branch: repository.default_branch.clone(),
        branches: list_watched_repository_branch_names(
            &mirror_dir,
            repository.default_branch.as_deref(),
        )?,
    }))
}

async fn list_watched_projects() -> Result<Json<WatchedProjectListDto>, PackageApiError> {
    Ok(Json(WatchedProjectListDto {
        watched_projects: read_all_watched_projects(None)?,
    }))
}

async fn list_projects() -> Result<Json<ProjectListDto>, PackageApiError> {
    Ok(Json(ProjectListDto {
        projects: read_all_watched_projects(None)?,
    }))
}

async fn list_watched_projects_for_repository(
    AxumPath(repository_id): AxumPath<String>,
) -> Result<Json<ProjectListDto>, PackageApiError> {
    read_watched_repository_metadata(&repository_id)?;
    Ok(Json(ProjectListDto {
        projects: read_all_watched_projects(Some(&repository_id))?,
    }))
}

async fn create_watched_project(
    AxumPath(repository_id): AxumPath<String>,
    Json(request): Json<WatchedProjectCreateRequestDto>,
) -> Result<Json<WatchedProjectMetadataDto>, PackageApiError> {
    let repository = read_watched_repository_metadata(&repository_id)?;
    let root_path = normalize_watched_project_path(request.root_path.as_deref().unwrap_or("."))?;
    let descriptor_path = request
        .descriptor_path
        .as_deref()
        .map(normalize_watched_project_path)
        .transpose()?;
    let source_globs = if request.source_globs.is_empty() {
        normalize_source_globs(vec!["**/*.sysml".to_string(), "**/*.kerml".to_string()])?
    } else {
        normalize_source_globs(request.source_globs)?
    };
    let name = request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if root_path == "." {
                repository.name.clone()
            } else {
                format!("{}:{}", repository.name, root_path)
            }
        });
    let watched_project_id = unique_project_id(&name);
    let project_dir = watched_project_dir(&repository_id, &watched_project_id)?;
    if project_dir.exists() {
        return Err(PackageApiError::conflict(format!(
            "watched project already exists: {watched_project_id}"
        )));
    }

    std::fs::create_dir_all(&project_dir).map_err(|err| {
        PackageApiError::internal(format!("failed to create watched project: {err}"))
    })?;
    let now = unix_time_ms();
    let metadata = WatchedProjectMetadataDto {
        id: watched_project_id,
        repository_id,
        name,
        root_path,
        descriptor_path,
        source_globs,
        stdlib_profile: request
            .stdlib_profile
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("bundled")
            .to_string(),
        created_at_unix_ms: now,
        updated_at_unix_ms: now,
        latest_commit: None,
        latest_artifact_id: None,
    };
    write_watched_project_metadata(&project_dir, &metadata)?;
    Ok(Json(metadata))
}

async fn get_watched_project(
    AxumPath(watched_project_id): AxumPath<String>,
) -> Result<Json<WatchedProjectMetadataDto>, PackageApiError> {
    Ok(Json(read_watched_project_metadata_by_id(
        &watched_project_id,
    )?))
}

async fn delete_watched_project(
    AxumPath(watched_project_id): AxumPath<String>,
) -> Result<StatusCode, PackageApiError> {
    let (project_dir, watched_project) =
        read_watched_project_metadata_with_dir(&watched_project_id)?;
    delete_watched_project_artifacts(&watched_project.id)?;
    std::fs::remove_dir_all(&project_dir).map_err(|err| {
        PackageApiError::internal(format!("failed to delete watched project: {err}"))
    })?;
    Ok(StatusCode::NO_CONTENT)
}

async fn build_watched_project_artifact(
    AxumPath(watched_project_id): AxumPath<String>,
    Json(request): Json<WatchedProjectArtifactBuildRequestDto>,
) -> Result<Json<WatchedProjectArtifactDto>, PackageApiError> {
    Ok(Json(build_watched_project_artifact_from_checkout(
        &watched_project_id,
        &PathBuf::from(request.checkout_path.trim()),
        request.commit.trim(),
        request.ref_name.as_deref(),
    )?))
}

async fn refresh_watched_project(
    AxumPath(watched_project_id): AxumPath<String>,
    Json(request): Json<WatchedProjectRefreshRequestDto>,
) -> Result<Json<WatchedProjectArtifactDto>, PackageApiError> {
    Ok(Json(refresh_watched_project_from_git(
        &watched_project_id,
        request.ref_name.as_deref(),
    )?))
}

async fn list_watched_project_indexing_statuses()
-> Result<Json<WatchedProjectIndexingStatusListDto>, PackageApiError> {
    let mut statuses = indexing_jobs()
        .lock()
        .map_err(|_| PackageApiError::internal("indexing job registry is unavailable"))?
        .values()
        .map(indexing_job_status_dto)
        .collect::<Vec<_>>();
    statuses.sort_by(|left, right| left.watched_project_id.cmp(&right.watched_project_id));
    Ok(Json(WatchedProjectIndexingStatusListDto { statuses }))
}

async fn get_watched_project_indexing_status(
    AxumPath(watched_project_id): AxumPath<String>,
) -> Result<Json<WatchedProjectIndexingStatusDto>, PackageApiError> {
    read_watched_project_metadata_by_id(&watched_project_id)?;
    Ok(Json(indexing_status_for_project(&watched_project_id)?))
}

async fn start_watched_project_indexing(
    AxumPath(watched_project_id): AxumPath<String>,
    Json(request): Json<WatchedProjectRefreshRequestDto>,
) -> Result<Json<WatchedProjectIndexingStatusDto>, PackageApiError> {
    read_watched_project_metadata_by_id(&watched_project_id)?;
    let ref_name = request
        .ref_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let cancel_requested = Arc::new(AtomicBool::new(false));
    let job = WatchedProjectIndexingJob {
        watched_project_id: watched_project_id.clone(),
        status: "running".to_string(),
        ref_name: ref_name.clone(),
        message: Some("Indexing queued".to_string()),
        artifact_id: None,
        started_at_unix_ms: Some(unix_time_ms()),
        finished_at_unix_ms: None,
        cancel_requested: cancel_requested.clone(),
    };
    {
        let mut jobs = indexing_jobs()
            .lock()
            .map_err(|_| PackageApiError::internal("indexing job registry is unavailable"))?;
        if jobs
            .get(&watched_project_id)
            .is_some_and(|job| job.status == "running" || job.status == "canceling")
        {
            return Err(PackageApiError::conflict(format!(
                "watched project is already indexing: {watched_project_id}"
            )));
        }
        jobs.insert(watched_project_id.clone(), job);
    }

    let worker_project_id = watched_project_id.clone();
    let worker_ref_name = ref_name.clone();
    std::thread::spawn(move || {
        finish_watched_project_indexing_job(
            &worker_project_id,
            worker_ref_name.as_deref(),
            cancel_requested,
        );
    });

    Ok(Json(indexing_status_for_project(&watched_project_id)?))
}

async fn cancel_watched_project_indexing(
    AxumPath(watched_project_id): AxumPath<String>,
) -> Result<Json<WatchedProjectIndexingStatusDto>, PackageApiError> {
    read_watched_project_metadata_by_id(&watched_project_id)?;
    let mut jobs = indexing_jobs()
        .lock()
        .map_err(|_| PackageApiError::internal("indexing job registry is unavailable"))?;
    if let Some(job) = jobs.get_mut(&watched_project_id) {
        if job.status == "running" {
            job.status = "canceling".to_string();
            job.message = Some("Cancel requested".to_string());
            job.cancel_requested.store(true, Ordering::SeqCst);
        }
        return Ok(Json(indexing_job_status_dto(job)));
    }
    Ok(Json(idle_indexing_status(&watched_project_id)))
}

fn build_watched_project_artifact_from_checkout(
    watched_project_id: &str,
    checkout_path: &Path,
    commit: &str,
    ref_name: Option<&str>,
) -> Result<WatchedProjectArtifactDto, PackageApiError> {
    let (project_dir, mut watched_project) =
        read_watched_project_metadata_with_dir(watched_project_id)?;
    let repository = read_watched_repository_metadata(&watched_project.repository_id)?;
    if !checkout_path.is_dir() {
        return Err(PackageApiError::bad_request(format!(
            "checkout path is not a directory: {}",
            checkout_path.display()
        )));
    }
    if commit.is_empty() {
        return Err(PackageApiError::bad_request("commit cannot be empty"));
    }

    let source_root = resolve_watched_project_checkout_root(&checkout_path, &watched_project)?;
    let source_digest = compute_files_revision(&source_root)?.replace("rev:", "src:");
    let workspace =
        WorkspaceService::from_workspace_root_compiled(&checkout_path).map_err(|err| {
            PackageApiError::internal(format!("failed to compile watched project checkout: {err}"))
        })?;
    let semantic = workspace
        .compiled_project_scope_response(&watched_project.root_path)
        .map_err(package_error_from_api_error)?;
    let artifact_id = artifact_id_for(&watched_project.id, commit, &source_digest);
    let manifest = WatchedProjectArtifactManifestDto {
        id: artifact_id.clone(),
        watched_project_id: watched_project.id.clone(),
        repository_id: watched_project.repository_id.clone(),
        repository_url: repository.url,
        commit: commit.to_string(),
        ref_name: ref_name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        branches: git_branches_containing_commit(checkout_path, commit).unwrap_or_default(),
        parent_commits: git_parent_commits(checkout_path, commit).unwrap_or_default(),
        root_path: watched_project.root_path.clone(),
        descriptor_path: watched_project.descriptor_path.clone(),
        source_globs: watched_project.source_globs.clone(),
        stdlib_profile: watched_project.stdlib_profile.clone(),
        source_digest,
        compiler: format!("mercurio-core@{}", env!("CARGO_PKG_VERSION")),
        kir_schema: "kir.v1".to_string(),
        built_at_unix_ms: unix_time_ms(),
        ok: semantic.ok,
        file_count: semantic.file_count,
        success_count: semantic.success_count,
        failure_count: semantic.failure_count,
    };
    let artifact_model = serde_json::to_value(workspace.model_metadata()).map_err(|err| {
        PackageApiError::internal(format!(
            "failed to serialize artifact model metadata: {err}"
        ))
    })?;
    let artifact_graph = workspace.graph("l2");
    let mut artifact_elements = BTreeMap::new();
    for node in &artifact_graph.nodes {
        if let Ok(element) = workspace.element(&node.id) {
            let element = serde_json::to_value(element).map_err(|err| {
                PackageApiError::internal(format!("failed to serialize artifact element: {err}"))
            })?;
            artifact_elements.insert(node.id.clone(), element);
        }
    }
    let artifact_graph = serde_json::to_value(artifact_graph).map_err(|err| {
        PackageApiError::internal(format!("failed to serialize artifact graph: {err}"))
    })?;
    let semantic = serde_json::to_value(semantic).map_err(|err| {
        PackageApiError::internal(format!(
            "failed to serialize artifact semantic result: {err}"
        ))
    })?;
    let artifact = WatchedProjectArtifactDto {
        manifest,
        semantic,
        model: artifact_model,
        graph: artifact_graph,
        elements: artifact_elements,
    };
    write_watched_project_artifact(&artifact)?;

    watched_project.latest_commit = Some(commit.to_string());
    watched_project.latest_artifact_id = Some(artifact_id);
    watched_project.updated_at_unix_ms = unix_time_ms();
    write_watched_project_metadata(&project_dir, &watched_project)?;

    Ok(artifact)
}

async fn get_watched_project_artifact(
    AxumPath(artifact_id): AxumPath<String>,
) -> Result<Json<WatchedProjectArtifactDto>, PackageApiError> {
    Ok(Json(read_watched_project_artifact(&artifact_id)?))
}

async fn get_watched_project_artifact_model(
    AxumPath(artifact_id): AxumPath<String>,
) -> Result<Json<Value>, PackageApiError> {
    Ok(Json(read_watched_project_artifact(&artifact_id)?.model))
}

async fn get_watched_project_artifact_graph(
    AxumPath(artifact_id): AxumPath<String>,
) -> Result<Json<Value>, PackageApiError> {
    Ok(Json(read_watched_project_artifact(&artifact_id)?.graph))
}

async fn list_watched_project_artifact_elements(
    AxumPath(artifact_id): AxumPath<String>,
    Query(query): Query<WatchedArtifactElementQueryDto>,
) -> Result<Json<WatchedArtifactElementSummaryListDto>, PackageApiError> {
    let artifact = read_watched_project_artifact(&artifact_id)?;
    let q = query
        .q
        .and_then(non_empty_string)
        .map(|value| value.to_ascii_lowercase());
    let limit = query.limit.unwrap_or(100).clamp(1, 250);
    let mut elements = artifact
        .elements
        .iter()
        .filter_map(|(id, value)| {
            let label = artifact_element_label(id, value);
            let kind = artifact_element_kind(value);
            if let Some(q) = &q {
                let haystack = format!("{id} {label} {kind}").to_ascii_lowercase();
                if !haystack.contains(q) {
                    return None;
                }
            }
            Some(WatchedArtifactElementSummaryDto {
                id: id.clone(),
                label,
                kind,
            })
        })
        .collect::<Vec<_>>();
    elements.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.label.cmp(&right.label))
            .then_with(|| left.id.cmp(&right.id))
    });
    elements.truncate(limit);
    Ok(Json(WatchedArtifactElementSummaryListDto {
        artifact_id,
        elements,
    }))
}

async fn get_watched_project_artifact_element(
    AxumPath((artifact_id, element_id)): AxumPath<(String, String)>,
) -> Result<Json<Value>, PackageApiError> {
    let artifact = read_watched_project_artifact(&artifact_id)?;
    artifact
        .elements
        .get(&element_id)
        .cloned()
        .map(Json)
        .ok_or_else(|| {
            PackageApiError::not_found(format!(
                "artifact element not found: {artifact_id}/{element_id}"
            ))
        })
}

async fn diff_watched_project_artifacts(
    Json(request): Json<WatchedArtifactDiffRequestDto>,
) -> Result<Json<WatchedArtifactDiffDto>, PackageApiError> {
    Ok(Json(diff_watched_artifacts(
        &request.base_artifact_id,
        &request.target_artifact_id,
    )?))
}

async fn list_watched_project_artifacts(
    AxumPath(watched_project_id): AxumPath<String>,
) -> Result<Json<WatchedProjectArtifactListDto>, PackageApiError> {
    read_watched_project_metadata_by_id(&watched_project_id)?;
    Ok(Json(WatchedProjectArtifactListDto {
        watched_project_id: watched_project_id.clone(),
        artifacts: read_watched_project_artifact_manifests(&watched_project_id)?,
    }))
}

async fn get_latest_watched_project_artifact(
    AxumPath(watched_project_id): AxumPath<String>,
) -> Result<Json<WatchedProjectArtifactDto>, PackageApiError> {
    let watched_project = read_watched_project_metadata_by_id(&watched_project_id)?;
    let artifact_id = watched_project.latest_artifact_id.ok_or_else(|| {
        PackageApiError::not_found(format!(
            "watched project has no cached artifacts: {watched_project_id}"
        ))
    })?;
    Ok(Json(read_watched_project_artifact(&artifact_id)?))
}

async fn get_project_commit_dag(
    AxumPath(watched_project_id): AxumPath<String>,
    Query(query): Query<ProjectCommitDagQueryDto>,
) -> Result<Json<ProjectCommitDagDto>, PackageApiError> {
    let watched_project = read_watched_project_metadata_by_id(&watched_project_id)?;
    let repository = read_watched_repository_metadata(&watched_project.repository_id)?;
    let mirror_dir = watched_repo_cache_dir(&repository.id)?;
    ensure_watched_repository_mirror(&repository, &mirror_dir)?;
    fetch_watched_repository_mirror(&mirror_dir)?;

    let ref_name = query
        .ref_name
        .as_ref()
        .and_then(|value| non_empty_string(value.clone()))
        .or_else(|| repository.default_branch.clone())
        .unwrap_or_else(|| "HEAD".to_string());
    let base_ref = query
        .base_ref
        .as_ref()
        .and_then(|value| non_empty_string(value.clone()));
    let max_commits = query.max_commits.unwrap_or(100).clamp(1, 500);
    let include_diffs = query.include_diffs.unwrap_or(true);

    Ok(Json(project_commit_dag(
        &watched_project,
        &repository,
        &mirror_dir,
        &ref_name,
        base_ref.as_deref(),
        max_commits,
        include_diffs,
    )?))
}

async fn get_watched_project_editor_delta(
    AxumPath(watched_project_id): AxumPath<String>,
    Query(query): Query<WatchedProjectEditorDeltaQuery>,
) -> Result<Json<WatchedProjectEditorSnapshotDto>, PackageApiError> {
    Ok(Json(load_watched_project_editor_snapshot(
        &watched_project_id,
        query,
    )?))
}

async fn commit_watched_project_editor_delta(
    AxumPath(watched_project_id): AxumPath<String>,
    Json(request): Json<WatchedProjectEditorCommitRequestDto>,
) -> Result<Json<WatchedProjectEditorCommitResponseDto>, PackageApiError> {
    Ok(Json(commit_watched_project_editor_changes(
        &watched_project_id,
        request,
    )?))
}

async fn compile_watched_project_editor_delta(
    AxumPath(watched_project_id): AxumPath<String>,
    Json(request): Json<WatchedProjectEditorCompileRequestDto>,
) -> Result<Json<SemanticProjectCompileResponseDto>, PackageApiError> {
    Ok(Json(compile_watched_project_editor_changes(
        &watched_project_id,
        request,
    )?))
}

async fn list_server_projects() -> Result<Json<ServerProjectListDto>, PackageApiError> {
    let projects_root = server_projects_root();
    if !projects_root.exists() {
        return Ok(Json(ServerProjectListDto {
            projects: Vec::new(),
        }));
    }

    let mut projects = Vec::new();
    let entries = std::fs::read_dir(&projects_root).map_err(|err| {
        PackageApiError::internal(format!("failed to read server projects: {err}"))
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| PackageApiError::internal(err.to_string()))?;
        if !entry
            .file_type()
            .map_err(|err| PackageApiError::internal(err.to_string()))?
            .is_dir()
        {
            continue;
        }
        let metadata_path = entry.path().join("metadata.json");
        if !metadata_path.exists() {
            continue;
        }
        let metadata = read_server_project_metadata_by_dir(&entry.path())?;
        projects.push(ServerProjectSummaryDto {
            id: metadata.id,
            name: metadata.name,
            description: metadata.description,
            updated_at_unix_ms: metadata.updated_at_unix_ms,
            revision_id: metadata.revision_id,
            updated_by: metadata.updated_by,
        });
    }
    projects.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));

    Ok(Json(ServerProjectListDto { projects }))
}

async fn create_server_project(
    Json(request): Json<ServerProjectCreateRequestDto>,
) -> Result<Json<ServerProjectMetadataDto>, PackageApiError> {
    let name = request.name.trim();
    if name.is_empty() {
        return Err(PackageApiError::bad_request("project name cannot be empty"));
    }

    let project_id = unique_project_id(name);
    let project_dir = server_project_dir(&project_id)?;
    if project_dir.exists() {
        return Err(PackageApiError::conflict(format!(
            "server project already exists: {project_id}"
        )));
    }
    std::fs::create_dir_all(project_dir.join("files")).map_err(|err| {
        PackageApiError::internal(format!("failed to create server project: {err}"))
    })?;

    let starter_path = request
        .starter_file_name
        .as_deref()
        .unwrap_or("model.sysml")
        .trim();
    let starter_content = request
        .starter_file_content
        .unwrap_or_else(|| format!("package {} {{\n}}\n", project_package_name(name)));
    let relative_path = normalize_server_project_file_path(starter_path)?;
    let file_path = server_project_file_path(&project_id, &relative_path)?;
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            PackageApiError::internal(format!("failed to create server project folder: {err}"))
        })?;
    }
    std::fs::write(&file_path, starter_content)
        .map_err(|err| PackageApiError::internal(format!("failed to write starter file: {err}")))?;

    let now = unix_time_ms();
    let user = default_server_user();
    let mut metadata = ServerProjectMetadataDto {
        id: project_id.clone(),
        name: name.to_string(),
        description: request.description,
        created_at_unix_ms: now,
        updated_at_unix_ms: now,
        revision_id: "rev:pending".to_string(),
        created_by: Some(user.clone()),
        updated_by: Some(user.clone()),
    };
    metadata.revision_id = compute_server_project_revision(&project_id)?;
    write_server_project_metadata(&project_dir, &metadata)?;
    initialize_server_project_git(&project_dir, "Create server project", &user);

    Ok(Json(metadata))
}

async fn get_server_project(
    AxumPath(project_id): AxumPath<String>,
) -> Result<Json<ServerProjectMetadataDto>, PackageApiError> {
    validate_package_identifier(&project_id, "project id")?;
    Ok(Json(read_server_project_metadata(&project_id)?))
}

async fn update_server_project(
    AxumPath(project_id): AxumPath<String>,
    Json(request): Json<ServerProjectUpdateRequestDto>,
) -> Result<Json<ServerProjectMetadataDto>, PackageApiError> {
    validate_package_identifier(&project_id, "project id")?;
    let project_dir = server_project_dir(&project_id)?;
    let mut metadata = read_server_project_metadata(&project_id)?;

    if let Some(name) = request.name {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(PackageApiError::bad_request("project name cannot be empty"));
        }
        metadata.name = trimmed.to_string();
    }
    if let Some(description) = request.description {
        metadata.description = description.and_then(|description| {
            let trimmed = description.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        });
    }
    metadata.updated_at_unix_ms = unix_time_ms();
    metadata.updated_by = Some(default_server_user());

    write_server_project_metadata(&project_dir, &metadata)?;
    commit_server_project_git(
        &project_dir,
        "Update project metadata",
        metadata.updated_by.as_ref().unwrap(),
    );

    Ok(Json(metadata))
}

async fn delete_server_project(
    AxumPath(project_id): AxumPath<String>,
) -> Result<StatusCode, PackageApiError> {
    validate_package_identifier(&project_id, "project id")?;
    let project_dir = server_project_dir(&project_id)?;
    if !project_dir.exists() {
        return Err(PackageApiError::not_found(format!(
            "server project not found: {project_id}"
        )));
    }
    std::fs::remove_dir_all(&project_dir).map_err(|err| {
        PackageApiError::internal(format!("failed to delete server project: {err}"))
    })?;
    let working_dir = server_project_working_dir(&project_id)?;
    if working_dir.exists() {
        std::fs::remove_dir_all(&working_dir).map_err(|err| {
            PackageApiError::internal(format!(
                "failed to delete server project working copy: {err}"
            ))
        })?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn list_server_project_files(
    AxumPath(project_id): AxumPath<String>,
) -> Result<Json<ServerProjectFileListDto>, PackageApiError> {
    validate_package_identifier(&project_id, "project id")?;
    read_server_project_metadata(&project_id)?;
    let files_root = server_project_working_files_root(&project_id)?;
    let mut files = Vec::new();
    if files_root.exists() {
        collect_server_project_files(&files_root, &files_root, &mut files)?;
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));

    Ok(Json(ServerProjectFileListDto {
        project_id,
        revision_id: compute_files_revision(&files_root)?,
        files,
    }))
}

async fn get_server_project_file(
    AxumPath((project_id, path)): AxumPath<(String, String)>,
) -> Result<Json<ServerProjectFileContentsDto>, PackageApiError> {
    validate_package_identifier(&project_id, "project id")?;
    read_server_project_metadata(&project_id)?;
    let relative_path = normalize_server_project_file_path(&path)?;
    let files_root = server_project_working_files_root(&project_id)?;
    let file_path = files_root.join(&relative_path);
    let content = std::fs::read_to_string(&file_path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            PackageApiError::not_found(format!("server project file not found: {relative_path}"))
        } else {
            PackageApiError::internal(format!("failed to read server project file: {err}"))
        }
    })?;

    Ok(Json(ServerProjectFileContentsDto {
        project_id,
        path: relative_path,
        content,
        revision_id: compute_files_revision(&files_root)?,
    }))
}

async fn put_server_project_file(
    AxumPath((project_id, path)): AxumPath<(String, String)>,
    Json(request): Json<ServerProjectFileWriteRequestDto>,
) -> Result<Json<ServerProjectFileContentsDto>, PackageApiError> {
    validate_package_identifier(&project_id, "project id")?;
    read_server_project_metadata(&project_id)?;
    let files_root = server_project_working_files_root(&project_id)?;
    let working_revision = compute_files_revision(&files_root)?;
    if let Some(base_revision_id) = request.base_revision_id.as_deref()
        && base_revision_id != working_revision
    {
        return Err(PackageApiError::conflict(format!(
            "base revision mismatch: current revision is {}",
            working_revision
        )));
    }

    let relative_path = normalize_server_project_file_path(&path)?;
    let file_path = files_root.join(&relative_path);
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            PackageApiError::internal(format!("failed to create server project folder: {err}"))
        })?;
    }
    std::fs::write(&file_path, &request.content).map_err(|err| {
        PackageApiError::internal(format!("failed to write server project file: {err}"))
    })?;

    let revision_id = compute_files_revision(&files_root)?;

    Ok(Json(ServerProjectFileContentsDto {
        project_id,
        path: relative_path,
        content: request.content,
        revision_id,
    }))
}

async fn rename_server_project_file(
    AxumPath((project_id, path)): AxumPath<(String, String)>,
    Json(request): Json<ServerProjectFileRenameRequestDto>,
) -> Result<Json<ServerProjectFileContentsDto>, PackageApiError> {
    validate_package_identifier(&project_id, "project id")?;
    read_server_project_metadata(&project_id)?;
    let old_relative_path = normalize_server_project_file_path(&path)?;
    let new_relative_path = normalize_server_project_file_path(&request.new_path)?;
    if old_relative_path == new_relative_path {
        return get_server_project_file(AxumPath((project_id, old_relative_path))).await;
    }

    let files_root = server_project_working_files_root(&project_id)?;
    let old_file_path = files_root.join(&old_relative_path);
    if !old_file_path.exists() {
        return Err(PackageApiError::not_found(format!(
            "server project file not found: {old_relative_path}"
        )));
    }

    let new_file_path = files_root.join(&new_relative_path);
    if new_file_path.exists() {
        return Err(PackageApiError::conflict(format!(
            "server project file already exists: {new_relative_path}"
        )));
    }
    if let Some(parent) = new_file_path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            PackageApiError::internal(format!("failed to create server project folder: {err}"))
        })?;
    }
    std::fs::rename(&old_file_path, &new_file_path).map_err(|err| {
        PackageApiError::internal(format!("failed to rename server project file: {err}"))
    })?;

    let content = std::fs::read_to_string(&new_file_path).map_err(|err| {
        PackageApiError::internal(format!("failed to read renamed server project file: {err}"))
    })?;
    let revision_id = compute_files_revision(&files_root)?;

    Ok(Json(ServerProjectFileContentsDto {
        project_id,
        path: new_relative_path,
        content,
        revision_id,
    }))
}

async fn delete_server_project_file(
    AxumPath((project_id, path)): AxumPath<(String, String)>,
) -> Result<StatusCode, PackageApiError> {
    validate_package_identifier(&project_id, "project id")?;
    read_server_project_metadata(&project_id)?;
    let relative_path = normalize_server_project_file_path(&path)?;
    let file_path = server_project_working_files_root(&project_id)?.join(&relative_path);
    if !file_path.exists() {
        return Err(PackageApiError::not_found(format!(
            "server project file not found: {relative_path}"
        )));
    }
    std::fs::remove_file(&file_path).map_err(|err| {
        PackageApiError::internal(format!("failed to delete server project file: {err}"))
    })?;

    Ok(StatusCode::NO_CONTENT)
}

async fn validate_server_project(
    AxumPath(project_id): AxumPath<String>,
) -> Result<Json<ServerProjectValidationResponseDto>, PackageApiError> {
    validate_package_identifier(&project_id, "project id")?;
    read_server_project_metadata(&project_id)?;
    let revision_id = compute_files_revision(&server_project_working_files_root(&project_id)?)?;
    let (workspace, staged_files) = load_server_project_workspace(&project_id)?;
    let semantic = workspace
        .compile_project_scope(".", &staged_files)
        .map_err(|err| {
            PackageApiError::internal(format!("failed to validate server project: {err}"))
        })?;
    let lint = workspace.lint_project_scope(".", &[]).map_err(|err| {
        PackageApiError::internal(format!("failed to lint server project: {err}"))
    })?;
    let ok = semantic.ok && lint.ok;

    Ok(Json(ServerProjectValidationResponseDto {
        project_id,
        revision_id,
        ok,
        semantic,
        lint,
    }))
}

async fn compile_server_project(
    AxumPath(project_id): AxumPath<String>,
) -> Result<Json<ServerProjectCompileResponseDto>, PackageApiError> {
    validate_package_identifier(&project_id, "project id")?;
    read_server_project_metadata(&project_id)?;
    let revision_id = compute_files_revision(&server_project_working_files_root(&project_id)?)?;
    let (workspace, staged_files) = load_server_project_workspace(&project_id)?;
    let semantic = workspace
        .compile_project_scope(".", &staged_files)
        .map_err(|err| {
            PackageApiError::internal(format!("failed to compile server project: {err}"))
        })?;
    let ok = semantic.ok;

    Ok(Json(ServerProjectCompileResponseDto {
        project_id,
        revision_id,
        ok,
        semantic,
    }))
}

async fn get_server_project_semantic_element(
    AxumPath(project_id): AxumPath<String>,
    Query(query): Query<SemanticElementQuery>,
) -> Result<Json<ElementDetailsDto>, PackageApiError> {
    validate_package_identifier(&project_id, "project id")?;
    read_server_project_metadata(&project_id)?;
    let relative_path = normalize_server_project_file_path(&query.path)?;
    let files_root = server_project_working_files_root(&project_id)?;
    let workspace =
        WorkspaceService::from_workspace_root_diagnostics_only(&files_root).map_err(|err| {
            PackageApiError::internal(format!("failed to load server project: {err}"))
        })?;
    workspace
        .semantic_element(&relative_path, &query.id)
        .map(Json)
        .map_err(package_error_from_api_error)
}

async fn get_server_project_history(
    AxumPath(project_id): AxumPath<String>,
) -> Result<Json<ServerProjectHistoryDto>, PackageApiError> {
    validate_package_identifier(&project_id, "project id")?;
    let metadata = read_server_project_metadata(&project_id)?;
    let project_dir = server_project_dir(&project_id)?;
    let entries = read_server_project_git_history(&project_dir)?;

    Ok(Json(ServerProjectHistoryDto {
        project_id,
        revision_id: metadata.revision_id,
        entries,
    }))
}

async fn commit_server_project_working_copy(
    AxumPath(project_id): AxumPath<String>,
    Json(request): Json<ServerProjectCommitRequestDto>,
) -> Result<Json<ServerProjectMetadataDto>, PackageApiError> {
    validate_package_identifier(&project_id, "project id")?;
    let user = default_server_user();
    let project_dir = server_project_dir(&project_id)?;
    let canonical_files_root = server_project_files_root(&project_id)?;
    let working_files_root = server_project_working_files_root(&project_id)?;
    let mut metadata = read_server_project_metadata(&project_id)?;

    replace_directory_contents(&canonical_files_root, &working_files_root)?;
    metadata.updated_at_unix_ms = unix_time_ms();
    metadata.revision_id = compute_server_project_revision(&project_id)?;
    metadata.updated_by = Some(user.clone());
    write_server_project_metadata(&project_dir, &metadata)?;
    commit_server_project_git(
        &project_dir,
        request
            .summary
            .as_deref()
            .filter(|summary| !summary.trim().is_empty())
            .unwrap_or("Commit working copy"),
        &user,
    );
    reset_server_project_working_copy(&project_id)?;

    Ok(Json(metadata))
}

async fn discard_server_project_working_copy(
    AxumPath(project_id): AxumPath<String>,
) -> Result<Json<ServerProjectMetadataDto>, PackageApiError> {
    validate_package_identifier(&project_id, "project id")?;
    let metadata = read_server_project_metadata(&project_id)?;
    reset_server_project_working_copy(&project_id)?;
    Ok(Json(metadata))
}

async fn list_packages() -> Result<Json<PackageListDto>, PackageApiError> {
    let package_root = package_registry_root();
    if !package_root.exists() {
        return Ok(Json(PackageListDto {
            packages: Vec::new(),
        }));
    }

    let mut packages = Vec::new();
    let entries = std::fs::read_dir(&package_root).map_err(|err| {
        PackageApiError::internal(format!("failed to read package registry: {err}"))
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| PackageApiError::internal(err.to_string()))?;
        if !entry
            .file_type()
            .map_err(|err| PackageApiError::internal(err.to_string()))?
            .is_dir()
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let mut versions = list_package_versions_from_dir(&entry.path())?;
        versions.sort();
        packages.push(PackageSummaryDto { name, versions });
    }
    packages.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(Json(PackageListDto { packages }))
}

async fn get_package(
    AxumPath(name): AxumPath<String>,
) -> Result<Json<PackageSummaryDto>, PackageApiError> {
    let package_dir = package_dir(&name)?;
    if !package_dir.exists() {
        return Err(PackageApiError::not_found(format!(
            "package not found: {name}"
        )));
    }
    let mut versions = list_package_versions_from_dir(&package_dir)?;
    versions.sort();
    Ok(Json(PackageSummaryDto { name, versions }))
}

async fn get_package_version(
    AxumPath((name, version)): AxumPath<(String, String)>,
) -> Result<Json<PackageManifestDto>, PackageApiError> {
    Ok(Json(read_package_manifest(&name, &version)?))
}

async fn download_package_version(
    AxumPath((name, version)): AxumPath<(String, String)>,
) -> Result<Response, PackageApiError> {
    let manifest = read_package_manifest(&name, &version)?;
    let version_dir = package_version_dir(&name, &version)?;
    let artifact_path = version_dir.join(&manifest.artifact_name);
    let bytes = std::fs::read(&artifact_path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            PackageApiError::not_found(format!("package artifact not found: {name} {version}"))
        } else {
            PackageApiError::internal(format!("failed to read package artifact: {err}"))
        }
    })?;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", manifest.artifact_name),
        )
        .body(Body::from(bytes))
        .map_err(|err| PackageApiError::internal(err.to_string()))
}

async fn publish_package_version(
    AxumPath((name, version)): AxumPath<(String, String)>,
    Json(request): Json<PackagePublishRequestDto>,
) -> Result<Json<PackageManifestDto>, PackageApiError> {
    validate_package_identifier(&name, "package name")?;
    validate_package_identifier(&version, "package version")?;
    validate_artifact_name(&request.artifact_name)?;
    let artifact_bytes = decode_package_artifact(&request)?;
    let manifest = publish_package_artifact(
        name,
        version,
        request.package_kind,
        request.schema_version,
        request.artifact_name,
        artifact_bytes,
        request.dependencies,
        request.provenance,
        request.metadata,
    )?;

    Ok(Json(manifest))
}

async fn publish_model_package(
    Json(request): Json<ModelPackagePublishRequestDto>,
) -> Result<Json<PackageManifestDto>, PackageApiError> {
    validate_package_identifier(&request.name, "package name")?;
    validate_package_identifier(&request.version, "package version")?;

    let artifact_id = if let Some(artifact_id) = request
        .artifact_id
        .as_ref()
        .and_then(|value| non_empty_string(value.clone()))
    {
        artifact_id
    } else {
        let project_id = request
            .project_id
            .as_ref()
            .and_then(|value| non_empty_string(value.clone()))
            .ok_or_else(|| PackageApiError::bad_request("projectId or artifactId is required"))?;
        let watched_project = read_watched_project_metadata_by_id(&project_id)?;
        watched_project.latest_artifact_id.ok_or_else(|| {
            PackageApiError::not_found(format!(
                "watched project has no cached artifacts: {project_id}"
            ))
        })?
    };

    let artifact = read_watched_project_artifact(&artifact_id)?;
    if let Some(project_id) = request
        .project_id
        .as_ref()
        .and_then(|value| non_empty_string(value.clone()))
    {
        if artifact.manifest.watched_project_id != project_id {
            return Err(PackageApiError::bad_request(format!(
                "artifact {artifact_id} does not belong to project {project_id}"
            )));
        }
    }
    if !artifact.manifest.ok {
        return Err(PackageApiError::bad_request(
            "cannot publish a model package from an artifact with diagnostics",
        ));
    }

    let artifact_name = format!("{}.kpar", request.name);
    validate_artifact_name(&artifact_name)?;
    let provenance = PackageProvenanceDto {
        source_kind: Some("watched_git".to_string()),
        source_git_commit: Some(artifact.manifest.commit.clone()),
        source_git_branch: artifact
            .manifest
            .ref_name
            .clone()
            .or_else(|| artifact.manifest.branches.first().cloned()),
        source_repository_url: Some(artifact.manifest.repository_url.clone()),
        source_project_id: Some(artifact.manifest.watched_project_id.clone()),
        source_revision_id: Some(artifact.manifest.id.clone()),
    };
    let metadata = request.metadata;
    let dependencies = request.dependencies;
    let artifact_bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "schemaVersion": default_package_schema_version(),
        "kind": default_package_kind(),
        "package": {
            "name": request.name,
            "version": request.version,
            "metadata": metadata,
            "dependencies": dependencies,
        },
        "provenance": provenance,
        "semanticArtifact": {
            "manifest": artifact.manifest,
            "semantic": artifact.semantic,
            "model": artifact.model,
            "graph": artifact.graph,
            "elements": artifact.elements,
        }
    }))
    .map_err(|err| PackageApiError::internal(format!("failed to encode model package: {err}")))?;

    let manifest = publish_package_artifact(
        request.name,
        request.version,
        default_package_kind(),
        default_package_schema_version(),
        artifact_name,
        artifact_bytes,
        dependencies,
        provenance,
        metadata,
    )?;

    Ok(Json(manifest))
}

fn publish_package_artifact(
    name: String,
    version: String,
    package_kind: String,
    schema_version: u32,
    artifact_name: String,
    artifact_bytes: Vec<u8>,
    dependencies: Vec<PackageDependencyDto>,
    provenance: PackageProvenanceDto,
    metadata: PackageMetadataDto,
) -> Result<PackageManifestDto, PackageApiError> {
    let version_dir = package_version_dir(&name, &version)?;
    if version_dir.exists() {
        return Err(PackageApiError::conflict(format!(
            "package version already exists: {name} {version}"
        )));
    }
    std::fs::create_dir_all(&version_dir).map_err(|err| {
        PackageApiError::internal(format!("failed to create package version directory: {err}"))
    })?;

    let artifact_path = version_dir.join(&artifact_name);
    std::fs::write(&artifact_path, &artifact_bytes).map_err(|err| {
        PackageApiError::internal(format!("failed to write package artifact: {err}"))
    })?;
    let checksum = checksum_hex(&artifact_bytes);
    let manifest = PackageManifestDto {
        name,
        version,
        package_kind,
        schema_version,
        artifact_name,
        artifact_path: artifact_path.to_string_lossy().to_string(),
        byte_count: artifact_bytes.len(),
        checksum,
        published_at_unix_ms: unix_time_ms(),
        dependencies,
        provenance,
        metadata,
    };
    write_package_manifest(&version_dir, &manifest)?;

    Ok(manifest)
}

async fn get_workspace_status(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Result<Json<WorkspaceStatus>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(workspace_status(&state)))
}

async fn get_default_workspace_path() -> Json<String> {
    Json(
        mercurio_core::paths::default_workspace_root()
            .to_string_lossy()
            .to_string(),
    )
}

async fn open_workspace(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(request): Json<OpenWorkspaceRequest>,
) -> Result<Json<WorkspaceStatus>, HttpApiError> {
    let mut state = write_server_state(&state);
    let workspace =
        workspace_from_open_mode(Path::new(&request.path), request.mode.unwrap_or_default())?;
    state.replace_default_workspace(workspace);
    Ok(Json(workspace_status(&state)))
}

async fn list_workspaces(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Result<Json<Vec<WorkspaceSummaryDto>>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.list_workspaces()))
}

async fn open_scoped_workspace(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(request): Json<OpenWorkspaceRequest>,
) -> Result<Json<WorkspaceOpenResponse>, HttpApiError> {
    let mut state = write_server_state(&state);
    Ok(Json(state.open_workspace(
        Path::new(&request.path),
        request.mode.unwrap_or_default(),
    )?))
}

async fn get_scoped_workspace(
    State(state): State<Arc<RwLock<ServerState>>>,
    AxumPath(workspace_id): AxumPath<String>,
) -> Result<Json<WorkspaceSummaryDto>, HttpApiError> {
    let workspace = workspace_handle(&state, &workspace_id)?;
    let workspace = read_workspace_state(&workspace);
    let status = workspace_status_for(&workspace);
    Ok(Json(WorkspaceSummaryDto {
        workspace_id,
        workspace_root: status.workspace_root.unwrap_or_default(),
        active_path: status.active_path,
        project: status.project,
    }))
}

async fn delete_scoped_workspace(
    State(state): State<Arc<RwLock<ServerState>>>,
    AxumPath(workspace_id): AxumPath<String>,
) -> Result<StatusCode, HttpApiError> {
    let mut state = write_server_state(&state);
    state.delete_workspace(&workspace_id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_scoped_model(
    State(state): State<Arc<RwLock<ServerState>>>,
    AxumPath(workspace_id): AxumPath<String>,
) -> Result<Json<ModelMetadataDto>, HttpApiError> {
    let workspace = workspace_handle(&state, &workspace_id)?;
    let workspace = read_workspace_state(&workspace);
    Ok(Json(workspace.model_metadata()))
}

async fn get_scoped_graph(
    State(state): State<Arc<RwLock<ServerState>>>,
    AxumPath(workspace_id): AxumPath<String>,
    Query(query): Query<GraphQuery>,
) -> Result<Json<GraphDto>, HttpApiError> {
    let workspace = workspace_handle(&state, &workspace_id)?;
    let workspace = read_workspace_state(&workspace);
    Ok(Json(workspace.graph(
        query.scope.as_deref().unwrap_or(GraphScope::L2.as_str()),
    )))
}

async fn get_scoped_element(
    State(state): State<Arc<RwLock<ServerState>>>,
    AxumPath((workspace_id, id)): AxumPath<(String, String)>,
) -> Result<Json<ElementDetailsDto>, HttpApiError> {
    let workspace = workspace_handle(&state, &workspace_id)?;
    let workspace = read_workspace_state(&workspace);
    Ok(Json(workspace.element(&id)?))
}

async fn search_scoped_elements(
    State(state): State<Arc<RwLock<ServerState>>>,
    AxumPath(workspace_id): AxumPath<String>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<SearchResultDto>>, HttpApiError> {
    let workspace = workspace_handle(&state, &workspace_id)?;
    let workspace = read_workspace_state(&workspace);
    Ok(Json(
        workspace.search(query.q.as_deref().unwrap_or_default()),
    ))
}

async fn get_model(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Result<Json<ModelMetadataDto>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.model_metadata()))
}

async fn get_graph(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(query): Query<GraphQuery>,
) -> Result<Json<GraphDto>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.graph(
        query.scope.as_deref().unwrap_or(GraphScope::L2.as_str()),
    )))
}

async fn get_diagram_kinds() -> Json<Vec<DiagramKindDto>> {
    Json(list_diagram_kinds())
}

async fn post_render_diagram(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(request): Json<DiagramRenderRequestDto>,
) -> Result<Json<DiagramViewDto>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.render_diagram(request)?))
}

async fn get_requirements_table_view(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Result<Json<RequirementTableViewDto>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.requirements_table()))
}

async fn get_metatype_explorer(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(query): Query<MetatypeExplorerQuery>,
) -> Result<Json<MetatypeExplorerGraphDto>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.metatype_explorer(
        &MetatypeExplorerRequestDto {
            seed_id: query.seed_id,
            expanded_parents: query.expanded_parents,
            expanded_children: query.expanded_children,
        },
    )?))
}

async fn get_l2_explorer(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(query): Query<L2ExplorerQuery>,
) -> Result<Json<L2ExplorerGraphDto>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.l2_explorer(&L2ExplorerRequestDto {
        seed_id: query.seed_id,
        expanded_parents: query.expanded_parents,
        expanded_children: query.expanded_children,
        include_reference_edges: query.include_reference_edges.unwrap_or(true),
    })?))
}

async fn get_library_tree(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Result<Json<Vec<LibraryTreeNodeDto>>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.library_tree()))
}

async fn get_mounted_library_trees(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Result<Json<Vec<MountedLibraryTreeDto>>, HttpApiError> {
    let mut state = write_server_state(&state);
    Ok(Json(state.load_mounted_library_trees()?))
}

async fn get_element(
    State(state): State<Arc<RwLock<ServerState>>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ElementDetailsDto>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.element(&id)?))
}

async fn search_elements(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<SearchResultDto>>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.search(query.q.as_deref().unwrap_or_default())))
}

async fn evaluate_expression(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(request): Json<EvaluateExpressionRequestDto>,
) -> Result<Json<EvaluateExpressionResponseDto>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.evaluate_expression(&request)))
}

async fn get_editor_files(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Result<Json<EditorFileListDto>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.list_editor_files()?))
}

async fn get_semantic_workspace_session(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Result<Json<SemanticWorkspaceSessionDto>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.semantic_workspace_session()?))
}

async fn get_scoped_editor_files(
    State(state): State<Arc<RwLock<ServerState>>>,
    AxumPath(workspace_id): AxumPath<String>,
) -> Result<Json<EditorFileListDto>, HttpApiError> {
    let workspace = workspace_handle(&state, &workspace_id)?;
    let workspace = read_workspace_state(&workspace);
    Ok(Json(workspace.list_editor_files()?))
}

async fn create_scoped_editor_file(
    State(state): State<Arc<RwLock<ServerState>>>,
    AxumPath(workspace_id): AxumPath<String>,
    Json(request): Json<CreateEditorFileRequest>,
) -> Result<Json<EditorFileContentDto>, HttpApiError> {
    let workspace = workspace_handle(&state, &workspace_id)?;
    let mut workspace = write_workspace_state(&workspace);
    Ok(Json(workspace.create_editor_file(
        &request.path,
        request.template.as_deref(),
    )?))
}

async fn get_scoped_editor_file(
    State(state): State<Arc<RwLock<ServerState>>>,
    AxumPath(workspace_id): AxumPath<String>,
    Query(query): Query<EditorPathQuery>,
) -> Result<Json<EditorFileContentDto>, HttpApiError> {
    let workspace = workspace_handle(&state, &workspace_id)?;
    let workspace = read_workspace_state(&workspace);
    Ok(Json(workspace.read_editor_file(&query.path)?))
}

async fn put_scoped_editor_file(
    State(state): State<Arc<RwLock<ServerState>>>,
    AxumPath(workspace_id): AxumPath<String>,
    Query(query): Query<EditorPathQuery>,
    Json(request): Json<UpdateEditorFileRequest>,
) -> Result<StatusCode, HttpApiError> {
    let workspace = workspace_handle(&state, &workspace_id)?;
    let mut workspace = write_workspace_state(&workspace);
    workspace.write_editor_file(&query.path, &request.content)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn parse_scoped_editor_content(
    State(state): State<Arc<RwLock<ServerState>>>,
    AxumPath(workspace_id): AxumPath<String>,
    Json(request): Json<EditorParseRequest>,
) -> Result<Json<EditorParseResponseDto>, HttpApiError> {
    let workspace = workspace_handle(&state, &workspace_id)?;
    let workspace = read_workspace_state(&workspace);
    Ok(Json(
        workspace.parse_editor_content(&request.path, &request.content)?,
    ))
}

async fn format_scoped_editor_content(
    State(state): State<Arc<RwLock<ServerState>>>,
    AxumPath(workspace_id): AxumPath<String>,
    Json(request): Json<EditorParseRequest>,
) -> Result<Json<EditorFormatResponseDto>, HttpApiError> {
    let workspace = workspace_handle(&state, &workspace_id)?;
    let workspace = read_workspace_state(&workspace);
    Ok(Json(
        workspace.format_editor_content(&request.path, &request.content)?,
    ))
}

async fn compile_scoped_editor_semantic_content(
    State(state): State<Arc<RwLock<ServerState>>>,
    AxumPath(workspace_id): AxumPath<String>,
    Json(request): Json<EditorParseRequest>,
) -> Result<Json<EditorSemanticCompileResponseDto>, HttpApiError> {
    let workspace = workspace_handle(&state, &workspace_id)?;
    let workspace = read_workspace_state(&workspace);
    Ok(Json(workspace.compile_editor_semantic_content(
        &request.path,
        &request.content,
    )?))
}

async fn lint_scoped_editor_content(
    State(state): State<Arc<RwLock<ServerState>>>,
    AxumPath(workspace_id): AxumPath<String>,
    Json(request): Json<EditorParseRequest>,
) -> Result<Json<EditorLintResponseDto>, HttpApiError> {
    let workspace = workspace_handle(&state, &workspace_id)?;
    let workspace = read_workspace_state(&workspace);
    Ok(Json(
        workspace.lint_editor_content(&request.path, &request.content)?,
    ))
}

async fn refresh_scoped_editor_model(
    State(state): State<Arc<RwLock<ServerState>>>,
    AxumPath(workspace_id): AxumPath<String>,
    Json(request): Json<EditorRefreshRequest>,
) -> Result<Json<EditorRefreshResponseDto>, HttpApiError> {
    let workspace = workspace_handle(&state, &workspace_id)?;
    let mut workspace = write_workspace_state(&workspace);
    Ok(Json(workspace.refresh_from_editor_path(&request.path)?))
}

async fn get_scoped_semantic_workspace_session(
    State(state): State<Arc<RwLock<ServerState>>>,
    AxumPath(workspace_id): AxumPath<String>,
) -> Result<Json<SemanticWorkspaceSessionDto>, HttpApiError> {
    let workspace = workspace_handle(&state, &workspace_id)?;
    let workspace = read_workspace_state(&workspace);
    Ok(Json(workspace.semantic_workspace_session()?))
}

async fn compile_scoped_semantic_project(
    State(state): State<Arc<RwLock<ServerState>>>,
    AxumPath(workspace_id): AxumPath<String>,
    Json(request): Json<SemanticProjectCompileRequestDto>,
) -> Result<Json<SemanticProjectCompileResponseDto>, HttpApiError> {
    let workspace = workspace_handle(&state, &workspace_id)?;
    let workspace = read_workspace_state(&workspace);
    Ok(Json(workspace.compile_project_scope(
        &request.project_path,
        &request.staged_files,
    )?))
}

async fn lint_scoped_semantic_project(
    State(state): State<Arc<RwLock<ServerState>>>,
    AxumPath(workspace_id): AxumPath<String>,
    Json(request): Json<SemanticProjectCompileRequestDto>,
) -> Result<Json<ProjectLintResponseDto>, HttpApiError> {
    let workspace = workspace_handle(&state, &workspace_id)?;
    let workspace = read_workspace_state(&workspace);
    Ok(Json(workspace.lint_project_scope(
        &request.project_path,
        &request.staged_files,
    )?))
}

async fn create_editor_file(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(request): Json<CreateEditorFileRequest>,
) -> Result<Json<EditorFileContentDto>, HttpApiError> {
    let mut state = write_server_state(&state);
    Ok(Json(state.create_editor_file(
        &request.path,
        request.template.as_deref(),
    )?))
}

async fn get_editor_file(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(query): Query<EditorPathQuery>,
) -> Result<Json<EditorFileContentDto>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.read_editor_file(&query.path)?))
}

async fn get_editor_outline(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(query): Query<EditorPathQuery>,
) -> Result<Json<Vec<EditorOutlineNodeDto>>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.editor_outline(&query.path)?))
}

async fn get_editor_semantic_outline(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(query): Query<EditorPathQuery>,
) -> Result<Json<Vec<EditorOutlineNodeDto>>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.editor_semantic_outline(&query.path)?))
}

async fn get_editor_semantic_element(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(query): Query<SemanticElementQuery>,
) -> Result<Json<ElementDetailsDto>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.semantic_element(&query.path, &query.id)?))
}

async fn put_editor_file(
    State(state): State<Arc<RwLock<ServerState>>>,
    Query(query): Query<EditorPathQuery>,
    Json(request): Json<UpdateEditorFileRequest>,
) -> Result<StatusCode, HttpApiError> {
    let mut state = write_server_state(&state);
    state.write_editor_file(&query.path, &request.content)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn parse_editor_content(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(request): Json<EditorParseRequest>,
) -> Result<Json<EditorParseResponseDto>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(
        state.parse_editor_content(&request.path, &request.content)?,
    ))
}

async fn format_editor_content(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(request): Json<EditorParseRequest>,
) -> Result<Json<EditorFormatResponseDto>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(
        state.format_editor_content(&request.path, &request.content)?,
    ))
}

async fn compile_editor_semantic_content(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(request): Json<EditorParseRequest>,
) -> Result<Json<EditorSemanticCompileResponseDto>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.compile_editor_semantic_content(
        &request.path,
        &request.content,
    )?))
}

async fn lint_editor_content(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(request): Json<EditorParseRequest>,
) -> Result<Json<EditorLintResponseDto>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(
        state.lint_editor_content(&request.path, &request.content)?,
    ))
}

async fn refresh_editor_model(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(request): Json<EditorRefreshRequest>,
) -> Result<Json<EditorRefreshResponseDto>, HttpApiError> {
    let mut state = write_server_state(&state);
    Ok(Json(state.refresh_from_editor_path(&request.path)?))
}

async fn compile_semantic_project(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(request): Json<SemanticProjectCompileRequestDto>,
) -> Result<Json<SemanticProjectCompileResponseDto>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.compile_project_scope(
        &request.project_path,
        &request.staged_files,
    )?))
}

async fn lint_semantic_project(
    State(state): State<Arc<RwLock<ServerState>>>,
    Json(request): Json<SemanticProjectCompileRequestDto>,
) -> Result<Json<ProjectLintResponseDto>, HttpApiError> {
    let state = read_server_state(&state);
    Ok(Json(state.lint_project_scope(
        &request.project_path,
        &request.staged_files,
    )?))
}

async fn get_v2_status() -> Json<V2StatusDto> {
    Json(V2StatusDto {
        service: "mercurio-v2-api".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        store_root: v2_store_root().display().to_string(),
    })
}

async fn get_v2_git_status(
    State(state): State<Arc<RwLock<ServerState>>>,
) -> Result<Json<V2GitStatusDto>, PackageApiError> {
    let state = read_server_state(&state);
    let repository_root = state.workspace_root().to_path_buf();
    let branch = git_output(&repository_root, ["branch", "--show-current"]).ok();
    let head = git_output(&repository_root, ["rev-parse", "HEAD"]).ok();
    let dirty_files = git_output(&repository_root, ["status", "--short"])
        .map(|output| {
            output
                .lines()
                .filter_map(|line| line.get(3..).map(str::trim))
                .filter(|path| !path.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let recent_commits = read_server_project_git_history(&repository_root).unwrap_or_default();

    Ok(Json(V2GitStatusDto {
        repository_root: repository_root.display().to_string(),
        branch: branch.filter(|value| !value.is_empty()),
        head: head.filter(|value| !value.is_empty()),
        dirty_files,
        recent_commits,
    }))
}

async fn list_v2_proposals() -> Result<Json<V2ProposalListDto>, PackageApiError> {
    Ok(Json(V2ProposalListDto {
        items: read_all_v2_proposals()?,
    }))
}

async fn create_v2_proposal(
    Json(request): Json<V2ProposalCreateRequestDto>,
) -> Result<Json<V2ProposalDto>, PackageApiError> {
    let title = request.title.trim();
    if title.is_empty() {
        return Err(PackageApiError::bad_request(
            "proposal title cannot be empty",
        ));
    }

    let key = match request.key {
        Some(key) => normalize_v2_proposal_key(&key)?,
        None => next_v2_proposal_key()?,
    };
    let item_path = v2_proposal_path(&key)?;
    if item_path.exists() {
        return Err(PackageApiError::conflict(format!(
            "proposal already exists: {key}"
        )));
    }

    let now = unix_time_ms();
    let item = V2ProposalDto {
        key,
        title: title.to_string(),
        description: request.description.and_then(non_empty_string),
        item_type: request
            .item_type
            .and_then(non_empty_string)
            .unwrap_or_else(|| "task".to_string()),
        status: request
            .status
            .and_then(non_empty_string)
            .unwrap_or_else(|| "Backlog".to_string()),
        priority: request
            .priority
            .and_then(non_empty_string)
            .unwrap_or_else(|| "Medium".to_string()),
        labels: normalize_string_list(request.labels),
        created_at_unix_ms: now,
        updated_at_unix_ms: now,
        linked_commits: Vec::new(),
        linked_semantic_elements: normalize_string_list(request.linked_semantic_elements),
        external_bindings: normalize_v2_external_bindings(request.external_bindings),
        related_work_items: normalize_string_list(request.related_work_items),
        pull_request: request
            .pull_request
            .and_then(normalize_v2_proposal_pull_request),
    };
    write_v2_proposal(&item)?;
    Ok(Json(item))
}

async fn get_v2_proposal(
    AxumPath(key): AxumPath<String>,
) -> Result<Json<V2ProposalDto>, PackageApiError> {
    Ok(Json(read_v2_proposal(&key)?))
}

async fn update_v2_proposal(
    AxumPath(key): AxumPath<String>,
    Json(request): Json<V2ProposalUpdateRequestDto>,
) -> Result<Json<V2ProposalDto>, PackageApiError> {
    let mut item = read_v2_proposal(&key)?;
    if let Some(title) = request.title {
        let title = title.trim();
        if title.is_empty() {
            return Err(PackageApiError::bad_request(
                "proposal title cannot be empty",
            ));
        }
        item.title = title.to_string();
    }
    if let Some(description) = request.description {
        item.description = description.and_then(non_empty_string);
    }
    if let Some(item_type) = request.item_type.and_then(non_empty_string) {
        item.item_type = item_type;
    }
    if let Some(status) = request.status.and_then(non_empty_string) {
        item.status = status;
    }
    if let Some(priority) = request.priority.and_then(non_empty_string) {
        item.priority = priority;
    }
    if let Some(labels) = request.labels {
        item.labels = normalize_string_list(labels);
    }
    if let Some(elements) = request.linked_semantic_elements {
        item.linked_semantic_elements = normalize_string_list(elements);
    }
    if let Some(bindings) = request.external_bindings {
        item.external_bindings = normalize_v2_external_bindings(bindings);
    }
    if let Some(related) = request.related_work_items {
        item.related_work_items = normalize_string_list(related);
    }
    if let Some(pull_request) = request.pull_request {
        item.pull_request = pull_request.and_then(normalize_v2_proposal_pull_request);
    }
    item.updated_at_unix_ms = unix_time_ms();
    write_v2_proposal(&item)?;
    Ok(Json(item))
}

async fn update_v2_proposal_pull_request(
    AxumPath(key): AxumPath<String>,
    Json(request): Json<V2ProposalPullRequestUpdateDto>,
) -> Result<Json<V2ProposalDto>, PackageApiError> {
    let mut item = read_v2_proposal(&key)?;
    let branch = non_empty_string(request.branch)
        .ok_or_else(|| PackageApiError::bad_request("pull request branch cannot be empty"))?;
    let provider = request
        .provider
        .and_then(non_empty_string)
        .unwrap_or_else(|| "gitea".to_string());
    let base_branch = request
        .base_branch
        .and_then(non_empty_string)
        .unwrap_or_else(|| "main".to_string());
    let state = request
        .pull_request_state
        .and_then(non_empty_string)
        .unwrap_or_else(|| "planned".to_string());
    item.pull_request = normalize_v2_proposal_pull_request(V2ProposalPullRequestDto {
        provider,
        repository_id: request.repository_id.and_then(non_empty_string),
        repository_external_id: request.repository_external_id.and_then(non_empty_string),
        branch,
        base_branch,
        pull_request_id: request.pull_request_id.and_then(non_empty_string),
        pull_request_url: request.pull_request_url.and_then(non_empty_string),
        pull_request_state: state,
        mergeable: request.mergeable,
        review_state: request.review_state.and_then(non_empty_string),
        last_synced_at_unix_ms: Some(unix_time_ms()),
        metadata: request.metadata,
    });
    item.status = "PR planned".to_string();
    item.updated_at_unix_ms = unix_time_ms();
    write_v2_proposal(&item)?;
    Ok(Json(item))
}

async fn create_v2_problem_report(
    AxumPath(key): AxumPath<String>,
    Json(request): Json<V2ProblemReportCreateRequestDto>,
) -> Result<Json<V2ProblemReportCreateResponseDto>, PackageApiError> {
    let mut discussion = read_v2_proposal(&key)?;
    let title = request.title.trim();
    if title.is_empty() {
        return Err(PackageApiError::bad_request(
            "problem report title cannot be empty",
        ));
    }

    let now = unix_time_ms();
    let report_key = next_v2_proposal_key()?;
    let repository_external_id = request
        .repository_external_id
        .clone()
        .and_then(non_empty_string)
        .or_else(|| first_repository_external_id_for_demo().ok().flatten());
    let repository_id = request
        .repository_id
        .clone()
        .and_then(non_empty_string)
        .or_else(|| first_repository_id_for_demo().ok().flatten());
    let binding = make_gitea_problem_report_binding(
        &repository_id,
        &repository_external_id,
        title,
        request.description.as_deref().unwrap_or(""),
    );
    let mut labels = vec!["problem-report".to_string(), "backing-system".to_string()];
    labels.extend(binding.state.clone().map(|state| format!("gitea-{state}")));

    let problem_report = V2ProposalDto {
        key: report_key.clone(),
        title: title.to_string(),
        description: request.description.and_then(non_empty_string),
        item_type: "problem_report".to_string(),
        status: "Open".to_string(),
        priority: "High".to_string(),
        labels: normalize_string_list(labels),
        created_at_unix_ms: now,
        updated_at_unix_ms: now,
        linked_commits: Vec::new(),
        linked_semantic_elements: normalize_string_list(request.linked_semantic_elements),
        external_bindings: normalize_v2_external_bindings(vec![binding]),
        related_work_items: normalize_string_list(vec![discussion.key.clone()]),
        pull_request: None,
    };

    discussion.related_work_items.push(report_key.clone());
    discussion.related_work_items = normalize_string_list(discussion.related_work_items);
    if !discussion.labels.iter().any(|label| label == "discussion") {
        discussion.labels.push("discussion".to_string());
        discussion.labels = normalize_string_list(discussion.labels);
    }
    if discussion.item_type == "task" || discussion.item_type == "proposal" {
        discussion.item_type = "discussion".to_string();
    }
    discussion.status = "Problem reports linked".to_string();
    discussion.updated_at_unix_ms = unix_time_ms();

    write_v2_proposal(&problem_report)?;
    write_v2_proposal(&discussion)?;
    Ok(Json(V2ProblemReportCreateResponseDto {
        discussion,
        problem_report,
    }))
}

async fn advance_v2_demo_cycle(
    AxumPath(key): AxumPath<String>,
    Json(request): Json<V2DemoCycleAdvanceRequestDto>,
) -> Result<Json<V2ProposalDto>, PackageApiError> {
    let mut item = read_v2_proposal(&key)?;
    let action = non_empty_string(request.action)
        .ok_or_else(|| PackageApiError::bad_request("demo cycle action cannot be empty"))?;
    let now = unix_time_ms();
    match action.as_str() {
        "task_ai" => {
            let repository_external_id = request
                .repository_external_id
                .and_then(non_empty_string)
                .or_else(|| first_repository_external_id_for_demo().ok().flatten());
            let repository_id = request
                .repository_id
                .and_then(non_empty_string)
                .or_else(|| first_repository_id_for_demo().ok().flatten());
            let base_branch = request
                .base_branch
                .and_then(non_empty_string)
                .unwrap_or_else(|| "main".to_string());
            let branch = format!(
                "ai/{}/{}",
                item.key.to_ascii_lowercase(),
                slug_for_branch(&item.title)
            );
            item.pull_request = normalize_v2_proposal_pull_request(V2ProposalPullRequestDto {
                provider: "gitea".to_string(),
                repository_id,
                repository_external_id,
                branch,
                base_branch,
                pull_request_id: None,
                pull_request_url: None,
                pull_request_state: "implementation_tasked".to_string(),
                mergeable: None,
                review_state: Some("pending_semantic_review".to_string()),
                last_synced_at_unix_ms: Some(now),
                metadata: BTreeMap::from([
                    (
                        "source".to_string(),
                        Value::String("demo-cycle".to_string()),
                    ),
                    (
                        "actor".to_string(),
                        Value::String("ask-mercurio".to_string()),
                    ),
                ]),
            });
            add_label(&mut item.labels, "ai-tasked");
            item.status = "AI implementation tasked".to_string();
        }
        "review_semantic_diff" => {
            add_external_binding(
                &mut item.external_bindings,
                V2ExternalBindingDto {
                    provider: "mercurio".to_string(),
                    kind: "semantic_review".to_string(),
                    repository_id: None,
                    repository_external_id: None,
                    external_id: Some(format!("semantic-review-{}", item.key.to_ascii_lowercase())),
                    external_url: None,
                    state: Some("ready".to_string()),
                    title: Some(format!("Semantic review for {}", item.key)),
                    last_synced_at_unix_ms: Some(now),
                    metadata: BTreeMap::from([(
                        "summary".to_string(),
                        Value::String("Semantic differences ready for human review".to_string()),
                    )]),
                },
            );
            if let Some(pull_request) = item.pull_request.as_mut() {
                pull_request.review_state = Some("semantic_review_ready".to_string());
                pull_request.last_synced_at_unix_ms = Some(now);
            }
            add_label(&mut item.labels, "semantic-review");
            item.status = "Semantic review ready".to_string();
        }
        "approve" => {
            add_external_binding(
                &mut item.external_bindings,
                V2ExternalBindingDto {
                    provider: "mercurio".to_string(),
                    kind: "review".to_string(),
                    repository_id: None,
                    repository_external_id: None,
                    external_id: Some(format!("approval-{}", item.key.to_ascii_lowercase())),
                    external_url: None,
                    state: Some("approved".to_string()),
                    title: Some(format!("Human approval for {}", item.key)),
                    last_synced_at_unix_ms: Some(now),
                    metadata: BTreeMap::new(),
                },
            );
            if let Some(pull_request) = item.pull_request.as_mut() {
                pull_request.review_state = Some("approved".to_string());
                pull_request.mergeable = Some(true);
                pull_request.last_synced_at_unix_ms = Some(now);
            }
            add_label(&mut item.labels, "approved");
            item.status = "Approved".to_string();
        }
        "merge" => {
            if let Some(pull_request) = item.pull_request.as_mut() {
                pull_request.pull_request_state = "merged".to_string();
                pull_request.review_state = Some("merged".to_string());
                pull_request.last_synced_at_unix_ms = Some(now);
            }
            add_external_binding(
                &mut item.external_bindings,
                V2ExternalBindingDto {
                    provider: "gitea".to_string(),
                    kind: "merge".to_string(),
                    repository_id: item
                        .pull_request
                        .as_ref()
                        .and_then(|pr| pr.repository_id.clone()),
                    repository_external_id: item
                        .pull_request
                        .as_ref()
                        .and_then(|pr| pr.repository_external_id.clone()),
                    external_id: Some(format!("merge-{}", item.key.to_ascii_lowercase())),
                    external_url: None,
                    state: Some("merged".to_string()),
                    title: Some(format!("Merged {}", item.key)),
                    last_synced_at_unix_ms: Some(now),
                    metadata: BTreeMap::new(),
                },
            );
            add_label(&mut item.labels, "merged");
            item.status = "Merged".to_string();
        }
        _ => {
            return Err(PackageApiError::bad_request(
                "demo cycle action must be task_ai, review_semantic_diff, approve, or merge",
            ));
        }
    }
    item.external_bindings = normalize_v2_external_bindings(item.external_bindings);
    item.labels = normalize_string_list(item.labels);
    item.updated_at_unix_ms = now;
    write_v2_proposal(&item)?;
    Ok(Json(item))
}

async fn commit_v2_proposal_changes(
    AxumPath(key): AxumPath<String>,
    Json(request): Json<V2ProposalCommitRequestDto>,
) -> Result<Json<V2ProposalCommitResponseDto>, PackageApiError> {
    let mut item = read_v2_proposal(&key)?;
    if request.changed_files.is_empty() {
        return Err(PackageApiError::bad_request(
            "commit request must include at least one changed file",
        ));
    }
    let message = request.message.trim();
    if message.is_empty() {
        return Err(PackageApiError::bad_request(
            "commit message cannot be empty",
        ));
    }

    let project_id = match request.project_id {
        Some(project_id) => {
            validate_package_identifier(&project_id, "project id")?;
            project_id
        }
        None => infer_single_server_project_id()?,
    };
    read_server_project_metadata(&project_id)?;
    let working_files_root = server_project_working_files_root(&project_id)?;
    let working_revision = compute_files_revision(&working_files_root)?;
    if let Some(base_revision_id) = request.base_revision_id.as_deref()
        && base_revision_id != working_revision
    {
        return Err(PackageApiError::conflict(format!(
            "base revision mismatch: current revision is {}",
            working_revision
        )));
    }

    let staged_files = request
        .changed_files
        .iter()
        .map(|file| {
            Ok(StagedEditorFileDto {
                path: normalize_server_project_file_path(&file.path)?,
                content: file.content.clone(),
            })
        })
        .collect::<Result<Vec<_>, PackageApiError>>()?;
    let workspace = WorkspaceService::from_workspace_root_diagnostics_only(&working_files_root)
        .map_err(|err| {
            PackageApiError::internal(format!("failed to load server project: {err}"))
        })?;
    let semantic = workspace
        .compile_project_scope(".", &staged_files)
        .map_err(|err| {
            PackageApiError::internal(format!("failed to compile v2 commit changes: {err}"))
        })?;
    if !semantic.ok {
        return Err(PackageApiError::bad_request(
            "commit rejected because staged SysML/KerML does not compile",
        ));
    }

    for file in &staged_files {
        let file_path = working_files_root.join(&file.path);
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                PackageApiError::internal(format!("failed to create server project folder: {err}"))
            })?;
        }
        std::fs::write(&file_path, &file.content).map_err(|err| {
            PackageApiError::internal(format!("failed to write server project file: {err}"))
        })?;
    }

    let project = commit_server_project_working_copy(
        AxumPath(project_id.clone()),
        Json(ServerProjectCommitRequestDto {
            summary: Some(format!("{key}: {message}")),
        }),
    )
    .await?
    .0;
    let project_dir = server_project_dir(&project_id)?;
    let commit = read_server_project_git_history(&project_dir)?
        .into_iter()
        .next()
        .ok_or_else(|| PackageApiError::internal("v2 commit did not produce git history"))?;
    if !item.linked_commits.iter().any(|id| id == &commit.id) {
        item.linked_commits.push(commit.id.clone());
    }
    item.status = "In Progress".to_string();
    item.updated_at_unix_ms = unix_time_ms();
    write_v2_proposal(&item)?;

    Ok(Json(V2ProposalCommitResponseDto {
        proposal: item,
        project: project.clone(),
        commit,
        semantic: ServerProjectCompileResponseDto {
            project_id,
            revision_id: project.revision_id,
            ok: semantic.ok,
            semantic,
        },
    }))
}

fn workspace_status(state: &ServerState) -> WorkspaceStatus {
    workspace_status_for(state)
}

fn workspace_status_for(workspace: &WorkspaceService) -> WorkspaceStatus {
    WorkspaceStatus {
        is_open: true,
        workspace_root: Some(workspace.workspace_root().display().to_string()),
        active_path: workspace.active_path(),
        project: Some(workspace.project_info()),
    }
}

fn workspace_from_open_mode(
    path: &Path,
    mode: WorkspaceOpenMode,
) -> Result<WorkspaceService, ApiError> {
    match mode {
        WorkspaceOpenMode::Shell => WorkspaceService::from_workspace_root_shell(path),
        WorkspaceOpenMode::Lazy => WorkspaceService::from_open_path_lazy_model_sources(path),
        WorkspaceOpenMode::Compiled => WorkspaceService::from_workspace_root_compiled(path),
    }
}

fn default_package_artifact_name() -> String {
    "package.kpar".to_string()
}

fn default_package_kind() -> String {
    "mercurio.model.package".to_string()
}

fn default_package_schema_version() -> u32 {
    1
}

fn server_data_root() -> PathBuf {
    std::env::var_os("MERCURIO_SERVER_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| mercurio_core::paths::default_workspace_root().join(".server"))
}

fn server_projects_root() -> PathBuf {
    server_data_root().join("projects")
}

fn v2_store_root() -> PathBuf {
    server_data_root().join("v2")
}

fn v2_proposals_root() -> PathBuf {
    v2_store_root().join("work-items")
}

fn ai_settings_path() -> PathBuf {
    server_data_root().join("ai-settings.json")
}

fn ai_secrets_path() -> PathBuf {
    server_data_root().join("ai-secrets.json")
}

fn ai_usage_path() -> PathBuf {
    server_data_root().join("ai-usage.json")
}

fn v2_proposal_path(key: &str) -> Result<PathBuf, PackageApiError> {
    let key = normalize_v2_proposal_key(key)?;
    Ok(v2_proposals_root().join(format!("{key}.json")))
}

fn watched_repositories_root() -> PathBuf {
    server_data_root().join("repositories")
}

fn watched_artifacts_root() -> PathBuf {
    server_data_root().join("artifacts")
}

fn watched_artifact_dir(artifact_id: &str) -> Result<PathBuf, PackageApiError> {
    validate_package_identifier(artifact_id, "artifact id")?;
    Ok(watched_artifacts_root().join(artifact_id))
}

fn watched_repo_cache_root() -> PathBuf {
    server_data_root().join("repo-cache")
}

fn watched_repo_snapshot_root() -> PathBuf {
    server_data_root().join("repo-snapshots")
}

fn watched_repo_cache_dir(repository_id: &str) -> Result<PathBuf, PackageApiError> {
    validate_package_identifier(repository_id, "repository id")?;
    Ok(watched_repo_cache_root().join(format!("{repository_id}.git")))
}

fn watched_repository_dir(repository_id: &str) -> Result<PathBuf, PackageApiError> {
    validate_package_identifier(repository_id, "repository id")?;
    Ok(watched_repositories_root().join(repository_id))
}

fn watched_repository_projects_root(repository_id: &str) -> Result<PathBuf, PackageApiError> {
    Ok(watched_repository_dir(repository_id)?.join("watched-projects"))
}

fn watched_project_dir(
    repository_id: &str,
    watched_project_id: &str,
) -> Result<PathBuf, PackageApiError> {
    validate_package_identifier(watched_project_id, "watched project id")?;
    Ok(watched_repository_projects_root(repository_id)?.join(watched_project_id))
}

fn server_workspaces_root() -> PathBuf {
    server_data_root().join("workspaces")
}

fn server_project_dir(project_id: &str) -> Result<PathBuf, PackageApiError> {
    validate_package_identifier(project_id, "project id")?;
    Ok(server_projects_root().join(project_id))
}

fn default_server_user() -> ServerUserDto {
    ServerUserDto {
        id: "mark".to_string(),
        display_name: "Mark".to_string(),
        email: Some("mark@mercurio.local".to_string()),
    }
}

fn server_project_working_dir(project_id: &str) -> Result<PathBuf, PackageApiError> {
    validate_package_identifier(project_id, "project id")?;
    Ok(server_workspaces_root()
        .join(default_server_user().id)
        .join(project_id))
}

fn server_project_files_root(project_id: &str) -> Result<PathBuf, PackageApiError> {
    Ok(server_project_dir(project_id)?.join("files"))
}

fn server_project_working_files_root(project_id: &str) -> Result<PathBuf, PackageApiError> {
    ensure_server_project_working_copy(project_id)?;
    Ok(server_project_working_dir(project_id)?.join("files"))
}

fn server_project_file_path(
    project_id: &str,
    relative_path: &str,
) -> Result<PathBuf, PackageApiError> {
    Ok(server_project_files_root(project_id)?.join(relative_path))
}

fn read_all_watched_repositories() -> Result<Vec<WatchedRepositoryMetadataDto>, PackageApiError> {
    let root = watched_repositories_root();
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut repositories = Vec::new();
    let entries = std::fs::read_dir(&root).map_err(|err| {
        PackageApiError::internal(format!("failed to read watched repositories: {err}"))
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| PackageApiError::internal(err.to_string()))?;
        if !entry
            .file_type()
            .map_err(|err| PackageApiError::internal(err.to_string()))?
            .is_dir()
        {
            continue;
        }
        let metadata_path = entry.path().join("metadata.json");
        if !metadata_path.exists() {
            continue;
        }
        repositories.push(read_watched_repository_metadata_by_dir(&entry.path())?);
    }
    repositories.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    Ok(repositories)
}

fn read_watched_repository_metadata(
    repository_id: &str,
) -> Result<WatchedRepositoryMetadataDto, PackageApiError> {
    let repository_dir = watched_repository_dir(repository_id)?;
    if !repository_dir.exists() {
        return Err(PackageApiError::not_found(format!(
            "watched repository not found: {repository_id}"
        )));
    }
    read_watched_repository_metadata_by_dir(&repository_dir)
}

fn read_watched_repository_metadata_by_dir(
    repository_dir: &Path,
) -> Result<WatchedRepositoryMetadataDto, PackageApiError> {
    let content = std::fs::read_to_string(repository_dir.join("metadata.json")).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            PackageApiError::not_found("watched repository metadata not found")
        } else {
            PackageApiError::internal(format!("failed to read watched repository metadata: {err}"))
        }
    })?;
    serde_json::from_str(&content).map_err(|err| {
        PackageApiError::internal(format!("invalid watched repository metadata: {err}"))
    })
}

fn write_watched_repository_metadata(
    repository_dir: &Path,
    metadata: &WatchedRepositoryMetadataDto,
) -> Result<(), PackageApiError> {
    let content = serde_json::to_string_pretty(metadata)
        .map_err(|err| PackageApiError::internal(err.to_string()))?;
    std::fs::write(repository_dir.join("metadata.json"), content).map_err(|err| {
        PackageApiError::internal(format!(
            "failed to write watched repository metadata: {err}"
        ))
    })
}

fn read_all_watched_projects(
    repository_filter: Option<&str>,
) -> Result<Vec<WatchedProjectMetadataDto>, PackageApiError> {
    let repositories = match repository_filter {
        Some(repository_id) => vec![read_watched_repository_metadata(repository_id)?],
        None => read_all_watched_repositories()?,
    };

    let mut projects = Vec::new();
    for repository in repositories {
        let projects_root = watched_repository_projects_root(&repository.id)?;
        if !projects_root.exists() {
            continue;
        }
        let entries = std::fs::read_dir(&projects_root).map_err(|err| {
            PackageApiError::internal(format!("failed to read watched projects: {err}"))
        })?;
        for entry in entries {
            let entry = entry.map_err(|err| PackageApiError::internal(err.to_string()))?;
            if !entry
                .file_type()
                .map_err(|err| PackageApiError::internal(err.to_string()))?
                .is_dir()
            {
                continue;
            }
            let metadata_path = entry.path().join("metadata.json");
            if !metadata_path.exists() {
                continue;
            }
            projects.push(read_watched_project_metadata_by_dir(&entry.path())?);
        }
    }
    projects.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    Ok(projects)
}

fn read_watched_project_metadata_by_id(
    watched_project_id: &str,
) -> Result<WatchedProjectMetadataDto, PackageApiError> {
    Ok(read_watched_project_metadata_with_dir(watched_project_id)?.1)
}

fn read_watched_project_metadata_with_dir(
    watched_project_id: &str,
) -> Result<(PathBuf, WatchedProjectMetadataDto), PackageApiError> {
    validate_package_identifier(watched_project_id, "watched project id")?;
    for repository in read_all_watched_repositories()? {
        let project_dir = watched_project_dir(&repository.id, watched_project_id)?;
        if project_dir.join("metadata.json").exists() {
            return Ok((
                project_dir.clone(),
                read_watched_project_metadata_by_dir(&project_dir)?,
            ));
        }
    }
    Err(PackageApiError::not_found(format!(
        "watched project not found: {watched_project_id}"
    )))
}

fn read_watched_project_metadata_by_dir(
    project_dir: &Path,
) -> Result<WatchedProjectMetadataDto, PackageApiError> {
    let content = std::fs::read_to_string(project_dir.join("metadata.json")).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            PackageApiError::not_found("watched project metadata not found")
        } else {
            PackageApiError::internal(format!("failed to read watched project metadata: {err}"))
        }
    })?;
    serde_json::from_str(&content).map_err(|err| {
        PackageApiError::internal(format!("invalid watched project metadata: {err}"))
    })
}

fn write_watched_project_metadata(
    project_dir: &Path,
    metadata: &WatchedProjectMetadataDto,
) -> Result<(), PackageApiError> {
    let content = serde_json::to_string_pretty(metadata)
        .map_err(|err| PackageApiError::internal(err.to_string()))?;
    std::fs::write(project_dir.join("metadata.json"), content).map_err(|err| {
        PackageApiError::internal(format!("failed to write watched project metadata: {err}"))
    })
}

fn write_watched_project_artifact(
    artifact: &WatchedProjectArtifactDto,
) -> Result<(), PackageApiError> {
    let artifact_dir = watched_artifact_dir(&artifact.manifest.id)?;
    std::fs::create_dir_all(&artifact_dir).map_err(|err| {
        PackageApiError::internal(format!("failed to create watched project artifact: {err}"))
    })?;
    let content = serde_json::to_string_pretty(artifact)
        .map_err(|err| PackageApiError::internal(err.to_string()))?;
    std::fs::write(artifact_dir.join("artifact.json"), content).map_err(|err| {
        PackageApiError::internal(format!("failed to write watched project artifact: {err}"))
    })
}

fn read_watched_project_artifact(
    artifact_id: &str,
) -> Result<WatchedProjectArtifactDto, PackageApiError> {
    let artifact_dir = watched_artifact_dir(artifact_id)?;
    let content = std::fs::read_to_string(artifact_dir.join("artifact.json")).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            PackageApiError::not_found(format!("watched project artifact not found: {artifact_id}"))
        } else {
            PackageApiError::internal(format!("failed to read watched project artifact: {err}"))
        }
    })?;
    serde_json::from_str(&content).map_err(|err| {
        PackageApiError::internal(format!("invalid watched project artifact: {err}"))
    })
}

fn read_watched_project_artifact_manifests(
    watched_project_id: &str,
) -> Result<Vec<WatchedProjectArtifactManifestDto>, PackageApiError> {
    validate_package_identifier(watched_project_id, "watched project id")?;
    let root = watched_artifacts_root();
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut manifests = Vec::new();
    let entries = std::fs::read_dir(&root).map_err(|err| {
        PackageApiError::internal(format!("failed to read watched project artifacts: {err}"))
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| PackageApiError::internal(err.to_string()))?;
        if !entry
            .file_type()
            .map_err(|err| PackageApiError::internal(err.to_string()))?
            .is_dir()
        {
            continue;
        }
        let artifact_path = entry.path().join("artifact.json");
        if !artifact_path.exists() {
            continue;
        }
        let content = std::fs::read_to_string(&artifact_path).map_err(|err| {
            PackageApiError::internal(format!("failed to read watched project artifact: {err}"))
        })?;
        let artifact: WatchedProjectArtifactDto =
            serde_json::from_str(&content).map_err(|err| {
                PackageApiError::internal(format!("invalid watched project artifact: {err}"))
            })?;
        if artifact.manifest.watched_project_id == watched_project_id {
            manifests.push(hydrate_watched_artifact_manifest_git_metadata(
                artifact.manifest,
            ));
        }
    }
    manifests.sort_by(|left, right| {
        right
            .built_at_unix_ms
            .cmp(&left.built_at_unix_ms)
            .then(left.id.cmp(&right.id))
    });
    Ok(manifests)
}

fn project_commit_dag(
    watched_project: &WatchedProjectMetadataDto,
    repository: &WatchedRepositoryMetadataDto,
    mirror_dir: &Path,
    ref_name: &str,
    base_ref: Option<&str>,
    max_commits: usize,
    include_diffs: bool,
) -> Result<ProjectCommitDagDto, PackageApiError> {
    let ref_commit = resolve_watched_repository_commit(mirror_dir, ref_name)?;
    let range = if let Some(base_ref) = base_ref {
        let base_commit = resolve_watched_repository_commit(mirror_dir, base_ref)?;
        format!("{base_commit}..{ref_commit}")
    } else {
        ref_commit
    };
    let commits = git_commit_dag_entries(mirror_dir, &range, max_commits)?;
    let node_commits = commits
        .iter()
        .map(|commit| commit.commit.clone())
        .collect::<BTreeSet<_>>();
    let artifacts_by_commit = latest_watched_artifact_manifest_by_commit(&watched_project.id)?;

    let mut nodes = Vec::with_capacity(commits.len());
    for commit in commits {
        let refs = git_refs_pointing_at_commit(mirror_dir, &commit.commit).unwrap_or_default();
        let branches = refs
            .iter()
            .filter_map(|value| normalize_watched_repository_branch_ref(value))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let semantic_artifact = artifacts_by_commit.get(&commit.commit).cloned();
        nodes.push(ProjectCommitDagNodeDto {
            commit: commit.commit,
            parents: commit.parents,
            summary: commit.summary,
            author: commit.author,
            timestamp: commit.timestamp,
            refs,
            branches,
            semantic_artifact,
        });
    }

    let mut edges = Vec::new();
    for node in &nodes {
        for parent in &node.parents {
            if !node_commits.contains(parent) {
                continue;
            }
            let semantic_diff = if include_diffs {
                artifacts_by_commit
                    .get(parent)
                    .zip(artifacts_by_commit.get(&node.commit))
                    .and_then(|(base, target)| diff_watched_artifacts(&base.id, &target.id).ok())
            } else {
                None
            };
            edges.push(ProjectCommitDagEdgeDto {
                parent: parent.clone(),
                child: node.commit.clone(),
                semantic_diff,
            });
        }
    }

    Ok(ProjectCommitDagDto {
        project_id: watched_project.id.clone(),
        repository_id: repository.id.clone(),
        repository_url: repository.url.clone(),
        ref_name: ref_name.to_string(),
        base_ref: base_ref.map(str::to_string),
        max_commits,
        nodes,
        edges,
    })
}

#[derive(Debug, Clone)]
struct GitCommitDagEntry {
    commit: String,
    parents: Vec<String>,
    author: String,
    timestamp: String,
    summary: String,
}

fn git_commit_dag_entries(
    mirror_dir: &Path,
    range: &str,
    max_commits: usize,
) -> Result<Vec<GitCommitDagEntry>, PackageApiError> {
    let max_count = format!("--max-count={max_commits}");
    let output = git_output(
        mirror_dir,
        [
            "log",
            "--date-order",
            "--topo-order",
            "--date=iso-strict",
            "--format=%H%x1f%P%x1f%an%x1f%aI%x1f%s",
            &max_count,
            range,
        ],
    )?;
    Ok(output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\u{1f}');
            let commit = fields.next()?.trim().to_string();
            let parents = fields
                .next()
                .unwrap_or_default()
                .split_whitespace()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect();
            let author = fields.next().unwrap_or_default().to_string();
            let timestamp = fields.next().unwrap_or_default().to_string();
            let summary = fields.next().unwrap_or_default().to_string();
            if commit.is_empty() {
                return None;
            }
            Some(GitCommitDagEntry {
                commit,
                parents,
                author,
                timestamp,
                summary,
            })
        })
        .collect())
}

fn git_refs_pointing_at_commit(
    mirror_dir: &Path,
    commit: &str,
) -> Result<Vec<String>, PackageApiError> {
    let refs = git_output(
        mirror_dir,
        [
            "for-each-ref",
            "--points-at",
            commit,
            "--format=%(refname:short)",
        ],
    )?;
    Ok(refs
        .lines()
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "HEAD" && !value.ends_with("/HEAD"))
        .map(str::to_string)
        .collect())
}

fn latest_watched_artifact_manifest_by_commit(
    watched_project_id: &str,
) -> Result<BTreeMap<String, WatchedProjectArtifactManifestDto>, PackageApiError> {
    let mut by_commit = BTreeMap::<String, WatchedProjectArtifactManifestDto>::new();
    for artifact in read_watched_project_artifact_manifests(watched_project_id)? {
        let replace = by_commit
            .get(&artifact.commit)
            .map(|existing| artifact.built_at_unix_ms > existing.built_at_unix_ms)
            .unwrap_or(true);
        if replace {
            by_commit.insert(artifact.commit.clone(), artifact);
        }
    }
    Ok(by_commit)
}

fn delete_watched_project_artifacts(watched_project_id: &str) -> Result<(), PackageApiError> {
    validate_package_identifier(watched_project_id, "watched project id")?;
    let root = watched_artifacts_root();
    if !root.exists() {
        return Ok(());
    }

    let entries = std::fs::read_dir(&root).map_err(|err| {
        PackageApiError::internal(format!("failed to read watched project artifacts: {err}"))
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| PackageApiError::internal(err.to_string()))?;
        if !entry
            .file_type()
            .map_err(|err| PackageApiError::internal(err.to_string()))?
            .is_dir()
        {
            continue;
        }
        let artifact_path = entry.path().join("artifact.json");
        if !artifact_path.exists() {
            continue;
        }
        let content = std::fs::read_to_string(&artifact_path).map_err(|err| {
            PackageApiError::internal(format!("failed to read watched project artifact: {err}"))
        })?;
        let artifact: WatchedProjectArtifactDto =
            serde_json::from_str(&content).map_err(|err| {
                PackageApiError::internal(format!("invalid watched project artifact: {err}"))
            })?;
        if artifact.manifest.watched_project_id == watched_project_id {
            std::fs::remove_dir_all(entry.path()).map_err(|err| {
                PackageApiError::internal(format!(
                    "failed to delete watched project artifact: {err}"
                ))
            })?;
        }
    }
    Ok(())
}

fn diff_watched_artifacts(
    base_artifact_id: &str,
    target_artifact_id: &str,
) -> Result<WatchedArtifactDiffDto, PackageApiError> {
    let base = read_watched_project_artifact(base_artifact_id)?;
    let target = read_watched_project_artifact(target_artifact_id)?;
    if base.manifest.watched_project_id != target.manifest.watched_project_id {
        return Err(PackageApiError::bad_request(
            "artifact diff requires artifacts from the same watched project",
        ));
    }

    let base_ids = base.elements.keys().cloned().collect::<BTreeSet<_>>();
    let target_ids = target.elements.keys().cloned().collect::<BTreeSet<_>>();

    let mut added = target_ids
        .difference(&base_ids)
        .filter_map(|id| {
            target
                .elements
                .get(id)
                .map(|element| diff_element_summary(id, element))
        })
        .collect::<Vec<_>>();
    let mut removed = base_ids
        .difference(&target_ids)
        .filter_map(|id| {
            base.elements
                .get(id)
                .map(|element| diff_element_summary(id, element))
        })
        .collect::<Vec<_>>();
    let mut changed = base_ids
        .intersection(&target_ids)
        .filter_map(|id| {
            let before = base.elements.get(id)?;
            let after = target.elements.get(id)?;
            (before != after).then(|| WatchedArtifactChangedElementDto {
                id: id.clone(),
                label: diff_element_string(after, "label")
                    .or_else(|| diff_element_string(before, "label")),
                kind: diff_element_string(after, "kind")
                    .or_else(|| diff_element_string(before, "kind")),
                before: before.clone(),
                after: after.clone(),
            })
        })
        .collect::<Vec<_>>();

    added.sort_by(|left, right| left.id.cmp(&right.id));
    removed.sort_by(|left, right| left.id.cmp(&right.id));
    changed.sort_by(|left, right| left.id.cmp(&right.id));

    Ok(WatchedArtifactDiffDto {
        base_artifact_id: base.manifest.id,
        target_artifact_id: target.manifest.id,
        base_commit: base.manifest.commit,
        target_commit: target.manifest.commit,
        added_count: added.len(),
        removed_count: removed.len(),
        changed_count: changed.len(),
        added,
        removed,
        changed,
    })
}

fn hydrate_watched_artifact_manifest_git_metadata(
    mut manifest: WatchedProjectArtifactManifestDto,
) -> WatchedProjectArtifactManifestDto {
    if let Ok(mirror_dir) = watched_repo_cache_dir(&manifest.repository_id) {
        if mirror_dir.exists() {
            if manifest.parent_commits.is_empty() {
                if let Ok(parents) = git_parent_commits(&mirror_dir, &manifest.commit) {
                    manifest.parent_commits = parents;
                }
            }
            if manifest.branches.is_empty() {
                if let Ok(branches) = git_branches_containing_commit(&mirror_dir, &manifest.commit)
                {
                    manifest.branches = branches;
                }
            }
        }
    }
    if manifest.ref_name.is_none() && manifest.branches.len() == 1 {
        manifest.ref_name = manifest.branches.first().cloned();
    }
    manifest
}

fn git_parent_commits(current_dir: &Path, commit: &str) -> Result<Vec<String>, PackageApiError> {
    let parents = git_output(current_dir, ["show", "-s", "--format=%P", commit])?;
    Ok(parents
        .split_whitespace()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect())
}

fn git_branches_containing_commit(
    current_dir: &Path,
    commit: &str,
) -> Result<Vec<String>, PackageApiError> {
    let refs = git_output(
        current_dir,
        [
            "for-each-ref",
            "--contains",
            commit,
            "--format=%(refname:short)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    Ok(refs
        .lines()
        .filter_map(normalize_watched_repository_branch_ref)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

fn diff_element_summary(id: &str, element: &Value) -> WatchedArtifactDiffElementDto {
    WatchedArtifactDiffElementDto {
        id: id.to_string(),
        label: diff_element_string(element, "label"),
        kind: diff_element_string(element, "kind"),
    }
}

fn diff_element_string(element: &Value, field: &str) -> Option<String> {
    element
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn indexing_jobs() -> &'static Arc<Mutex<BTreeMap<String, WatchedProjectIndexingJob>>> {
    static JOBS: OnceLock<Arc<Mutex<BTreeMap<String, WatchedProjectIndexingJob>>>> =
        OnceLock::new();
    JOBS.get_or_init(|| Arc::new(Mutex::new(BTreeMap::new())))
}

fn indexing_status_for_project(
    watched_project_id: &str,
) -> Result<WatchedProjectIndexingStatusDto, PackageApiError> {
    let jobs = indexing_jobs()
        .lock()
        .map_err(|_| PackageApiError::internal("indexing job registry is unavailable"))?;
    Ok(jobs
        .get(watched_project_id)
        .map(indexing_job_status_dto)
        .unwrap_or_else(|| idle_indexing_status(watched_project_id)))
}

fn idle_indexing_status(watched_project_id: &str) -> WatchedProjectIndexingStatusDto {
    WatchedProjectIndexingStatusDto {
        watched_project_id: watched_project_id.to_string(),
        status: "idle".to_string(),
        ref_name: None,
        message: None,
        artifact_id: None,
        started_at_unix_ms: None,
        finished_at_unix_ms: None,
        cancel_requested: false,
    }
}

fn indexing_job_status_dto(job: &WatchedProjectIndexingJob) -> WatchedProjectIndexingStatusDto {
    WatchedProjectIndexingStatusDto {
        watched_project_id: job.watched_project_id.clone(),
        status: job.status.clone(),
        ref_name: job.ref_name.clone(),
        message: job.message.clone(),
        artifact_id: job.artifact_id.clone(),
        started_at_unix_ms: job.started_at_unix_ms,
        finished_at_unix_ms: job.finished_at_unix_ms,
        cancel_requested: job.cancel_requested.load(Ordering::SeqCst),
    }
}

fn finish_watched_project_indexing_job(
    watched_project_id: &str,
    ref_name: Option<&str>,
    cancel_requested: Arc<AtomicBool>,
) {
    let result = refresh_watched_project_from_git_with_cancel(
        watched_project_id,
        ref_name,
        &cancel_requested,
    );
    let mut jobs = match indexing_jobs().lock() {
        Ok(jobs) => jobs,
        Err(_) => return,
    };
    if let Some(job) = jobs.get_mut(watched_project_id) {
        job.finished_at_unix_ms = Some(unix_time_ms());
        if cancel_requested.load(Ordering::SeqCst) {
            job.status = "canceled".to_string();
            job.message = Some("Indexing canceled".to_string());
            return;
        }
        match result {
            Ok(artifact) => {
                job.status = "succeeded".to_string();
                job.artifact_id = Some(artifact.manifest.id);
                job.message = Some("Indexing complete".to_string());
            }
            Err(err) => {
                job.status = "failed".to_string();
                job.message = Some(err.message);
            }
        }
    }
}

fn refresh_watched_project_from_git(
    watched_project_id: &str,
    requested_ref: Option<&str>,
) -> Result<WatchedProjectArtifactDto, PackageApiError> {
    let never_cancel = Arc::new(AtomicBool::new(false));
    refresh_watched_project_from_git_with_cancel(watched_project_id, requested_ref, &never_cancel)
}

fn refresh_watched_project_from_git_with_cancel(
    watched_project_id: &str,
    requested_ref: Option<&str>,
    cancel_requested: &AtomicBool,
) -> Result<WatchedProjectArtifactDto, PackageApiError> {
    let watched_project = read_watched_project_metadata_by_id(watched_project_id)?;
    let repository = read_watched_repository_metadata(&watched_project.repository_id)?;
    let mirror_dir = watched_repo_cache_dir(&repository.id)?;
    check_indexing_canceled(cancel_requested)?;
    ensure_watched_repository_mirror(&repository, &mirror_dir)?;
    check_indexing_canceled(cancel_requested)?;
    fetch_watched_repository_mirror(&mirror_dir)?;
    check_indexing_canceled(cancel_requested)?;

    let ref_name = requested_ref
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or(repository.default_branch.as_deref())
        .unwrap_or("HEAD");
    let commit = resolve_watched_repository_commit(&mirror_dir, ref_name)?;
    check_indexing_canceled(cancel_requested)?;
    let snapshot_dir =
        materialize_watched_repository_snapshot(&mirror_dir, watched_project_id, &commit)?;
    if cancel_requested.load(Ordering::SeqCst) {
        let _ = std::fs::remove_dir_all(&snapshot_dir);
        return Err(PackageApiError::bad_request("indexing canceled"));
    }
    let artifact = build_watched_project_artifact_from_checkout(
        watched_project_id,
        &snapshot_dir,
        &commit,
        Some(ref_name),
    );
    let cleanup_result = std::fs::remove_dir_all(&snapshot_dir);
    if artifact.is_ok() {
        cleanup_result.map_err(|err| {
            PackageApiError::internal(format!("failed to remove repository snapshot: {err}"))
        })?;
    }
    artifact
}

fn load_watched_project_editor_snapshot(
    watched_project_id: &str,
    query: WatchedProjectEditorDeltaQuery,
) -> Result<WatchedProjectEditorSnapshotDto, PackageApiError> {
    let watched_project = read_watched_project_metadata_by_id(watched_project_id)?;
    let repository = read_watched_repository_metadata(&watched_project.repository_id)?;
    let mirror_dir = watched_repo_cache_dir(&repository.id)?;
    ensure_watched_repository_mirror(&repository, &mirror_dir)?;
    fetch_watched_repository_mirror(&mirror_dir)?;
    let (branch, base_commit) = resolve_watched_project_editor_branch_base(
        &mirror_dir,
        &repository,
        &watched_project,
        query,
    )?;
    let base_commit = non_empty_string(base_commit)
        .ok_or_else(|| PackageApiError::bad_request("watched project has no commit to edit"))?;
    let snapshot_dir =
        materialize_watched_repository_snapshot(&mirror_dir, watched_project_id, &base_commit)?;
    let files = read_watched_project_editor_files(&snapshot_dir, &watched_project);
    let cleanup_result = std::fs::remove_dir_all(&snapshot_dir);
    let files = files?;
    cleanup_result.map_err(|err| {
        PackageApiError::internal(format!("failed to remove repository snapshot: {err}"))
    })?;
    Ok(WatchedProjectEditorSnapshotDto {
        project: watched_project,
        repository,
        base_commit,
        branch,
        files,
    })
}

fn commit_watched_project_editor_changes(
    watched_project_id: &str,
    request: WatchedProjectEditorCommitRequestDto,
) -> Result<WatchedProjectEditorCommitResponseDto, PackageApiError> {
    if request.files.is_empty() {
        return Err(PackageApiError::bad_request(
            "editor commit must include at least one changed file",
        ));
    }
    let base_commit = non_empty_string(request.base_commit.clone())
        .ok_or_else(|| PackageApiError::bad_request("baseCommit is required"))?;
    let message = non_empty_string(request.message)
        .ok_or_else(|| PackageApiError::bad_request("commit message is required"))?;
    let watched_project = read_watched_project_metadata_by_id(watched_project_id)?;
    let repository = read_watched_repository_metadata(&watched_project.repository_id)?;
    let mirror_dir = watched_repo_cache_dir(&repository.id)?;
    ensure_watched_repository_mirror(&repository, &mirror_dir)?;
    fetch_watched_repository_mirror(&mirror_dir)?;
    let branch =
        watched_project_editor_branch_from_request(&repository, request.branch.as_deref())?;
    let base_branch = watched_project_editor_base_branch_from_request(
        &repository,
        request.base_branch.as_deref(),
    )?;
    match resolve_watched_repository_commit(&mirror_dir, &branch) {
        Ok(current_commit) => {
            if current_commit != base_commit {
                return Err(PackageApiError::conflict(format!(
                    "project branch moved from {base_commit} to {current_commit}; refresh the editor delta before committing"
                )));
            }
        }
        Err(_) => {
            let current_base_commit = resolve_watched_repository_commit(&mirror_dir, &base_branch)?;
            if current_base_commit != base_commit {
                return Err(PackageApiError::conflict(format!(
                    "base branch moved from {base_commit} to {current_base_commit}; refresh the editor delta before creating `{branch}`"
                )));
            }
        }
    }

    let checkout_dir =
        materialize_watched_repository_snapshot(&mirror_dir, watched_project_id, &base_commit)?;
    let result = commit_watched_project_editor_changes_in_checkout(
        &checkout_dir,
        &watched_project,
        &repository,
        &branch,
        &message,
        request.files,
    );
    let cleanup_result = std::fs::remove_dir_all(&checkout_dir);
    let commit = result?;
    cleanup_result.map_err(|err| {
        PackageApiError::internal(format!("failed to remove editor checkout: {err}"))
    })?;

    fetch_watched_repository_mirror(&mirror_dir)?;
    let artifact = refresh_watched_project_from_git(watched_project_id, Some(&branch)).ok();
    let project = read_watched_project_metadata_by_id(watched_project_id)?;
    Ok(WatchedProjectEditorCommitResponseDto {
        project,
        branch,
        base_commit,
        commit,
        artifact_id: artifact.map(|artifact| artifact.manifest.id),
        pushed: true,
    })
}

fn compile_watched_project_editor_changes(
    watched_project_id: &str,
    request: WatchedProjectEditorCompileRequestDto,
) -> Result<SemanticProjectCompileResponseDto, PackageApiError> {
    let base_commit = non_empty_string(request.base_commit.clone())
        .ok_or_else(|| PackageApiError::bad_request("baseCommit is required"))?;
    let watched_project = read_watched_project_metadata_by_id(watched_project_id)?;
    let repository = read_watched_repository_metadata(&watched_project.repository_id)?;
    let mirror_dir = watched_repo_cache_dir(&repository.id)?;
    ensure_watched_repository_mirror(&repository, &mirror_dir)?;
    fetch_watched_repository_mirror(&mirror_dir)?;
    resolve_watched_repository_commit(&mirror_dir, &base_commit)?;

    let checkout_dir =
        materialize_watched_repository_snapshot(&mirror_dir, watched_project_id, &base_commit)?;
    let result = compile_watched_project_editor_changes_in_checkout(
        &checkout_dir,
        &watched_project,
        request,
    );
    let cleanup_result = std::fs::remove_dir_all(&checkout_dir);
    let response = result?;
    cleanup_result.map_err(|err| {
        PackageApiError::internal(format!("failed to remove editor compile checkout: {err}"))
    })?;
    Ok(response)
}

fn compile_watched_project_editor_changes_in_checkout(
    checkout_dir: &Path,
    watched_project: &WatchedProjectMetadataDto,
    request: WatchedProjectEditorCompileRequestDto,
) -> Result<SemanticProjectCompileResponseDto, PackageApiError> {
    let project_root = resolve_watched_project_checkout_root(checkout_dir, watched_project)?;
    let root_path = normalize_watched_project_path(&watched_project.root_path)?;
    let mut staged_files = Vec::with_capacity(request.staged_files.len());
    for file in request.staged_files {
        let relative_path = normalize_watched_project_path(&file.path)?;
        if relative_path == "." {
            return Err(PackageApiError::bad_request(
                "file path cannot be project root",
            ));
        }
        let target = project_root.join(&relative_path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                PackageApiError::internal(format!("failed to create editor file directory: {err}"))
            })?;
        }
        ensure_path_within_root(&project_root, &target, "editor file path")?;
        std::fs::write(&target, file.content).map_err(|err| {
            PackageApiError::internal(format!("failed to write editor file: {err}"))
        })?;
        staged_files.push(StagedEditorFileDto {
            path: join_watched_project_paths(&root_path, &relative_path),
            content: std::fs::read_to_string(&target).map_err(|err| {
                PackageApiError::internal(format!("failed to read staged editor file: {err}"))
            })?,
        });
    }

    let workspace =
        WorkspaceService::from_workspace_root_diagnostics_only(checkout_dir).map_err(|err| {
            PackageApiError::internal(format!("failed to load editor delta checkout: {err}"))
        })?;
    let project_path = request
        .project_path
        .as_deref()
        .map(normalize_watched_project_path)
        .transpose()?
        .unwrap_or_else(|| ".".to_string());
    let workspace_project_path = join_watched_project_paths(&root_path, &project_path);
    let mut response = workspace
        .compile_project_scope(&workspace_project_path, &staged_files)
        .map_err(package_error_from_api_error)?;
    relativize_editor_delta_compile_response(&mut response, &root_path, &project_path);
    Ok(response)
}

fn commit_watched_project_editor_changes_in_checkout(
    checkout_dir: &Path,
    watched_project: &WatchedProjectMetadataDto,
    repository: &WatchedRepositoryMetadataDto,
    branch: &str,
    message: &str,
    files: Vec<WatchedProjectEditorFileDto>,
) -> Result<String, PackageApiError> {
    let project_root = resolve_watched_project_checkout_root(checkout_dir, watched_project)?;
    for file in files {
        let relative_path = normalize_watched_project_path(&file.path)?;
        if relative_path == "." {
            return Err(PackageApiError::bad_request(
                "file path cannot be project root",
            ));
        }
        let target = project_root.join(&relative_path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                PackageApiError::internal(format!("failed to create editor file directory: {err}"))
            })?;
        }
        ensure_path_within_root(&project_root, &target, "editor file path")?;
        std::fs::write(&target, file.content).map_err(|err| {
            PackageApiError::internal(format!("failed to write editor file: {err}"))
        })?;
    }
    run_git(Some(checkout_dir), ["checkout", "-B", branch])?;
    run_git(Some(checkout_dir), ["add", "--all"])?;
    let porcelain = git_output(checkout_dir, ["status", "--porcelain"])?;
    if porcelain.trim().is_empty() {
        return Err(PackageApiError::bad_request(
            "editor delta does not contain any file changes",
        ));
    }
    run_git(
        Some(checkout_dir),
        ["config", "user.name", "Mercurio Editor"],
    )?;
    run_git(
        Some(checkout_dir),
        ["config", "user.email", "mercurio-editor@localhost"],
    )?;
    run_git(Some(checkout_dir), ["commit", "-m", message])?;
    let commit = git_output(checkout_dir, ["rev-parse", "HEAD"])?;
    let push_url = authenticated_watched_repository_push_url(&repository.url);
    run_git(
        Some(checkout_dir),
        vec![
            "push".to_string(),
            push_url,
            format!("HEAD:refs/heads/{branch}"),
        ],
    )?;
    Ok(commit)
}

fn watched_project_editor_branch(repository: &WatchedRepositoryMetadataDto) -> String {
    repository
        .default_branch
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("main")
        .to_string()
}

fn watched_project_editor_branch_from_request(
    repository: &WatchedRepositoryMetadataDto,
    requested_branch: Option<&str>,
) -> Result<String, PackageApiError> {
    let branch = requested_branch
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| watched_project_editor_branch(repository));
    validate_git_branch_name(&branch)?;
    Ok(branch)
}

fn watched_project_editor_base_branch_from_request(
    repository: &WatchedRepositoryMetadataDto,
    requested_base_branch: Option<&str>,
) -> Result<String, PackageApiError> {
    let branch = requested_base_branch
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| watched_project_editor_branch(repository));
    validate_git_branch_name(&branch)?;
    Ok(branch)
}

fn resolve_watched_project_editor_branch_base(
    mirror_dir: &Path,
    repository: &WatchedRepositoryMetadataDto,
    watched_project: &WatchedProjectMetadataDto,
    query: WatchedProjectEditorDeltaQuery,
) -> Result<(String, String), PackageApiError> {
    let requested_branch = query.branch.as_deref().and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then_some(trimmed)
    });
    let branch = watched_project_editor_branch_from_request(repository, requested_branch)?;
    if requested_branch.is_none() {
        let base_commit = watched_project.latest_commit.clone().unwrap_or_else(|| {
            resolve_watched_repository_commit(mirror_dir, &branch).unwrap_or_default()
        });
        return Ok((branch, base_commit));
    }

    match resolve_watched_repository_commit(mirror_dir, &branch) {
        Ok(commit) => Ok((branch, commit)),
        Err(_) => {
            let base_branch = watched_project_editor_base_branch_from_request(
                repository,
                query.base_branch.as_deref(),
            )?;
            let commit = resolve_watched_repository_commit(mirror_dir, &base_branch)?;
            Ok((branch, commit))
        }
    }
}

fn validate_git_branch_name(branch: &str) -> Result<(), PackageApiError> {
    if branch.is_empty()
        || branch.starts_with('-')
        || branch.starts_with('/')
        || branch.ends_with('/')
        || branch.ends_with('.')
        || branch.contains("..")
        || branch.contains("//")
        || branch.contains("@{")
        || branch.ends_with(".lock")
        || branch.chars().any(|ch| {
            ch.is_control()
                || ch.is_whitespace()
                || matches!(ch, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
    {
        return Err(PackageApiError::bad_request(format!(
            "git branch name is invalid: {branch}"
        )));
    }
    Ok(())
}

fn read_watched_project_editor_files(
    checkout_dir: &Path,
    watched_project: &WatchedProjectMetadataDto,
) -> Result<Vec<WatchedProjectEditorFileDto>, PackageApiError> {
    let project_root = resolve_watched_project_checkout_root(checkout_dir, watched_project)?;
    let mut paths = Vec::new();
    collect_editor_source_files(&project_root, &project_root, &mut paths)?;
    if let Some(descriptor_path) = watched_project.descriptor_path.as_deref() {
        let descriptor_path = normalize_watched_project_path(descriptor_path)?;
        if descriptor_path != "." && !paths.iter().any(|path| path == &descriptor_path) {
            paths.push(descriptor_path);
        }
    }
    paths.sort();
    paths.dedup();
    let mut files = Vec::new();
    for path in paths {
        let file_path = project_root.join(&path);
        if !file_path.is_file() {
            continue;
        }
        let content = std::fs::read_to_string(&file_path).map_err(|err| {
            PackageApiError::internal(format!("failed to read editor snapshot file: {err}"))
        })?;
        files.push(WatchedProjectEditorFileDto { path, content });
    }
    Ok(files)
}

fn collect_editor_source_files(
    root: &Path,
    dir: &Path,
    paths: &mut Vec<String>,
) -> Result<(), PackageApiError> {
    for entry in std::fs::read_dir(dir).map_err(|err| {
        PackageApiError::internal(format!("failed to read editor snapshot directory: {err}"))
    })? {
        let entry = entry.map_err(|err| PackageApiError::internal(err.to_string()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|err| PackageApiError::internal(err.to_string()))?;
        if file_type.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == ".git" || name == "target" || name == "node_modules" {
                continue;
            }
            collect_editor_source_files(root, &path, paths)?;
        } else if file_type.is_file() && is_editor_source_file(&path) {
            let relative = path.strip_prefix(root).map_err(|err| {
                PackageApiError::internal(format!("failed to relativize editor source path: {err}"))
            })?;
            paths.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

fn is_editor_source_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("sysml") | Some("kerml")
    )
}

fn ensure_path_within_root(root: &Path, path: &Path, label: &str) -> Result<(), PackageApiError> {
    let root = root.canonicalize().map_err(|err| {
        PackageApiError::internal(format!("failed to canonicalize {label} root: {err}"))
    })?;
    let candidate_parent = path.parent().unwrap_or(path);
    let canonical_parent = candidate_parent.canonicalize().map_err(|err| {
        PackageApiError::internal(format!("failed to canonicalize {label}: {err}"))
    })?;
    if canonical_parent.starts_with(&root) {
        Ok(())
    } else {
        Err(PackageApiError::bad_request(format!(
            "{label} escapes project root"
        )))
    }
}

fn authenticated_watched_repository_push_url(url: &str) -> String {
    let rewritten = git_clone_url_for_watched_repository(url);
    let Some(token) = configured_gitea_token_for_demo() else {
        return rewritten;
    };
    let username = std::env::var("MERCURIO_GITEA_SERVICE_USERNAME")
        .unwrap_or_else(|_| "mercurio-service".to_string());
    if let Some(rest) = rewritten.strip_prefix("http://") {
        return format!("http://{}:{}@{}", username, token, rest);
    }
    if let Some(rest) = rewritten.strip_prefix("https://") {
        return format!("https://{}:{}@{}", username, token, rest);
    }
    rewritten
}

fn check_indexing_canceled(cancel_requested: &AtomicBool) -> Result<(), PackageApiError> {
    if cancel_requested.load(Ordering::SeqCst) {
        return Err(PackageApiError::bad_request("indexing canceled"));
    }
    Ok(())
}

fn resolve_watched_repository_commit(
    mirror_dir: &Path,
    ref_name: &str,
) -> Result<String, PackageApiError> {
    let commit_ref = format!("{ref_name}^{{commit}}");
    match git_output(&mirror_dir, ["rev-parse", "--verify", &commit_ref]) {
        Ok(commit) => Ok(commit),
        Err(_) if has_no_repository_commits(mirror_dir)? => {
            Err(PackageApiError::bad_request(format!(
                "watched repository has no commits on `{ref_name}`; push an initial commit or choose a branch with commits"
            )))
        }
        Err(_) => Err(PackageApiError::bad_request(format!(
            "watched repository ref `{ref_name}` does not resolve to a commit"
        ))),
    }
}

fn has_no_repository_commits(mirror_dir: &Path) -> Result<bool, PackageApiError> {
    Ok(git_output(mirror_dir, ["show-ref", "--head"]).is_err())
}

fn ensure_watched_repository_mirror(
    repository: &WatchedRepositoryMetadataDto,
    mirror_dir: &Path,
) -> Result<(), PackageApiError> {
    if mirror_dir.exists() {
        return Ok(());
    }
    let parent = mirror_dir.parent().ok_or_else(|| {
        PackageApiError::internal("repository cache path has no parent directory")
    })?;
    std::fs::create_dir_all(parent).map_err(|err| {
        PackageApiError::internal(format!("failed to create repository cache: {err}"))
    })?;
    run_git(
        None,
        vec![
            "clone".to_string(),
            "--mirror".to_string(),
            git_clone_url_for_watched_repository(&repository.url),
            mirror_dir.display().to_string(),
        ],
    )
}

fn git_clone_url_for_watched_repository(url: &str) -> String {
    let external = std::env::var("MERCURIO_GITEA_URL").ok();
    let internal = std::env::var("MERCURIO_GITEA_INTERNAL_URL").ok();
    match (external.as_deref(), internal.as_deref()) {
        (Some(external), Some(internal)) => rewrite_url_base(url, external, internal),
        _ => url.to_string(),
    }
}

fn rewrite_url_base(url: &str, external_base: &str, internal_base: &str) -> String {
    let internal = internal_base.trim_end_matches('/');
    for external in loopback_equivalent_bases(external_base) {
        if let Some(suffix) = url.strip_prefix(&external) {
            return format!("{internal}{suffix}");
        }
    }
    url.to_string()
}

fn loopback_equivalent_bases(base: &str) -> Vec<String> {
    let normalized = base.trim_end_matches('/').to_string();
    let mut bases = vec![normalized.clone()];
    if normalized.contains("localhost") {
        bases.push(normalized.replace("localhost", "127.0.0.1"));
    }
    if normalized.contains("127.0.0.1") {
        bases.push(normalized.replace("127.0.0.1", "localhost"));
    }
    bases.sort();
    bases.dedup();
    bases
}

fn fetch_watched_repository_mirror(mirror_dir: &Path) -> Result<(), PackageApiError> {
    run_git(Some(mirror_dir), ["fetch", "--prune"])
}

fn list_watched_repository_branch_names(
    mirror_dir: &Path,
    default_branch: Option<&str>,
) -> Result<Vec<String>, PackageApiError> {
    let refs = git_output(
        mirror_dir,
        [
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    let mut branches = refs
        .lines()
        .filter_map(normalize_watched_repository_branch_ref)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if let Some(default_branch) = default_branch
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if !branches.iter().any(|branch| branch == default_branch) {
            branches.insert(0, default_branch.to_string());
        } else if let Some(index) = branches.iter().position(|branch| branch == default_branch) {
            let branch = branches.remove(index);
            branches.insert(0, branch);
        }
    }
    Ok(branches)
}

fn normalize_watched_repository_branch_ref(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "HEAD" || trimmed.ends_with("/HEAD") {
        return None;
    }
    Some(
        trimmed
            .strip_prefix("origin/")
            .unwrap_or(trimmed)
            .to_string(),
    )
}

fn materialize_watched_repository_snapshot(
    mirror_dir: &Path,
    watched_project_id: &str,
    commit: &str,
) -> Result<PathBuf, PackageApiError> {
    let snapshot_parent = watched_repo_snapshot_root();
    std::fs::create_dir_all(&snapshot_parent).map_err(|err| {
        PackageApiError::internal(format!("failed to create repository snapshot root: {err}"))
    })?;
    let snapshot_dir = snapshot_parent.join(format!(
        "{}-{}",
        slugify_identifier(watched_project_id),
        unix_time_ms()
    ));
    if snapshot_dir.exists() {
        return Err(PackageApiError::conflict(format!(
            "repository snapshot already exists: {}",
            snapshot_dir.display()
        )));
    }
    run_git(
        None,
        vec![
            "clone".to_string(),
            "--quiet".to_string(),
            "--no-checkout".to_string(),
            mirror_dir.display().to_string(),
            snapshot_dir.display().to_string(),
        ],
    )?;
    run_git(Some(&snapshot_dir), ["checkout", "--quiet", commit])?;
    Ok(snapshot_dir)
}

fn run_git<I, S>(current_dir: Option<&Path>, args: I) -> Result<(), PackageApiError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let args = args.into_iter().collect::<Vec<_>>();
    let display_args = git_args_display(&args);
    let mut command = Command::new("git");
    command.args(&args);
    command.env("GIT_TERMINAL_PROMPT", "0");
    if let Some(current_dir) = current_dir {
        command.current_dir(current_dir);
    }
    let output = command.output().map_err(|err| {
        PackageApiError::internal(format!("failed to run git {display_args}: {err}"))
    })?;
    if output.status.success() {
        return Ok(());
    }
    Err(PackageApiError::internal(format!(
        "git {} failed: {}",
        display_args,
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

fn git_output<I, S>(current_dir: &Path, args: I) -> Result<String, PackageApiError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let args = args.into_iter().collect::<Vec<_>>();
    let display_args = git_args_display(&args);
    let output = Command::new("git")
        .args(&args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .current_dir(current_dir)
        .output()
        .map_err(|err| {
            PackageApiError::internal(format!("failed to run git {display_args}: {err}"))
        })?;
    if !output.status.success() {
        return Err(PackageApiError::internal(format!(
            "git {} failed: {}",
            display_args,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_args_display<S: AsRef<std::ffi::OsStr>>(args: &[S]) -> String {
    args.iter()
        .map(|arg| arg.as_ref().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

fn resolve_watched_project_checkout_root(
    checkout_path: &Path,
    watched_project: &WatchedProjectMetadataDto,
) -> Result<PathBuf, PackageApiError> {
    let root_path = normalize_watched_project_path(&watched_project.root_path)?;
    let candidate = if root_path == "." {
        checkout_path.to_path_buf()
    } else {
        checkout_path.join(&root_path)
    };
    if !candidate.is_dir() {
        return Err(PackageApiError::bad_request(format!(
            "watched project root is not a directory in checkout: {}",
            candidate.display()
        )));
    }
    Ok(candidate)
}

fn artifact_id_for(watched_project_id: &str, commit: &str, source_digest: &str) -> String {
    let seed = format!("{watched_project_id}\0{commit}\0{source_digest}");
    checksum_hex(seed.as_bytes()).replace("fnv1a64:", "artifact-")
}

fn read_server_project_metadata(
    project_id: &str,
) -> Result<ServerProjectMetadataDto, PackageApiError> {
    let project_dir = server_project_dir(project_id)?;
    if !project_dir.exists() {
        return Err(PackageApiError::not_found(format!(
            "server project not found: {project_id}"
        )));
    }
    read_server_project_metadata_by_dir(&project_dir)
}

fn read_server_project_metadata_by_dir(
    project_dir: &Path,
) -> Result<ServerProjectMetadataDto, PackageApiError> {
    let content = std::fs::read_to_string(project_dir.join("metadata.json")).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            PackageApiError::not_found("server project metadata not found")
        } else {
            PackageApiError::internal(format!("failed to read server project metadata: {err}"))
        }
    })?;
    serde_json::from_str(&content)
        .map_err(|err| PackageApiError::internal(format!("invalid server project metadata: {err}")))
}

fn write_server_project_metadata(
    project_dir: &Path,
    metadata: &ServerProjectMetadataDto,
) -> Result<(), PackageApiError> {
    let content = serde_json::to_string_pretty(metadata)
        .map_err(|err| PackageApiError::internal(err.to_string()))?;
    std::fs::write(project_dir.join("metadata.json"), content).map_err(|err| {
        PackageApiError::internal(format!("failed to write server project metadata: {err}"))
    })
}

fn read_all_v2_proposals() -> Result<Vec<V2ProposalDto>, PackageApiError> {
    let root = v2_proposals_root();
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut items = Vec::new();
    let entries = std::fs::read_dir(&root)
        .map_err(|err| PackageApiError::internal(format!("failed to read proposals: {err}")))?;
    for entry in entries {
        let entry = entry.map_err(|err| PackageApiError::internal(err.to_string()))?;
        if !entry
            .file_type()
            .map_err(|err| PackageApiError::internal(err.to_string()))?
            .is_file()
        {
            continue;
        }
        if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        items.push(read_v2_proposal_from_path(&entry.path())?);
    }
    items.sort_by(|left, right| {
        right
            .updated_at_unix_ms
            .cmp(&left.updated_at_unix_ms)
            .then(left.key.cmp(&right.key))
    });
    Ok(items)
}

fn read_ai_settings_dto() -> Result<AiSettingsDto, PackageApiError> {
    Ok(ai_settings_dto(
        read_stored_ai_settings()?,
        read_stored_ai_secrets()?,
    ))
}

fn stored_reasoning_provider_config() -> Result<
    (
        ReasoningProviderConfigOverrides,
        ReasoningProviderSecretOverrides,
    ),
    PackageApiError,
> {
    let settings = read_stored_ai_settings()?;
    let secrets = read_stored_ai_secrets()?;
    Ok((
        ReasoningProviderConfigOverrides {
            provider: match settings.provider.as_deref() {
                Some(value) if normalize_ai_provider(value)? != "auto" => {
                    Some(reasoning_provider_kind_from_name(value)?)
                }
                _ => None,
            },
            openai_model: settings.openai_model,
            openai_base_url: settings.openai_base_url,
            azure_openai_deployment: settings.azure_openai_deployment,
            azure_openai_base_url: settings.azure_openai_base_url,
        },
        ReasoningProviderSecretOverrides {
            openai_api_key: secrets.openai_api_key,
            azure_openai_api_key: secrets.azure_openai_api_key,
        },
    ))
}

fn ai_settings_dto(settings: StoredAiSettings, secrets: StoredAiSecrets) -> AiSettingsDto {
    AiSettingsDto {
        provider: settings.provider.unwrap_or_else(|| "auto".to_string()),
        openai_model: settings.openai_model,
        openai_base_url: settings.openai_base_url,
        openai_api_key_configured: secrets
            .openai_api_key
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty()),
        azure_openai_deployment: settings.azure_openai_deployment,
        azure_openai_base_url: settings.azure_openai_base_url,
        azure_openai_api_key_configured: secrets
            .azure_openai_api_key
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty()),
        daily_token_limit: settings.daily_token_limit,
        per_request_token_limit: settings.per_request_token_limit,
        usage: read_current_ai_usage().unwrap_or_else(|_| empty_ai_usage_for_today()),
    }
}

fn normalize_optional_token_limit(value: Option<u64>) -> Option<u64> {
    value.filter(|limit| *limit > 0)
}

fn ai_usage_scope(
    config: &ReasoningProviderConfigOverrides,
    secrets: &ReasoningProviderSecretOverrides,
) -> bool {
    !matches!(
        configured_reasoning_provider(config.clone(), secrets.clone())
            .provider_status()
            .kind,
        ReasoningProviderKind::Heuristic
    )
}

fn reserve_ai_tokens(
    settings: &StoredAiSettings,
    external_provider: bool,
    estimated_input_tokens: u64,
) -> Result<(), PackageApiError> {
    if !external_provider {
        return Ok(());
    }
    if let Some(limit) = settings.per_request_token_limit
        && estimated_input_tokens > limit
    {
        increment_ai_rejections()?;
        return Err(PackageApiError::too_many_requests(format!(
            "AI request estimated at {estimated_input_tokens} input tokens, above the per-request limit of {limit}"
        )));
    }
    if let Some(limit) = settings.daily_token_limit {
        let usage = read_current_ai_usage()?;
        let used = usage.input_tokens.saturating_add(usage.output_tokens);
        if used.saturating_add(estimated_input_tokens) > limit {
            increment_ai_rejections()?;
            return Err(PackageApiError::too_many_requests(format!(
                "AI daily token budget exhausted: {used}/{limit} used, request needs about {estimated_input_tokens} more"
            )));
        }
    }
    Ok(())
}

fn estimate_ask_request_tokens(request: &AskMercurioRequest, context: &[String]) -> u64 {
    request
        .messages
        .iter()
        .map(|message| estimate_text_tokens(&message.content).saturating_add(4))
        .sum::<u64>()
        .saturating_add(
            context
                .iter()
                .map(|entry| estimate_text_tokens(entry).saturating_add(4))
                .sum::<u64>(),
        )
        .saturating_add(64)
}

fn estimate_chat_request_tokens(request: &ChatCompletionRequest) -> u64 {
    request
        .messages
        .iter()
        .map(|message| estimate_text_tokens(&message.content).saturating_add(4))
        .sum::<u64>()
        .saturating_add(
            request
                .context
                .iter()
                .map(|entry| estimate_text_tokens(entry).saturating_add(4))
                .sum::<u64>(),
        )
}

fn estimate_text_tokens(value: &str) -> u64 {
    let chars = value.chars().count() as u64;
    chars.saturating_add(3) / 4
}

fn record_ai_usage(input_tokens: u64, output_tokens: u64) -> Result<(), PackageApiError> {
    let mut usage = read_current_ai_usage()?;
    usage.input_tokens = usage.input_tokens.saturating_add(input_tokens);
    usage.output_tokens = usage.output_tokens.saturating_add(output_tokens);
    usage.requests = usage.requests.saturating_add(1);
    write_ai_usage(&usage)
}

fn increment_ai_rejections() -> Result<(), PackageApiError> {
    let mut usage = read_current_ai_usage()?;
    usage.rejected_requests = usage.rejected_requests.saturating_add(1);
    write_ai_usage(&usage)
}

fn read_current_ai_usage() -> Result<AiUsageDto, PackageApiError> {
    let path = ai_usage_path();
    if !path.exists() {
        return Ok(empty_ai_usage_for_today());
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|err| PackageApiError::internal(format!("failed to read AI usage: {err}")))?;
    let usage = serde_json::from_str::<AiUsageDto>(&content)
        .map_err(|err| PackageApiError::internal(format!("invalid AI usage: {err}")))?;
    if usage.day == current_ai_usage_day() {
        Ok(usage)
    } else {
        Ok(empty_ai_usage_for_today())
    }
}

fn write_ai_usage(usage: &AiUsageDto) -> Result<(), PackageApiError> {
    std::fs::create_dir_all(server_data_root()).map_err(|err| {
        PackageApiError::internal(format!("failed to create server data directory: {err}"))
    })?;
    let content = serde_json::to_string_pretty(usage)
        .map_err(|err| PackageApiError::internal(err.to_string()))?;
    std::fs::write(ai_usage_path(), content)
        .map_err(|err| PackageApiError::internal(format!("failed to write AI usage: {err}")))
}

fn empty_ai_usage_for_today() -> AiUsageDto {
    AiUsageDto {
        day: current_ai_usage_day(),
        ..AiUsageDto::default()
    }
}

fn current_ai_usage_day() -> String {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() / 86_400)
        .unwrap_or_default();
    format!("epoch-day-{days}")
}

fn read_stored_ai_settings() -> Result<StoredAiSettings, PackageApiError> {
    let path = ai_settings_path();
    if !path.exists() {
        return Ok(StoredAiSettings::default());
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|err| PackageApiError::internal(format!("failed to read AI settings: {err}")))?;
    serde_json::from_str(&content)
        .map_err(|err| PackageApiError::internal(format!("invalid AI settings: {err}")))
}

fn read_stored_ai_secrets() -> Result<StoredAiSecrets, PackageApiError> {
    let path = ai_secrets_path();
    if !path.exists() {
        return Ok(StoredAiSecrets::default());
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|err| PackageApiError::internal(format!("failed to read AI secrets: {err}")))?;
    serde_json::from_str(&content)
        .map_err(|err| PackageApiError::internal(format!("invalid AI secrets: {err}")))
}

fn write_stored_ai_settings(settings: &StoredAiSettings) -> Result<(), PackageApiError> {
    std::fs::create_dir_all(server_data_root()).map_err(|err| {
        PackageApiError::internal(format!("failed to create server data directory: {err}"))
    })?;
    let content = serde_json::to_string_pretty(settings)
        .map_err(|err| PackageApiError::internal(err.to_string()))?;
    std::fs::write(ai_settings_path(), content)
        .map_err(|err| PackageApiError::internal(format!("failed to write AI settings: {err}")))
}

fn write_stored_ai_secrets(secrets: &StoredAiSecrets) -> Result<(), PackageApiError> {
    std::fs::create_dir_all(server_data_root()).map_err(|err| {
        PackageApiError::internal(format!("failed to create server data directory: {err}"))
    })?;
    let content = serde_json::to_string_pretty(secrets)
        .map_err(|err| PackageApiError::internal(err.to_string()))?;
    std::fs::write(ai_secrets_path(), content)
        .map_err(|err| PackageApiError::internal(format!("failed to write AI secrets: {err}")))
}

fn normalize_ai_provider(value: &str) -> Result<String, PackageApiError> {
    let normalized = value.trim().to_ascii_lowercase().replace('-', "_");
    match normalized.as_str() {
        "" | "auto" => Ok("auto".to_string()),
        "heuristic" | "openai" | "azure_openai" => Ok(normalized),
        _ => Err(PackageApiError::bad_request(format!(
            "unsupported AI provider: {value}"
        ))),
    }
}

fn reasoning_provider_kind_from_name(
    value: &str,
) -> Result<ReasoningProviderKind, PackageApiError> {
    match normalize_ai_provider(value)?.as_str() {
        "heuristic" => Ok(ReasoningProviderKind::Heuristic),
        "openai" => Ok(ReasoningProviderKind::OpenAi),
        "azure_openai" => Ok(ReasoningProviderKind::AzureOpenAi),
        _ => Ok(ReasoningProviderKind::Heuristic),
    }
}

fn non_empty_string_option(value: Option<String>) -> Option<String> {
    value.and_then(non_empty_string)
}

fn read_v2_proposal(key: &str) -> Result<V2ProposalDto, PackageApiError> {
    let key = normalize_v2_proposal_key(key)?;
    let path = v2_proposal_path(&key)?;
    if !path.exists() {
        return Err(PackageApiError::not_found(format!(
            "proposal not found: {key}"
        )));
    }
    read_v2_proposal_from_path(&path)
}

fn read_v2_proposal_from_path(path: &Path) -> Result<V2ProposalDto, PackageApiError> {
    let content = std::fs::read_to_string(path)
        .map_err(|err| PackageApiError::internal(format!("failed to read proposal: {err}")))?;
    serde_json::from_str(&content)
        .map_err(|err| PackageApiError::internal(format!("invalid proposal: {err}")))
}

fn write_v2_proposal(item: &V2ProposalDto) -> Result<(), PackageApiError> {
    let root = v2_proposals_root();
    std::fs::create_dir_all(&root).map_err(|err| {
        PackageApiError::internal(format!("failed to create v2 proposal store: {err}"))
    })?;
    let content = serde_json::to_string_pretty(item)
        .map_err(|err| PackageApiError::internal(err.to_string()))?;
    std::fs::write(v2_proposal_path(&item.key)?, content)
        .map_err(|err| PackageApiError::internal(format!("failed to write proposal: {err}")))
}

fn normalize_v2_proposal_key(key: &str) -> Result<String, PackageApiError> {
    let key = key.trim().to_ascii_uppercase();
    if key.is_empty() {
        return Err(PackageApiError::bad_request("proposal key cannot be empty"));
    }
    let valid = key
        .chars()
        .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '-')
        && key.chars().any(|ch| ch.is_ascii_alphabetic())
        && key.chars().any(|ch| ch.is_ascii_digit());
    if !valid || key.starts_with('-') || key.ends_with('-') || key.contains("--") {
        return Err(PackageApiError::bad_request(
            "proposal key must look like MER-123",
        ));
    }
    Ok(key)
}

fn next_v2_proposal_key() -> Result<String, PackageApiError> {
    let mut next = 1_u64;
    for item in read_all_v2_proposals()? {
        if let Some(number) = item
            .key
            .strip_prefix("MER-")
            .and_then(|suffix| suffix.parse::<u64>().ok())
        {
            next = next.max(number + 1);
        }
    }
    Ok(format!("MER-{next}"))
}

fn normalize_v2_proposal_pull_request(
    pull_request: V2ProposalPullRequestDto,
) -> Option<V2ProposalPullRequestDto> {
    Some(V2ProposalPullRequestDto {
        provider: non_empty_string(pull_request.provider)?,
        repository_id: pull_request.repository_id.and_then(non_empty_string),
        repository_external_id: pull_request
            .repository_external_id
            .and_then(non_empty_string),
        branch: non_empty_string(pull_request.branch)?,
        base_branch: non_empty_string(pull_request.base_branch)
            .unwrap_or_else(|| "main".to_string()),
        pull_request_id: pull_request.pull_request_id.and_then(non_empty_string),
        pull_request_url: pull_request.pull_request_url.and_then(non_empty_string),
        pull_request_state: non_empty_string(pull_request.pull_request_state)
            .unwrap_or_else(|| "planned".to_string()),
        mergeable: pull_request.mergeable,
        review_state: pull_request.review_state.and_then(non_empty_string),
        last_synced_at_unix_ms: pull_request.last_synced_at_unix_ms,
        metadata: pull_request.metadata,
    })
}

fn normalize_v2_external_bindings(
    bindings: Vec<V2ExternalBindingDto>,
) -> Vec<V2ExternalBindingDto> {
    let mut normalized = bindings
        .into_iter()
        .filter_map(normalize_v2_external_binding)
        .collect::<Vec<_>>();
    normalized.sort_by(|left, right| {
        (
            &left.provider,
            &left.kind,
            &left.repository_external_id,
            &left.external_id,
            &left.external_url,
        )
            .cmp(&(
                &right.provider,
                &right.kind,
                &right.repository_external_id,
                &right.external_id,
                &right.external_url,
            ))
    });
    normalized.dedup_by(|left, right| {
        left.provider == right.provider
            && left.kind == right.kind
            && left.repository_external_id == right.repository_external_id
            && left.external_id == right.external_id
            && left.external_url == right.external_url
    });
    normalized
}

fn normalize_v2_external_binding(binding: V2ExternalBindingDto) -> Option<V2ExternalBindingDto> {
    Some(V2ExternalBindingDto {
        provider: non_empty_string(binding.provider)?,
        kind: non_empty_string(binding.kind)?,
        repository_id: binding.repository_id.and_then(non_empty_string),
        repository_external_id: binding.repository_external_id.and_then(non_empty_string),
        external_id: binding.external_id.and_then(non_empty_string),
        external_url: binding.external_url.and_then(non_empty_string),
        state: binding.state.and_then(non_empty_string),
        title: binding.title.and_then(non_empty_string),
        last_synced_at_unix_ms: binding.last_synced_at_unix_ms,
        metadata: binding.metadata,
    })
}

fn add_external_binding(bindings: &mut Vec<V2ExternalBindingDto>, binding: V2ExternalBindingDto) {
    bindings.push(binding);
    *bindings = normalize_v2_external_bindings(std::mem::take(bindings));
}

fn add_label(labels: &mut Vec<String>, label: &str) {
    labels.push(label.to_string());
    *labels = normalize_string_list(std::mem::take(labels));
}

fn first_repository_id_for_demo() -> Result<Option<String>, PackageApiError> {
    Ok(read_all_watched_repositories()?
        .into_iter()
        .next()
        .map(|repository| repository.id))
}

fn first_repository_external_id_for_demo() -> Result<Option<String>, PackageApiError> {
    Ok(read_all_watched_repositories()?
        .into_iter()
        .next()
        .map(|repository| repository.url))
}

fn make_gitea_problem_report_binding(
    repository_id: &Option<String>,
    repository_external_id: &Option<String>,
    title: &str,
    body: &str,
) -> V2ExternalBindingDto {
    let now = unix_time_ms();
    let mut metadata = BTreeMap::from([(
        "source".to_string(),
        Value::String("mercurio-demo-cycle".to_string()),
    )]);
    let provider = "gitea".to_string();
    let token = configured_gitea_token_for_demo();
    if let (Some(token), Some(repository_url), Some((owner, repo))) = (
        token,
        repository_external_id.as_ref(),
        repository_external_id
            .as_ref()
            .and_then(|url| parse_repository_owner_name(url)),
    ) {
        let gitea_base = std::env::var("MERCURIO_GITEA_INTERNAL_URL")
            .ok()
            .and_then(non_empty_string)
            .or_else(|| {
                std::env::var("MERCURIO_GITEA_URL")
                    .ok()
                    .and_then(non_empty_string)
            })
            .unwrap_or_else(|| "http://localhost:3001".to_string());
        let endpoint = format!(
            "{}/api/v1/repos/{}/{}/issues",
            gitea_base.trim_end_matches('/'),
            owner,
            repo
        );
        let response = reqwest::blocking::Client::new()
            .post(endpoint)
            .header("Authorization", format!("token {token}"))
            .json(&serde_json::json!({
                "title": title,
                "body": body,
            }))
            .send();
        match response {
            Ok(response) if response.status().is_success() => {
                if let Ok(json) = response.json::<Value>() {
                    let external_id = json
                        .get("number")
                        .and_then(Value::as_i64)
                        .map(|number| number.to_string())
                        .or_else(|| {
                            json.get("id")
                                .and_then(Value::as_i64)
                                .map(|id| id.to_string())
                        });
                    let external_url = json
                        .get("html_url")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or_else(|| {
                            issue_url_from_repository_url(repository_url, external_id.as_deref())
                        });
                    return V2ExternalBindingDto {
                        provider,
                        kind: "issue".to_string(),
                        repository_id: repository_id.clone(),
                        repository_external_id: Some(repository_url.clone()),
                        external_id,
                        external_url,
                        state: Some(
                            json.get("state")
                                .and_then(Value::as_str)
                                .unwrap_or("open")
                                .to_string(),
                        ),
                        title: Some(title.to_string()),
                        last_synced_at_unix_ms: Some(now),
                        metadata,
                    };
                }
                metadata.insert(
                    "warning".to_string(),
                    Value::String("Gitea issue response was not valid JSON".to_string()),
                );
            }
            Ok(response) => {
                metadata.insert(
                    "warning".to_string(),
                    Value::String(format!(
                        "Gitea issue creation returned {}",
                        response.status()
                    )),
                );
            }
            Err(err) => {
                metadata.insert(
                    "warning".to_string(),
                    Value::String(format!("Gitea issue creation failed: {err}")),
                );
            }
        }
    } else {
        metadata.insert(
            "warning".to_string(),
            Value::String("Gitea token or repository owner/name was not configured".to_string()),
        );
    }

    V2ExternalBindingDto {
        provider,
        kind: "issue".to_string(),
        repository_id: repository_id.clone(),
        repository_external_id: repository_external_id.clone(),
        external_id: None,
        external_url: repository_external_id
            .as_ref()
            .and_then(|repository_url| issue_url_from_repository_url(repository_url, None)),
        state: Some("planned".to_string()),
        title: Some(title.to_string()),
        last_synced_at_unix_ms: Some(now),
        metadata,
    }
}

fn parse_repository_owner_name(repository_url: &str) -> Option<(String, String)> {
    let trimmed = repository_url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let without_scheme = trimmed.split("://").nth(1).unwrap_or(trimmed);
    let path = without_scheme
        .split_once(':')
        .map(|(_, path)| path)
        .unwrap_or_else(|| {
            without_scheme
                .split_once('/')
                .map(|(_, path)| path)
                .unwrap_or("")
        });
    let mut parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let repo = parts.pop()?.to_string();
    let owner = parts.pop()?.to_string();
    Some((owner, repo))
}

fn issue_url_from_repository_url(
    repository_url: &str,
    issue_number: Option<&str>,
) -> Option<String> {
    let base = repository_url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git");
    if base.is_empty() {
        return None;
    }
    Some(match issue_number {
        Some(number) => format!("{base}/issues/{number}"),
        None => format!("{base}/issues"),
    })
}

fn configured_gitea_token_for_demo() -> Option<String> {
    std::env::var("MERCURIO_GITEA_TOKEN")
        .ok()
        .and_then(non_empty_string)
        .or_else(read_stored_gitea_token_for_demo)
}

fn read_stored_gitea_token_for_demo() -> Option<String> {
    let path = server_data_root().join("source-control-secrets.json");
    let content = std::fs::read_to_string(path).ok()?;
    let json = serde_json::from_str::<Value>(&content).ok()?;
    json.get("giteaToken")
        .and_then(Value::as_str)
        .map(str::to_string)
        .and_then(non_empty_string)
}

fn slug_for_branch(value: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            previous_dash = false;
        } else if !previous_dash && !slug.is_empty() {
            slug.push('-');
            previous_dash = true;
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "implementation".to_string()
    } else {
        slug.to_string()
    }
}

fn non_empty_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn normalize_string_list(values: Vec<String>) -> Vec<String> {
    let mut values = values
        .into_iter()
        .filter_map(non_empty_string)
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn infer_single_server_project_id() -> Result<String, PackageApiError> {
    let projects = read_all_server_project_metadata()?;
    match projects.as_slice() {
        [project] => Ok(project.id.clone()),
        [] => Err(PackageApiError::bad_request(
            "projectId is required because no server projects exist",
        )),
        _ => Err(PackageApiError::bad_request(
            "projectId is required because multiple server projects exist",
        )),
    }
}

fn read_all_server_project_metadata() -> Result<Vec<ServerProjectMetadataDto>, PackageApiError> {
    let root = server_projects_root();
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut projects = Vec::new();
    let entries = std::fs::read_dir(&root).map_err(|err| {
        PackageApiError::internal(format!("failed to read server projects: {err}"))
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| PackageApiError::internal(err.to_string()))?;
        if !entry
            .file_type()
            .map_err(|err| PackageApiError::internal(err.to_string()))?
            .is_dir()
        {
            continue;
        }
        if entry.path().join("metadata.json").exists() {
            projects.push(read_server_project_metadata_by_dir(&entry.path())?);
        }
    }
    projects.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(projects)
}

fn normalize_watched_project_path(path: &str) -> Result<String, PackageApiError> {
    let trimmed = path.trim().trim_start_matches('/');
    if trimmed.is_empty() || trimmed == "." {
        return Ok(".".to_string());
    }

    let normalized = trimmed.replace('\\', "/");
    let mut segments = Vec::new();
    for segment in normalized.split('/') {
        match segment {
            "" | "." => continue,
            ".." => {
                return Err(PackageApiError::bad_request(
                    "watched project paths must stay inside the repository",
                ));
            }
            _ if segment.contains(':') => {
                return Err(PackageApiError::bad_request(
                    "watched project paths must be repository-relative",
                ));
            }
            _ => segments.push(segment),
        }
    }

    if segments.is_empty() {
        Ok(".".to_string())
    } else {
        Ok(segments.join("/"))
    }
}

fn join_watched_project_paths(root_path: &str, relative_path: &str) -> String {
    match (root_path, relative_path) {
        (".", ".") => ".".to_string(),
        (".", relative) => relative.to_string(),
        (root, ".") => root.to_string(),
        (root, relative) => format!("{}/{}", root.trim_end_matches('/'), relative),
    }
}

fn relativize_editor_delta_compile_response(
    response: &mut SemanticProjectCompileResponseDto,
    root_path: &str,
    project_path: &str,
) {
    response.project_path = project_path.to_string();
    if root_path == "." {
        return;
    }

    let prefix = format!("{}/", root_path.trim_end_matches('/'));
    for result in &mut response.results {
        if result.path == root_path {
            result.path = ".".to_string();
        } else if let Some(relative_path) = result.path.strip_prefix(&prefix) {
            result.path = relative_path.to_string();
        }
    }
}

fn normalize_source_globs(source_globs: Vec<String>) -> Result<Vec<String>, PackageApiError> {
    let mut normalized = Vec::new();
    for source_glob in source_globs {
        let trimmed = source_glob.trim().replace('\\', "/");
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('/') || trimmed.contains("..") || trimmed.contains(':') {
            return Err(PackageApiError::bad_request(
                "source globs must be repository-relative",
            ));
        }
        normalized.push(trimmed);
    }
    if normalized.is_empty() {
        return Err(PackageApiError::bad_request(
            "at least one source glob must be provided",
        ));
    }
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

fn normalize_server_project_file_path(path: &str) -> Result<String, PackageApiError> {
    let trimmed = path.trim().trim_start_matches('/');
    if trimmed.is_empty() {
        return Err(PackageApiError::bad_request("file path cannot be empty"));
    }
    let normalized = trimmed.replace('\\', "/");
    let mut segments = Vec::new();
    for segment in normalized.split('/') {
        match segment {
            "" | "." => continue,
            ".." => {
                return Err(PackageApiError::bad_request(
                    "file path must stay inside the server project",
                ));
            }
            _ if segment.contains(':') => {
                return Err(PackageApiError::bad_request(
                    "file path must be relative to the server project",
                ));
            }
            _ => segments.push(segment),
        }
    }
    if segments.is_empty() {
        return Err(PackageApiError::bad_request("file path cannot be empty"));
    }
    let normalized = segments.join("/");
    if !normalized.ends_with(".sysml") && !normalized.ends_with(".kerml") {
        return Err(PackageApiError::bad_request(
            "server project files must end with .sysml or .kerml",
        ));
    }
    Ok(normalized)
}

fn collect_server_project_files(
    root: &Path,
    current: &Path,
    files: &mut Vec<ServerProjectFileEntryDto>,
) -> Result<(), PackageApiError> {
    let entries = std::fs::read_dir(current).map_err(|err| {
        PackageApiError::internal(format!("failed to read server project files: {err}"))
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| PackageApiError::internal(err.to_string()))?;
        let file_type = entry
            .file_type()
            .map_err(|err| PackageApiError::internal(err.to_string()))?;
        if file_type.is_dir() {
            collect_server_project_files(root, &entry.path(), files)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        let relative_path = path
            .strip_prefix(root)
            .map_err(|err| PackageApiError::internal(err.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        if !relative_path.ends_with(".sysml") && !relative_path.ends_with(".kerml") {
            continue;
        }
        let metadata = entry
            .metadata()
            .map_err(|err| PackageApiError::internal(err.to_string()))?;
        let updated_at_unix_ms = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        files.push(ServerProjectFileEntryDto {
            path: relative_path,
            byte_count: usize::try_from(metadata.len()).unwrap_or(usize::MAX),
            updated_at_unix_ms,
        });
    }
    Ok(())
}

fn compute_server_project_revision(project_id: &str) -> Result<String, PackageApiError> {
    let files_root = server_project_files_root(project_id)?;
    compute_files_revision(&files_root)
}

fn compute_files_revision(files_root: &Path) -> Result<String, PackageApiError> {
    let mut files = Vec::new();
    if files_root.exists() {
        collect_server_project_files(files_root, files_root, &mut files)?;
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));

    let mut bytes = Vec::new();
    for file in files {
        bytes.extend_from_slice(file.path.as_bytes());
        bytes.push(0);
        let content = std::fs::read(files_root.join(&file.path)).map_err(|err| {
            PackageApiError::internal(format!("failed to read server project file: {err}"))
        })?;
        bytes.extend_from_slice(&content);
        bytes.push(0);
    }
    Ok(checksum_hex(&bytes).replace("fnv1a64:", "rev:"))
}

fn ensure_server_project_working_copy(project_id: &str) -> Result<(), PackageApiError> {
    let working_dir = server_project_working_dir(project_id)?;
    if working_dir.exists() {
        return Ok(());
    }
    reset_server_project_working_copy(project_id)
}

fn reset_server_project_working_copy(project_id: &str) -> Result<(), PackageApiError> {
    let project_dir = server_project_dir(project_id)?;
    if !project_dir.exists() {
        return Err(PackageApiError::not_found(format!(
            "server project not found: {project_id}"
        )));
    }

    let working_dir = server_project_working_dir(project_id)?;
    if working_dir.exists() {
        std::fs::remove_dir_all(&working_dir).map_err(|err| {
            PackageApiError::internal(format!("failed to reset project working copy: {err}"))
        })?;
    }
    if let Some(parent) = working_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            PackageApiError::internal(format!("failed to create project workspace: {err}"))
        })?;
    }

    let clone_output = Command::new("git")
        .arg("clone")
        .arg("--quiet")
        .arg(&project_dir)
        .arg(&working_dir)
        .output();
    if clone_output
        .as_ref()
        .map(|output| output.status.success())
        .unwrap_or(false)
    {
        return Ok(());
    }

    copy_directory_recursive(&project_dir, &working_dir)
}

fn replace_directory_contents(target: &Path, source: &Path) -> Result<(), PackageApiError> {
    if target.exists() {
        std::fs::remove_dir_all(target).map_err(|err| {
            PackageApiError::internal(format!("failed to replace committed files: {err}"))
        })?;
    }
    copy_directory_recursive(source, target)
}

fn copy_directory_recursive(source: &Path, target: &Path) -> Result<(), PackageApiError> {
    std::fs::create_dir_all(target).map_err(|err| {
        PackageApiError::internal(format!("failed to create directory copy: {err}"))
    })?;
    for entry in std::fs::read_dir(source).map_err(|err| {
        PackageApiError::internal(format!("failed to read directory copy source: {err}"))
    })? {
        let entry = entry.map_err(|err| PackageApiError::internal(err.to_string()))?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|err| PackageApiError::internal(err.to_string()))?;
        if file_type.is_dir() {
            copy_directory_recursive(&source_path, &target_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&source_path, &target_path).map_err(|err| {
                PackageApiError::internal(format!("failed to copy project file: {err}"))
            })?;
        }
    }
    Ok(())
}

fn load_server_project_workspace(
    project_id: &str,
) -> Result<(WorkspaceService, Vec<StagedEditorFileDto>), PackageApiError> {
    let files_root = server_project_working_files_root(project_id)?;
    let workspace =
        WorkspaceService::from_workspace_root_diagnostics_only(&files_root).map_err(|err| {
            PackageApiError::internal(format!("failed to load server project: {err}"))
        })?;
    let staged_files = server_project_staged_files(project_id)?;
    Ok((workspace, staged_files))
}

fn server_project_staged_files(
    project_id: &str,
) -> Result<Vec<StagedEditorFileDto>, PackageApiError> {
    let files_root = server_project_working_files_root(project_id)?;
    let mut files = Vec::new();
    if files_root.exists() {
        collect_server_project_files(&files_root, &files_root, &mut files)?;
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files
        .into_iter()
        .map(|file| {
            let content = std::fs::read_to_string(files_root.join(&file.path)).map_err(|err| {
                PackageApiError::internal(format!("failed to read server project file: {err}"))
            })?;
            Ok(StagedEditorFileDto {
                path: file.path,
                content,
            })
        })
        .collect()
}

fn unique_project_id(name: &str) -> String {
    let slug = slugify_identifier(name);
    format!("{slug}-{}", unix_time_ms())
}

fn repository_name_from_url(url: &str) -> String {
    let without_trailing_slash = url.trim_end_matches('/');
    let last_segment = without_trailing_slash
        .rsplit(['/', ':'])
        .next()
        .unwrap_or(without_trailing_slash)
        .trim();
    let name = last_segment.strip_suffix(".git").unwrap_or(last_segment);
    if name.is_empty() {
        "repository".to_string()
    } else {
        name.to_string()
    }
}

fn slugify_identifier(value: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "project".to_string()
    } else {
        trimmed.to_string()
    }
}

fn project_package_name(name: &str) -> String {
    let mut result = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            result.push(ch);
        }
    }
    if result.is_empty() {
        "Project".to_string()
    } else if result
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
    {
        result
    } else {
        format!("Project{result}")
    }
}

fn initialize_server_project_git(project_dir: &Path, message: &str, user: &ServerUserDto) {
    let _ = Command::new("git")
        .arg("init")
        .current_dir(project_dir)
        .output();
    commit_server_project_git(project_dir, message, user);
}

fn commit_server_project_git(project_dir: &Path, message: &str, user: &ServerUserDto) {
    if !project_dir.join(".git").exists() {
        return;
    }
    let email = user.email.as_deref().unwrap_or("mark@mercurio.local");
    let _ = Command::new("git")
        .args(["add", "."])
        .current_dir(project_dir)
        .output();
    let _ = Command::new("git")
        .args([
            "-c",
            &format!("user.name={}", user.display_name),
            "-c",
            &format!("user.email={email}"),
            "commit",
            "-m",
            message,
        ])
        .current_dir(project_dir)
        .output();
}

fn read_server_project_git_history(
    project_dir: &Path,
) -> Result<Vec<ServerProjectHistoryEntryDto>, PackageApiError> {
    if !project_dir.join(".git").exists() {
        return Ok(Vec::new());
    }

    let output = Command::new("git")
        .args([
            "log",
            "--date=iso-strict",
            "--pretty=format:%H%x1f%an%x1f%ad%x1f%s",
        ])
        .current_dir(project_dir)
        .output()
        .map_err(|err| {
            PackageApiError::internal(format!("failed to read project history: {err}"))
        })?;
    if !output.status.success() {
        return Ok(Vec::new());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\u{1f}');
            let id = fields.next()?.to_string();
            let author = fields.next()?.to_string();
            let timestamp = fields.next()?.to_string();
            let summary = fields.next().unwrap_or_default().to_string();
            Some(ServerProjectHistoryEntryDto {
                id,
                summary,
                author,
                timestamp,
            })
        })
        .collect())
}

fn decode_package_artifact(request: &PackagePublishRequestDto) -> Result<Vec<u8>, PackageApiError> {
    if let Some(content) = request.artifact_content_base64.as_deref() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(content)
            .map_err(|err| {
                PackageApiError::bad_request(format!("invalid base64 artifact: {err}"))
            })?;
        if bytes.is_empty() {
            return Err(PackageApiError::bad_request(
                "package artifact content cannot be empty",
            ));
        }
        return Ok(bytes);
    }

    if let Some(content) = request.artifact_content.as_deref() {
        if content.is_empty() {
            return Err(PackageApiError::bad_request(
                "package artifact content cannot be empty",
            ));
        }
        return Ok(content.as_bytes().to_vec());
    }

    Err(PackageApiError::bad_request(
        "package artifact content is required",
    ))
}

fn package_registry_root() -> PathBuf {
    server_data_root().join("packages")
}

fn package_dir(name: &str) -> Result<PathBuf, PackageApiError> {
    validate_package_identifier(name, "package name")?;
    Ok(package_registry_root().join(name))
}

fn package_version_dir(name: &str, version: &str) -> Result<PathBuf, PackageApiError> {
    validate_package_identifier(version, "package version")?;
    Ok(package_dir(name)?.join(version))
}

fn list_package_versions_from_dir(package_dir: &Path) -> Result<Vec<String>, PackageApiError> {
    let mut versions = Vec::new();
    let entries = std::fs::read_dir(package_dir)
        .map_err(|err| PackageApiError::internal(format!("failed to read package: {err}")))?;
    for entry in entries {
        let entry = entry.map_err(|err| PackageApiError::internal(err.to_string()))?;
        if !entry
            .file_type()
            .map_err(|err| PackageApiError::internal(err.to_string()))?
            .is_dir()
        {
            continue;
        }
        if entry.path().join("manifest.json").exists() {
            versions.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    Ok(versions)
}

fn read_package_manifest(name: &str, version: &str) -> Result<PackageManifestDto, PackageApiError> {
    let manifest_path = package_version_dir(name, version)?.join("manifest.json");
    let content = std::fs::read_to_string(&manifest_path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            PackageApiError::not_found(format!("package version not found: {name} {version}"))
        } else {
            PackageApiError::internal(format!("failed to read package manifest: {err}"))
        }
    })?;
    serde_json::from_str(&content)
        .map_err(|err| PackageApiError::internal(format!("invalid package manifest: {err}")))
}

fn write_package_manifest(
    version_dir: &Path,
    manifest: &PackageManifestDto,
) -> Result<(), PackageApiError> {
    let content = serde_json::to_string_pretty(manifest)
        .map_err(|err| PackageApiError::internal(err.to_string()))?;
    std::fs::write(version_dir.join("manifest.json"), content).map_err(|err| {
        PackageApiError::internal(format!("failed to write package manifest: {err}"))
    })
}

fn validate_package_identifier(value: &str, label: &str) -> Result<(), PackageApiError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed != value || trimmed == "." || trimmed == ".." {
        return Err(PackageApiError::bad_request(format!("{label} is invalid")));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        return Err(PackageApiError::bad_request(format!(
            "{label} may only contain letters, numbers, '.', '_', and '-'"
        )));
    }
    Ok(())
}

fn validate_artifact_name(value: &str) -> Result<(), PackageApiError> {
    validate_package_identifier(value, "artifact name")?;
    if !value.ends_with(".kpar") && !value.ends_with(".zip") && !value.ends_with(".json") {
        return Err(PackageApiError::bad_request(
            "artifact name must end with .kpar, .zip, or .json",
        ));
    }
    Ok(())
}

fn checksum_hex(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

fn unix_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn read_server_state(state: &Arc<RwLock<ServerState>>) -> RwLockReadGuard<'_, ServerState> {
    match state.read() {
        Ok(guard) => guard,
        Err(poisoned) => {
            eprintln!("Server state lock was poisoned; recovering read access");
            poisoned.into_inner()
        }
    }
}

fn write_server_state(state: &Arc<RwLock<ServerState>>) -> RwLockWriteGuard<'_, ServerState> {
    match state.write() {
        Ok(guard) => guard,
        Err(poisoned) => {
            eprintln!("Server state lock was poisoned; recovering write access");
            poisoned.into_inner()
        }
    }
}

fn workspace_handle(
    state: &Arc<RwLock<ServerState>>,
    workspace_id: &str,
) -> Result<Arc<RwLock<WorkspaceService>>, ApiError> {
    read_server_state(state).workspace(workspace_id)
}

fn read_workspace_state(
    workspace: &Arc<RwLock<WorkspaceService>>,
) -> RwLockReadGuard<'_, WorkspaceService> {
    match workspace.read() {
        Ok(guard) => guard,
        Err(poisoned) => {
            eprintln!("Workspace state lock was poisoned; recovering read access");
            poisoned.into_inner()
        }
    }
}

fn write_workspace_state(
    workspace: &Arc<RwLock<WorkspaceService>>,
) -> RwLockWriteGuard<'_, WorkspaceService> {
    match workspace.write() {
        Ok(guard) => guard,
        Err(poisoned) => {
            eprintln!("Workspace state lock was poisoned; recovering write access");
            poisoned.into_inner()
        }
    }
}

fn build_metadata(graph: &Graph, stdlib_document: &KirDocument) -> ModelMetadataDto {
    let layers = graph
        .elements()
        .iter()
        .map(|element| element.layer)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let relations = graph
        .edges()
        .iter()
        .map(|edge| edge.relation.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let library_element_count = graph
        .elements()
        .iter()
        .filter(|element| element.layer < 2)
        .count();
    let user_element_count = graph
        .elements()
        .iter()
        .filter(|element| element.layer == 2)
        .count();

    ModelMetadataDto {
        element_count: graph.elements().len(),
        edge_count: graph.edge_count(),
        library_element_count,
        user_element_count,
        library_version: metadata_string(stdlib_document, "stdlib_version"),
        layers,
        relations,
        graph_scopes: GraphScope::all(),
        default_graph_scope: GraphScope::L2.as_str().to_string(),
    }
}

fn metadata_string(document: &KirDocument, key: &str) -> Option<String> {
    document
        .metadata
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            document
                .metadata
                .get("merged_sources")
                .and_then(Value::as_array)
                .and_then(|sources| {
                    sources
                        .iter()
                        .find_map(|source| source.get(key).and_then(Value::as_str))
                })
                .map(str::to_string)
        })
}

pub(crate) fn build_editor_outline(
    relative_path: &str,
    module: &SysmlModule,
    element_index: &HashMap<EditorOutlineKey, String>,
) -> Vec<EditorOutlineNodeDto> {
    if let Some(package) = &module.package {
        return vec![package_outline_node(relative_path, package, element_index)];
    }

    let mut nodes = outline_nodes_for_declarations(relative_path, &module.members, element_index);
    if nodes.is_empty() {
        nodes.extend(
            module
                .imports
                .iter()
                .map(|import| import_outline_node(relative_path, import, element_index)),
        );
        nodes.extend(module.definitions.iter().map(|definition| {
            part_definition_outline_node(relative_path, definition, element_index)
        }));
    }

    nodes
}

fn outline_nodes_for_declarations(
    relative_path: &str,
    declarations: &[Declaration],
    element_index: &HashMap<EditorOutlineKey, String>,
) -> Vec<EditorOutlineNodeDto> {
    declarations
        .iter()
        .map(|declaration| declaration_outline_node(relative_path, declaration, element_index))
        .collect()
}

fn declaration_outline_node(
    relative_path: &str,
    declaration: &Declaration,
    element_index: &HashMap<EditorOutlineKey, String>,
) -> EditorOutlineNodeDto {
    match declaration {
        Declaration::Package(package) => {
            package_outline_node(relative_path, package, element_index)
        }
        Declaration::Import(import) => import_outline_node(relative_path, import, element_index),
        Declaration::PartDefinition(definition) => {
            part_definition_outline_node(relative_path, definition, element_index)
        }
        Declaration::PartUsage(usage) => {
            part_usage_outline_node(relative_path, usage, element_index)
        }
        Declaration::GenericDefinition(definition) => {
            generic_definition_outline_node(relative_path, definition, element_index)
        }
        Declaration::GenericUsage(usage) => {
            generic_usage_outline_node(relative_path, usage, element_index)
        }
        Declaration::Alias(alias) => alias_outline_node(relative_path, alias, element_index),
    }
}

fn package_outline_node(
    relative_path: &str,
    package: &PackageDecl,
    element_index: &HashMap<EditorOutlineKey, String>,
) -> EditorOutlineNodeDto {
    let mut children =
        outline_nodes_for_declarations(relative_path, &package.members, element_index);
    if children.is_empty() {
        children.extend(
            package
                .imports
                .iter()
                .map(|import| import_outline_node(relative_path, import, element_index)),
        );
        children.extend(package.definitions.iter().map(|definition| {
            part_definition_outline_node(relative_path, definition, element_index)
        }));
    }

    outline_node(
        relative_path,
        element_index,
        "package",
        &package.name.as_colon_string(),
        "package",
        &package.span,
        BTreeMap::from([
            (
                "declared_name".to_string(),
                Value::String(
                    package
                        .name
                        .segments
                        .last()
                        .cloned()
                        .unwrap_or_else(|| package.name.as_colon_string()),
                ),
            ),
            (
                "qualified_name".to_string(),
                Value::String(package.name.as_colon_string()),
            ),
            (
                "member_count".to_string(),
                Value::from(children.len() as u64),
            ),
            (
                "docs".to_string(),
                Value::Array(package.docs.iter().cloned().map(Value::String).collect()),
            ),
            (
                "modifiers".to_string(),
                Value::Array(
                    package
                        .modifiers
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            ),
        ]),
        children,
    )
}

fn import_outline_node(
    relative_path: &str,
    import: &ImportDecl,
    element_index: &HashMap<EditorOutlineKey, String>,
) -> EditorOutlineNodeDto {
    outline_node(
        relative_path,
        element_index,
        "import",
        &import.path.as_colon_string(),
        "import",
        &import.span,
        BTreeMap::from([
            (
                "path".to_string(),
                Value::String(import.path.as_colon_string()),
            ),
            (
                "docs".to_string(),
                Value::Array(import.docs.iter().cloned().map(Value::String).collect()),
            ),
            (
                "modifiers".to_string(),
                Value::Array(
                    import
                        .modifiers
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            ),
        ]),
        Vec::new(),
    )
}

fn part_definition_outline_node(
    relative_path: &str,
    definition: &PartDefinitionDecl,
    element_index: &HashMap<EditorOutlineKey, String>,
) -> EditorOutlineNodeDto {
    let mut children =
        outline_nodes_for_declarations(relative_path, &definition.members, element_index);
    if children.is_empty() {
        children.extend(
            definition
                .part_members
                .iter()
                .map(|member| part_usage_outline_node(relative_path, member, element_index)),
        );
    }

    outline_node(
        relative_path,
        element_index,
        "part_definition",
        &definition.name,
        "part def",
        &definition.span,
        BTreeMap::from([
            ("name".to_string(), Value::String(definition.name.clone())),
            (
                "specializes".to_string(),
                Value::Array(
                    definition
                        .specializes
                        .iter()
                        .map(|item| Value::String(item.as_colon_string()))
                        .collect(),
                ),
            ),
            (
                "member_count".to_string(),
                Value::from(children.len() as u64),
            ),
            (
                "docs".to_string(),
                Value::Array(definition.docs.iter().cloned().map(Value::String).collect()),
            ),
            (
                "modifiers".to_string(),
                Value::Array(
                    definition
                        .modifiers
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            ),
        ]),
        children,
    )
}

fn part_usage_outline_node(
    relative_path: &str,
    usage: &PartUsageDecl,
    element_index: &HashMap<EditorOutlineKey, String>,
) -> EditorOutlineNodeDto {
    outline_node(
        relative_path,
        element_index,
        "part_usage",
        &usage.name,
        "part",
        &usage.span,
        BTreeMap::from([
            ("name".to_string(), Value::String(usage.name.clone())),
            (
                "type".to_string(),
                usage
                    .ty
                    .as_ref()
                    .map(|item| Value::String(item.as_colon_string()))
                    .unwrap_or(Value::Null),
            ),
            (
                "additional_types".to_string(),
                Value::Array(
                    usage
                        .additional_types
                        .iter()
                        .map(|item| Value::String(item.as_colon_string()))
                        .collect(),
                ),
            ),
            (
                "specializes".to_string(),
                Value::Array(
                    usage
                        .specializes
                        .iter()
                        .map(|item| Value::String(item.as_colon_string()))
                        .collect(),
                ),
            ),
            (
                "member_count".to_string(),
                Value::from(usage.body_members.len() as u64),
            ),
            (
                "docs".to_string(),
                Value::Array(usage.docs.iter().cloned().map(Value::String).collect()),
            ),
            (
                "modifiers".to_string(),
                Value::Array(usage.modifiers.iter().cloned().map(Value::String).collect()),
            ),
        ]),
        outline_nodes_for_declarations(relative_path, &usage.body_members, element_index),
    )
}

fn generic_definition_outline_node(
    relative_path: &str,
    definition: &GenericDefinitionDecl,
    element_index: &HashMap<EditorOutlineKey, String>,
) -> EditorOutlineNodeDto {
    outline_node(
        relative_path,
        element_index,
        &format!("{}_definition", definition.keyword),
        &definition.name,
        &format!("{} def", definition.keyword),
        &definition.span,
        BTreeMap::from([
            (
                "keyword".to_string(),
                Value::String(definition.keyword.clone()),
            ),
            ("name".to_string(), Value::String(definition.name.clone())),
            (
                "specializes".to_string(),
                Value::Array(
                    definition
                        .specializes
                        .iter()
                        .map(|item| Value::String(item.as_colon_string()))
                        .collect(),
                ),
            ),
            (
                "member_count".to_string(),
                Value::from(definition.members.len() as u64),
            ),
            (
                "docs".to_string(),
                Value::Array(definition.docs.iter().cloned().map(Value::String).collect()),
            ),
            (
                "modifiers".to_string(),
                Value::Array(
                    definition
                        .modifiers
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            ),
        ]),
        outline_nodes_for_declarations(relative_path, &definition.members, element_index),
    )
}

fn generic_usage_outline_node(
    relative_path: &str,
    usage: &GenericUsageDecl,
    element_index: &HashMap<EditorOutlineKey, String>,
) -> EditorOutlineNodeDto {
    outline_node(
        relative_path,
        element_index,
        &format!("{}_usage", usage.keyword),
        &usage.name,
        &usage.keyword,
        &usage.span,
        BTreeMap::from([
            ("keyword".to_string(), Value::String(usage.keyword.clone())),
            ("name".to_string(), Value::String(usage.name.clone())),
            (
                "type".to_string(),
                usage
                    .ty
                    .as_ref()
                    .map(|item| Value::String(item.as_colon_string()))
                    .unwrap_or(Value::Null),
            ),
            (
                "additional_types".to_string(),
                Value::Array(
                    usage
                        .additional_types
                        .iter()
                        .map(|item| Value::String(item.as_colon_string()))
                        .collect(),
                ),
            ),
            (
                "specializes".to_string(),
                Value::Array(
                    usage
                        .specializes
                        .iter()
                        .map(|item| Value::String(item.as_colon_string()))
                        .collect(),
                ),
            ),
            (
                "member_count".to_string(),
                Value::from(usage.body_members.len() as u64),
            ),
            (
                "docs".to_string(),
                Value::Array(usage.docs.iter().cloned().map(Value::String).collect()),
            ),
            (
                "modifiers".to_string(),
                Value::Array(usage.modifiers.iter().cloned().map(Value::String).collect()),
            ),
        ]),
        outline_nodes_for_declarations(relative_path, &usage.body_members, element_index),
    )
}

fn alias_outline_node(
    relative_path: &str,
    alias: &AliasDecl,
    element_index: &HashMap<EditorOutlineKey, String>,
) -> EditorOutlineNodeDto {
    outline_node(
        relative_path,
        element_index,
        "alias",
        &alias.name,
        "alias",
        &alias.span,
        BTreeMap::from([
            ("name".to_string(), Value::String(alias.name.clone())),
            (
                "target".to_string(),
                Value::String(alias.target.as_colon_string()),
            ),
            (
                "docs".to_string(),
                Value::Array(alias.docs.iter().cloned().map(Value::String).collect()),
            ),
            (
                "modifiers".to_string(),
                Value::Array(alias.modifiers.iter().cloned().map(Value::String).collect()),
            ),
        ]),
        Vec::new(),
    )
}

fn outline_node(
    relative_path: &str,
    element_index: &HashMap<EditorOutlineKey, String>,
    kind_key: &str,
    label: &str,
    kind: &str,
    span: &SourceSpan,
    properties: BTreeMap<String, Value>,
    children: Vec<EditorOutlineNodeDto>,
) -> EditorOutlineNodeDto {
    let element_id = element_index
        .get(&EditorOutlineKey::new(relative_path, span))
        .cloned();
    EditorOutlineNodeDto {
        id: format!(
            "{relative_path}:{kind_key}:{}:{}:{}:{}",
            span.start_line, span.start_col, span.end_line, span.end_col
        ),
        element_id,
        label: label.to_string(),
        kind: kind.to_string(),
        start_line_number: span.start_line,
        start_column: span.start_col,
        end_line_number: span.end_line,
        end_column: span.end_col,
        properties,
        children,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct EditorOutlineKey {
    source_file: String,
    start_line: usize,
    start_column: usize,
    end_line: usize,
    end_column: usize,
}

impl EditorOutlineKey {
    fn new(source_file: &str, span: &SourceSpan) -> Self {
        Self {
            source_file: normalize_source_file_key(source_file),
            start_line: span.start_line,
            start_column: span.start_col,
            end_line: span.end_line,
            end_column: span.end_col,
        }
    }
}

#[cfg(test)]
pub(crate) fn build_editor_outline_index(
    elements: &HashMap<String, ElementDetailsDto>,
) -> HashMap<EditorOutlineKey, String> {
    let mut index = HashMap::new();
    for (element_id, details) in elements {
        let Some((source_file, start_line, start_column, end_line, end_column)) =
            editor_outline_key_parts_for_properties(&details.direct_properties)
        else {
            continue;
        };
        for candidate_source_file in source_file_suffix_candidates(&source_file) {
            index
                .entry(EditorOutlineKey {
                    source_file: candidate_source_file,
                    start_line,
                    start_column,
                    end_line,
                    end_column,
                })
                .or_insert_with(|| element_id.clone());
        }
    }
    index
}

pub(crate) fn build_editor_outline_index_for_graph(
    graph: &Graph,
) -> HashMap<EditorOutlineKey, String> {
    let mut index = HashMap::new();
    for element in graph.elements() {
        let Some((source_file, start_line, start_column, end_line, end_column)) =
            editor_outline_key_parts_for_properties(&element.properties)
        else {
            continue;
        };
        for candidate_source_file in source_file_suffix_candidates(&source_file) {
            index
                .entry(EditorOutlineKey {
                    source_file: candidate_source_file,
                    start_line,
                    start_column,
                    end_line,
                    end_column,
                })
                .or_insert_with(|| element.element_id.clone());
        }
    }
    index
}

fn editor_outline_key_parts_for_properties(
    properties: &BTreeMap<String, Value>,
) -> Option<(String, usize, usize, usize, usize)> {
    let metadata = properties.get("metadata")?.as_object()?;
    let source_file = metadata.get("source_file")?.as_str()?;
    let span = metadata.get("source_span")?.as_object()?;

    Some((
        normalize_source_file_key(source_file),
        span.get("start_line")?.as_u64()? as usize,
        span.get("start_col")?.as_u64()? as usize,
        span.get("end_line")?.as_u64()? as usize,
        span.get("end_col")?.as_u64()? as usize,
    ))
}

fn normalize_source_file_key(source_file: &str) -> String {
    source_file.replace('\\', "/")
}

fn source_file_suffix_candidates(source_file: &str) -> Vec<String> {
    let normalized = normalize_source_file_key(source_file);
    let segments = normalized.split('/').collect::<Vec<_>>();
    let mut candidates = Vec::with_capacity(segments.len());
    for start in 0..segments.len() {
        candidates.push(segments[start..].join("/"));
    }
    candidates
}

#[cfg(test)]
fn build_semantic_editor_outline(
    relative_path: &str,
    elements: &HashMap<String, ElementDetailsDto>,
) -> Vec<EditorOutlineNodeDto> {
    let mut items = elements
        .values()
        .filter_map(|element| semantic_outline_item(relative_path, element))
        .collect::<Vec<_>>();
    items.sort_by(semantic_outline_item_order);

    let items_by_id = items
        .iter()
        .cloned()
        .map(|item| (item.id.clone(), item))
        .collect::<HashMap<_, _>>();
    let parent_by_id = items
        .iter()
        .map(|item| {
            (
                item.id.clone(),
                semantic_outline_parent_id(item, &items, &items_by_id),
            )
        })
        .collect::<HashMap<_, _>>();

    semantic_outline_nodes(None, &items, &parent_by_id)
}

pub(crate) fn build_semantic_editor_outline_from_document(
    relative_path: &str,
    document: &KirDocument,
) -> Vec<EditorOutlineNodeDto> {
    let mut items = document
        .elements
        .iter()
        .filter_map(|element| semantic_outline_item_from_kir(relative_path, element))
        .collect::<Vec<_>>();
    items.sort_by(semantic_outline_item_order);

    let items_by_id = items
        .iter()
        .cloned()
        .map(|item| (item.id.clone(), item))
        .collect::<HashMap<_, _>>();
    let parent_by_id = items
        .iter()
        .map(|item| {
            (
                item.id.clone(),
                semantic_outline_parent_id(item, &items, &items_by_id),
            )
        })
        .collect::<HashMap<_, _>>();

    semantic_outline_nodes(None, &items, &parent_by_id)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SemanticOutlineItem {
    id: String,
    label: String,
    kind: String,
    properties: BTreeMap<String, Value>,
    span: EditorOutlineKey,
    owner_id: Option<String>,
}

#[cfg(test)]
fn semantic_outline_item(
    relative_path: &str,
    element: &ElementDetailsDto,
) -> Option<SemanticOutlineItem> {
    if element.layer != 2 {
        return None;
    }

    let (source_file, start_line, start_column, end_line, end_column) =
        editor_outline_key_parts_for_properties(&element.direct_properties)?;
    if !source_file_matches_relative_path(&source_file, relative_path) {
        return None;
    }

    Some(SemanticOutlineItem {
        id: element.id.clone(),
        label: semantic_outline_label(element),
        kind: semantic_outline_kind(&element.kind),
        properties: element.direct_properties.clone(),
        span: EditorOutlineKey {
            source_file: normalize_source_file_key(relative_path),
            start_line,
            start_column,
            end_line,
            end_column,
        },
        owner_id: element
            .direct_properties
            .get("owner")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn semantic_outline_item_from_kir(
    relative_path: &str,
    element: &KirElement,
) -> Option<SemanticOutlineItem> {
    if element.layer != 2 {
        return None;
    }

    let (source_file, start_line, start_column, end_line, end_column) =
        editor_outline_key_parts_for_properties(&element.properties)?;
    if !source_file_matches_relative_path(&source_file, relative_path) {
        return None;
    }

    Some(SemanticOutlineItem {
        id: element.id.clone(),
        label: semantic_outline_label_from_properties(&element.id, &element.properties),
        kind: semantic_outline_kind(&element.kind),
        properties: element.properties.clone(),
        span: EditorOutlineKey {
            source_file: normalize_source_file_key(relative_path),
            start_line,
            start_column,
            end_line,
            end_column,
        },
        owner_id: element
            .properties
            .get("owner")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

#[cfg(test)]
fn semantic_outline_label(element: &ElementDetailsDto) -> String {
    semantic_outline_label_from_properties(&element.id, &element.direct_properties)
}

fn semantic_outline_label_from_properties(
    element_id: &str,
    properties: &BTreeMap<String, Value>,
) -> String {
    properties
        .get("declared_name")
        .and_then(Value::as_str)
        .or_else(|| properties.get("qualified_name").and_then(Value::as_str))
        .map(str::to_string)
        .unwrap_or_else(|| label_for_id(element_id))
}

fn semantic_outline_kind(kind: &str) -> String {
    kind.rsplit("::").next().unwrap_or(kind).to_string()
}

fn source_file_matches_relative_path(source_file: &str, relative_path: &str) -> bool {
    let relative_path = normalize_source_file_key(relative_path);
    source_file_suffix_candidates(source_file)
        .into_iter()
        .any(|candidate| candidate == relative_path)
}

fn semantic_outline_parent_id(
    item: &SemanticOutlineItem,
    items: &[SemanticOutlineItem],
    items_by_id: &HashMap<String, SemanticOutlineItem>,
) -> Option<String> {
    if let Some(owner_id) = item.owner_id.as_ref() {
        if let Some(owner) = items_by_id.get(owner_id) {
            if owner.span != item.span && span_contains(&owner.span, &item.span) {
                return Some(owner.id.clone());
            }
        }
    }

    items
        .iter()
        .filter(|candidate| candidate.id != item.id)
        .filter(|candidate| span_contains(&candidate.span, &item.span))
        .min_by(|left, right| {
            (span_extent(&left.span), &left.id).cmp(&(span_extent(&right.span), &right.id))
        })
        .map(|candidate| candidate.id.clone())
}

fn semantic_outline_nodes(
    parent_id: Option<&str>,
    items: &[SemanticOutlineItem],
    parent_by_id: &HashMap<String, Option<String>>,
) -> Vec<EditorOutlineNodeDto> {
    let mut children = items
        .iter()
        .filter(|item| {
            parent_by_id
                .get(&item.id)
                .and_then(|value| value.as_deref())
                == parent_id
        })
        .cloned()
        .collect::<Vec<_>>();
    children.sort_by(semantic_outline_item_order);

    children
        .into_iter()
        .map(|item| EditorOutlineNodeDto {
            id: item.id.clone(),
            element_id: Some(item.id.clone()),
            label: item.label,
            kind: item.kind,
            start_line_number: item.span.start_line,
            start_column: item.span.start_column,
            end_line_number: item.span.end_line,
            end_column: item.span.end_column,
            properties: item.properties,
            children: semantic_outline_nodes(Some(&item.id), items, parent_by_id),
        })
        .collect()
}

fn semantic_outline_item_order(
    left: &SemanticOutlineItem,
    right: &SemanticOutlineItem,
) -> std::cmp::Ordering {
    (
        left.span.start_line,
        left.span.start_column,
        span_extent(&left.span),
        &left.id,
    )
        .cmp(&(
            right.span.start_line,
            right.span.start_column,
            span_extent(&right.span),
            &right.id,
        ))
}

fn span_contains(container: &EditorOutlineKey, candidate: &EditorOutlineKey) -> bool {
    if container.source_file != candidate.source_file || container == candidate {
        return false;
    }

    let starts_before_or_at = (container.start_line, container.start_column)
        <= (candidate.start_line, candidate.start_column);
    let ends_after_or_at =
        (container.end_line, container.end_column) >= (candidate.end_line, candidate.end_column);

    starts_before_or_at && ends_after_or_at
}

fn span_extent(span: &EditorOutlineKey) -> (usize, usize, usize, usize) {
    (
        span.end_line.saturating_sub(span.start_line),
        span.end_column.saturating_sub(span.start_column),
        span.end_line,
        span.end_column,
    )
}

fn build_graph_dto(graph: &Graph, scope: GraphScope) -> GraphDto {
    let visible_ids = collect_graph_scope_ids(graph, scope);
    let mut nodes = graph
        .elements()
        .iter()
        .filter(|element| visible_ids.contains(&element.id))
        .map(|element| GraphNodeDto {
            id: element.element_id.clone(),
            label: label_for_id(&element.element_id),
            kind: element.kind.clone(),
            layer: element.layer,
            property_count: element.properties.len(),
        })
        .collect::<Vec<_>>();
    nodes.sort_by(|left, right| left.id.cmp(&right.id));

    let mut edges = graph
        .edges()
        .iter()
        .filter(|edge| visible_ids.contains(&edge.source) && visible_ids.contains(&edge.target))
        .map(|edge| to_edge_dto(graph, edge))
        .collect::<Vec<_>>();
    edges.sort_by(|left, right| left.id.cmp(&right.id));

    GraphDto { nodes, edges }
}

fn build_metatype_explorer_dto(
    graph: &Graph,
    metamodel_registry: &MetamodelAttributeRegistry,
    request: &MetatypeExplorerRequestDto,
) -> Result<MetatypeExplorerGraphDto, ApiError> {
    let seed = graph
        .element_by_element_id(&request.seed_id)
        .ok_or_else(|| ApiError::MissingElement(request.seed_id.clone()))?;
    let expanded_parents = request
        .expanded_parents
        .iter()
        .filter_map(|id| graph.node_id(id))
        .collect::<BTreeSet<_>>();
    let expanded_children = request
        .expanded_children
        .iter()
        .filter_map(|id| graph.node_id(id))
        .collect::<BTreeSet<_>>();
    let mut visible_ids = BTreeSet::from([seed.id]);

    for edge in graph.outgoing(seed.id, "specializes") {
        visible_ids.insert(edge.target);
    }

    for node_id in &expanded_parents {
        visible_ids.insert(*node_id);
        for edge in graph.outgoing(*node_id, "specializes") {
            visible_ids.insert(edge.target);
        }
    }

    for node_id in &expanded_children {
        visible_ids.insert(*node_id);
        for edge in graph.incoming(*node_id, "specializes") {
            visible_ids.insert(edge.source);
        }
    }

    let mut nodes = visible_ids
        .iter()
        .filter_map(|node_id| graph.element(*node_id))
        .map(|element| {
            build_metatype_explorer_node_dto(graph, metamodel_registry, element, seed.id)
        })
        .collect::<Vec<_>>();
    nodes.sort_by(|left, right| left.id.cmp(&right.id));

    let mut edge_keys = BTreeSet::new();
    for node_id in &visible_ids {
        for edge in graph.outgoing(*node_id, "specializes") {
            if !visible_ids.contains(&edge.target) {
                continue;
            }

            let Some(source_id) = graph.element_id(edge.source) else {
                continue;
            };
            let Some(target_id) = graph.element_id(edge.target) else {
                continue;
            };
            edge_keys.insert((source_id.to_string(), target_id.to_string()));
        }
    }

    let edges = edge_keys
        .into_iter()
        .map(|(source, target)| MetatypeExplorerEdgeDto {
            id: format!("specializes:{source}->{target}"),
            source,
            target,
            relation: "specializes".to_string(),
        })
        .collect();

    Ok(MetatypeExplorerGraphDto {
        seed_id: seed.element_id.clone(),
        nodes,
        edges,
    })
}

fn build_metatype_explorer_node_dto(
    graph: &Graph,
    metamodel_registry: &MetamodelAttributeRegistry,
    element: &mercurio_core::graph::Element,
    seed_id: u32,
) -> MetatypeExplorerNodeDto {
    let mut attributes = metamodel_registry
        .declared_attributes_for(&element.element_id)
        .iter()
        .map(|declaration| ExplorerAttributeDto {
            name: declaration.name.clone(),
            type_label: declaration.type_label.clone(),
        })
        .collect::<Vec<_>>();
    attributes.sort_by(|left, right| left.name.cmp(&right.name));

    MetatypeExplorerNodeDto {
        id: element.element_id.clone(),
        label: label_for_id(&element.element_id),
        kind: element.kind.clone(),
        layer: element.layer,
        attributes,
        specializes_count: graph.outgoing(element.id, "specializes").count(),
        specialized_by_count: graph.incoming(element.id, "specializes").count(),
        is_seed: element.id == seed_id,
    }
}

fn build_l2_explorer_dto(
    graph: &Graph,
    request: &L2ExplorerRequestDto,
) -> Result<L2ExplorerGraphDto, ApiError> {
    let seed = graph
        .element_by_element_id(&request.seed_id)
        .ok_or_else(|| ApiError::MissingElement(request.seed_id.clone()))?;
    if seed.layer != 2 {
        return Err(ApiError::MissingElement(request.seed_id.clone()));
    }

    let expanded_parents = request
        .expanded_parents
        .iter()
        .filter_map(|id| graph.node_id(id))
        .collect::<BTreeSet<_>>();
    let expanded_children = request
        .expanded_children
        .iter()
        .filter_map(|id| graph.node_id(id))
        .collect::<BTreeSet<_>>();
    let mut visible_ids = BTreeSet::from([seed.id]);

    for edge in graph.outgoing(seed.id, "specializes") {
        if graph
            .element(edge.target)
            .is_some_and(|element| element.layer == 2)
        {
            visible_ids.insert(edge.target);
        }
    }

    for node_id in &expanded_parents {
        let Some(element) = graph.element(*node_id) else {
            continue;
        };
        if element.layer != 2 {
            continue;
        }
        visible_ids.insert(*node_id);
        for edge in graph.outgoing(*node_id, "specializes") {
            if graph
                .element(edge.target)
                .is_some_and(|target| target.layer == 2)
            {
                visible_ids.insert(edge.target);
            }
        }
    }

    for node_id in &expanded_children {
        let Some(element) = graph.element(*node_id) else {
            continue;
        };
        if element.layer != 2 {
            continue;
        }
        visible_ids.insert(*node_id);
        for edge in graph.incoming(*node_id, "specializes") {
            if graph
                .element(edge.source)
                .is_some_and(|source| source.layer == 2)
            {
                visible_ids.insert(edge.source);
            }
        }
    }

    let mut nodes = visible_ids
        .iter()
        .filter_map(|node_id| graph.element(*node_id))
        .map(|element| build_l2_explorer_node_dto(graph, element, seed.id))
        .collect::<Vec<_>>();
    nodes.sort_by(|left, right| left.id.cmp(&right.id));

    let mut edge_keys = BTreeSet::new();
    for node_id in &visible_ids {
        for edge in graph.outgoing(*node_id, "specializes") {
            if !visible_ids.contains(&edge.target) {
                continue;
            }
            let Some(source_id) = graph.element_id(edge.source) else {
                continue;
            };
            let Some(target_id) = graph.element_id(edge.target) else {
                continue;
            };
            edge_keys.insert((
                source_id.to_string(),
                target_id.to_string(),
                "specializes".to_string(),
            ));
        }
    }

    if request.include_reference_edges {
        for node_id in &visible_ids {
            for edge in graph.outgoing_edges(*node_id) {
                if !visible_ids.contains(&edge.target)
                    || !include_l2_reference_relation(&edge.relation)
                {
                    continue;
                }
                let Some(source_id) = graph.element_id(edge.source) else {
                    continue;
                };
                let Some(target_id) = graph.element_id(edge.target) else {
                    continue;
                };
                edge_keys.insert((
                    source_id.to_string(),
                    target_id.to_string(),
                    edge.relation.clone(),
                ));
            }
        }
    }

    let edges = edge_keys
        .into_iter()
        .map(|(source, target, relation)| L2ExplorerEdgeDto {
            id: format!("{relation}:{source}->{target}"),
            source,
            target,
            relation,
        })
        .collect();

    Ok(L2ExplorerGraphDto {
        seed_id: seed.element_id.clone(),
        nodes,
        edges,
    })
}

fn build_l2_explorer_node_dto(
    graph: &Graph,
    element: &mercurio_core::graph::Element,
    seed_id: u32,
) -> L2ExplorerNodeDto {
    let mut attributes = owned_feature_attributes(graph, element);
    attributes.sort_by(|left, right| left.name.cmp(&right.name));

    L2ExplorerNodeDto {
        id: element.element_id.clone(),
        label: label_for_id(&element.element_id),
        kind: element.kind.clone(),
        layer: element.layer,
        attributes,
        specializes_count: graph
            .outgoing(element.id, "specializes")
            .filter(|edge| {
                graph
                    .element(edge.target)
                    .is_some_and(|target| target.layer == 2)
            })
            .count(),
        specialized_by_count: graph
            .incoming(element.id, "specializes")
            .filter(|edge| {
                graph
                    .element(edge.source)
                    .is_some_and(|source| source.layer == 2)
            })
            .count(),
        is_seed: element.id == seed_id,
    }
}

fn include_l2_reference_relation(relation: &str) -> bool {
    !matches!(relation, "specializes" | "owner" | "metatype")
}

fn owned_feature_attributes(
    graph: &Graph,
    element: &mercurio_core::graph::Element,
) -> Vec<ExplorerAttributeDto> {
    element
        .properties
        .get("features")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|feature_id| graph.element_by_element_id(feature_id))
        .filter_map(|feature| {
            explorer_declared_name(feature).map(|name| ExplorerAttributeDto {
                name,
                type_label: explorer_type_label(feature),
            })
        })
        .collect()
}

fn explorer_declared_name(element: &mercurio_core::graph::Element) -> Option<String> {
    element
        .properties
        .get("declared_name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            element
                .properties
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn explorer_type_label(element: &mercurio_core::graph::Element) -> Option<String> {
    relation_type_label(element.properties.get("type"))
}

fn relation_type_label(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(entry)) => Some(label_for_id(entry)),
        Some(Value::Array(entries)) => entries.iter().find_map(|entry| match entry {
            Value::String(item) => Some(label_for_id(item)),
            _ => None,
        }),
        _ => None,
    }
}

fn collect_graph_scope_ids(graph: &Graph, scope: GraphScope) -> BTreeSet<u32> {
    let mut visible_ids = graph
        .elements()
        .iter()
        .filter(|element| match scope {
            GraphScope::L2 | GraphScope::L2PlusContext => element.layer == 2,
            GraphScope::Full => true,
        })
        .map(|element| element.id)
        .collect::<BTreeSet<_>>();

    if scope == GraphScope::L2PlusContext {
        let l2_ids = visible_ids.iter().copied().collect::<Vec<_>>();
        for node_id in l2_ids {
            for edge in graph.outgoing_edges(node_id) {
                visible_ids.insert(edge.target);
            }
            for edge in graph.incoming_edges(node_id) {
                visible_ids.insert(edge.source);
            }
        }
    }

    visible_ids
}

fn build_element_details(
    graph: &Graph,
    metamodel_registry: &MetamodelAttributeRegistry,
    element: &mercurio_core::graph::Element,
    inbound: Vec<GraphEdgeDto>,
    outbound: Vec<GraphEdgeDto>,
) -> ElementDetailsDto {
    let ancestors = collect_specialization_ancestors(graph, element.id);
    let specialization_chain = ancestors
        .iter()
        .map(|ancestor| element_summary_dto(ancestor))
        .collect::<Vec<_>>();
    let inherited_properties = ancestors
        .iter()
        .filter(|ancestor| !ancestor.properties.is_empty())
        .map(|ancestor| InheritedPropertiesDto {
            element: element_summary_dto(ancestor),
            properties: ancestor.properties.clone(),
        })
        .collect::<Vec<_>>();

    let derived_properties = derived_properties(graph, element);
    let effective_properties = mercurio_core::metamodel::effective_properties_with_derived(
        &ancestors,
        &element.properties,
        &derived_properties,
    );
    let attribute_query = query_element_attributes(graph, metamodel_registry, element.id, None)
        .unwrap_or_else(|| mercurio_core::metamodel::ElementAttributeQuery {
            metatype: None,
            metatype_specialization_chain: Vec::new(),
            rows: Vec::new(),
        });

    ElementDetailsDto {
        id: element.element_id.clone(),
        label: label_for_id(&element.element_id),
        kind: element.kind.clone(),
        layer: element.layer,
        metatype: attribute_query.metatype.map(element_summary_from_query),
        metatype_specialization_chain: attribute_query
            .metatype_specialization_chain
            .into_iter()
            .map(element_summary_from_query)
            .collect(),
        direct_properties: element.properties.clone(),
        inherited_properties,
        effective_properties,
        property_table: ElementPropertyTableDto {
            rows: attribute_query
                .rows
                .into_iter()
                .map(property_row_from_query)
                .collect(),
        },
        specialization_chain,
        inbound,
        outbound,
    }
}

fn element_summary_from_query(
    summary: mercurio_core::metamodel::ElementSummary,
) -> ElementSummaryDto {
    ElementSummaryDto {
        id: summary.id,
        label: summary.label,
        kind: summary.kind,
        layer: summary.layer,
    }
}

fn inherited_value_from_query(value: AttributeValueSource) -> InheritedPropertyValueDto {
    InheritedPropertyValueDto {
        element: element_summary_from_query(value.element),
        value: value.value,
    }
}

fn property_row_from_query(row: AttributeRow) -> ElementPropertyRowDto {
    ElementPropertyRowDto {
        name: row.name,
        declared_by: row.declared_by.map(element_summary_from_query),
        origin_kind: row.origin_kind,
        has_direct_value: row.has_direct_value,
        direct_value: row.direct_value,
        has_effective_value: row.has_effective_value,
        effective_value: row.effective_value,
        inherited_values: row
            .inherited_values
            .into_iter()
            .map(inherited_value_from_query)
            .collect(),
    }
}

fn element_summary_dto(element: &mercurio_core::graph::Element) -> ElementSummaryDto {
    ElementSummaryDto {
        id: element.element_id.clone(),
        label: label_for_id(&element.element_id),
        kind: element.kind.clone(),
        layer: element.layer,
    }
}

fn build_library_tree(graph: &Graph) -> Vec<LibraryTreeNodeDto> {
    build_tree_from_graph(graph, |element| element.layer < 2)
}

fn build_tree_from_graph(
    graph: &Graph,
    include_element: impl Fn(&mercurio_core::graph::Element) -> bool,
) -> Vec<LibraryTreeNodeDto> {
    let mut root = TreeNode::root();
    let mut library_elements = graph
        .elements()
        .iter()
        .filter(|element| include_element(element))
        .collect::<Vec<_>>();
    library_elements.sort_by(|left, right| left.element_id.cmp(&right.element_id));

    for element in library_elements {
        root.insert(path_segments_for_tree(&element.element_id), element);
    }

    root.into_children()
}

fn path_segments_for_tree(element_id: &str) -> Vec<String> {
    if element_id.contains("::") {
        element_id.split("::").map(str::to_string).collect()
    } else {
        element_id.split('.').map(str::to_string).collect()
    }
}

fn to_edge_dto(graph: &Graph, edge: &Edge) -> GraphEdgeDto {
    let source = graph
        .element_id(edge.source)
        .unwrap_or("<unknown-source>")
        .to_string();
    let target = graph
        .element_id(edge.target)
        .unwrap_or("<unknown-target>")
        .to_string();

    GraphEdgeDto {
        id: format!("{source}:{}:{target}", edge.relation),
        source,
        target,
        relation: edge.relation.clone(),
    }
}

fn label_for_id(id: &str) -> String {
    let tail = id.rsplit("::").next().unwrap_or(id);
    tail.rsplit('.').next().unwrap_or(tail).to_string()
}

fn value_type_label(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn default_true() -> bool {
    true
}

pub(crate) fn is_model_source_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("sysml" | "kerml")
    )
}

#[allow(dead_code)]
pub(crate) fn is_sysml_file(path: &Path) -> bool {
    is_model_source_file(path)
}

pub(crate) fn collect_editor_files(
    root: &Path,
    current: &Path,
    files: &mut Vec<EditorFileSummaryDto>,
) -> Result<(), ApiError> {
    let mut entries = std::fs::read_dir(current)?.collect::<Result<Vec<_>, std::io::Error>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_editor_files(root, &path, files)?;
            continue;
        }

        if is_model_source_file(&path) {
            let relative = path
                .strip_prefix(root)
                .expect("workspace file should remain under root");
            files.push(EditorFileSummaryDto {
                path: normalize_relative_path(relative),
            });
        }
    }

    Ok(())
}

pub(crate) fn resolve_workspace_scope(
    root: &Path,
    relative_path: &str,
) -> Result<PathBuf, ApiError> {
    if relative_path == "." {
        return Ok(root.to_path_buf());
    }

    let candidate = Path::new(relative_path);
    if candidate.as_os_str().is_empty() {
        return Err(ApiError::InvalidPath(relative_path.to_string()));
    }

    for component in candidate.components() {
        match component {
            Component::Normal(_) => {}
            _ => return Err(ApiError::InvalidPath(relative_path.to_string())),
        }
    }

    let path = root.join(candidate);
    if !path.exists() || !path.is_dir() {
        return Err(ApiError::InvalidPath(relative_path.to_string()));
    }

    Ok(path)
}

pub(crate) fn resolve_workspace_file(
    root: &Path,
    relative_path: &str,
) -> Result<PathBuf, ApiError> {
    let candidate = Path::new(relative_path);
    if candidate.as_os_str().is_empty() {
        return Err(ApiError::InvalidPath(relative_path.to_string()));
    }

    for component in candidate.components() {
        match component {
            Component::Normal(_) => {}
            _ => return Err(ApiError::InvalidPath(relative_path.to_string())),
        }
    }

    if !is_model_source_file(candidate) {
        return Err(ApiError::InvalidPath(relative_path.to_string()));
    }

    Ok(root.join(candidate))
}

pub(crate) fn normalize_relative_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            Component::CurDir => Some(".".to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn diagnostic_to_dto(diagnostic: &Diagnostic) -> EditorDiagnosticDto {
    let (start_line_number, start_column, end_line_number, end_column) =
        diagnostic.span.as_ref().map_or((1, 1, 1, 1), |span| {
            (span.start_line, span.start_col, span.end_line, span.end_col)
        });

    EditorDiagnosticDto {
        message: diagnostic.message.clone(),
        severity: "error".to_string(),
        start_line_number,
        start_column,
        end_line_number,
        end_column,
    }
}

pub(crate) fn lint_diagnostic_to_dto(diagnostic: &LintDiagnostic) -> EditorDiagnosticDto {
    let (start_line_number, start_column, end_line_number, end_column) =
        diagnostic.span.as_ref().map_or((1, 1, 1, 1), |span| {
            (span.start_line, span.start_col, span.end_line, span.end_col)
        });

    EditorDiagnosticDto {
        message: format!("[{}] {}", diagnostic.code, diagnostic.message),
        severity: match diagnostic.severity {
            LintSeverity::Error => "error",
            LintSeverity::Warning => "warning",
            LintSeverity::Info => "info",
        }
        .to_string(),
        start_line_number,
        start_column,
        end_line_number,
        end_column,
    }
}

pub(crate) fn source_language_for_path(path: &str) -> Option<SourceLanguage> {
    SourceLanguage::from_path(Path::new(path))
}

pub(crate) const DEFAULT_SYSML_TEMPLATE: &str =
    "package Demo {\n  part def Vehicle {\n    part engine: Engine;\n  }\n}\n";

pub(crate) const DEFAULT_KERML_TEMPLATE: &str = "package Demo {\n  classifier Vehicle {\n    feature engine : Engine;\n  }\n\n  classifier Engine;\n}\n";

#[derive(Debug, Default)]
struct TreeNode {
    label: String,
    element_id: Option<String>,
    children: BTreeMap<String, TreeNode>,
}

impl TreeNode {
    fn root() -> Self {
        Self::default()
    }

    fn insert(&mut self, segments: Vec<String>, element: &mercurio_core::graph::Element) {
        if segments.is_empty() {
            return;
        }

        let mut current = self;
        let path_len = segments.len();
        for (index, segment) in segments.into_iter().enumerate() {
            current = current
                .children
                .entry(segment.clone())
                .or_insert_with(|| TreeNode {
                    label: segment,
                    element_id: None,
                    children: BTreeMap::new(),
                });

            if index + 1 == path_len {
                current.element_id = Some(element.element_id.clone());
            }
        }
    }

    fn into_children(self) -> Vec<LibraryTreeNodeDto> {
        self.children
            .into_iter()
            .map(|(key, child)| child.into_dto(key))
            .collect()
    }

    fn into_dto(self, id: String) -> LibraryTreeNodeDto {
        let TreeNode {
            label,
            element_id,
            children,
        } = self;
        let children = children
            .into_iter()
            .map(|(child_id, child)| child.into_dto(child_id))
            .collect::<Vec<_>>();
        let node_type = if element_id.is_some() {
            "element"
        } else {
            "namespace"
        };

        LibraryTreeNodeDto {
            id,
            label,
            node_type: node_type.to_string(),
            element_id,
            child_count: children.len(),
            children,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::{
        EditorOutlineKey, ElementDetailsDto, ElementPropertyTableDto, GraphScope, StoredAiSettings,
        build_editor_outline_index, build_router, build_semantic_editor_outline, load_server_state,
        read_current_ai_usage, reserve_ai_tokens, rewrite_url_base, write_stored_ai_settings,
    };
    use mercurio_core::repo_path;

    static SERVER_DATA_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn sample_state() -> super::ServerState {
        load_server_state(&repo_path("examples/vehicle_model.json")).unwrap()
    }

    fn temp_workspace_file(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("mercurio_api_{name}_{unique}"));
        std::fs::create_dir_all(&root).unwrap();
        root.join("model.sysml")
    }

    fn temp_workspace_named_file(name: &str, file_name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("mercurio_api_{name}_{unique}"));
        std::fs::create_dir_all(&root).unwrap();
        root.join(file_name)
    }

    fn temp_server_data(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("mercurio_server_data_{name}_{unique}"));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    async fn response_json(response: axum::response::Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn open_test_workspace(app: axum::Router, root: &std::path::Path) -> String {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/workspaces")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "path": root.display().to_string(),
                            "mode": "lazy"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response_json(response)
            .await
            .get("workspaceId")
            .and_then(Value::as_str)
            .unwrap()
            .to_string()
    }

    fn run_test_git(root: &std::path::Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap_or_else(|err| panic!("failed to run git {}: {err}", args.join(" ")));
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn run_test_git_output(root: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap_or_else(|err| panic!("failed to run git {}: {err}", args.join(" ")));
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn watched_repository_clone_url_uses_internal_gitea_base_when_configured() {
        assert_eq!(
            rewrite_url_base(
                "http://127.0.0.1:3001/mark/test1.git",
                "http://localhost:3001/",
                "http://gitea:3000/",
            ),
            "http://gitea:3000/mark/test1.git"
        );
        assert_eq!(
            rewrite_url_base(
                "https://github.com/acme/model.git",
                "http://127.0.0.1:3001",
                "http://gitea:3000",
            ),
            "https://github.com/acme/model.git"
        );
    }

    #[tokio::test]
    async fn ask_mercurio_endpoint_returns_draft_without_project_context() {
        let _env_guard = SERVER_DATA_ENV_LOCK.lock().await;
        let server_data = temp_server_data("ask_without_project");
        unsafe {
            std::env::set_var("MERCURIO_SERVER_DATA", &server_data);
        }
        write_stored_ai_settings(&StoredAiSettings {
            provider: Some("heuristic".to_string()),
            ..StoredAiSettings::default()
        })
        .unwrap();
        let app = build_router(sample_state());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/ai/ask")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "taskHint": "pr_draft",
                            "messages": [
                                {
                                    "role": "user",
                                    "content": "Draft a pull request for the model update"
                                }
                            ]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["task"], "proposal_draft");
        assert_eq!(json["artifacts"][0]["kind"], "proposal_draft");
        assert_eq!(json["artifacts"][0]["data"]["suggestedBaseBranch"], "main");
        assert!(
            json["message"]
                .as_str()
                .unwrap()
                .contains("Configure OpenAI")
        );

        unsafe {
            std::env::remove_var("MERCURIO_SERVER_DATA");
        }
        std::fs::remove_dir_all(server_data).unwrap();
    }

    #[tokio::test]
    async fn ai_settings_endpoint_persists_config_without_returning_secret() {
        let _env_guard = SERVER_DATA_ENV_LOCK.lock().await;
        let server_data = temp_server_data("ai_settings");
        unsafe {
            std::env::set_var("MERCURIO_SERVER_DATA", &server_data);
        }
        let app = build_router(sample_state());

        let save_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/ai/settings")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "provider": "openai",
                            "openaiModel": "gpt-test",
                            "openaiBaseUrl": "https://example.test/v1/responses",
                            "openaiApiKey": "test-secret"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(save_response.status(), StatusCode::OK);
        let body = save_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["provider"], "openai");
        assert_eq!(json["openaiApiKeyConfigured"], true);
        assert!(json.get("openaiApiKey").is_none());

        let status_response = app
            .oneshot(
                Request::builder()
                    .uri("/api/ai/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status_response.status(), StatusCode::OK);
        let body = status_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let status: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(status["provider_label"], "OpenAI");
        assert_eq!(status["model_label"], "gpt-test");

        std::fs::remove_dir_all(server_data).unwrap();
    }

    #[test]
    fn ai_token_budget_rejects_oversized_external_requests() {
        let _env_guard = SERVER_DATA_ENV_LOCK.blocking_lock();
        let server_data = temp_server_data("ai_token_budget");
        unsafe {
            std::env::set_var("MERCURIO_SERVER_DATA", &server_data);
        }
        let settings = StoredAiSettings {
            per_request_token_limit: Some(10),
            daily_token_limit: Some(100),
            ..StoredAiSettings::default()
        };

        let result = reserve_ai_tokens(&settings, true, 11);

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
        let usage = read_current_ai_usage().unwrap();
        assert_eq!(usage.rejected_requests, 1);

        std::fs::remove_dir_all(server_data).unwrap();
    }

    #[tokio::test]
    async fn proposal_endpoint_saves_ask_mercurio_proposal_draft() {
        let _env_guard = SERVER_DATA_ENV_LOCK.lock().await;
        let server_data = temp_server_data("ask_proposal_save");
        unsafe {
            std::env::set_var("MERCURIO_SERVER_DATA", &server_data);
        }
        let app = build_router(sample_state());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/proposals")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "title": "Draft brake model proposal",
                            "description": "Prepared by Ask Mercurio",
                            "itemType": "proposal",
                            "status": "draft",
                            "priority": "medium",
                            "labels": ["ask-mercurio"],
                            "linkedSemanticElements": ["type.Vehicle"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["itemType"], "proposal");
        assert_eq!(json["status"], "draft");
        assert_eq!(json["labels"][0], "ask-mercurio");
        assert_eq!(json["linkedSemanticElements"][0], "type.Vehicle");

        std::fs::remove_dir_all(server_data).unwrap();
    }

    #[tokio::test]
    async fn proposal_pull_request_endpoint_records_provider_binding() {
        let _env_guard = SERVER_DATA_ENV_LOCK.lock().await;
        let server_data = temp_server_data("proposal_pull_request_binding");
        unsafe {
            std::env::set_var("MERCURIO_SERVER_DATA", &server_data);
        }
        let app = build_router(sample_state());
        let create_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/proposals")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "key": "MER-77",
                            "title": "Add reviewer workflow",
                            "itemType": "proposal"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_response.status(), StatusCode::OK);

        let update_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/proposals/MER-77/pull-request")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "provider": "gitea",
                            "repositoryId": "repo-1",
                            "branch": "proposal/mer-77-reviewer-workflow",
                            "baseBranch": "develop",
                            "pullRequestState": "planned"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(update_response.status(), StatusCode::OK);
        let body = update_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "PR planned");
        assert_eq!(json["pullRequest"]["provider"], "gitea");
        assert_eq!(
            json["pullRequest"]["branch"],
            "proposal/mer-77-reviewer-workflow"
        );
        assert_eq!(json["pullRequest"]["baseBranch"], "develop");
        assert!(json["pullRequest"]["lastSyncedAtUnixMs"].as_u64().is_some());

        std::fs::remove_dir_all(server_data).unwrap();
    }

    #[tokio::test]
    async fn problem_report_endpoint_links_discussion_to_gitea_issue_binding() {
        let _env_guard = SERVER_DATA_ENV_LOCK.lock().await;
        let server_data = temp_server_data("discussion_problem_report_binding");
        unsafe {
            std::env::set_var("MERCURIO_SERVER_DATA", &server_data);
            std::env::remove_var("MERCURIO_GITEA_TOKEN");
        }
        let app = build_router(sample_state());
        let create_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/proposals")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "key": "MER-88",
                            "title": "Discuss validation workflow",
                            "itemType": "discussion"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_response.status(), StatusCode::OK);

        let report_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/proposals/MER-88/problem-reports")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "title": "Validation workflow drops backing issue state",
                            "repositoryExternalId": "http://localhost:3001/acme/models.git",
                            "linkedSemanticElements": ["type.Vehicle"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(report_response.status(), StatusCode::OK);
        let body = report_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["discussion"]["status"], "Problem reports linked");
        assert_eq!(json["problemReport"]["itemType"], "problem_report");
        assert_eq!(json["problemReport"]["relatedWorkItems"][0], "MER-88");
        assert_eq!(
            json["problemReport"]["externalBindings"][0]["provider"],
            "gitea"
        );
        assert_eq!(
            json["problemReport"]["externalBindings"][0]["kind"],
            "issue"
        );
        assert_eq!(
            json["problemReport"]["externalBindings"][0]["state"],
            "planned"
        );

        std::fs::remove_dir_all(server_data).unwrap();
    }

    #[tokio::test]
    async fn demo_cycle_endpoint_advances_implementation_review_and_merge_state() {
        let _env_guard = SERVER_DATA_ENV_LOCK.lock().await;
        let server_data = temp_server_data("demo_cycle_advance");
        unsafe {
            std::env::set_var("MERCURIO_SERVER_DATA", &server_data);
        }
        let app = build_router(sample_state());
        let create_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/proposals")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "key": "MER-89",
                            "title": "Implement validation workflow",
                            "itemType": "proposal"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_response.status(), StatusCode::OK);

        let task_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/proposals/MER-89/demo-cycle")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "action": "task_ai",
                            "repositoryExternalId": "http://localhost:3001/acme/models.git",
                            "baseBranch": "develop"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(task_response.status(), StatusCode::OK);
        let body = task_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "AI implementation tasked");
        assert_eq!(json["pullRequest"]["provider"], "gitea");
        assert_eq!(json["pullRequest"]["baseBranch"], "develop");

        for (action, expected_status) in [
            ("review_semantic_diff", "Semantic review ready"),
            ("approve", "Approved"),
            ("merge", "Merged"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v2/proposals/MER-89/demo-cycle")
                        .header("content-type", "application/json")
                        .body(Body::from(json!({ "action": action }).to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = response.into_body().collect().await.unwrap().to_bytes();
            let json: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["status"], expected_status);
        }

        std::fs::remove_dir_all(server_data).unwrap();
    }

    #[tokio::test]
    async fn watched_repository_catalog_creates_projects_without_server_files() {
        let _env_guard = SERVER_DATA_ENV_LOCK.lock().await;
        let server_data = temp_server_data("watched_catalog");
        unsafe {
            std::env::set_var("MERCURIO_SERVER_DATA", &server_data);
        }
        let app = build_router(sample_state());

        let create_repository_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/repositories")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "name": "Vehicle Models",
                            "url": "https://example.com/acme/vehicle-models.git",
                            "defaultBranch": "main"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_repository_response.status(), StatusCode::OK);
        let body = create_repository_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let repository: Value = serde_json::from_slice(&body).unwrap();
        let repository_id = repository["id"].as_str().unwrap();

        let create_project_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/repositories/{repository_id}/projects"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "name": "Brake Controller",
                            "rootPath": "systems/brake",
                            "descriptorPath": "systems/brake/mercurio.project.json"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_project_response.status(), StatusCode::OK);
        let body = create_project_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let watched_project: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(watched_project["repositoryId"], repository_id);
        let watched_project_id = watched_project["id"].as_str().unwrap();
        assert_eq!(watched_project["rootPath"], "systems/brake");
        assert_eq!(watched_project["sourceGlobs"][0], "**/*.kerml");
        assert_eq!(watched_project["sourceGlobs"][1], "**/*.sysml");

        let checkout = temp_server_data("watched_checkout");
        let project_root = checkout.join("systems").join("brake");
        std::fs::create_dir_all(&project_root).unwrap();
        std::fs::write(
            project_root.join("model.sysml"),
            "package BrakeSystem { part def Brake; }\n",
        )
        .unwrap();
        let build_artifact_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/projects/{watched_project_id}/artifacts/build"
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "checkoutPath": checkout.display().to_string(),
                            "commit": "abc123"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(build_artifact_response.status(), StatusCode::OK);
        let body = build_artifact_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let artifact: Value = serde_json::from_slice(&body).unwrap();
        let artifact_id = artifact["manifest"]["id"].as_str().unwrap();
        assert_eq!(artifact["manifest"]["watchedProjectId"], watched_project_id);
        assert_eq!(artifact["manifest"]["commit"], "abc123");
        assert_eq!(artifact["manifest"]["ok"], true);
        assert_eq!(artifact["manifest"]["fileCount"], 1);
        assert_eq!(artifact["semantic"]["success_count"], 1);
        assert!(artifact["model"]["user_element_count"].as_u64().unwrap() > 0);
        assert!(
            artifact["graph"]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| { node["id"] == "type.BrakeSystem.Brake" })
        );

        let get_artifact_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/artifacts/{artifact_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_artifact_response.status(), StatusCode::OK);

        let publish_model_package_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/model-packages/publish")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "name": "BrakeSystem",
                            "version": "1.0.0",
                            "projectId": watched_project_id,
                            "metadata": {
                                "displayName": "Brake System",
                                "groupId": "mercurio.tests",
                                "tags": ["brake", "model"]
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(publish_model_package_response.status(), StatusCode::OK);
        let body = publish_model_package_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let model_package_manifest: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            model_package_manifest["packageKind"],
            "mercurio.model.package"
        );
        assert_eq!(model_package_manifest["artifactName"], "BrakeSystem.kpar");
        assert_eq!(
            model_package_manifest["metadata"]["groupId"],
            "mercurio.tests"
        );
        assert_eq!(
            model_package_manifest["provenance"]["sourceRevisionId"],
            artifact_id
        );

        let get_artifact_model_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/artifacts/{artifact_id}/model"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_artifact_model_response.status(), StatusCode::OK);

        let get_artifact_graph_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/artifacts/{artifact_id}/graph"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_artifact_graph_response.status(), StatusCode::OK);
        let body = get_artifact_graph_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let graph: Value = serde_json::from_slice(&body).unwrap();
        assert!(
            graph["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| { node["id"] == "type.BrakeSystem.Brake" })
        );

        let get_artifact_element_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/artifacts/{artifact_id}/elements/type.BrakeSystem.Brake"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_artifact_element_response.status(), StatusCode::OK);

        let list_artifact_elements_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/artifacts/{artifact_id}/elements?q=Brake"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list_artifact_elements_response.status(), StatusCode::OK);
        let body = list_artifact_elements_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let element_list: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(element_list["artifactId"], artifact_id);
        assert!(
            element_list["elements"]
                .as_array()
                .unwrap()
                .iter()
                .any(|element| { element["id"].as_str().unwrap_or_default().contains("Brake") })
        );

        let list_artifacts_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/projects/{watched_project_id}/artifacts"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list_artifacts_response.status(), StatusCode::OK);
        let body = list_artifacts_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let artifact_list: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(artifact_list["watchedProjectId"], watched_project_id);
        assert_eq!(artifact_list["artifacts"][0]["id"], artifact_id);

        let latest_artifact_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/projects/{watched_project_id}/artifacts/latest"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(latest_artifact_response.status(), StatusCode::OK);
        let body = latest_artifact_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let latest_artifact: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(latest_artifact["manifest"]["id"], artifact_id);

        std::fs::write(
            project_root.join("model.sysml"),
            "package BrakeSystem { part def Brake; part def BrakePad; }\n",
        )
        .unwrap();
        let build_updated_artifact_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/projects/{watched_project_id}/artifacts/build"
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "checkoutPath": checkout.display().to_string(),
                            "commit": "def456"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(build_updated_artifact_response.status(), StatusCode::OK);
        let body = build_updated_artifact_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let updated_artifact: Value = serde_json::from_slice(&body).unwrap();
        let updated_artifact_id = updated_artifact["manifest"]["id"].as_str().unwrap();
        assert_ne!(artifact_id, updated_artifact_id);

        let diff_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/artifacts/diff")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "baseArtifactId": artifact_id,
                            "targetArtifactId": updated_artifact_id
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(diff_response.status(), StatusCode::OK);
        let body = diff_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let diff: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(diff["baseCommit"], "abc123");
        assert_eq!(diff["targetCommit"], "def456");
        assert!(
            diff["added"]
                .as_array()
                .unwrap()
                .iter()
                .any(|element| { element["id"] == "type.BrakeSystem.BrakePad" })
        );

        let clone_info_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/repositories/{repository_id}/clone-info"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(clone_info_response.status(), StatusCode::OK);
        let body = clone_info_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let clone_info: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            clone_info["url"],
            "https://example.com/acme/vehicle-models.git"
        );
        assert_eq!(clone_info["suggestedDirectoryName"], "vehicle-models");

        let list_projects_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/projects")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list_projects_response.status(), StatusCode::OK);
        let body = list_projects_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let list: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(list["projects"].as_array().unwrap().len(), 1);
        assert_eq!(
            list["projects"][0]["latestArtifactId"],
            updated_artifact["manifest"]["id"]
        );
        assert!(server_data.join("artifacts").join(artifact_id).exists());
        assert!(
            server_data
                .join("artifacts")
                .join(updated_artifact_id)
                .exists()
        );
        assert!(!server_data.join("projects").exists());

        let delete_project_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/projects/{watched_project_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(delete_project_response.status(), StatusCode::NO_CONTENT);
        let get_deleted_project_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/projects/{watched_project_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_deleted_project_response.status(), StatusCode::NOT_FOUND);
        assert!(!server_data.join("artifacts").join(artifact_id).exists());
        assert!(
            !server_data
                .join("artifacts")
                .join(updated_artifact_id)
                .exists()
        );

        std::fs::remove_dir_all(checkout).unwrap();
        std::fs::remove_dir_all(server_data).unwrap();
    }

    #[tokio::test]
    async fn watched_project_refresh_builds_artifact_from_git_cache() {
        let _env_guard = SERVER_DATA_ENV_LOCK.lock().await;
        let server_data = temp_server_data("watched_refresh");
        unsafe {
            std::env::set_var("MERCURIO_SERVER_DATA", &server_data);
        }
        let source_repo = temp_server_data("watched_source_repo");
        let project_root = source_repo.join("systems").join("brake");
        std::fs::create_dir_all(&project_root).unwrap();
        std::fs::write(
            project_root.join("model.sysml"),
            "package BrakeSystem { part def Brake; }\n",
        )
        .unwrap();
        run_test_git(&source_repo, &["init"]);
        run_test_git(
            &source_repo,
            &["config", "user.email", "test@mercurio.local"],
        );
        run_test_git(&source_repo, &["config", "user.name", "Mercurio Test"]);
        run_test_git(&source_repo, &["add", "."]);
        run_test_git(&source_repo, &["commit", "-m", "initial model"]);
        let first_commit = run_test_git_output(&source_repo, &["rev-parse", "HEAD"]);

        let app = build_router(sample_state());
        let create_repository_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/repositories")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "name": "Local Vehicle Models",
                            "url": source_repo.display().to_string()
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_repository_response.status(), StatusCode::OK);
        let body = create_repository_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let repository: Value = serde_json::from_slice(&body).unwrap();
        let repository_id = repository["id"].as_str().unwrap();

        let create_project_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/repositories/{repository_id}/projects"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "name": "Brake Controller",
                            "rootPath": "systems/brake"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_project_response.status(), StatusCode::OK);
        let body = create_project_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let watched_project: Value = serde_json::from_slice(&body).unwrap();
        let watched_project_id = watched_project["id"].as_str().unwrap();

        let refresh_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/projects/{watched_project_id}/refresh"))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "refName": "HEAD" }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(refresh_response.status(), StatusCode::OK);
        let body = refresh_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let artifact: Value = serde_json::from_slice(&body).unwrap();
        let artifact_id = artifact["manifest"]["id"].as_str().unwrap();
        assert_eq!(artifact["manifest"]["rootPath"], "systems/brake");
        assert_eq!(artifact["manifest"]["ok"], true);
        assert!(
            artifact["graph"]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| { node["id"] == "type.BrakeSystem.Brake" })
        );
        assert!(
            server_data
                .join("repo-cache")
                .join(format!("{repository_id}.git"))
                .exists()
        );
        assert!(server_data.join("artifacts").join(artifact_id).exists());
        assert!(
            !server_data.join("repo-snapshots").exists()
                || std::fs::read_dir(server_data.join("repo-snapshots"))
                    .unwrap()
                    .next()
                    .is_none()
        );

        let delta_compile_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/editor/projects/{watched_project_id}/delta/compile"
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "baseCommit": first_commit,
                            "projectPath": ".",
                            "stagedFiles": [
                                {
                                    "path": "model.sysml",
                                    "content": "package BrakeSystem { part def Brake; part def Pedal; }\n"
                                }
                            ]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(delta_compile_response.status(), StatusCode::OK);
        let body = delta_compile_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let delta_compile: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(delta_compile["project_path"], ".");
        let delta_file = delta_compile["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|result| result["path"] == "model.sysml")
            .expect("delta compile should return project-relative file paths");
        assert_eq!(delta_file["ok"], true);
        assert!(delta_file["element_count"].as_u64().unwrap() > 0);

        std::fs::write(
            project_root.join("model.sysml"),
            "package BrakeSystem { part def Brake; part def BrakePad; }\n",
        )
        .unwrap();
        run_test_git(&source_repo, &["add", "."]);
        run_test_git(&source_repo, &["commit", "-m", "add brake pad"]);
        let second_commit = run_test_git_output(&source_repo, &["rev-parse", "HEAD"]);

        let second_refresh_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/projects/{watched_project_id}/refresh"))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "refName": "HEAD" }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second_refresh_response.status(), StatusCode::OK);

        let dag_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/projects/{watched_project_id}/commit-dag?refName=HEAD&maxCommits=10"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(dag_response.status(), StatusCode::OK);
        let body = dag_response.into_body().collect().await.unwrap().to_bytes();
        let dag: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(dag["projectId"], watched_project_id);
        assert_eq!(dag["nodes"].as_array().unwrap().len(), 2);
        assert!(dag["nodes"].as_array().unwrap().iter().any(|node| {
            node["commit"] == first_commit && node["semanticArtifact"]["commit"] == first_commit
        }));
        assert!(dag["nodes"].as_array().unwrap().iter().any(|node| {
            node["commit"] == second_commit && node["semanticArtifact"]["commit"] == second_commit
        }));
        let edge = dag["edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|edge| edge["parent"] == first_commit && edge["child"] == second_commit)
            .expect("commit DAG should contain semantic edge");
        assert_eq!(edge["semanticDiff"]["baseCommit"], first_commit);
        assert_eq!(edge["semanticDiff"]["targetCommit"], second_commit);
        assert!(edge["semanticDiff"]["addedCount"].as_u64().unwrap() > 0);

        std::fs::remove_dir_all(source_repo).unwrap();
        std::fs::remove_dir_all(server_data).unwrap();
    }

    #[test]
    fn graph_dto_contains_expected_vehicle_node() {
        let state = sample_state();
        let graph = state.app_state().graph(GraphScope::L2);
        let node = graph
            .nodes
            .iter()
            .find(|node| node.id == "type.Vehicle")
            .unwrap();

        assert_eq!(node.label, "Vehicle");
        assert_eq!(node.kind, "SysML::Systems::PartDefinition");
        assert_eq!(node.layer, 2);
    }

    #[test]
    fn load_state_from_file_path() {
        let state = load_server_state(&repo_path("examples/vehicle_model.json")).unwrap();
        assert!(state.app_state().metadata().element_count > 10_000);
        assert!(state.app_state().metadata().library_element_count > 10_000);
        assert!(state.app_state().metadata().user_element_count > 0);
        assert!(state.app_state().element("Base::Anything").is_some());
        assert!(state.app_state().element("type.Vehicle").is_some());
    }

    #[tokio::test]
    async fn model_endpoint_returns_metadata() {
        let app = build_router(sample_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/model")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json["element_count"].as_u64().unwrap() > 10_000);
        assert!(json["edge_count"].as_u64().unwrap() > 50_000);
        assert_eq!(json["default_graph_scope"], "l2");
        assert_eq!(json["library_version"], "0.57.0-SNAPSHOT");
        assert!(
            json["graph_scopes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|scope| scope == "full")
        );
        assert!(
            json["layers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|layer| layer == 0)
        );
        assert!(
            json["layers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|layer| layer == 1)
        );
        assert!(
            json["layers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|layer| layer == 2)
        );
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let app = build_router(sample_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["service"], "mercurio-core");
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn version_endpoint_returns_api_version() {
        let app = build_router(sample_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/version")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["service"], "mercurio-core");
        assert_eq!(body["apiVersion"], 1);
    }

    #[tokio::test]
    async fn workspace_scoped_routes_keep_projects_isolated() {
        let first_path = temp_workspace_named_file("multi_workspace_a", "model.sysml");
        let second_path = temp_workspace_named_file("multi_workspace_b", "model.sysml");
        let first_root = first_path.parent().unwrap().to_path_buf();
        let second_root = second_path.parent().unwrap().to_path_buf();
        std::fs::write(&first_path, "package Alpha {\n  part def First;\n}\n").unwrap();
        std::fs::write(&second_path, "package Beta {\n  part def Second;\n}\n").unwrap();

        let app = build_router(sample_state());
        let first_id = open_test_workspace(app.clone(), &first_root).await;
        let second_id = open_test_workspace(app.clone(), &second_root).await;
        assert_ne!(first_id, second_id);

        let first_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/workspaces/{first_id}/semantic/project/compile"
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "project_path": ".",
                            "staged_files": []
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first_response.status(), StatusCode::OK);
        let first_json = response_json(first_response).await;
        assert_eq!(first_json["file_count"], 1);
        assert_eq!(first_json["results"][0]["path"], "model.sysml");

        let second_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/workspaces/{second_id}/semantic/project/compile"
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "project_path": ".",
                            "staged_files": []
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second_response.status(), StatusCode::OK);
        let second_json = response_json(second_response).await;
        assert_eq!(second_json["file_count"], 1);
        assert_eq!(second_json["results"][0]["path"], "model.sysml");

        std::fs::remove_dir_all(first_root).unwrap();
        std::fs::remove_dir_all(second_root).unwrap();
    }

    #[tokio::test]
    async fn workspace_scoped_staged_compile_does_not_write_file() {
        let model_path = temp_workspace_named_file("staged_workspace", "model.sysml");
        let workspace_root = model_path.parent().unwrap().to_path_buf();
        let original = "package Demo {\n}\n";
        let staged = "package Demo {\n  part def Vehicle;\n}\n";
        std::fs::write(&model_path, original).unwrap();

        let app = build_router(sample_state());
        let workspace_id = open_test_workspace(app.clone(), &workspace_root).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/workspaces/{workspace_id}/semantic/project/compile"
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "project_path": ".",
                            "staged_files": [
                                {
                                    "path": "model.sysml",
                                    "content": staged
                                }
                            ]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["file_count"], 1);
        assert_eq!(std::fs::read_to_string(&model_path).unwrap(), original);

        std::fs::remove_dir_all(workspace_root).unwrap();
    }

    #[tokio::test]
    async fn package_registry_publishes_lists_and_downloads_package() {
        let _env_guard = SERVER_DATA_ENV_LOCK.lock().await;
        let server_data = temp_server_data("package_publish");
        unsafe {
            std::env::set_var("MERCURIO_SERVER_DATA", &server_data);
        }
        let app = build_router(sample_state());
        let publish_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/packages/BrakeSystem/versions/1.2.0/publish")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "artifactName": "brake-system.kpar",
                            "artifactContent": "package bytes",
                            "dependencies": [
                                { "name": "SysML", "version": "2.0.0" }
                            ],
                            "provenance": {
                                "sourceKind": "local_git",
                                "sourceGitCommit": "abc123"
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(publish_response.status(), StatusCode::OK);
        let body = publish_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let manifest: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(manifest["name"], "BrakeSystem");
        assert_eq!(manifest["version"], "1.2.0");
        assert_eq!(manifest["byteCount"], 13);
        assert_eq!(manifest["provenance"]["sourceGitCommit"], "abc123");

        let list_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/packages")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list_response.status(), StatusCode::OK);
        let body = list_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let list_body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(list_body["packages"][0]["name"], "BrakeSystem");
        assert_eq!(list_body["packages"][0]["versions"][0], "1.2.0");

        let download_response = app
            .oneshot(
                Request::builder()
                    .uri("/api/packages/BrakeSystem/versions/1.2.0/download")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(download_response.status(), StatusCode::OK);
        let body = download_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        assert_eq!(&body[..], b"package bytes");

        unsafe {
            std::env::remove_var("MERCURIO_SERVER_DATA");
        }
        std::fs::remove_dir_all(server_data).unwrap();
    }

    #[tokio::test]
    #[ignore = "server-owned editable project API was removed; accepted source is Git-owned"]
    async fn server_projects_create_list_read_and_write_files() {
        let server_data = temp_server_data("server_project_files");
        unsafe {
            std::env::set_var("MERCURIO_SERVER_DATA", &server_data);
        }
        let app = build_router(sample_state());
        let create_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/projects")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "name": "Brake System",
                            "starterFileName": "model.sysml",
                            "starterFileContent": "package BrakeSystem {}\n"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(create_response.status(), StatusCode::OK);
        let body = create_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let created: Value = serde_json::from_slice(&body).unwrap();
        let project_id = created["id"].as_str().unwrap();
        let base_revision = created["revisionId"].as_str().unwrap();

        let list_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/projects")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list_response.status(), StatusCode::OK);
        let body = list_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let list: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(list["projects"][0]["id"], project_id);

        let update_project_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/projects/{project_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "name": "Brake System Edited",
                            "description": "Server managed test project"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(update_project_response.status(), StatusCode::OK);
        let body = update_project_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let updated_project: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(updated_project["name"], "Brake System Edited");
        assert_eq!(
            updated_project["description"],
            "Server managed test project"
        );

        let files_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/projects/{project_id}/files"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(files_response.status(), StatusCode::OK);
        let body = files_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let files: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(files["files"][0]["path"], "model.sysml");

        let write_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{project_id}/files/model.sysml"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "baseRevisionId": base_revision,
                            "content": "package BrakeSystem { part def Brake; }\n"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(write_response.status(), StatusCode::OK);
        let body = write_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let written: Value = serde_json::from_slice(&body).unwrap();
        assert_ne!(written["revisionId"], base_revision);

        let stale_write_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{project_id}/files/model.sysml"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "baseRevisionId": base_revision,
                            "content": "package BrakeSystem {}\n"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stale_write_response.status(), StatusCode::CONFLICT);

        let read_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/projects/{project_id}/files/model.sysml"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(read_response.status(), StatusCode::OK);
        let body = read_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let read: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(read["content"], "package BrakeSystem { part def Brake; }\n");

        let create_file_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{project_id}/files/extra.sysml"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "content": "package Extra {}\n"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_file_response.status(), StatusCode::OK);

        let rename_file_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/projects/{project_id}/files/extra.sysml"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "newPath": "renamed.sysml"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rename_file_response.status(), StatusCode::OK);
        let body = rename_file_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let renamed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(renamed["path"], "renamed.sysml");

        let delete_file_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/projects/{project_id}/files/renamed.sysml"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(delete_file_response.status(), StatusCode::NO_CONTENT);

        let delete_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/projects/{project_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

        let get_deleted_response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/projects/{project_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_deleted_response.status(), StatusCode::NOT_FOUND);

        unsafe {
            std::env::remove_var("MERCURIO_SERVER_DATA");
        }
        std::fs::remove_dir_all(server_data).unwrap();
    }

    #[tokio::test]
    #[ignore = "server-owned editable project API was removed; accepted source is Git-owned"]
    async fn server_project_validation_reports_semantic_diagnostics() {
        let server_data = temp_server_data("server_project_validation");
        unsafe {
            std::env::set_var("MERCURIO_SERVER_DATA", &server_data);
        }
        let app = build_router(sample_state());
        let create_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/projects")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "name": "Validation Demo",
                            "starterFileName": "model.sysml",
                            "starterFileContent": "package ValidationDemo { part p : Missing; }\n"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(create_response.status(), StatusCode::OK);
        let body = create_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let created: Value = serde_json::from_slice(&body).unwrap();
        let project_id = created["id"].as_str().unwrap();
        let revision_id = created["revisionId"].as_str().unwrap();

        let validate_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/projects/{project_id}/validate"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(validate_response.status(), StatusCode::OK);
        let body = validate_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let validation: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(validation["projectId"], project_id);
        assert_eq!(validation["revisionId"], revision_id);
        assert_eq!(validation["ok"], false);
        assert_eq!(validation["semantic"]["ok"], false);
        assert!(
            validation["semantic"]["results"][0]["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| diagnostic["message"].as_str().unwrap().contains("Missing"))
        );

        unsafe {
            std::env::remove_var("MERCURIO_SERVER_DATA");
        }
        std::fs::remove_dir_all(server_data).unwrap();
    }

    #[tokio::test]
    #[ignore = "proposal commit endpoint was removed; proposals submit through PR bindings"]
    async fn v2_proposal_commit_writes_files_and_links_git_commit() {
        let server_data = temp_server_data("v2_proposal_commit");
        unsafe {
            std::env::set_var("MERCURIO_SERVER_DATA", &server_data);
        }
        let app = build_router(sample_state());
        let create_project_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/projects")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "name": "V2 Commit Project",
                            "starterFileName": "model.sysml",
                            "starterFileContent": "package V2CommitProject {}\n"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_project_response.status(), StatusCode::OK);
        let body = create_project_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let project: Value = serde_json::from_slice(&body).unwrap();
        let project_id = project["id"].as_str().unwrap();

        let create_item_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/proposals")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "key": "MER-123",
                            "title": "Add brake controller"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_item_response.status(), StatusCode::OK);

        let commit_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/proposals/MER-123")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "projectId": project_id,
                            "message": "Add brake controller",
                            "changedFiles": [
                                {
                                    "path": "model.sysml",
                                    "content": "package V2CommitProject { part def BrakeController; }\n"
                                }
                            ]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(commit_response.status(), StatusCode::OK);
        let body = commit_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let commit: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(commit["workItem"]["key"], "MER-123");
        assert_eq!(commit["semantic"]["ok"], true);
        assert!(
            commit["workItem"]["linkedCommits"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id.as_str() == commit["commit"]["id"].as_str())
        );

        let file_response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/projects/{project_id}/files/model.sysml"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(file_response.status(), StatusCode::OK);
        let body = file_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let file: Value = serde_json::from_slice(&body).unwrap();
        assert!(
            file["content"]
                .as_str()
                .unwrap()
                .contains("BrakeController")
        );

        unsafe {
            std::env::remove_var("MERCURIO_SERVER_DATA");
        }
        std::fs::remove_dir_all(server_data).unwrap();
    }

    #[tokio::test]
    async fn graph_endpoint_returns_flattened_graph() {
        let app = build_router(sample_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/graph")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json["nodes"].as_array().unwrap().len() < 20);
        assert!(
            json["edges"]
                .as_array()
                .unwrap()
                .iter()
                .any(|edge| edge["source"] == "type.Vehicle" && edge["target"] == "feature.engine")
        );
        assert!(
            json["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|node| node["layer"] == 2)
        );
    }

    #[tokio::test]
    async fn graph_endpoint_can_expand_to_l2_context() {
        let app = build_router(sample_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/graph?scope=l2_plus_context")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(
            json["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["id"] == "SysML::Systems::PartDefinition")
        );
    }

    #[tokio::test]
    async fn diagram_render_endpoint_returns_metamodel_specialization_view() {
        let app = build_router(sample_state());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/diagrams/render")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "spec": {
                                "version": 1,
                                "kind": "metamodel_specialization",
                                "title": "PartDefinition parents",
                                "root": "SysML::Systems::PartDefinition",
                                "query": {
                                    "direction": "parents",
                                    "depth": 2,
                                    "include_libraries": true,
                                    "include_user_model": false
                                }
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(
            json["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["id"] == "SysML::Systems::PartDefinition")
        );
        assert!(
            json["edges"]
                .as_array()
                .unwrap()
                .iter()
                .any(|edge| edge["relation"] == "specializes")
        );
    }

    #[tokio::test]
    async fn requirements_table_endpoint_returns_semantic_projection() {
        let app = build_router(
            load_server_state(&repo_path("examples/requirements_table_model.json")).unwrap(),
        );
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/views/requirements-table")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["title"], "Requirements");
        assert_eq!(json["rows"].as_array().unwrap().len(), 3);
        assert!(json["rows"].as_array().unwrap().iter().any(|row| {
            row["id"] == "req.VehicleSafety.BrakingDistance"
                && row["satisfiedBy"][0] == "part.VehicleSafety.BrakeController"
                && row["verifiedBy"][0] == "case.VehicleSafety.BrakingDistanceTest"
        }));
    }

    #[tokio::test]
    async fn library_tree_endpoint_returns_stdlib_hierarchy() {
        let app = build_router(sample_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/library/tree")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(
            json.as_array()
                .unwrap()
                .iter()
                .any(|node| node["label"] == "SysML")
        );
    }

    #[tokio::test]
    async fn mounted_library_trees_endpoint_loads_lazy_workspace_libraries() {
        let model_path = temp_workspace_named_file("mounted_library_trees_lazy", "model.sysml");
        let workspace_root = model_path.parent().unwrap().to_path_buf();
        std::fs::write(&model_path, "package Demo {\n}\n").unwrap();
        let app = build_router(
            crate::workspace::WorkspaceService::from_model_source_path_lazy(&model_path).unwrap(),
        );

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/library/mounted-trees")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_array().unwrap().iter().any(|tree| {
            tree["library_id"] == "stdlib"
                && tree["nodes"]
                    .as_array()
                    .map(|nodes| !nodes.is_empty())
                    .unwrap_or(false)
        }));

        std::fs::remove_dir_all(workspace_root).unwrap();
    }

    #[tokio::test]
    async fn element_endpoint_returns_relationships() {
        let app = build_router(sample_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/elements/type.Vehicle")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["kind"], "SysML::Systems::PartDefinition");
        assert_eq!(json["metatype"]["id"], "SysML::Systems::PartDefinition");
        assert!(
            json["specialization_chain"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["id"] == "SysML::Systems::PartDefinition")
        );
        assert!(
            json["outbound"]
                .as_array()
                .unwrap()
                .iter()
                .any(|edge| edge["relation"] == "features" && edge["target"] == "feature.engine")
        );
    }

    #[tokio::test]
    async fn element_endpoint_exposes_imported_documentation() {
        let app = build_router(sample_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/elements/Base::Anything")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["layer"], 0);
        assert!(json["metatype"].is_null());
        assert_eq!(json["direct_properties"]["doc"]["source"], "pilot");
        assert_eq!(json["effective_properties"]["doc"]["source"], "pilot");
    }

    #[tokio::test]
    async fn element_endpoint_merges_inherited_properties() {
        let app = build_router(sample_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/elements/type.Vehicle")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["direct_properties"]["features"][0], "feature.engine");
        assert!(
            json["inherited_properties"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["element"]["id"] == "SysML::Systems::PartDefinition")
        );
        assert!(
            json["effective_properties"]
                .as_object()
                .unwrap()
                .contains_key("features")
        );
        assert!(
            json["property_table"]["rows"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["name"] == "features"
                    && row["origin_kind"] == "direct"
                    && row["has_direct_value"] == true
                    && row["has_effective_value"] == true)
        );
        assert!(
            json["metatype_specialization_chain"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["id"] == "KerML::Root::Element")
        );
    }

    #[tokio::test]
    async fn search_endpoint_matches_by_kind_and_id() {
        let app = build_router(sample_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/search?q=engine")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let ids = json
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_str().unwrap())
            .collect::<Vec<_>>();

        assert!(ids.contains(&"type.Engine"));
        assert!(ids.contains(&"feature.engine"));
    }

    #[tokio::test]
    async fn editor_files_endpoint_lists_workspace_sysml_files() {
        let app = build_router(sample_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/editor/files")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(
            json["files"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["path"] == "minimal_vehicle.sysml")
        );
    }

    #[tokio::test]
    async fn editor_files_endpoint_lists_workspace_kerml_files() {
        let model_path = temp_workspace_named_file("kerml_files", "model.kerml");
        let workspace_root = model_path.parent().unwrap().to_path_buf();
        std::fs::write(&model_path, "package Demo {\n  classifier Vehicle;\n}\n").unwrap();
        let app = build_router(load_server_state(&model_path).unwrap());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/editor/files")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(
            json["files"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["path"] == "model.kerml")
        );

        std::fs::remove_dir_all(workspace_root).unwrap();
    }

    #[tokio::test]
    async fn semantic_workspace_session_endpoint_returns_semantic_outline_state() {
        let model_path = temp_workspace_named_file("semantic_session", "minimal_vehicle.sysml");
        let workspace_root = model_path.parent().unwrap().to_path_buf();
        std::fs::write(
            &model_path,
            "package Demo2 {\n  part def Engine;\n  part def Vehicle {\n    part engine: Engine;\n  }\n}\n",
        )
        .unwrap();
        let app = build_router(load_server_state(&model_path).unwrap());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/semantic/workspace-session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let files = json["files"].as_array().unwrap();
        let minimal_vehicle = files
            .iter()
            .find(|entry| entry["path"] == "minimal_vehicle.sysml")
            .unwrap();
        assert_eq!(json["project"]["descriptor_path"], Value::Null);
        assert!(
            json["project"]["libraries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|library| library["role"] == "baseline"
                    && library["source_kind"] == "bundled_stdlib")
        );
        assert_eq!(minimal_vehicle["semantic_ok"], true);
        assert!(
            minimal_vehicle["semantic_outline"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["element_id"] == "pkg.Demo2")
        );

        std::fs::remove_dir_all(workspace_root).unwrap();
    }

    #[tokio::test]
    async fn semantic_workspace_session_returns_partial_outline_for_semantic_errors() {
        let model_path = temp_workspace_named_file("partial_semantic_session", "partial.sysml");
        let workspace_root = model_path.parent().unwrap().to_path_buf();
        std::fs::write(
            &model_path,
            "package Demo {\n  part def Good;\n  part vehicle {\n    part good: Good;\n    part bad: Missing;\n  }\n}\n",
        )
        .unwrap();
        let app = build_router(load_server_state(&model_path).unwrap());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/semantic/workspace-session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let files = json["files"].as_array().unwrap();
        let partial = files
            .iter()
            .find(|entry| entry["path"] == "partial.sysml")
            .unwrap();

        assert_eq!(partial["semantic_ok"], false);
        assert_eq!(partial["semantic_status"], "partial");
        assert!(
            partial["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| diagnostic["message"]
                    .as_str()
                    .unwrap()
                    .contains("unresolved type `Missing`"))
        );
        assert!(
            partial["semantic_outline"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["element_id"] == "pkg.Demo")
        );

        std::fs::remove_dir_all(workspace_root).unwrap();
    }

    #[tokio::test]
    async fn semantic_workspace_session_returns_partial_outline_for_parse_errors() {
        let model_path = temp_workspace_named_file("partial_parse_session", "partial.sysml");
        let workspace_root = model_path.parent().unwrap().to_path_buf();
        std::fs::write(
            &model_path,
            "package Demo {\n  part def Good;\n  #servicedd :>> serviceDiscovery:ServiceDiscoveryDD {\n    #idd serviceDiscovery_HTTP;\n  }\n  part def AlsoGood;\n}\n",
        )
        .unwrap();
        let app = build_router(load_server_state(&model_path).unwrap());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/semantic/workspace-session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let files = json["files"].as_array().unwrap();
        let partial = files
            .iter()
            .find(|entry| entry["path"] == "partial.sysml")
            .unwrap();

        assert_eq!(partial["semantic_ok"], false);
        assert_eq!(partial["semantic_status"], "partial");
        assert!(
            partial["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|diagnostic| diagnostic["message"]
                    .as_str()
                    .unwrap()
                    .contains("unresolved type `ServiceDiscoveryDD`"))
        );
        let outline = partial["semantic_outline"].as_array().unwrap();
        assert!(outline.iter().any(|node| node["element_id"] == "pkg.Demo"));

        std::fs::remove_dir_all(workspace_root).unwrap();
    }

    #[tokio::test]
    async fn editor_outline_endpoint_returns_nested_source_tree() {
        let app = build_router(sample_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/editor/outline?path=minimal_vehicle.sysml")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let root_nodes = json.as_array().unwrap();
        assert_eq!(root_nodes[0]["label"], "Demo2");
        assert_eq!(root_nodes[0]["kind"], "package");
        assert!(
            root_nodes[0]["children"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["label"] == "Vehicle" && node["kind"] == "part def")
        );
        assert!(
            root_nodes[0]["children"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["label"] == "X" && node["kind"] == "package")
        );
    }

    #[tokio::test]
    async fn semantic_outline_endpoint_compiles_current_file_contents() {
        let model_path = temp_workspace_named_file("semantic_outline", "minimal_vehicle.sysml");
        let workspace_root = model_path.parent().unwrap().to_path_buf();
        std::fs::write(
            &model_path,
            "package Demo2 {\n  part def Engine;\n  part def Vehicle {\n    part engine: Engine;\n  }\n}\n",
        )
        .unwrap();
        let app = build_router(load_server_state(&model_path).unwrap());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/editor/semantic-outline?path=minimal_vehicle.sysml")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let root_nodes = json.as_array().unwrap();
        assert!(
            root_nodes
                .iter()
                .any(|node| node["element_id"] == "pkg.Demo2")
        );
        assert!(root_nodes.iter().any(|node| {
            node["children"]
                .as_array()
                .map(|children| {
                    children
                        .iter()
                        .any(|child| child["element_id"] == "type.Demo2.Vehicle")
                })
                .unwrap_or(false)
        }));

        std::fs::remove_dir_all(workspace_root).unwrap();
    }

    #[tokio::test]
    async fn kerml_editor_endpoints_parse_compile_outline_and_create_files() {
        let model_path = temp_workspace_named_file("kerml_editor", "model.kerml");
        let workspace_root = model_path.parent().unwrap().to_path_buf();
        std::fs::write(
            &model_path,
            "package Demo {\n  classifier Engine;\n  classifier Vehicle {\n    feature engine : Engine;\n  }\n}\n",
        )
        .unwrap();
        let app = build_router(load_server_state(&model_path).unwrap());

        let parse_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/editor/parse")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "path": "model.kerml",
                            "content": "package Demo {\n  classifier Engine;\n  classifier Vehicle {\n    feature engine : Engine;\n  }\n}\n"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(parse_response.status(), StatusCode::OK);
        let body = parse_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ok"], true);
        assert!(json["element_count"].as_u64().unwrap() > 0);

        let semantic_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/editor/semantic-compile")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "path": "model.kerml",
                            "content": "package Demo {\n  classifier Engine;\n  classifier Vehicle {\n    feature engine : Engine;\n  }\n}\n"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(semantic_response.status(), StatusCode::OK);
        let body = semantic_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ok"], true);
        assert!(
            json["semantic_outline"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["element_id"] == "pkg.Demo")
        );

        let outline_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/editor/outline?path=model.kerml")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(outline_response.status(), StatusCode::OK);
        let body = outline_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json[0]["label"], "Demo");
        assert!(
            json[0]["children"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["label"] == "Vehicle" && node["kind"] == "classifier def")
        );

        let create_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/editor/files")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "path": "created.kerml",
                            "template": "package Created {\n  classifier CreatedVehicle;\n}\n"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create_response.status(), StatusCode::OK);
        let body = create_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["path"], "created.kerml");
        assert!(
            json["content"]
                .as_str()
                .unwrap()
                .contains("classifier CreatedVehicle")
        );

        std::fs::remove_dir_all(workspace_root).unwrap();
    }

    #[tokio::test]
    async fn semantic_project_compile_endpoint_uses_staged_files_without_saving_them() {
        let model_path = temp_workspace_file("compile");
        let workspace_root = model_path.parent().unwrap().to_path_buf();
        std::fs::write(&model_path, "package Demo {\n}\n").unwrap();
        let app = build_router(load_server_state(&model_path).unwrap());

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/semantic/project/compile")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "project_path": ".",
                            "staged_files": [
                                {
                                    "path": "model.sysml",
                                    "content": "package Demo {\n  part def Vehicle;\n}\n"
                                }
                            ]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["file_count"], 1);
        assert_eq!(json["success_count"], 1);
        assert_eq!(json["results"][0]["path"], "model.sysml");
        assert_eq!(
            std::fs::read_to_string(&model_path).unwrap(),
            "package Demo {\n}\n"
        );

        std::fs::remove_dir_all(workspace_root).unwrap();
    }

    #[tokio::test]
    async fn saving_editor_file_updates_in_memory_project_semantics() {
        let model_path = temp_workspace_file("save_cache");
        let workspace_root = model_path.parent().unwrap().to_path_buf();
        std::fs::write(&model_path, "package Demo {\n}\n").unwrap();
        let app = build_router(load_server_state(&model_path).unwrap());

        let save_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/editor/file?path=model.sysml")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "content": "package Demo {\n  part def Vehicle;\n}\n"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(save_response.status(), StatusCode::NO_CONTENT);

        let element_response = app
            .oneshot(
                Request::builder()
                    .uri("/api/elements/type.Demo.Vehicle")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(element_response.status(), StatusCode::OK);
        let body = element_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["id"], "type.Demo.Vehicle");
        assert_eq!(json["kind"], "SysML::Systems::PartDefinition");

        std::fs::remove_dir_all(workspace_root).unwrap();
    }

    #[test]
    fn editor_outline_index_matches_source_file_suffixes() {
        let properties = BTreeMap::from([(
            "metadata".to_string(),
            Value::Object(serde_json::Map::from_iter([
                (
                    "source_file".to_string(),
                    Value::String("fixtures/l2/minimal_vehicle.sysml".to_string()),
                ),
                (
                    "source_span".to_string(),
                    Value::Object(serde_json::Map::from_iter([
                        ("start_line".to_string(), Value::from(11)),
                        ("start_col".to_string(), Value::from(3)),
                        ("end_line".to_string(), Value::from(13)),
                        ("end_col".to_string(), Value::from(3)),
                    ])),
                ),
            ])),
        )]);

        let elements = HashMap::from([(
            "type.Demo2.Vehicle".to_string(),
            ElementDetailsDto {
                id: "type.Demo2.Vehicle".to_string(),
                label: "Vehicle".to_string(),
                kind: "SysML::Systems::PartDefinition".to_string(),
                layer: 2,
                metatype: None,
                metatype_specialization_chain: Vec::new(),
                direct_properties: properties,
                inherited_properties: Vec::new(),
                effective_properties: BTreeMap::new(),
                property_table: ElementPropertyTableDto { rows: Vec::new() },
                specialization_chain: Vec::new(),
                inbound: Vec::new(),
                outbound: Vec::new(),
            },
        )]);

        let index = build_editor_outline_index(&elements);
        let key = EditorOutlineKey {
            source_file: "minimal_vehicle.sysml".to_string(),
            start_line: 11,
            start_column: 3,
            end_line: 13,
            end_column: 3,
        };

        assert_eq!(index.get(&key), Some(&"type.Demo2.Vehicle".to_string()));
    }

    #[test]
    fn semantic_outline_groups_elements_by_owner_and_source_span() {
        let package_properties = BTreeMap::from([
            (
                "declared_name".to_string(),
                Value::String("Demo".to_string()),
            ),
            (
                "metadata".to_string(),
                Value::Object(serde_json::Map::from_iter([
                    (
                        "source_file".to_string(),
                        Value::String("fixtures/l2/minimal_vehicle.sysml".to_string()),
                    ),
                    (
                        "source_span".to_string(),
                        Value::Object(serde_json::Map::from_iter([
                            ("start_line".to_string(), Value::from(1)),
                            ("start_col".to_string(), Value::from(1)),
                            ("end_line".to_string(), Value::from(5)),
                            ("end_col".to_string(), Value::from(1)),
                        ])),
                    ),
                ])),
            ),
        ]);
        let definition_properties = BTreeMap::from([
            (
                "declared_name".to_string(),
                Value::String("Vehicle".to_string()),
            ),
            (
                "metadata".to_string(),
                Value::Object(serde_json::Map::from_iter([
                    (
                        "source_file".to_string(),
                        Value::String("fixtures/l2/minimal_vehicle.sysml".to_string()),
                    ),
                    (
                        "source_span".to_string(),
                        Value::Object(serde_json::Map::from_iter([
                            ("start_line".to_string(), Value::from(2)),
                            ("start_col".to_string(), Value::from(3)),
                            ("end_line".to_string(), Value::from(4)),
                            ("end_col".to_string(), Value::from(3)),
                        ])),
                    ),
                ])),
            ),
        ]);
        let usage_properties = BTreeMap::from([
            (
                "declared_name".to_string(),
                Value::String("engine".to_string()),
            ),
            (
                "owner".to_string(),
                Value::String("type.Vehicle".to_string()),
            ),
            (
                "metadata".to_string(),
                Value::Object(serde_json::Map::from_iter([
                    (
                        "source_file".to_string(),
                        Value::String("fixtures/l2/minimal_vehicle.sysml".to_string()),
                    ),
                    (
                        "source_span".to_string(),
                        Value::Object(serde_json::Map::from_iter([
                            ("start_line".to_string(), Value::from(3)),
                            ("start_col".to_string(), Value::from(5)),
                            ("end_line".to_string(), Value::from(3)),
                            ("end_col".to_string(), Value::from(25)),
                        ])),
                    ),
                ])),
            ),
        ]);

        let elements = HashMap::from([
            (
                "pkg.Demo".to_string(),
                ElementDetailsDto {
                    id: "pkg.Demo".to_string(),
                    label: "Demo".to_string(),
                    kind: "SysML::Package".to_string(),
                    layer: 2,
                    metatype: None,
                    metatype_specialization_chain: Vec::new(),
                    direct_properties: package_properties,
                    inherited_properties: Vec::new(),
                    effective_properties: BTreeMap::new(),
                    property_table: ElementPropertyTableDto { rows: Vec::new() },
                    specialization_chain: Vec::new(),
                    inbound: Vec::new(),
                    outbound: Vec::new(),
                },
            ),
            (
                "type.Vehicle".to_string(),
                ElementDetailsDto {
                    id: "type.Vehicle".to_string(),
                    label: "Vehicle".to_string(),
                    kind: "SysML::Systems::PartDefinition".to_string(),
                    layer: 2,
                    metatype: None,
                    metatype_specialization_chain: Vec::new(),
                    direct_properties: definition_properties,
                    inherited_properties: Vec::new(),
                    effective_properties: BTreeMap::new(),
                    property_table: ElementPropertyTableDto { rows: Vec::new() },
                    specialization_chain: Vec::new(),
                    inbound: Vec::new(),
                    outbound: Vec::new(),
                },
            ),
            (
                "feature.engine".to_string(),
                ElementDetailsDto {
                    id: "feature.engine".to_string(),
                    label: "engine".to_string(),
                    kind: "KerML::Core::Feature".to_string(),
                    layer: 2,
                    metatype: None,
                    metatype_specialization_chain: Vec::new(),
                    direct_properties: usage_properties,
                    inherited_properties: Vec::new(),
                    effective_properties: BTreeMap::new(),
                    property_table: ElementPropertyTableDto { rows: Vec::new() },
                    specialization_chain: Vec::new(),
                    inbound: Vec::new(),
                    outbound: Vec::new(),
                },
            ),
        ]);

        let outline = build_semantic_editor_outline("minimal_vehicle.sysml", &elements);
        assert_eq!(outline.len(), 1);
        assert_eq!(outline[0].element_id.as_deref(), Some("pkg.Demo"));
        assert_eq!(outline[0].children.len(), 1);
        assert_eq!(
            outline[0].children[0].element_id.as_deref(),
            Some("type.Vehicle")
        );
        assert_eq!(
            outline[0].children[0].children[0].element_id.as_deref(),
            Some("feature.engine")
        );
    }

    #[tokio::test]
    async fn parse_endpoint_returns_marker_spans_for_invalid_source() {
        let app = build_router(sample_state());
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/editor/parse")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "path": "scratch.sysml",
                            "content": "package Demo { part def Vehicle { part engine: Engine } }"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ok"], false);
        assert_eq!(json["diagnostics"][0]["severity"], "error");
        assert!(
            json["diagnostics"][0]["start_line_number"]
                .as_u64()
                .unwrap()
                >= 1
        );
    }
}
