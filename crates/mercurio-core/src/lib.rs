pub mod ai;
pub mod assessment;
pub mod authoring;
pub mod datalog;
pub mod derived;
pub mod diagrams;
pub mod frontend;
pub mod graph;
pub mod ir;
pub mod library;
pub mod logging;
pub mod metamodel;
pub mod paths;
pub mod project;
pub mod proposal;
pub mod runtime;
pub mod semantic_compare;
pub mod source_set;
pub mod syntax_compare;
pub mod views;

pub use ai::{
    AskMercurioArtifact, AskMercurioCitation, AskMercurioProjectContext, AskMercurioRequest,
    AskMercurioResponse, AskMercurioTask, ChatCompletionRequest, ChatCompletionResponse,
    ChatMessageRole, ProposalDraft, ReasoningProvider, ReasoningProviderConfigOverrides,
    ReasoningProviderKind, ReasoningProviderSecretOverrides, ReasoningProviderStatus,
    ResolvedReasoningProvider, SemanticChangeItem, SemanticChangeKind, SemanticSummaryRequest,
    SemanticSummaryResponse, ask_mercurio, classify_ask_mercurio_task, complete_configured_chat,
    configured_reasoning_provider, default_reasoning_provider, default_reasoning_provider_status,
    default_reasoning_provider_status_with_secret_overrides,
    default_reasoning_provider_with_secret_overrides, summarize_semantic_changes,
    summarize_semantic_changes_with_secret_overrides,
    test_configured_reasoning_provider_connection, test_default_reasoning_provider_connection,
    test_default_reasoning_provider_connection_with_secret_overrides,
};
pub use assessment::{
    AssessmentAssertion, AssessmentAssertionReport, AssessmentError, AssessmentExpectation,
    AssessmentQuery, AssessmentReport, AssessmentSpec, AssessmentStatus, query_evaluation,
    run_evaluation_assessment, run_graph_assessment, sysml_module_assessment_facts,
};
pub use authoring::{
    Alias, AttributeWritePolicy, AuthoringError, AuthoringModule, AuthoringProject,
    ContainerSelector, Declaration, Definition, Import, Mutation, MutationResult, Package,
    QualifiedName, RenderedSpan, SemanticAttribute, SemanticEdit, Usage, ValidationReport,
    WriteBackMode, WriteBackResult, create_empty_model, load_authoring_project_from_kir,
    load_authoring_project_from_sysml,
};
pub use datalog::{
    Atom, CORE_RULEPACK_ID, CORE_RULEPACK_VERSION, DatalogError, DerivedIndexes, Evaluation,
    Explanation, Fact, Rule, RulePack, Term, evaluate, extract_graph_facts, load_default_rulepacks,
    materialize_core_indexes,
};
pub use derived::{DerivedPropertySource, DerivedPropertyValue, derived_properties};
pub use diagrams::{
    DiagramAttributeDto, DiagramDirectionDto, DiagramEdgeDto, DiagramKindDto,
    DiagramLayoutOptionsDto, DiagramNodeDto, DiagramQueryOptionsDto, DiagramRenderRequestDto,
    DiagramSpecDto, DiagramStyleOptionsDto, DiagramViewDto, list_diagram_kinds, render_diagram,
};
pub use frontend::format::{FormatError, format_path_text, format_sysml_text, format_text};
pub use frontend::kerml::{KermlError, compile_kerml_text, load_kerml_document, parse_kerml};
pub use frontend::lint::{
    LintDiagnostic, LintReport, LintSeverity, SourceLanguage, lint_kerml_text, lint_sysml_text,
    lint_text,
};
pub use frontend::pilot::{
    PilotDocumentationBlock, PilotExportDocument, PilotExportElement, PilotExportRelationship,
    PilotImportError, PilotSource, load_pilot_export, normalize_pilot_export,
    normalize_pilot_export_for_compare,
};
pub use frontend::sysml::{
    SemanticCompileReport, SemanticCompileStatus, SysmlError, compile_sysml_text,
    compile_sysml_text_with_context_report, load_sysml_document, parse_sysml,
};
pub use graph::{Edge, Graph, GraphError, NodeId};
pub use ir::{KirDocument, KirElement, KirError, load_model_stack};
pub use library::{
    BaselineLibraryConfig, KparPackageBuild, KparPackageSource, LibraryCacheMetadata,
    LibraryProviderConfig, ResolvedLibraryArtifact, load_baseline_library_document,
    write_kpar_package,
};
pub use metamodel::{
    AttributeRow, AttributeValueSource, ElementAttributeQuery, ElementSummary,
    MetamodelAttributeRegistry, MetatypeQueryOverride, collect_specialization_ancestors,
    effective_properties, effective_properties_with_derived, element_metatype,
    query_element_attributes,
};
pub use paths::{
    default_stdlib_path, default_stdlib_rulepack_path, default_workspace_root, repo_path, repo_root,
};
pub use project::{
    PROJECT_DESCRIPTOR_FILE_NAME, ProjectDescriptor, ProjectDescriptorError, ProjectLibraryRole,
    ResolvedProjectContext, ResolvedProjectLibrary, discover_project_descriptor_path,
    resolve_project_context,
};
pub use proposal::{
    Proposal, ProposalStatus, PullRequestBinding, PullRequestState, SemanticImpact,
    SemanticImpactStatus, SemanticImpactSummary,
};
pub use runtime::{ExecutionContext, QueryResult, Runtime, RuntimeError};
pub use semantic_compare::{
    SemanticCompareError, SemanticComparisonReport, SemanticElementMismatch, SemanticSnapshot,
    SemanticSnapshotAttribute, SemanticSnapshotElement, SemanticSourceSpan, SemanticValueMismatch,
    SnapshotMode, build_semantic_snapshot, build_semantic_snapshot_with_registry,
    compare_snapshots,
};
pub use syntax_compare::{
    SyntaxComparisonReport, SyntaxNodeMismatch, SyntaxSnapshot, SyntaxSnapshotNode,
    SyntaxSourceSpan, build_rust_syntax_snapshot, compare_syntax_snapshots,
};
pub use views::{
    RequirementSourceDto, RequirementTableColumnDto, RequirementTableRowDto,
    RequirementTableViewDto, requirements_table_view,
};
