pub mod assessment;
pub mod authoring;
pub mod datalog;
pub mod derived;
pub mod diagrams;
pub mod feasibility;
pub mod frontend;
pub mod goal;
pub mod graph;
pub mod ir;
pub mod library;
pub mod logging;
pub mod metamodel;
pub mod mutation;
pub mod paths;
pub mod project;
pub mod proposal;
pub mod query;
pub mod runtime;
pub mod semantic_compare;
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
pub use query::{
    FilterExpr, OrderBy, Projection, Query, QueryEngine, QueryError, QueryResultSet, QuerySource,
    SortDirection, TermPattern, TriplePattern, parse_query,
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
