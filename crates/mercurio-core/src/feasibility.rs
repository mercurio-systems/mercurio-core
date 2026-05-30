use std::collections::BTreeSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use crate::authoring::{
    AttributeWritePolicy, AuthoringModule, AuthoringProject, ContainerSelector, Declaration,
    Mutation, QualifiedName, SemanticEdit,
};
use crate::mutation::{
    ElementRef, MutationApplicationResult, MutationPlan, MutationProposal, RelationshipChange,
    SemanticDiff, SemanticMutation, WorkspaceRevision, diff_for_operation, merge_diff,
};

#[derive(Debug, Clone, PartialEq)]
pub struct MutationContext {
    pub project: AuthoringProject,
    pub workspace_revision: WorkspaceRevision,
}

impl MutationContext {
    pub fn from_project(project: AuthoringProject) -> Self {
        let workspace_revision = workspace_revision_for_project(&project);
        Self {
            project,
            workspace_revision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MutationFeasibilityReport {
    pub status: FeasibilityStatus,
    pub normalized_plan: Option<MutationPlan>,
    pub blocking_reasons: Vec<FeasibilityIssue>,
    pub warnings: Vec<FeasibilityIssue>,
    pub required_choices: Vec<RequiredChoice>,
    pub suggested_supporting_changes: Vec<SemanticMutation>,
    pub resulting_diff: Option<SemanticDiff>,
    pub checked_against: WorkspaceRevision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeasibilityStatus {
    Allowed,
    AllowedWithWarnings,
    Blocked,
    RequiresDisambiguation,
    RequiresSupportingChanges,
    UnsupportedByAuthoringBackend,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeasibilityIssue {
    pub kind: FeasibilityIssueKind,
    pub operation_index: Option<usize>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeasibilityIssueKind {
    SemanticRuleViolation,
    MetamodelViolation,
    ResolutionFailure,
    NameCollision,
    RequiresImport,
    RequiresSupportingChange,
    UnsupportedByAuthoringBackend,
    StaleWorkspaceRevision,
    ValidationFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredChoice {
    pub operation_index: usize,
    pub message: String,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityAnswer {
    Allowed,
    Denied(String),
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributePolicyAnswer {
    pub writable: bool,
    pub reason: Option<String>,
}

pub trait SemanticCapabilityOracle {
    fn can_contain(&self, container_kind: &str, child_kind: &str) -> CapabilityAnswer;
    fn can_specialize(&self, source_kind: &str, target_kind: &str) -> CapabilityAnswer;
    fn can_type_usage(&self, usage_kind: &str, definition_kind: &str) -> CapabilityAnswer;
    fn can_relate(
        &self,
        relationship_kind: &str,
        source_kind: &str,
        target_kind: &str,
    ) -> CapabilityAnswer;
    fn attribute_policy(&self, kind: &str, attribute: &str) -> AttributePolicyAnswer;
}

#[derive(Debug, Clone, Default)]
pub struct ConservativeSemanticCapabilityOracle;

impl SemanticCapabilityOracle for ConservativeSemanticCapabilityOracle {
    fn can_contain(&self, container_kind: &str, child_kind: &str) -> CapabilityAnswer {
        if container_kind.is_empty() || child_kind.is_empty() {
            CapabilityAnswer::Unknown("missing kind information".to_string())
        } else if !is_container_kind(container_kind) {
            CapabilityAnswer::Denied(format!(
                "`{container_kind}` cannot own `{child_kind}` declarations"
            ))
        } else if is_definition_keyword(child_kind)
            || is_usage_keyword(child_kind)
            || child_kind == "package"
        {
            CapabilityAnswer::Allowed
        } else {
            CapabilityAnswer::Unknown(format!("unknown child kind `{child_kind}`"))
        }
    }

    fn can_specialize(&self, source_kind: &str, target_kind: &str) -> CapabilityAnswer {
        if source_kind.is_empty() || target_kind.is_empty() {
            CapabilityAnswer::Unknown("missing kind information".to_string())
        } else {
            CapabilityAnswer::Allowed
        }
    }

    fn can_type_usage(&self, usage_kind: &str, definition_kind: &str) -> CapabilityAnswer {
        if usage_kind.is_empty() || definition_kind.is_empty() {
            CapabilityAnswer::Unknown("missing kind information".to_string())
        } else if !is_usage_keyword(usage_kind) {
            CapabilityAnswer::Denied(format!("`{usage_kind}` is not a usage kind"))
        } else if !definition_kind.to_ascii_lowercase().contains("def") {
            CapabilityAnswer::Denied(format!("`{definition_kind}` is not a definition-like type"))
        } else if usage_kind == "part" && !definition_kind.to_ascii_lowercase().contains("part") {
            CapabilityAnswer::Denied(format!(
                "part usages should be typed by part definitions, got `{definition_kind}`"
            ))
        } else {
            CapabilityAnswer::Allowed
        }
    }

    fn can_relate(
        &self,
        relationship_kind: &str,
        source_kind: &str,
        target_kind: &str,
    ) -> CapabilityAnswer {
        let relation = relationship_kind.to_ascii_lowercase();
        if !is_container_kind(source_kind) {
            return CapabilityAnswer::Denied(format!(
                "relationship source `{source_kind}` is not element-like"
            ));
        }
        let target = target_kind.to_ascii_lowercase();
        if relation.contains("satisfy") && !target.contains("requirement") {
            return CapabilityAnswer::Denied(
                "satisfy relationships must target a requirement-like element".to_string(),
            );
        }
        if relation.contains("verify") && !target.contains("requirement") {
            return CapabilityAnswer::Denied(
                "verify relationships must target a requirement-like element".to_string(),
            );
        }
        if !(relation.contains("satisfy") || relation.contains("verify")) {
            return CapabilityAnswer::Unknown(format!(
                "relationship kind `{relationship_kind}` is not yet governed"
            ));
        }
        CapabilityAnswer::Allowed
    }

    fn attribute_policy(&self, kind: &str, attribute: &str) -> AttributePolicyAnswer {
        let attribute = attribute.to_ascii_lowercase();
        let writable = matches!(
            attribute.as_str(),
            "declared_name"
                | "specializes"
                | "type"
                | "is_abstract"
                | "is_end"
                | "direction"
                | "target"
                | "imports"
                | "expression"
                | "doc"
                | "text"
                | "id"
                | "requirement_id"
        );
        AttributePolicyAnswer {
            writable,
            reason: (!writable).then(|| {
                format!("attribute `{attribute}` is not writable on `{kind}` by this service")
            }),
        }
    }
}

pub trait MutationFeasibilityService {
    fn check(
        &self,
        context: &MutationContext,
        proposal: &MutationProposal,
    ) -> MutationFeasibilityReport;
}

#[derive(Debug, Clone)]
pub struct CoreMutationFeasibilityService<O = ConservativeSemanticCapabilityOracle> {
    oracle: O,
}

impl CoreMutationFeasibilityService<ConservativeSemanticCapabilityOracle> {
    pub fn new() -> Self {
        Self {
            oracle: ConservativeSemanticCapabilityOracle,
        }
    }
}

impl Default for CoreMutationFeasibilityService<ConservativeSemanticCapabilityOracle> {
    fn default() -> Self {
        Self::new()
    }
}

impl<O> CoreMutationFeasibilityService<O>
where
    O: SemanticCapabilityOracle,
{
    pub fn with_oracle(oracle: O) -> Self {
        Self { oracle }
    }

    pub fn apply_checked_plan(
        &self,
        context: &MutationContext,
        plan: &MutationPlan,
    ) -> Result<MutationApplicationResult, FeasibilityIssue> {
        if context.workspace_revision != plan.checked_against {
            return Err(FeasibilityIssue {
                kind: FeasibilityIssueKind::StaleWorkspaceRevision,
                operation_index: None,
                message: "workspace changed after feasibility was checked".to_string(),
            });
        }

        let mut project = context.project.clone();
        let mut changed_files = BTreeSet::new();
        let mut changed_declarations = BTreeSet::new();
        let mut semantic_diff = SemanticDiff::default();

        for (index, operation) in plan.normalized_operations.iter().enumerate() {
            let result = match operation {
                SemanticMutation::SetAttribute {
                    element,
                    attribute,
                    value,
                } => project.apply_semantic_edit(SemanticEdit::SetAttribute {
                    element: element.as_qualified_name(),
                    attribute: attribute.clone(),
                    value: value.clone(),
                    policy: AttributeWritePolicy::UpsertDirect,
                }),
                _ => {
                    let Some(mutation) = self.authoring_mutation_for(&project, operation) else {
                        return Err(FeasibilityIssue {
                            kind: FeasibilityIssueKind::UnsupportedByAuthoringBackend,
                            operation_index: Some(index),
                            message: "operation is semantically represented but not yet writable"
                                .to_string(),
                        });
                    };
                    project.apply_mutation(mutation)
                }
            }
            .map_err(|err| FeasibilityIssue {
                kind: FeasibilityIssueKind::ValidationFailure,
                operation_index: Some(index),
                message: err.to_string(),
            })?;
            changed_files.extend(result.changed_files.iter().cloned());
            changed_declarations.extend(result.changed_declarations.iter().cloned());
            merge_diff(
                &mut semantic_diff,
                diff_for_operation(operation, Some(&result)),
            );
        }

        let write_back = project
            .write_back_changed_files(&changed_files)
            .map_err(|err| FeasibilityIssue {
                kind: FeasibilityIssueKind::ValidationFailure,
                operation_index: None,
                message: err.to_string(),
            })?;

        Ok(MutationApplicationResult {
            changed_files,
            edited_files: write_back.edited_files,
            changed_declarations,
            semantic_diff,
        })
    }
}

impl<O> MutationFeasibilityService for CoreMutationFeasibilityService<O>
where
    O: SemanticCapabilityOracle,
{
    fn check(
        &self,
        context: &MutationContext,
        proposal: &MutationProposal,
    ) -> MutationFeasibilityReport {
        let mut blocking_reasons = Vec::new();
        let mut warnings = Vec::new();
        let required_choices = Vec::new();
        let mut suggested_supporting_changes = Vec::new();
        let mut resulting_diff = SemanticDiff::default();
        let mut changed_files = BTreeSet::new();

        if context.workspace_revision != proposal.workspace_revision {
            blocking_reasons.push(FeasibilityIssue {
                kind: FeasibilityIssueKind::StaleWorkspaceRevision,
                operation_index: None,
                message: "proposal was produced for a different workspace revision".to_string(),
            });
        }

        let mut project = context.project.clone();
        let mut unsupported_backend = false;
        let mut requires_supporting_changes = false;

        for (index, operation) in proposal.operations.iter().enumerate() {
            self.check_references(
                &project,
                operation,
                index,
                &mut blocking_reasons,
                &mut warnings,
                &mut suggested_supporting_changes,
                &mut requires_supporting_changes,
            );
            if operation_requires_supporting_change(&project, operation) {
                merge_diff(&mut resulting_diff, diff_for_operation(operation, None));
                continue;
            }

            let result = match operation {
                SemanticMutation::SetAttribute {
                    element,
                    attribute,
                    value,
                } => project.apply_semantic_edit(SemanticEdit::SetAttribute {
                    element: element.as_qualified_name(),
                    attribute: attribute.clone(),
                    value: value.clone(),
                    policy: AttributeWritePolicy::UpsertDirect,
                }),
                _ => {
                    let Some(mutation) = self.authoring_mutation_for(&project, operation) else {
                        unsupported_backend = true;
                        warnings.push(FeasibilityIssue {
                            kind: FeasibilityIssueKind::UnsupportedByAuthoringBackend,
                            operation_index: Some(index),
                            message: "operation is represented semantically but has no authoring write-back path yet".to_string(),
                        });
                        merge_diff(&mut resulting_diff, diff_for_operation(operation, None));
                        continue;
                    };
                    project.apply_mutation(mutation)
                }
            };

            match result {
                Ok(result) => {
                    changed_files.extend(result.changed_files.iter().cloned());
                    merge_diff(
                        &mut resulting_diff,
                        diff_for_operation(operation, Some(&result)),
                    );
                }
                Err(err) => {
                    blocking_reasons.push(FeasibilityIssue {
                        kind: FeasibilityIssueKind::ValidationFailure,
                        operation_index: Some(index),
                        message: err.to_string(),
                    });
                }
            }
        }

        if !changed_files.is_empty()
            && !unsupported_backend
            && !requires_supporting_changes
            && blocking_reasons.is_empty()
            && let Err(err) = project.write_back_changed_files(&changed_files)
        {
            blocking_reasons.push(FeasibilityIssue {
                kind: FeasibilityIssueKind::ValidationFailure,
                operation_index: None,
                message: err.to_string(),
            });
        }

        let status = if !blocking_reasons.is_empty() {
            FeasibilityStatus::Blocked
        } else if requires_supporting_changes {
            FeasibilityStatus::RequiresSupportingChanges
        } else if unsupported_backend {
            FeasibilityStatus::UnsupportedByAuthoringBackend
        } else if !warnings.is_empty() {
            FeasibilityStatus::AllowedWithWarnings
        } else {
            FeasibilityStatus::Allowed
        };

        let normalized_plan = if matches!(
            status,
            FeasibilityStatus::Allowed
                | FeasibilityStatus::AllowedWithWarnings
                | FeasibilityStatus::UnsupportedByAuthoringBackend
        ) {
            Some(MutationPlan {
                proposal_id: proposal_id(proposal),
                normalized_operations: proposal.operations.clone(),
                required_supporting_changes: suggested_supporting_changes.clone(),
                checked_against: context.workspace_revision.clone(),
            })
        } else {
            None
        };

        MutationFeasibilityReport {
            status,
            normalized_plan,
            blocking_reasons,
            warnings,
            required_choices,
            suggested_supporting_changes,
            resulting_diff: Some(resulting_diff),
            checked_against: context.workspace_revision.clone(),
        }
    }
}

impl<O> CoreMutationFeasibilityService<O>
where
    O: SemanticCapabilityOracle,
{
    fn authoring_mutation_for(
        &self,
        project: &AuthoringProject,
        operation: &SemanticMutation,
    ) -> Option<Mutation> {
        match operation {
            SemanticMutation::AddPackage { target_file, name } => Some(Mutation::AddPackage {
                target_file: target_file.clone(),
                package_name: QualifiedName::parse(name),
            }),
            SemanticMutation::AddDefinition {
                container,
                keyword,
                name,
                specializes,
            } => Some(Mutation::AddDefinition {
                container: container_selector_for(project, container),
                keyword: normalize_definition_keyword(keyword),
                name: name.clone(),
                specializes: specializes
                    .iter()
                    .map(ElementRef::as_qualified_name)
                    .collect(),
            }),
            SemanticMutation::AddUsage {
                container,
                keyword,
                name,
                ty,
                specializes,
            } => Some(Mutation::AddUsage {
                container: container_selector_for(project, container),
                keyword: keyword.clone(),
                name: name.clone(),
                ty: ty.as_ref().map(ElementRef::as_qualified_name),
                specializes: specializes
                    .iter()
                    .map(ElementRef::as_qualified_name)
                    .collect(),
            }),
            SemanticMutation::RenameDeclaration { element, new_name } => {
                Some(Mutation::RenameDeclaration {
                    qualified_name: element.as_qualified_name(),
                    new_name: new_name.clone(),
                })
            }
            SemanticMutation::UpdateUsageType { element, ty } => Some(Mutation::UpdateUsageType {
                qualified_name: element.as_qualified_name(),
                ty: ty.as_ref().map(ElementRef::as_qualified_name),
            }),
            SemanticMutation::SetExpression {
                element,
                expression,
            } => Some(Mutation::SetExpression {
                qualified_name: element.as_qualified_name(),
                expression: expression.as_ref().map(|expr| expr.as_text().to_string()),
            }),
            SemanticMutation::UpdateSpecializations {
                element,
                specializes,
            } => Some(Mutation::UpdateSpecializations {
                qualified_name: element.as_qualified_name(),
                specializes: specializes
                    .iter()
                    .map(ElementRef::as_qualified_name)
                    .collect(),
            }),
            SemanticMutation::MoveDeclaration {
                element,
                destination,
            } => Some(Mutation::MoveDeclaration {
                qualified_name: element.as_qualified_name(),
                destination: container_selector_for(project, destination),
            }),
            SemanticMutation::AddRelationship {
                kind,
                source,
                target,
            } => Some(Mutation::AddRelationship {
                container: container_selector_for(project, source),
                kind: kind.clone(),
                source: source.as_qualified_name(),
                target: target.as_qualified_name(),
            }),
            SemanticMutation::AddMetadataAnnotation {
                element,
                metadata_type,
                properties,
            } => Some(Mutation::AddMetadataAnnotation {
                element: element.as_qualified_name(),
                metadata_type: metadata_type.clone(),
                properties: properties.clone(),
            }),
            SemanticMutation::SetAttribute { .. } => None,
        }
    }

    fn check_references(
        &self,
        project: &AuthoringProject,
        operation: &SemanticMutation,
        index: usize,
        blocking_reasons: &mut Vec<FeasibilityIssue>,
        warnings: &mut Vec<FeasibilityIssue>,
        suggested_supporting_changes: &mut Vec<SemanticMutation>,
        requires_supporting_changes: &mut bool,
    ) {
        match operation {
            SemanticMutation::AddPackage { .. } => {}
            SemanticMutation::AddDefinition {
                container,
                keyword,
                name: _,
                specializes,
            } => {
                self.require_existing(project, container, index, "container", blocking_reasons);
                let container_kind = declaration_kind_label(project, container)
                    .unwrap_or_else(|| "container".to_string());
                self.warn_capability(
                    self.oracle.can_contain(&container_kind, keyword),
                    index,
                    "container capability",
                    warnings,
                    blocking_reasons,
                );
                for target in specializes {
                    self.require_existing(
                        project,
                        target,
                        index,
                        "specialization",
                        blocking_reasons,
                    );
                }
            }
            SemanticMutation::AddUsage {
                container,
                keyword,
                ty,
                specializes,
                ..
            } => {
                self.require_existing(project, container, index, "container", blocking_reasons);
                let container_kind = declaration_kind_label(project, container)
                    .unwrap_or_else(|| "container".to_string());
                self.warn_capability(
                    self.oracle.can_contain(&container_kind, keyword),
                    index,
                    "container capability",
                    warnings,
                    blocking_reasons,
                );
                if let Some(ty) = ty {
                    if !exists(project, ty) {
                        *requires_supporting_changes = true;
                        suggested_supporting_changes.push(SemanticMutation::AddDefinition {
                            container: parent_ref(ty).unwrap_or_else(|| container.clone()),
                            keyword: "part".to_string(),
                            name: ty
                                .qualified_name
                                .rsplit('.')
                                .next()
                                .unwrap_or(&ty.qualified_name)
                                .to_string(),
                            specializes: Vec::new(),
                        });
                    } else {
                        let definition_kind = declaration_kind_label(project, ty)
                            .unwrap_or_else(|| "definition".to_string());
                        self.warn_capability(
                            self.oracle.can_type_usage(keyword, &definition_kind),
                            index,
                            "typing capability",
                            warnings,
                            blocking_reasons,
                        );
                    }
                }
                for target in specializes {
                    self.require_existing(
                        project,
                        target,
                        index,
                        "specialization",
                        blocking_reasons,
                    );
                }
            }
            SemanticMutation::AddRelationship {
                kind,
                source,
                target,
            } => {
                self.require_existing(
                    project,
                    source,
                    index,
                    "relationship source",
                    blocking_reasons,
                );
                self.require_existing(
                    project,
                    target,
                    index,
                    "relationship target",
                    blocking_reasons,
                );
                let target_kind = declaration_kind_label(project, target)
                    .unwrap_or_else(|| target.qualified_name.clone());
                let source_kind = declaration_kind_label(project, source)
                    .unwrap_or_else(|| source.qualified_name.clone());
                self.warn_capability(
                    self.oracle.can_relate(kind, &source_kind, &target_kind),
                    index,
                    "relationship capability",
                    warnings,
                    blocking_reasons,
                );
            }
            SemanticMutation::AddMetadataAnnotation {
                element,
                metadata_type,
                ..
            } => {
                self.require_existing(project, element, index, "metadata target", blocking_reasons);
                if metadata_type.trim().is_empty() {
                    blocking_reasons.push(FeasibilityIssue {
                        kind: FeasibilityIssueKind::ValidationFailure,
                        operation_index: Some(index),
                        message: "metadata annotation type must not be empty".to_string(),
                    });
                }
            }
            SemanticMutation::SetExpression { element, .. } => {
                self.require_existing(project, element, index, "element", blocking_reasons);
                let kind = declaration_kind_label(project, element)
                    .unwrap_or_else(|| "element".to_string());
                let policy = self.oracle.attribute_policy(&kind, "expression");
                if !policy.writable {
                    blocking_reasons.push(FeasibilityIssue {
                        kind: FeasibilityIssueKind::MetamodelViolation,
                        operation_index: Some(index),
                        message: policy
                            .reason
                            .unwrap_or_else(|| "expression is not writable".to_string()),
                    });
                }
            }
            SemanticMutation::SetAttribute {
                element, attribute, ..
            } => {
                self.require_existing(project, element, index, "element", blocking_reasons);
                let kind = declaration_kind_label(project, element)
                    .unwrap_or_else(|| "element".to_string());
                let policy = self.oracle.attribute_policy(&kind, attribute);
                if !policy.writable {
                    blocking_reasons.push(FeasibilityIssue {
                        kind: FeasibilityIssueKind::MetamodelViolation,
                        operation_index: Some(index),
                        message: policy
                            .reason
                            .unwrap_or_else(|| format!("attribute `{attribute}` is not writable")),
                    });
                }
            }
            SemanticMutation::RenameDeclaration { element, .. }
            | SemanticMutation::UpdateUsageType { element, .. }
            | SemanticMutation::UpdateSpecializations { element, .. }
            | SemanticMutation::MoveDeclaration { element, .. } => {
                self.require_existing(project, element, index, "element", blocking_reasons);
            }
        }
    }

    fn require_existing(
        &self,
        project: &AuthoringProject,
        element: &ElementRef,
        index: usize,
        role: &str,
        blocking_reasons: &mut Vec<FeasibilityIssue>,
    ) {
        if !exists(project, element) {
            blocking_reasons.push(FeasibilityIssue {
                kind: FeasibilityIssueKind::ResolutionFailure,
                operation_index: Some(index),
                message: format!("missing {role}: {}", element.qualified_name),
            });
        }
    }

    fn warn_capability(
        &self,
        answer: CapabilityAnswer,
        index: usize,
        subject: &str,
        warnings: &mut Vec<FeasibilityIssue>,
        blocking_reasons: &mut Vec<FeasibilityIssue>,
    ) {
        match answer {
            CapabilityAnswer::Allowed => {}
            CapabilityAnswer::Denied(message) => blocking_reasons.push(FeasibilityIssue {
                kind: FeasibilityIssueKind::MetamodelViolation,
                operation_index: Some(index),
                message: format!("{subject}: {message}"),
            }),
            CapabilityAnswer::Unknown(message) => warnings.push(FeasibilityIssue {
                kind: FeasibilityIssueKind::MetamodelViolation,
                operation_index: Some(index),
                message: format!("{subject}: {message}"),
            }),
        }
    }
}

fn exists(project: &AuthoringProject, element: &ElementRef) -> bool {
    project
        .semantic_attributes(&QualifiedName::parse(&element.qualified_name))
        .is_ok()
}

fn container_selector_for(project: &AuthoringProject, element: &ElementRef) -> ContainerSelector {
    let qualified_name = element.as_qualified_name();
    if is_package(project, element) {
        ContainerSelector::Package { qualified_name }
    } else {
        ContainerSelector::Declaration { qualified_name }
    }
}

fn is_package(project: &AuthoringProject, element: &ElementRef) -> bool {
    project.files().any(|(_, module)| {
        module
            .package
            .as_ref()
            .is_some_and(|package| package.name.as_dot_string() == element.qualified_name)
    })
}

fn operation_requires_supporting_change(
    project: &AuthoringProject,
    operation: &SemanticMutation,
) -> bool {
    matches!(
        operation,
        SemanticMutation::AddUsage { ty: Some(ty), .. } if !exists(project, ty)
    )
}

fn parent_ref(element: &ElementRef) -> Option<ElementRef> {
    element
        .qualified_name
        .rsplit_once('.')
        .map(|(parent, _)| ElementRef::new(parent.to_string()))
}

fn declaration_kind_label(project: &AuthoringProject, element: &ElementRef) -> Option<String> {
    for (_, module) in project.files() {
        if module
            .package
            .as_ref()
            .is_some_and(|package| package.name.as_dot_string() == element.qualified_name)
        {
            return Some("package".to_string());
        }
        if let Some(kind) = declaration_kind_label_in_module(module, &element.qualified_name) {
            return Some(kind);
        }
    }
    None
}

fn declaration_kind_label_in_module(
    module: &AuthoringModule,
    qualified_name: &str,
) -> Option<String> {
    for member in &module.members {
        if let Some(kind) = declaration_kind_label_in_declaration(member, "", qualified_name) {
            return Some(kind);
        }
    }
    if let Some(package) = &module.package {
        let package_name = package.name.as_dot_string();
        for member in &package.members {
            if let Some(kind) =
                declaration_kind_label_in_declaration(member, &package_name, qualified_name)
            {
                return Some(kind);
            }
        }
    }
    None
}

fn declaration_kind_label_in_declaration(
    declaration: &Declaration,
    parent: &str,
    qualified_name: &str,
) -> Option<String> {
    match declaration {
        Declaration::Package(package) => {
            let current = package.name.as_dot_string();
            if current == qualified_name {
                return Some("package".to_string());
            }
            for member in &package.members {
                if let Some(kind) =
                    declaration_kind_label_in_declaration(member, &current, qualified_name)
                {
                    return Some(kind);
                }
            }
        }
        Declaration::Definition(definition) => {
            let current = join_ref(parent, &definition.name);
            if current == qualified_name {
                return Some(format!("{} def", definition.keyword));
            }
            for member in &definition.members {
                if let Some(kind) =
                    declaration_kind_label_in_declaration(member, &current, qualified_name)
                {
                    return Some(kind);
                }
            }
        }
        Declaration::Usage(usage) => {
            let current = join_ref(parent, &usage.name);
            if current == qualified_name {
                return Some(usage.keyword.clone());
            }
            for member in &usage.members {
                if let Some(kind) =
                    declaration_kind_label_in_declaration(member, &current, qualified_name)
                {
                    return Some(kind);
                }
            }
        }
        Declaration::Alias(alias) => {
            if join_ref(parent, &alias.name) == qualified_name {
                return Some("alias".to_string());
            }
        }
        Declaration::Import(_) => {}
    }
    None
}

fn join_ref(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}.{name}")
    }
}

fn is_container_kind(kind: &str) -> bool {
    let lower = kind.to_ascii_lowercase();
    lower == "package" || lower.contains("def") || lower.contains("usage") || lower == "part"
}

fn is_definition_keyword(kind: &str) -> bool {
    matches!(
        kind,
        "part"
            | "attribute"
            | "requirement"
            | "item"
            | "connection"
            | "port"
            | "action"
            | "constraint"
            | "calc"
            | "state"
            | "view"
            | "verification"
    ) || kind.ends_with(" def")
}

fn is_usage_keyword(kind: &str) -> bool {
    matches!(
        kind,
        "part"
            | "attribute"
            | "requirement"
            | "item"
            | "connection"
            | "port"
            | "action"
            | "constraint"
            | "calc"
            | "state"
            | "satisfy"
            | "verify"
            | "ref"
            | "reference"
    )
}

fn normalize_definition_keyword(keyword: &str) -> String {
    keyword
        .strip_suffix(" def")
        .unwrap_or(keyword)
        .trim()
        .to_string()
}

pub fn workspace_revision_for_project(project: &AuthoringProject) -> WorkspaceRevision {
    let mut hasher = DefaultHasher::new();
    for (path, _) in project.files() {
        path.hash(&mut hasher);
        if let Ok(rendered) = project.render_new_file(path) {
            rendered.hash(&mut hasher);
        }
    }
    WorkspaceRevision {
        fingerprint: format!("{:016x}", hasher.finish()),
    }
}

fn proposal_id(proposal: &MutationProposal) -> String {
    let mut hasher = DefaultHasher::new();
    proposal.intent.hash(&mut hasher);
    proposal.workspace_revision.fingerprint.hash(&mut hasher);
    for operation in &proposal.operations {
        format!("{operation:?}").hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

#[allow(dead_code)]
fn _relationship_change(kind: &str, source: ElementRef, target: ElementRef) -> RelationshipChange {
    RelationshipChange {
        kind: kind.to_string(),
        source,
        target,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use super::*;
    use crate::authoring::load_authoring_project_from_sysml;
    use crate::datalog::{load_default_rulepacks, materialize_core_indexes};
    use crate::frontend::sysml::compile_sysml_text;
    use crate::graph::Graph;
    use crate::ir::KirDocument;
    use crate::mutation::{MutationEvidence, MutationProposal, SemanticExpression};
    use crate::paths::default_stdlib_path;

    fn hybrid_vehicle_project() -> AuthoringProject {
        load_authoring_project_from_sysml(BTreeMap::from([(
            "hybrid.sysml".to_string(),
            r#"
package HybridVehicle {
    part def HybridVehicle {
        part engine : InternalCombustionEngine;
        part motor : ElectricMotor;
        part battery : BatteryPack;
        part transmission : Transmission;
        part controlSystem : EnergyManagementController;
    }

    part def InternalCombustionEngine {
        attribute efficiency : Real;
        attribute fuelConsumption : Real;
    }

    part def ElectricMotor {
        attribute efficiency : Real;
        attribute maxPower : Real;
    }

    part def BatteryPack {
        attribute capacity : Real;
        attribute mass : Real;
    }

    part def Transmission;

    part def EnergyManagementController {
        attribute strategy : String;
    }

    requirement def ImproveEfficiency {
        attribute targetEfficiencyGain : Real;
    }
}
"#
            .to_string(),
        )]))
        .unwrap()
    }

    #[test]
    fn hybrid_vehicle_efficiency_proposal_is_feasible_for_supported_operations() {
        let context = MutationContext::from_project(hybrid_vehicle_project());
        let proposal = MutationProposal {
            intent: "Improve hybrid vehicle efficiency through regenerative braking".to_string(),
            affected_elements: vec![ElementRef::new("HybridVehicle.HybridVehicle")],
            operations: vec![
                SemanticMutation::AddDefinition {
                    container: ElementRef::new("HybridVehicle"),
                    keyword: "part def".to_string(),
                    name: "RegenerativeBrakingSystem".to_string(),
                    specializes: Vec::new(),
                },
                SemanticMutation::AddUsage {
                    container: ElementRef::new("HybridVehicle.HybridVehicle"),
                    keyword: "part".to_string(),
                    name: "regenerativeBraking".to_string(),
                    ty: Some(ElementRef::new("HybridVehicle.RegenerativeBrakingSystem")),
                    specializes: Vec::new(),
                },
            ],
            evidence: vec![MutationEvidence {
                element: Some(ElementRef::new("HybridVehicle.BatteryPack")),
                summary: "Battery storage exists and can receive recovered energy.".to_string(),
            }],
            rationale: Some(
                "Recovering braking energy can improve vehicle efficiency.".to_string(),
            ),
            workspace_revision: context.workspace_revision.clone(),
        };

        let service = CoreMutationFeasibilityService::new();
        let report = service.check(&context, &proposal);

        assert_eq!(report.status, FeasibilityStatus::Allowed, "{report:#?}");
        assert!(report.blocking_reasons.is_empty());
        let diff = report.resulting_diff.unwrap();
        assert!(
            diff.added_elements
                .contains(&ElementRef::new("HybridVehicle.RegenerativeBrakingSystem"))
        );
        assert!(diff.added_elements.contains(&ElementRef::new(
            "HybridVehicle.HybridVehicle.regenerativeBraking"
        )));

        let application = service
            .apply_checked_plan(&context, &report.normalized_plan.unwrap())
            .unwrap();
        assert!(
            application
                .changed_declarations
                .contains("HybridVehicle.RegenerativeBrakingSystem")
        );
        assert!(
            application
                .changed_declarations
                .contains("HybridVehicle.HybridVehicle.regenerativeBraking")
        );
    }

    #[test]
    fn missing_usage_type_suggests_supporting_definition() {
        let context = MutationContext::from_project(hybrid_vehicle_project());
        let proposal = MutationProposal {
            intent: "Add regenerative braking usage before its definition exists".to_string(),
            affected_elements: vec![ElementRef::new("HybridVehicle.HybridVehicle")],
            operations: vec![SemanticMutation::AddUsage {
                container: ElementRef::new("HybridVehicle.HybridVehicle"),
                keyword: "part".to_string(),
                name: "regenerativeBraking".to_string(),
                ty: Some(ElementRef::new("HybridVehicle.RegenerativeBrakingSystem")),
                specializes: Vec::new(),
            }],
            evidence: Vec::new(),
            rationale: None,
            workspace_revision: context.workspace_revision.clone(),
        };

        let report = CoreMutationFeasibilityService::new().check(&context, &proposal);

        assert_eq!(
            report.status,
            FeasibilityStatus::RequiresSupportingChanges,
            "{report:#?}"
        );
        assert_eq!(report.suggested_supporting_changes.len(), 1);
        assert!(matches!(
            &report.suggested_supporting_changes[0],
            SemanticMutation::AddDefinition { name, .. } if name == "RegenerativeBrakingSystem"
        ));
    }

    #[test]
    fn relationship_candidate_is_semantically_checked_and_writable() {
        let context = MutationContext::from_project(hybrid_vehicle_project());
        let proposal = MutationProposal {
            intent: "Trace regenerative braking to efficiency requirement".to_string(),
            affected_elements: vec![ElementRef::new("HybridVehicle.HybridVehicle")],
            operations: vec![SemanticMutation::AddRelationship {
                kind: "satisfy".to_string(),
                source: ElementRef::new("HybridVehicle.HybridVehicle"),
                target: ElementRef::new("HybridVehicle.ImproveEfficiency"),
            }],
            evidence: Vec::new(),
            rationale: None,
            workspace_revision: context.workspace_revision.clone(),
        };

        let report = CoreMutationFeasibilityService::new().check(&context, &proposal);

        assert_eq!(report.status, FeasibilityStatus::Allowed, "{report:#?}");
        assert!(report.warnings.is_empty());
        assert!(
            report
                .resulting_diff
                .unwrap()
                .added_relationships
                .contains(&RelationshipChange {
                    kind: "satisfy".to_string(),
                    source: ElementRef::new("HybridVehicle.HybridVehicle"),
                    target: ElementRef::new("HybridVehicle.ImproveEfficiency"),
                })
        );

        let application = CoreMutationFeasibilityService::new()
            .apply_checked_plan(&context, &report.normalized_plan.unwrap())
            .unwrap();
        assert!(
            application
                .changed_declarations
                .contains("HybridVehicle.HybridVehicle.ImproveEfficiency")
        );
        let mut project = context.project.clone();
        let result = project
            .apply_mutation(crate::authoring::Mutation::AddRelationship {
                container: crate::authoring::ContainerSelector::Declaration {
                    qualified_name: QualifiedName::parse("HybridVehicle.HybridVehicle"),
                },
                kind: "satisfy".to_string(),
                source: QualifiedName::parse("HybridVehicle.HybridVehicle"),
                target: QualifiedName::parse("HybridVehicle.ImproveEfficiency"),
            })
            .unwrap();
        let edited = project.write_back_mutation(&result).unwrap();
        let source = edited.edited_files.get("hybrid.sysml").unwrap();
        let stdlib = KirDocument::from_path(Path::new(&default_stdlib_path())).unwrap();
        let graph =
            Graph::from_document(compile_sysml_text(source, "hybrid.sysml", &stdlib).unwrap())
                .unwrap();
        let rulepacks = load_default_rulepacks().unwrap();
        let indexes = materialize_core_indexes(&graph, &rulepacks).unwrap();
        assert!(
            indexes
                .satisfied_by
                .get("type.HybridVehicle.ImproveEfficiency")
                .is_some_and(|sources| sources.contains("type.HybridVehicle.HybridVehicle")),
            "satisfied_by={:#?}; elements={:#?}",
            indexes.satisfied_by,
            graph
                .elements()
                .iter()
                .filter(|element| element.element_id.contains("HybridVehicle")
                    || element.kind.contains("Satisfy"))
                .map(|element| (
                    element.element_id.clone(),
                    element.kind.clone(),
                    element.properties.clone()
                ))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn stale_workspace_revision_blocks_feasibility() {
        let context = MutationContext::from_project(hybrid_vehicle_project());
        let proposal = MutationProposal {
            intent: "Stale proposal".to_string(),
            affected_elements: Vec::new(),
            operations: vec![SemanticMutation::AddDefinition {
                container: ElementRef::new("HybridVehicle"),
                keyword: "part def".to_string(),
                name: "LowRollingResistanceTire".to_string(),
                specializes: Vec::new(),
            }],
            evidence: Vec::new(),
            rationale: None,
            workspace_revision: WorkspaceRevision {
                fingerprint: "stale".to_string(),
            },
        };

        let report = CoreMutationFeasibilityService::new().check(&context, &proposal);

        assert_eq!(report.status, FeasibilityStatus::Blocked);
        assert!(
            report
                .blocking_reasons
                .iter()
                .any(|issue| { issue.kind == FeasibilityIssueKind::StaleWorkspaceRevision })
        );
    }

    #[test]
    fn mutation_plan_can_generate_model_from_empty_project() {
        let context = MutationContext::from_project(AuthoringProject::default());
        let proposal = MutationProposal {
            intent: "Generate a minimal hybrid vehicle model".to_string(),
            affected_elements: Vec::new(),
            operations: vec![
                SemanticMutation::AddPackage {
                    target_file: "hybrid.sysml".to_string(),
                    name: "HybridVehicle".to_string(),
                },
                SemanticMutation::AddDefinition {
                    container: ElementRef::new("HybridVehicle"),
                    keyword: "part".to_string(),
                    name: "HybridVehicle".to_string(),
                    specializes: Vec::new(),
                },
                SemanticMutation::AddDefinition {
                    container: ElementRef::new("HybridVehicle"),
                    keyword: "part".to_string(),
                    name: "InternalCombustionEngine".to_string(),
                    specializes: Vec::new(),
                },
                SemanticMutation::AddDefinition {
                    container: ElementRef::new("HybridVehicle"),
                    keyword: "part".to_string(),
                    name: "ElectricMotor".to_string(),
                    specializes: Vec::new(),
                },
                SemanticMutation::AddDefinition {
                    container: ElementRef::new("HybridVehicle"),
                    keyword: "part".to_string(),
                    name: "BatteryPack".to_string(),
                    specializes: Vec::new(),
                },
                SemanticMutation::AddDefinition {
                    container: ElementRef::new("HybridVehicle"),
                    keyword: "requirement".to_string(),
                    name: "ImproveEfficiency".to_string(),
                    specializes: Vec::new(),
                },
                SemanticMutation::AddUsage {
                    container: ElementRef::new("HybridVehicle.HybridVehicle"),
                    keyword: "part".to_string(),
                    name: "engine".to_string(),
                    ty: Some(ElementRef::new("HybridVehicle.InternalCombustionEngine")),
                    specializes: Vec::new(),
                },
                SemanticMutation::AddUsage {
                    container: ElementRef::new("HybridVehicle.HybridVehicle"),
                    keyword: "part".to_string(),
                    name: "motor".to_string(),
                    ty: Some(ElementRef::new("HybridVehicle.ElectricMotor")),
                    specializes: Vec::new(),
                },
                SemanticMutation::AddUsage {
                    container: ElementRef::new("HybridVehicle.HybridVehicle"),
                    keyword: "part".to_string(),
                    name: "battery".to_string(),
                    ty: Some(ElementRef::new("HybridVehicle.BatteryPack")),
                    specializes: Vec::new(),
                },
                SemanticMutation::AddRelationship {
                    kind: "satisfy".to_string(),
                    source: ElementRef::new("HybridVehicle.HybridVehicle"),
                    target: ElementRef::new("HybridVehicle.ImproveEfficiency"),
                },
            ],
            evidence: Vec::new(),
            rationale: Some(
                "Create a semantic model from typed construction operations.".to_string(),
            ),
            workspace_revision: context.workspace_revision.clone(),
        };

        let service = CoreMutationFeasibilityService::new();
        let report = service.check(&context, &proposal);

        assert_eq!(report.status, FeasibilityStatus::Allowed, "{report:#?}");
        let application = service
            .apply_checked_plan(&context, &report.normalized_plan.unwrap())
            .unwrap();
        assert!(application.changed_files.contains("hybrid.sysml"));
        assert!(
            application
                .changed_declarations
                .contains("HybridVehicle.HybridVehicle.engine")
        );
        assert!(
            application
                .changed_declarations
                .contains("HybridVehicle.HybridVehicle.ImproveEfficiency")
        );
    }

    #[test]
    fn mutation_plan_can_set_expression_and_emit_expression_ir() {
        let project = load_authoring_project_from_sysml(BTreeMap::from([(
            "vehicle.sysml".to_string(),
            r#"
package Demo {
    part vehicle {
        attribute efficiency : Real;
    }
}
"#
            .to_string(),
        )]))
        .unwrap();
        let context = MutationContext::from_project(project);
        let proposal = MutationProposal {
            intent: "Set vehicle efficiency expression".to_string(),
            affected_elements: vec![ElementRef::new("Demo.vehicle.efficiency")],
            operations: vec![SemanticMutation::SetExpression {
                element: ElementRef::new("Demo.vehicle.efficiency"),
                expression: Some(SemanticExpression::Text("0.42".to_string())),
            }],
            evidence: Vec::new(),
            rationale: None,
            workspace_revision: context.workspace_revision.clone(),
        };

        let service = CoreMutationFeasibilityService::new();
        let report = service.check(&context, &proposal);
        assert_eq!(report.status, FeasibilityStatus::Allowed, "{report:#?}");
        assert!(
            report
                .resulting_diff
                .unwrap()
                .changed_attributes
                .iter()
                .any(
                    |change| change.element == ElementRef::new("Demo.vehicle.efficiency")
                        && change.attribute == "expression"
                )
        );

        let mut project = context.project.clone();
        let result = project
            .apply_mutation(crate::authoring::Mutation::SetExpression {
                qualified_name: QualifiedName::parse("Demo.vehicle.efficiency"),
                expression: Some("0.42".to_string()),
            })
            .unwrap();
        let edited = project.write_back_mutation(&result).unwrap();
        let source = edited.edited_files.get("vehicle.sysml").unwrap();
        assert!(source.contains("attribute efficiency: Real = 0.42;"));

        let stdlib = KirDocument::from_path(Path::new(&default_stdlib_path())).unwrap();
        let graph =
            Graph::from_document(compile_sysml_text(source, "vehicle.sysml", &stdlib).unwrap())
                .unwrap();
        let efficiency = graph
            .element_by_element_id("feature.Demo.vehicle.efficiency")
            .unwrap();
        assert!(efficiency.properties.contains_key("expression_ir"));
    }

    #[test]
    fn feasibility_blocks_satisfy_relationship_to_non_requirement() {
        let context = MutationContext::from_project(hybrid_vehicle_project());
        let proposal = MutationProposal {
            intent: "Invalid satisfy target".to_string(),
            affected_elements: vec![ElementRef::new("HybridVehicle.HybridVehicle")],
            operations: vec![SemanticMutation::AddRelationship {
                kind: "satisfy".to_string(),
                source: ElementRef::new("HybridVehicle.HybridVehicle"),
                target: ElementRef::new("HybridVehicle.BatteryPack"),
            }],
            evidence: Vec::new(),
            rationale: None,
            workspace_revision: context.workspace_revision.clone(),
        };

        let report = CoreMutationFeasibilityService::new().check(&context, &proposal);

        assert_eq!(report.status, FeasibilityStatus::Blocked);
        assert!(report.blocking_reasons.iter().any(|issue| {
            issue.kind == FeasibilityIssueKind::MetamodelViolation
                && issue.message.contains("must target a requirement")
        }));
    }

    #[test]
    fn feasibility_blocks_part_usage_typed_by_requirement_definition() {
        let context = MutationContext::from_project(hybrid_vehicle_project());
        let proposal = MutationProposal {
            intent: "Invalid part typing".to_string(),
            affected_elements: vec![ElementRef::new("HybridVehicle.HybridVehicle")],
            operations: vec![SemanticMutation::AddUsage {
                container: ElementRef::new("HybridVehicle.HybridVehicle"),
                keyword: "part".to_string(),
                name: "invalidRequirementPart".to_string(),
                ty: Some(ElementRef::new("HybridVehicle.ImproveEfficiency")),
                specializes: Vec::new(),
            }],
            evidence: Vec::new(),
            rationale: None,
            workspace_revision: context.workspace_revision.clone(),
        };

        let report = CoreMutationFeasibilityService::new().check(&context, &proposal);

        assert_eq!(report.status, FeasibilityStatus::Blocked);
        assert!(report.blocking_reasons.iter().any(|issue| {
            issue.kind == FeasibilityIssueKind::MetamodelViolation
                && issue
                    .message
                    .contains("part usages should be typed by part definitions")
        }));
    }

    #[test]
    fn feasibility_blocks_unwritable_semantic_attribute() {
        let context = MutationContext::from_project(hybrid_vehicle_project());
        let proposal = MutationProposal {
            intent: "Invalid attribute write".to_string(),
            affected_elements: vec![ElementRef::new("HybridVehicle.HybridVehicle")],
            operations: vec![SemanticMutation::SetAttribute {
                element: ElementRef::new("HybridVehicle.HybridVehicle"),
                attribute: "owner".to_string(),
                value: serde_json::json!("pkg.Other"),
            }],
            evidence: Vec::new(),
            rationale: None,
            workspace_revision: context.workspace_revision.clone(),
        };

        let report = CoreMutationFeasibilityService::new().check(&context, &proposal);

        assert_eq!(report.status, FeasibilityStatus::Blocked);
        assert!(report.blocking_reasons.iter().any(|issue| {
            issue.kind == FeasibilityIssueKind::MetamodelViolation
                && issue.message.contains("not writable")
        }));
    }

    #[test]
    fn feasibility_applies_requirement_id_and_text_attributes() {
        let context = MutationContext::from_project(hybrid_vehicle_project());
        let proposal = MutationProposal {
            intent: "Fill requirement metadata".to_string(),
            affected_elements: vec![ElementRef::new("HybridVehicle.ImproveEfficiency")],
            operations: vec![
                SemanticMutation::SetAttribute {
                    element: ElementRef::new("HybridVehicle.ImproveEfficiency"),
                    attribute: "id".to_string(),
                    value: serde_json::json!("REQ-EFF-001"),
                },
                SemanticMutation::SetAttribute {
                    element: ElementRef::new("HybridVehicle.ImproveEfficiency"),
                    attribute: "text".to_string(),
                    value: serde_json::json!(
                        "The hybrid vehicle shall improve efficiency through energy recovery."
                    ),
                },
            ],
            evidence: Vec::new(),
            rationale: None,
            workspace_revision: context.workspace_revision.clone(),
        };

        let service = CoreMutationFeasibilityService::new();
        let report = service.check(&context, &proposal);

        assert_eq!(report.status, FeasibilityStatus::Allowed);
        let application = service
            .apply_checked_plan(&context, &report.normalized_plan.unwrap())
            .unwrap();
        let source = application.edited_files.get("hybrid.sysml").unwrap();
        assert!(source.contains("doc /* id: REQ-EFF-001 */"));
        assert!(source.contains(
            "doc /* The hybrid vehicle shall improve efficiency through energy recovery. */"
        ));
    }
}
