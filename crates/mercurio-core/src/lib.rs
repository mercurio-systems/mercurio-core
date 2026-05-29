pub mod assessment;
pub mod authoring;
pub mod behavior;
pub mod constraints;
pub mod datalog;
pub mod derived;
pub mod expression;
pub mod feasibility;
pub mod frontend;
pub mod goal;
pub mod graph;
pub mod ir;
pub mod language;
pub mod library;
pub mod logging;
pub mod metadata;
pub mod metamodel;
pub mod mutation;
pub mod outline;
pub mod paths;
pub mod project;
pub mod project_cache;
pub mod proposal;
pub mod query;
pub mod runtime;
pub mod semantic_compare;
pub mod simulation;
pub mod source_set;
pub mod syntax_compare;
pub mod views;

pub use assessment::{
    AssessmentAssertion, AssessmentAssertionReport, AssessmentError, AssessmentExpectation,
    AssessmentQuery, AssessmentReport, AssessmentSpec, AssessmentStatus, RuntimeAssessmentRequest,
    RuntimeAssessmentResult, query_evaluation, run_evaluation_assessment, run_graph_assessment,
    run_runtime_assessment, sysml_module_assessment_facts,
};
pub use authoring::{
    Alias, AttributeWritePolicy, AuthoringError, AuthoringModule, AuthoringProject,
    ContainerSelector, Declaration, Definition, Import, Mutation, MutationResult, Package,
    QualifiedName, RenderedSpan, SemanticAttribute, SemanticEdit, Usage, ValidationReport,
    WriteBackMode, WriteBackResult, create_empty_model, load_authoring_project_from_kir,
    load_authoring_project_from_sysml,
};
pub use behavior::{
    StateMachineExecutionReport, StateMachineExecutionStatus, StateMachineModel,
    StateMachineScenario, StateMachineScenarioEvent, StateMachineTraceStep,
    StateMachineValidationFinding, StateMachineValidationSeverity, StateNode,
    StateTransitionTriggerKind, TransitionNode, project_state_machines,
    project_state_machines_from_graph,
};
pub use constraints::{
    ConstraintDiagnosticDto, ConstraintExplanationDto, ConstraintGraphEdgeDto,
    ConstraintGraphRequestDto, ConstraintGraphViewDto, ConstraintRecordDto,
    ConstraintSolveRequestDto, ConstraintSolveResultDto, ConstraintStatusDto,
    ConstraintVariableDto, ConstraintVariableStatusDto, RequirementCheckDto, RequirementStatusDto,
    execution_context_from_nested_values, render_constraint_graph, solve_constraints,
};
pub use datalog::{
    Atom, CORE_RULEPACK_ID, CORE_RULEPACK_VERSION, DatalogError, DerivedIndexes, Evaluation,
    Explanation, Fact, Rule, RulePack, Term, evaluate, extract_graph_facts, load_default_rulepacks,
    materialize_core_indexes,
};
pub use derived::{DerivedPropertySource, DerivedPropertyValue, derived_properties};
pub use expression::{
    BinaryExpressionOp, ExpressionEvaluationContext, ExpressionEvaluationError, ExpressionIr,
    ExpressionIrError, ExpressionPathRoot, ExpressionPathSegment, ExpressionValidationError,
    UnaryExpressionOp,
};
pub use feasibility::{
    AttributePolicyAnswer, CapabilityAnswer, ConservativeSemanticCapabilityOracle,
    CoreMutationFeasibilityService, FeasibilityIssue, FeasibilityIssueKind, FeasibilityStatus,
    MutationContext, MutationFeasibilityReport, MutationFeasibilityService, RequiredChoice,
    SemanticCapabilityOracle, workspace_revision_for_project,
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
pub use goal::{
    GoalCheckEvaluation, GoalEvaluation, GoalPolicy, SemanticGoalCheck, SemanticGoalExplanation,
    SemanticGoalProfile, SemanticGoalProfileKind, SemanticGoalSpec, default_model_quality_profile,
    evaluate_semantic_goal, explain_semantic_goal,
};
pub use graph::{Edge, Graph, GraphError, NodeId};
pub use ir::{
    KIR_SCHEMA_VERSION, KirDocument, KirElement, KirError, KirFieldKind, KirFieldRegistry,
    KirFieldSpec, load_model_stack,
};
pub use language::{
    CURRENT_DEFAULT_PROFILE_ID, LanguageProfile, LanguageProfileError, MetamodelConceptRegistry,
    SemanticConcept, SourceLanguage as ProfileSourceLanguage, default_language_profile,
    default_metamodel_registry, load_language_profile,
};
pub use library::{
    BaselineLibraryConfig, KparLocator, KparPackageBuild, KparPackageSource, LibraryCacheMetadata,
    LibraryProviderConfig, LocalPackageManifest, LocalPackageRepository, LocalPackageSource,
    PackageKirCache, PackageKirCacheManifest, ResolvedLibraryArtifact,
    load_baseline_library_document, write_kpar_package,
};
pub use metadata::{
    KirMetadataAnnotation, metadata_annotations, metadata_annotations_named,
    metadata_string_property,
};
pub use metamodel::{
    AttributeRow, AttributeValueSource, ElementAttributeQuery, ElementSummary,
    MetamodelAttributeRegistry, MetatypeQueryOverride, collect_specialization_ancestors,
    effective_properties, effective_properties_with_derived, element_metatype,
    query_element_attributes,
};
pub use mutation::{
    ChangedAttribute, ChangedSpecialization, ElementRef, MovedElement, MutationApplicationResult,
    MutationEvidence, MutationPlan, MutationProposal, RelationshipChange, RenamedElement,
    RetypedUsage, SemanticAffordanceContext, SemanticDiff, SemanticElementContext,
    SemanticExpression, SemanticFactContext, SemanticMutation, SemanticMutationCapabilityContext,
    SemanticReasoningContext, SemanticRelationshipContext, WorkspaceRevision,
    default_semantic_mutation_capability_context,
    enrich_semantic_reasoning_context_with_child_affordances,
    enrich_semantic_reasoning_context_with_graph,
    semantic_reasoning_context_from_authoring_project,
};
pub use outline::{
    EditorOutlineKey, EditorOutlineNodeDto, build_editor_outline,
    build_editor_outline_index_for_graph, build_semantic_editor_outline_from_document,
};
pub use paths::{
    bundled_package_repo_path, default_package_kir_cache_path, default_package_repo_path,
    default_stdlib_path, default_stdlib_rulepack_path, default_workspace_root, repo_path,
    repo_root,
};
pub use project::{
    PROJECT_DESCRIPTOR_FILE_NAME, ProjectDescriptor, ProjectDescriptorError, ProjectLibraryRole,
    ResolvedProjectContext, ResolvedProjectLibrary, discover_project_descriptor_path,
    resolve_project_context,
};
pub use project_cache::{
    PersistentCacheStatus, PersistentCompileResult, PersistentProjectCache,
    ProjectCompileArtifactKey, ProjectCompileCacheManifest, ProjectCompileCacheOutputs,
    ProjectSourceFileFingerprint, project_compile_artifact_key, source_file_fingerprints,
};
pub use proposal::{
    Proposal, ProposalStatus, PullRequestBinding, PullRequestState, SemanticImpact,
    SemanticImpactStatus, SemanticImpactSummary,
};
pub use query::{
    FilterExpr, OrderBy, Projection, Query, QueryEngine, QueryError, QueryResultSet, QuerySource,
    SortDirection, TermPattern, TriplePattern, parse_query,
};
pub use runtime::{ExecutionContext, QueryResult, Runtime, RuntimeArtifact, RuntimeError};
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
    ElementDetailsDto, ElementPropertyRowDto, ElementPropertyTableDto, ElementSummaryDto,
    ExplorerAttributeDto, GraphDto, GraphEdgeDto, GraphNodeDto, GraphScope, InheritedPropertiesDto,
    InheritedPropertyValueDto, L2ExplorerEdgeDto, L2ExplorerGraphDto, L2ExplorerNodeDto,
    L2ExplorerRequestDto, LibraryTreeNodeDto, MetatypeExplorerEdgeDto, MetatypeExplorerGraphDto,
    MetatypeExplorerNodeDto, MetatypeExplorerRequestDto, ModelMetadataDto, RequirementSourceDto,
    RequirementTableColumnDto, RequirementTableRowDto, RequirementTableViewDto, SearchResultDto,
    document_model_metadata_view, element_details, graph_view, l2_explorer_view, library_tree_view,
    library_tree_view_from_document, metatype_explorer_view, model_metadata_view,
    requirements_table_view, search_view,
};
