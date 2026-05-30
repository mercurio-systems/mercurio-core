use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::authoring::{
    AuthoringModule, AuthoringProject, Declaration, MutationResult, QualifiedName,
};
use crate::graph::Graph;
use crate::ir::{KirDocument, KirElement};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ElementRef {
    pub qualified_name: String,
}

impl ElementRef {
    pub fn new(qualified_name: impl Into<String>) -> Self {
        Self {
            qualified_name: qualified_name.into(),
        }
    }

    pub fn as_qualified_name(&self) -> QualifiedName {
        QualifiedName::parse(&self.qualified_name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRevision {
    pub fingerprint: String,
}

impl WorkspaceRevision {
    pub fn unchecked() -> Self {
        Self {
            fingerprint: "unchecked".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MutationProposal {
    pub intent: String,
    pub affected_elements: Vec<ElementRef>,
    pub operations: Vec<SemanticMutation>,
    pub evidence: Vec<MutationEvidence>,
    pub rationale: Option<String>,
    pub workspace_revision: WorkspaceRevision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationEvidence {
    pub element: Option<ElementRef>,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MutationPlan {
    pub proposal_id: String,
    pub normalized_operations: Vec<SemanticMutation>,
    pub required_supporting_changes: Vec<SemanticMutation>,
    pub checked_against: WorkspaceRevision,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SemanticMutation {
    AddPackage {
        target_file: String,
        name: String,
    },
    AddDefinition {
        container: ElementRef,
        keyword: String,
        name: String,
        specializes: Vec<ElementRef>,
    },
    AddUsage {
        container: ElementRef,
        keyword: String,
        name: String,
        ty: Option<ElementRef>,
        specializes: Vec<ElementRef>,
    },
    AddRelationship {
        kind: String,
        source: ElementRef,
        target: ElementRef,
    },
    AddMetadataAnnotation {
        element: ElementRef,
        metadata_type: String,
        properties: BTreeMap<String, String>,
    },
    RenameDeclaration {
        element: ElementRef,
        new_name: String,
    },
    UpdateUsageType {
        element: ElementRef,
        ty: Option<ElementRef>,
    },
    SetExpression {
        element: ElementRef,
        expression: Option<SemanticExpression>,
    },
    UpdateSpecializations {
        element: ElementRef,
        specializes: Vec<ElementRef>,
    },
    MoveDeclaration {
        element: ElementRef,
        destination: ElementRef,
    },
    SetAttribute {
        element: ElementRef,
        attribute: String,
        value: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticMutationCapabilityContext {
    pub metamodel_version: String,
    pub supported_operations: Vec<String>,
    pub definition_keywords: Vec<String>,
    pub usage_keywords: Vec<String>,
    pub relationship_kinds: Vec<String>,
    pub guidance: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticReasoningContext {
    pub metamodel_version: String,
    pub workspace_revision: WorkspaceRevision,
    pub focus: Vec<ElementRef>,
    pub elements: Vec<SemanticElementContext>,
    pub relationships: Vec<SemanticRelationshipContext>,
    pub facts: Vec<SemanticFactContext>,
    pub affordances: Vec<SemanticAffordanceContext>,
    pub source_files: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticElementContext {
    pub element: ElementRef,
    pub kind: String,
    pub label: String,
    pub owner: Option<ElementRef>,
    pub attributes: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticRelationshipContext {
    pub kind: String,
    pub source: ElementRef,
    pub target: ElementRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticFactContext {
    pub predicate: String,
    pub terms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticAffordanceContext {
    pub element: ElementRef,
    pub operation: String,
    pub child_kind: String,
    pub status: String,
    pub reason: Option<String>,
}

pub fn semantic_reasoning_context_from_authoring_project(
    project: &AuthoringProject,
    workspace_revision: WorkspaceRevision,
    focus: Vec<ElementRef>,
    max_elements: usize,
) -> SemanticReasoningContext {
    let mut elements = Vec::new();
    let mut relationships = Vec::new();
    let mut source_files = Vec::new();
    let mut truncated = false;

    for (path, module) in project.files() {
        source_files.push(path.to_string());
        collect_module_semantic_context(
            module,
            path,
            None,
            max_elements,
            &mut elements,
            &mut relationships,
            &mut truncated,
        );
    }

    SemanticReasoningContext {
        metamodel_version: "sysml-v2-authoring-context-v1".to_string(),
        workspace_revision,
        focus,
        elements,
        relationships,
        facts: Vec::new(),
        affordances: Vec::new(),
        source_files,
        truncated,
    }
}

pub fn enrich_semantic_reasoning_context_with_child_affordances(
    context: &mut SemanticReasoningContext,
    max_affordances: usize,
) {
    let capability_context = default_semantic_mutation_capability_context();
    let focus = context
        .focus
        .iter()
        .map(|element| element.qualified_name.clone())
        .collect::<BTreeSet<_>>();
    let focused_only = !focus.is_empty();
    let containers = context
        .elements
        .iter()
        .filter(|element| {
            (!focused_only || focus.contains(&element.element.qualified_name))
                && semantic_element_can_own_children(element)
        })
        .map(|element| element.element.clone())
        .collect::<Vec<_>>();

    for element in containers {
        push_child_affordance(
            context,
            max_affordances,
            SemanticAffordanceContext {
                element: element.clone(),
                operation: "AddPackage".to_string(),
                child_kind: "package".to_string(),
                status: "candidate".to_string(),
                reason: Some(
                    "container-like elements can conservatively own nested packages".to_string(),
                ),
            },
        );
        for keyword in &capability_context.definition_keywords {
            push_child_affordance(
                context,
                max_affordances,
                SemanticAffordanceContext {
                    element: element.clone(),
                    operation: "AddDefinition".to_string(),
                    child_kind: keyword.clone(),
                    status: "candidate".to_string(),
                    reason: Some(
                        "candidate from core SysML v2 writable definition vocabulary; feasibility remains authoritative"
                            .to_string(),
                    ),
                },
            );
        }
        for keyword in &capability_context.usage_keywords {
            push_child_affordance(
                context,
                max_affordances,
                SemanticAffordanceContext {
                    element: element.clone(),
                    operation: "AddUsage".to_string(),
                    child_kind: keyword.clone(),
                    status: "candidate".to_string(),
                    reason: Some(
                        "candidate from core SysML v2 writable usage vocabulary; feasibility remains authoritative"
                            .to_string(),
                    ),
                },
            );
        }
    }
}

fn push_child_affordance(
    context: &mut SemanticReasoningContext,
    max_affordances: usize,
    affordance: SemanticAffordanceContext,
) {
    if context.affordances.len() >= max_affordances {
        context.truncated = true;
        return;
    }
    context.affordances.push(affordance);
}

fn semantic_element_can_own_children(element: &SemanticElementContext) -> bool {
    let kind = element.kind.to_ascii_lowercase();
    if kind == "package" || kind == "definition" || kind == "usage" {
        return true;
    }
    let Some(kir_kind) = element
        .attributes
        .get("kirKind")
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase)
    else {
        return false;
    };
    kir_kind.contains("package")
        || kir_kind.contains("definition")
        || kir_kind.contains("usage")
        || kir_kind.contains("type")
        || kir_kind.contains("namespace")
}

pub fn enrich_semantic_reasoning_context_with_graph(
    context: &mut SemanticReasoningContext,
    graph: &Graph,
    max_elements: usize,
    max_facts: usize,
) {
    for element in graph.elements() {
        if context.elements.len() >= max_elements {
            context.truncated = true;
            break;
        }
        if context
            .elements
            .iter()
            .any(|item| item.element.qualified_name == element.element_id)
        {
            continue;
        }
        let mut attributes = element.properties.clone();
        attributes.insert("kirKind".to_string(), Value::String(element.kind.clone()));
        attributes.insert("kirLayer".to_string(), Value::from(element.layer));
        context.elements.push(SemanticElementContext {
            element: ElementRef::new(element.element_id.clone()),
            kind: "kirElement".to_string(),
            label: element
                .properties
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&element.element_id)
                .to_string(),
            owner: element
                .properties
                .get("owner")
                .and_then(Value::as_str)
                .map(ElementRef::new),
            attributes,
        });
    }

    for edge in graph.edges() {
        let Some(source) = graph.element_id(edge.source) else {
            continue;
        };
        let Some(target) = graph.element_id(edge.target) else {
            continue;
        };
        context.relationships.push(SemanticRelationshipContext {
            kind: format!("kir.{}", edge.relation),
            source: ElementRef::new(source),
            target: ElementRef::new(target),
        });
        if context.facts.len() < max_facts {
            context.facts.push(SemanticFactContext {
                predicate: edge.relation.clone(),
                terms: vec![source.to_string(), target.to_string()],
            });
        } else {
            context.truncated = true;
        }
    }
}

fn collect_module_semantic_context(
    module: &AuthoringModule,
    source_file: &str,
    owner: Option<String>,
    max_elements: usize,
    elements: &mut Vec<SemanticElementContext>,
    relationships: &mut Vec<SemanticRelationshipContext>,
    truncated: &mut bool,
) {
    if let Some(package) = &module.package {
        let package_name = package.name.as_dot_string();
        push_semantic_element(
            elements,
            max_elements,
            truncated,
            SemanticElementContext {
                element: ElementRef::new(package_name.clone()),
                kind: "package".to_string(),
                label: package_name.clone(),
                owner: owner.as_ref().map(ElementRef::new),
                attributes: context_attributes([
                    ("sourceFile", Value::String(source_file.to_string())),
                    ("memberCount", Value::from(package.members.len())),
                ]),
            },
        );
        for member in &package.members {
            collect_declaration_semantic_context(
                member,
                source_file,
                Some(package_name.clone()),
                max_elements,
                elements,
                relationships,
                truncated,
            );
        }
    }

    for member in &module.members {
        collect_declaration_semantic_context(
            member,
            source_file,
            owner.clone(),
            max_elements,
            elements,
            relationships,
            truncated,
        );
    }
}

fn collect_declaration_semantic_context(
    declaration: &Declaration,
    source_file: &str,
    owner: Option<String>,
    max_elements: usize,
    elements: &mut Vec<SemanticElementContext>,
    relationships: &mut Vec<SemanticRelationshipContext>,
    truncated: &mut bool,
) {
    match declaration {
        Declaration::Package(package) => {
            let qname = qualify_context_name(owner.as_deref(), &package.name.as_dot_string());
            push_semantic_element(
                elements,
                max_elements,
                truncated,
                SemanticElementContext {
                    element: ElementRef::new(qname.clone()),
                    kind: "package".to_string(),
                    label: package.name.as_dot_string(),
                    owner: owner.as_ref().map(ElementRef::new),
                    attributes: context_attributes([
                        ("sourceFile", Value::String(source_file.to_string())),
                        ("memberCount", Value::from(package.members.len())),
                    ]),
                },
            );
            for member in &package.members {
                collect_declaration_semantic_context(
                    member,
                    source_file,
                    Some(qname.clone()),
                    max_elements,
                    elements,
                    relationships,
                    truncated,
                );
            }
        }
        Declaration::Definition(definition) => {
            let qname = qualify_context_name(owner.as_deref(), &definition.name);
            let mut attributes = context_attributes([
                ("sourceFile", Value::String(source_file.to_string())),
                ("keyword", Value::String(definition.keyword.clone())),
                ("memberCount", Value::from(definition.members.len())),
            ]);
            insert_doc_context_attributes(&mut attributes, &definition.docs);
            if !definition.specializes.is_empty() {
                attributes.insert(
                    "specializes".to_string(),
                    Value::Array(
                        definition
                            .specializes
                            .iter()
                            .map(|item| Value::String(item.as_dot_string()))
                            .collect(),
                    ),
                );
            }
            push_semantic_element(
                elements,
                max_elements,
                truncated,
                SemanticElementContext {
                    element: ElementRef::new(qname.clone()),
                    kind: "definition".to_string(),
                    label: definition.name.clone(),
                    owner: owner.as_ref().map(ElementRef::new),
                    attributes,
                },
            );
            for target in &definition.specializes {
                relationships.push(SemanticRelationshipContext {
                    kind: "specializes".to_string(),
                    source: ElementRef::new(qname.clone()),
                    target: ElementRef::new(target.as_dot_string()),
                });
            }
            for member in &definition.members {
                collect_declaration_semantic_context(
                    member,
                    source_file,
                    Some(qname.clone()),
                    max_elements,
                    elements,
                    relationships,
                    truncated,
                );
            }
        }
        Declaration::Usage(usage) => {
            let qname = qualify_context_name(owner.as_deref(), &usage.name);
            let mut attributes = context_attributes([
                ("sourceFile", Value::String(source_file.to_string())),
                ("keyword", Value::String(usage.keyword.clone())),
                ("memberCount", Value::from(usage.members.len())),
            ]);
            insert_doc_context_attributes(&mut attributes, &usage.docs);
            if let Some(ty) = &usage.ty {
                attributes.insert("type".to_string(), Value::String(ty.as_dot_string()));
                relationships.push(SemanticRelationshipContext {
                    kind: "typedBy".to_string(),
                    source: ElementRef::new(qname.clone()),
                    target: ElementRef::new(ty.as_dot_string()),
                });
            }
            if let Some(target) = &usage.reference_target {
                attributes.insert(
                    "referenceTarget".to_string(),
                    Value::String(target.as_dot_string()),
                );
                relationships.push(SemanticRelationshipContext {
                    kind: usage.keyword.clone(),
                    source: ElementRef::new(qname.clone()),
                    target: ElementRef::new(target.as_dot_string()),
                });
                if semantic_trace_relationship_uses_owner_source(&usage.keyword) {
                    if let Some(owner) = &owner {
                        relationships.push(SemanticRelationshipContext {
                            kind: usage.keyword.clone(),
                            source: ElementRef::new(owner.clone()),
                            target: ElementRef::new(target.as_dot_string()),
                        });
                    }
                }
            } else if semantic_trace_relationship_uses_owner_source(&usage.keyword) {
                if let Some(owner) = &owner {
                    relationships.push(SemanticRelationshipContext {
                        kind: usage.keyword.clone(),
                        source: ElementRef::new(owner.clone()),
                        target: ElementRef::new(usage.name.clone()),
                    });
                }
            }
            if let Some(expression) = &usage.expression {
                attributes.insert("expression".to_string(), Value::String(expression.clone()));
            }
            push_semantic_element(
                elements,
                max_elements,
                truncated,
                SemanticElementContext {
                    element: ElementRef::new(qname.clone()),
                    kind: "usage".to_string(),
                    label: usage.name.clone(),
                    owner: owner.as_ref().map(ElementRef::new),
                    attributes,
                },
            );
            for member in &usage.members {
                collect_declaration_semantic_context(
                    member,
                    source_file,
                    Some(qname.clone()),
                    max_elements,
                    elements,
                    relationships,
                    truncated,
                );
            }
        }
        Declaration::Import(import) => {
            if let Some(owner) = &owner {
                relationships.push(SemanticRelationshipContext {
                    kind: "imports".to_string(),
                    source: ElementRef::new(owner.clone()),
                    target: ElementRef::new(import.path.as_dot_string()),
                });
            }
        }
        Declaration::Alias(alias) => {
            let qname = qualify_context_name(owner.as_deref(), &alias.name);
            push_semantic_element(
                elements,
                max_elements,
                truncated,
                SemanticElementContext {
                    element: ElementRef::new(qname.clone()),
                    kind: "alias".to_string(),
                    label: alias.name.clone(),
                    owner: owner.as_ref().map(ElementRef::new),
                    attributes: context_attributes([
                        ("sourceFile", Value::String(source_file.to_string())),
                        ("target", Value::String(alias.target.as_dot_string())),
                    ]),
                },
            );
            relationships.push(SemanticRelationshipContext {
                kind: "aliases".to_string(),
                source: ElementRef::new(qname),
                target: ElementRef::new(alias.target.as_dot_string()),
            });
        }
    }
}

fn push_semantic_element(
    elements: &mut Vec<SemanticElementContext>,
    max_elements: usize,
    truncated: &mut bool,
    element: SemanticElementContext,
) {
    if elements.len() >= max_elements {
        *truncated = true;
        return;
    }
    elements.push(element);
}

fn context_attributes(
    attributes: impl IntoIterator<Item = (&'static str, Value)>,
) -> BTreeMap<String, Value> {
    attributes
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn insert_doc_context_attributes(attributes: &mut BTreeMap<String, Value>, docs: &[String]) {
    if docs.is_empty() {
        return;
    }
    attributes.insert(
        "docs".to_string(),
        Value::Array(docs.iter().map(|doc| Value::String(doc.clone())).collect()),
    );
    if let Some(id) = requirement_id_from_docs(docs) {
        attributes.insert("id".to_string(), Value::String(id));
    }
    if let Some(text) = requirement_text_from_docs(docs) {
        attributes.insert("text".to_string(), Value::String(text));
    }
}

fn requirement_id_from_docs(docs: &[String]) -> Option<String> {
    docs.iter().find_map(|doc| {
        let trimmed = doc.trim();
        trimmed
            .strip_prefix("id:")
            .or_else(|| trimmed.strip_prefix("ID:"))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn requirement_text_from_docs(docs: &[String]) -> Option<String> {
    docs.iter()
        .find(|doc| !is_requirement_id_doc(doc) && !doc.trim().is_empty())
        .cloned()
}

fn is_requirement_id_doc(doc: &str) -> bool {
    let trimmed = doc.trim();
    trimmed.starts_with("id:") || trimmed.starts_with("ID:")
}

fn qualify_context_name(owner: Option<&str>, name: &str) -> String {
    if name.contains('.') || name.contains("::") {
        return QualifiedName::parse(name).as_dot_string();
    }
    owner
        .filter(|owner| !owner.is_empty())
        .map(|owner| format!("{owner}.{name}"))
        .unwrap_or_else(|| name.to_string())
}

fn semantic_trace_relationship_uses_owner_source(keyword: &str) -> bool {
    matches!(
        keyword.to_ascii_lowercase().as_str(),
        "satisfy" | "verify" | "refine"
    )
}

pub fn default_semantic_mutation_capability_context() -> SemanticMutationCapabilityContext {
    SemanticMutationCapabilityContext {
        metamodel_version: "sysml-v2-writable-mutation-v1".to_string(),
        supported_operations: vec![
            "AddPackage".to_string(),
            "AddDefinition".to_string(),
            "AddUsage".to_string(),
            "AddRelationship".to_string(),
            "AddMetadataAnnotation".to_string(),
            "RenameDeclaration".to_string(),
            "UpdateUsageType".to_string(),
            "SetExpression".to_string(),
            "UpdateSpecializations".to_string(),
            "MoveDeclaration".to_string(),
            "SetAttribute".to_string(),
        ],
        definition_keywords: vec![
            "part".to_string(),
            "attribute".to_string(),
            "requirement".to_string(),
            "item".to_string(),
            "connection".to_string(),
            "port".to_string(),
            "action".to_string(),
            "constraint".to_string(),
            "calc".to_string(),
            "state".to_string(),
            "view".to_string(),
            "verification".to_string(),
        ],
        usage_keywords: vec![
            "part".to_string(),
            "attribute".to_string(),
            "requirement".to_string(),
            "item".to_string(),
            "connection".to_string(),
            "port".to_string(),
            "action".to_string(),
            "constraint".to_string(),
            "calc".to_string(),
            "state".to_string(),
            "satisfy".to_string(),
            "verify".to_string(),
            "ref".to_string(),
            "reference".to_string(),
        ],
        relationship_kinds: vec![
            "satisfy".to_string(),
            "verify".to_string(),
            "trace".to_string(),
            "refine".to_string(),
        ],
        guidance: vec![
            "Use SysML v2 textual concepts, not SysML v1 block terminology.".to_string(),
            "Never use keyword `block`; use `part` for part definitions and part usages."
                .to_string(),
            "Requirement definitions should carry explicit `id` and `text` semantic attributes; use SetAttribute on existing requirement elements when those fields are missing."
                .to_string(),
            "Return semantic mutations, not source text edits.".to_string(),
            "Core feasibility remains authoritative for contextual legality.".to_string(),
        ],
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SemanticExpression {
    Text(String),
}

impl SemanticExpression {
    pub fn as_text(&self) -> &str {
        match self {
            Self::Text(value) => value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationApplicationResult {
    pub changed_files: BTreeSet<String>,
    pub edited_files: BTreeMap<String, String>,
    pub changed_declarations: BTreeSet<String>,
    pub semantic_diff: SemanticDiff,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SemanticDiff {
    pub added_elements: Vec<ElementRef>,
    pub removed_elements: Vec<ElementRef>,
    pub renamed_elements: Vec<RenamedElement>,
    pub moved_elements: Vec<MovedElement>,
    pub retyped_usages: Vec<RetypedUsage>,
    pub changed_specializations: Vec<ChangedSpecialization>,
    pub changed_attributes: Vec<ChangedAttribute>,
    pub added_relationships: Vec<RelationshipChange>,
    pub removed_relationships: Vec<RelationshipChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenamedElement {
    pub from: ElementRef,
    pub to: ElementRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovedElement {
    pub element: ElementRef,
    pub from: Option<ElementRef>,
    pub to: ElementRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetypedUsage {
    pub element: ElementRef,
    pub ty: Option<ElementRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedSpecialization {
    pub element: ElementRef,
    pub specializes: Vec<ElementRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedAttribute {
    pub element: ElementRef,
    pub attribute: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RelationshipChange {
    pub kind: String,
    pub source: ElementRef,
    pub target: ElementRef,
}

pub fn diff_kir_documents(before: &KirDocument, after: &KirDocument) -> SemanticDiff {
    let mut diff = SemanticDiff::default();
    let before_elements = before
        .elements
        .iter()
        .map(|element| (element.id.as_str(), element))
        .collect::<BTreeMap<_, _>>();
    let after_elements = after
        .elements
        .iter()
        .map(|element| (element.id.as_str(), element))
        .collect::<BTreeMap<_, _>>();

    for id in before_elements.keys() {
        if !after_elements.contains_key(id) {
            diff.removed_elements
                .push(ElementRef::new((*id).to_string()));
        }
    }
    for (id, after_element) in &after_elements {
        let Some(before_element) = before_elements.get(id) else {
            diff.added_elements.push(ElementRef::new((*id).to_string()));
            continue;
        };
        collect_element_property_diff(&mut diff, before_element, after_element);
    }

    collect_relationship_diff(before, after, &mut diff);
    diff
}

fn collect_element_property_diff(diff: &mut SemanticDiff, before: &KirElement, after: &KirElement) {
    if before.kind != after.kind {
        diff.changed_attributes.push(ChangedAttribute {
            element: ElementRef::new(after.id.clone()),
            attribute: "kind".to_string(),
        });
    }
    if before.layer != after.layer {
        diff.changed_attributes.push(ChangedAttribute {
            element: ElementRef::new(after.id.clone()),
            attribute: "layer".to_string(),
        });
    }

    let property_names = before
        .properties
        .keys()
        .chain(after.properties.keys())
        .collect::<BTreeSet<_>>();
    for name in property_names {
        if before.properties.get(name) != after.properties.get(name) {
            diff.changed_attributes.push(ChangedAttribute {
                element: ElementRef::new(after.id.clone()),
                attribute: name.clone(),
            });
        }
    }
}

fn collect_relationship_diff(before: &KirDocument, after: &KirDocument, diff: &mut SemanticDiff) {
    let before_relationships = document_relationships(before);
    let after_relationships = document_relationships(after);

    for relationship in before_relationships.difference(&after_relationships) {
        diff.removed_relationships.push(relationship.clone());
    }
    for relationship in after_relationships.difference(&before_relationships) {
        diff.added_relationships.push(relationship.clone());
    }
}

fn document_relationships(document: &KirDocument) -> BTreeSet<RelationshipChange> {
    let Ok(graph) = Graph::from_document(document.clone()) else {
        return BTreeSet::new();
    };
    graph
        .edges()
        .iter()
        .filter_map(|edge| {
            let source = graph.element_id(edge.source)?;
            let target = graph.element_id(edge.target)?;
            Some(RelationshipChange {
                kind: edge.relation.clone(),
                source: ElementRef::new(source.to_string()),
                target: ElementRef::new(target.to_string()),
            })
        })
        .collect()
}

pub(crate) fn diff_for_operation(
    operation: &SemanticMutation,
    result: Option<&MutationResult>,
) -> SemanticDiff {
    let mut diff = SemanticDiff::default();
    match operation {
        SemanticMutation::AddPackage { name, .. } => {
            diff.added_elements.push(ElementRef::new(name.clone()));
        }
        SemanticMutation::AddDefinition {
            container, name, ..
        }
        | SemanticMutation::AddUsage {
            container, name, ..
        } => {
            diff.added_elements.push(ElementRef::new(format!(
                "{}.{name}",
                container.qualified_name
            )));
        }
        SemanticMutation::AddRelationship {
            kind,
            source,
            target,
        } => diff.added_relationships.push(RelationshipChange {
            kind: kind.clone(),
            source: source.clone(),
            target: target.clone(),
        }),
        SemanticMutation::AddMetadataAnnotation { element, .. } => {
            diff.changed_attributes.push(ChangedAttribute {
                element: element.clone(),
                attribute: "metadata".to_string(),
            });
        }
        SemanticMutation::RenameDeclaration { element, new_name } => {
            let parent = element
                .qualified_name
                .rsplit_once('.')
                .map(|(parent, _)| parent.to_string());
            let to = parent
                .map(|parent| format!("{parent}.{new_name}"))
                .unwrap_or_else(|| new_name.clone());
            diff.renamed_elements.push(RenamedElement {
                from: element.clone(),
                to: ElementRef::new(to),
            });
        }
        SemanticMutation::UpdateUsageType { element, ty } => {
            diff.retyped_usages.push(RetypedUsage {
                element: element.clone(),
                ty: ty.clone(),
            });
        }
        SemanticMutation::SetExpression { element, .. } => {
            diff.changed_attributes.push(ChangedAttribute {
                element: element.clone(),
                attribute: "expression".to_string(),
            });
        }
        SemanticMutation::UpdateSpecializations {
            element,
            specializes,
        } => diff.changed_specializations.push(ChangedSpecialization {
            element: element.clone(),
            specializes: specializes.clone(),
        }),
        SemanticMutation::MoveDeclaration {
            element,
            destination,
        } => diff.moved_elements.push(MovedElement {
            element: element.clone(),
            from: None,
            to: destination.clone(),
        }),
        SemanticMutation::SetAttribute {
            element, attribute, ..
        } => diff.changed_attributes.push(ChangedAttribute {
            element: element.clone(),
            attribute: attribute.clone(),
        }),
    }

    if let Some(result) = result {
        for declaration in &result.changed_declarations {
            let element = ElementRef::new(declaration.clone());
            if !diff.added_elements.contains(&element)
                && !diff
                    .changed_attributes
                    .iter()
                    .any(|item| item.element == element)
                && !diff
                    .changed_specializations
                    .iter()
                    .any(|item| item.element == element)
                && !diff
                    .retyped_usages
                    .iter()
                    .any(|item| item.element == element)
            {
                diff.changed_attributes.push(ChangedAttribute {
                    element,
                    attribute: "declaration".to_string(),
                });
            }
        }
    }

    diff
}

pub(crate) fn merge_diff(target: &mut SemanticDiff, source: SemanticDiff) {
    target.added_elements.extend(source.added_elements);
    target.removed_elements.extend(source.removed_elements);
    target.renamed_elements.extend(source.renamed_elements);
    target.moved_elements.extend(source.moved_elements);
    target.retyped_usages.extend(source.retyped_usages);
    target
        .changed_specializations
        .extend(source.changed_specializations);
    target.changed_attributes.extend(source.changed_attributes);
    target
        .added_relationships
        .extend(source.added_relationships);
    target
        .removed_relationships
        .extend(source.removed_relationships);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        ElementRef, WorkspaceRevision, default_semantic_mutation_capability_context,
        diff_kir_documents, enrich_semantic_reasoning_context_with_child_affordances,
        enrich_semantic_reasoning_context_with_graph,
        semantic_reasoning_context_from_authoring_project,
    };
    use crate::authoring::AuthoringProject;
    use crate::graph::Graph;
    use crate::ir::{KirDocument, KirElement};

    #[test]
    fn default_capability_context_exposes_writable_sysml_v2_vocabulary() {
        let context = default_semantic_mutation_capability_context();

        assert_eq!(context.metamodel_version, "sysml-v2-writable-mutation-v1");
        assert!(
            context
                .supported_operations
                .contains(&"AddDefinition".to_string())
        );
        assert!(context.definition_keywords.contains(&"part".to_string()));
        assert!(!context.definition_keywords.contains(&"block".to_string()));
        assert!(context.relationship_kinds.contains(&"satisfy".to_string()));
        assert!(
            context
                .guidance
                .iter()
                .any(|item| item.contains("Never use keyword `block`"))
        );
    }

    #[test]
    fn semantic_diff_compares_kir_documents() {
        let before = KirDocument {
            metadata: BTreeMap::new(),
            elements: vec![
                KirElement {
                    id: "pkg.Demo".to_string(),
                    kind: "Package".to_string(),
                    layer: 2,
                    properties: BTreeMap::from([(
                        "members".to_string(),
                        serde_json::json!(["req.startup"]),
                    )]),
                },
                KirElement {
                    id: "req.startup".to_string(),
                    kind: "RequirementUsage".to_string(),
                    layer: 2,
                    properties: BTreeMap::new(),
                },
            ],
        };
        let after = KirDocument {
            metadata: BTreeMap::new(),
            elements: vec![
                KirElement {
                    id: "pkg.Demo".to_string(),
                    kind: "Package".to_string(),
                    layer: 2,
                    properties: BTreeMap::from([(
                        "members".to_string(),
                        serde_json::json!(["req.startup", "case.verifyStartup"]),
                    )]),
                },
                KirElement {
                    id: "req.startup".to_string(),
                    kind: "RequirementUsage".to_string(),
                    layer: 2,
                    properties: BTreeMap::from([(
                        "metadata".to_string(),
                        serde_json::json!([{"type": "ReviewTag"}]),
                    )]),
                },
                KirElement {
                    id: "case.verifyStartup".to_string(),
                    kind: "VerificationCaseUsage".to_string(),
                    layer: 2,
                    properties: BTreeMap::new(),
                },
            ],
        };

        let diff = diff_kir_documents(&before, &after);

        assert!(
            diff.added_elements
                .contains(&ElementRef::new("case.verifyStartup"))
        );
        assert!(diff.changed_attributes.iter().any(|change| {
            change.element == ElementRef::new("req.startup") && change.attribute == "metadata"
        }));
        assert!(diff.added_relationships.iter().any(|relationship| {
            relationship.kind == "members"
                && relationship.source == ElementRef::new("pkg.Demo")
                && relationship.target == ElementRef::new("case.verifyStartup")
        }));
    }

    #[test]
    fn semantic_reasoning_context_summarizes_authoring_project() {
        let files = BTreeMap::from([(
            "hybrid.sysml".to_string(),
            r#"
package HybridVehicle {
    part HybridVehicle {
        part battery : BatteryPack;
        attribute efficiency;
    }

    part BatteryPack;
}
"#
            .to_string(),
        )]);
        let project = AuthoringProject::from_sysml_files(files).expect("project parses");

        let context = semantic_reasoning_context_from_authoring_project(
            &project,
            WorkspaceRevision::unchecked(),
            vec![ElementRef::new("HybridVehicle.HybridVehicle")],
            64,
        );

        assert_eq!(context.metamodel_version, "sysml-v2-authoring-context-v1");
        assert_eq!(context.source_files, vec!["hybrid.sysml".to_string()]);
        assert!(!context.truncated);
        assert!(
            context
                .elements
                .iter()
                .any(|item| item.element.qualified_name == "HybridVehicle.HybridVehicle")
        );
        assert!(context.elements.iter().any(|item| {
            item.element.qualified_name == "HybridVehicle.HybridVehicle.battery"
                && item.attributes.get("type").and_then(|value| value.as_str())
                    == Some("BatteryPack")
        }));
        assert!(context.relationships.iter().any(|relationship| {
            relationship.kind == "typedBy"
                && relationship.source.qualified_name == "HybridVehicle.HybridVehicle.battery"
                && relationship.target.qualified_name == "BatteryPack"
        }));
    }

    #[test]
    fn semantic_reasoning_context_normalizes_trace_relationship_source_to_owner() {
        let files = BTreeMap::from([(
            "hybrid.sysml".to_string(),
            r#"
package HybridVehicle {
    part def Vehicle {
        action def RegenerativeBraking {
            satisfy requirement EfficiencyRequirement;
        }
    }

    requirement def EfficiencyRequirement;
}
"#
            .to_string(),
        )]);
        let project = AuthoringProject::from_sysml_files(files).expect("project parses");

        let context = semantic_reasoning_context_from_authoring_project(
            &project,
            WorkspaceRevision::unchecked(),
            Vec::new(),
            64,
        );

        assert!(context.relationships.iter().any(|relationship| {
            relationship.kind == "satisfy"
                && relationship.source.qualified_name == "HybridVehicle.Vehicle.RegenerativeBraking"
                && relationship
                    .target
                    .qualified_name
                    .ends_with("EfficiencyRequirement")
        }));
    }

    #[test]
    fn semantic_reasoning_context_exposes_focus_child_affordances() {
        let files = BTreeMap::from([(
            "hybrid.sysml".to_string(),
            r#"
package HybridVehicle {
    part HybridVehicle;
}
"#
            .to_string(),
        )]);
        let project = AuthoringProject::from_sysml_files(files).expect("project parses");
        let mut context = semantic_reasoning_context_from_authoring_project(
            &project,
            WorkspaceRevision::unchecked(),
            vec![ElementRef::new("HybridVehicle.HybridVehicle")],
            64,
        );

        enrich_semantic_reasoning_context_with_child_affordances(&mut context, 64);

        assert!(context.affordances.iter().any(|affordance| {
            affordance.element.qualified_name == "HybridVehicle.HybridVehicle"
                && affordance.operation == "AddUsage"
                && affordance.child_kind == "part"
                && affordance.status == "candidate"
        }));
        assert!(context.affordances.iter().all(|affordance| {
            affordance.element.qualified_name == "HybridVehicle.HybridVehicle"
        }));
    }

    #[test]
    fn semantic_reasoning_context_can_include_kir_graph_facts() {
        let mut context = semantic_reasoning_context_from_authoring_project(
            &AuthoringProject::default(),
            WorkspaceRevision::unchecked(),
            vec![ElementRef::new("type.Vehicle")],
            64,
        );
        let graph = Graph::from_document(KirDocument {
            metadata: BTreeMap::new(),
            elements: vec![
                KirElement {
                    id: "type.Vehicle".to_string(),
                    kind: "part_definition".to_string(),
                    layer: 2,
                    properties: BTreeMap::from([(
                        "owned_feature".to_string(),
                        serde_json::Value::String("feature.Vehicle.battery".to_string()),
                    )]),
                },
                KirElement {
                    id: "feature.Vehicle.battery".to_string(),
                    kind: "part_usage".to_string(),
                    layer: 2,
                    properties: BTreeMap::from([(
                        "owner".to_string(),
                        serde_json::Value::String("type.Vehicle".to_string()),
                    )]),
                },
            ],
        })
        .expect("graph builds");

        enrich_semantic_reasoning_context_with_graph(&mut context, &graph, 64, 64);

        assert!(
            context
                .elements
                .iter()
                .any(|item| item.element.qualified_name == "type.Vehicle"
                    && item.kind == "kirElement")
        );
        assert!(context.relationships.iter().any(|relationship| {
            relationship.kind == "kir.owned_feature"
                && relationship.source.qualified_name == "type.Vehicle"
                && relationship.target.qualified_name == "feature.Vehicle.battery"
        }));
        assert!(context.facts.iter().any(|fact| {
            fact.predicate == "owned_feature"
                && fact.terms
                    == vec![
                        "type.Vehicle".to_string(),
                        "feature.Vehicle.battery".to_string(),
                    ]
        }));
    }
}
