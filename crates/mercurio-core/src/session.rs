use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::authoring::AuthoringProject;
use crate::feasibility::{
    CoreMutationFeasibilityService, FeasibilityIssue, FeasibilityIssueKind, FeasibilityStatus,
    MutationContext, MutationFeasibilityService,
};
use crate::frontend::sysml::compile_sysml_text;
use crate::identity::workspace_revision_for_kir_document;
use crate::ir::{KirDocument, KirElement, KirError};
use crate::mutation::{
    ElementRef, MutationEvidence, MutationProposal, SemanticDiff, SemanticMutation,
    WorkspaceRevision, diff_kir_documents,
};
use crate::paths::default_stdlib_path;

const GENERATED_FILE_THRESHOLD: usize = 100;

#[derive(Debug, Clone)]
pub struct WorkspaceSnapshot {
    pub revision: WorkspaceRevision,
    pub kir: Arc<KirDocument>,
    pub profile_id: Option<String>,
    source_project: Option<AuthoringProject>,
}

#[derive(Debug, Clone)]
pub struct ModelWorkspace {
    current: Arc<RwLock<Arc<WorkspaceSnapshot>>>,
}

#[derive(Debug, Clone)]
pub struct ModelSession {
    snapshot: Arc<WorkspaceSnapshot>,
    workspace: Option<ModelWorkspace>,
}

#[derive(Debug, Clone)]
pub struct ModelFork {
    label: Option<String>,
    base: Arc<WorkspaceSnapshot>,
    workspace: Option<ModelWorkspace>,
    overlay: KirOverlay,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct KirOverlay {
    pub added_elements: BTreeMap<String, KirElement>,
    pub updated_properties: BTreeMap<String, BTreeMap<String, Value>>,
    pub added_members: BTreeMap<String, Vec<String>>,
    pub removed_elements: BTreeSet<String>,
    operations: Vec<ForkOperation>,
}

#[derive(Debug, Clone, PartialEq)]
enum ForkOperation {
    AddPackage {
        id: String,
        qualified_name: String,
        owner: Option<String>,
        source_file: String,
    },
    AddRequirement {
        id: String,
        qualified_name: String,
        owner: String,
        source_file: String,
    },
    RenameDeclaration {
        element: ElementRef,
        new_name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitMode {
    PreserveSource,
    RewriteSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkElement {
    pub id: String,
    pub qualified_name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommitResult {
    pub mode: CommitMode,
    pub strategy_used: CommitStrategy,
    pub base_revision: WorkspaceRevision,
    pub new_revision: WorkspaceRevision,
    pub changed_files: BTreeSet<String>,
    pub edited_files: BTreeMap<String, String>,
    pub semantic_diff: SemanticDiff,
    pub generated_elements: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitStrategy {
    MutatorPlan,
    GeneratedCompanionFiles,
    RewriteGeneratedSource,
    NoOp,
}

#[derive(Debug)]
pub enum SessionError {
    StaleWorkspace {
        base_revision: WorkspaceRevision,
        current_revision: WorkspaceRevision,
    },
    DuplicateElement(String),
    MissingElement(String),
    InvalidInput(String),
    Unsupported(String),
    Kir(KirError),
    Authoring(crate::authoring::AuthoringError),
    Feasibility(FeasibilityIssue),
}

impl WorkspaceSnapshot {
    pub fn new(kir: KirDocument) -> Result<Self, SessionError> {
        Self::with_profile(kir, None)
    }

    pub fn with_profile(
        kir: KirDocument,
        profile_id: Option<String>,
    ) -> Result<Self, SessionError> {
        let revision = workspace_revision_for_kir_document(&kir)?;
        Ok(Self {
            revision,
            kir: Arc::new(kir),
            profile_id,
            source_project: None,
        })
    }

    pub fn from_authoring_project(project: AuthoringProject) -> Result<Self, SessionError> {
        let context = MutationContext::from_project(project.clone());
        let kir = project.compile_kir_document()?;
        Ok(Self {
            revision: context.workspace_revision,
            kir: Arc::new(kir),
            profile_id: None,
            source_project: Some(project),
        })
    }

    pub fn session(self: &Arc<Self>) -> ModelSession {
        ModelSession {
            snapshot: Arc::clone(self),
            workspace: None,
        }
    }

    pub fn source_project(&self) -> Option<&AuthoringProject> {
        self.source_project.as_ref()
    }
}

impl ModelWorkspace {
    pub fn new(snapshot: WorkspaceSnapshot) -> Self {
        Self {
            current: Arc::new(RwLock::new(Arc::new(snapshot))),
        }
    }

    pub fn current_snapshot(&self) -> Arc<WorkspaceSnapshot> {
        Arc::clone(&self.current.read().expect("workspace lock poisoned"))
    }

    pub fn session(&self) -> ModelSession {
        ModelSession {
            snapshot: self.current_snapshot(),
            workspace: Some(self.clone()),
        }
    }

    fn publish_snapshot(&self, snapshot: WorkspaceSnapshot) {
        *self.current.write().expect("workspace lock poisoned") = Arc::new(snapshot);
    }
}

impl ModelSession {
    pub fn snapshot(&self) -> Arc<WorkspaceSnapshot> {
        Arc::clone(&self.snapshot)
    }

    pub fn fork(&self, label: impl Into<String>) -> ModelFork {
        ModelFork {
            label: Some(label.into()),
            base: Arc::clone(&self.snapshot),
            workspace: self.workspace.clone(),
            overlay: KirOverlay::default(),
        }
    }
}

impl ModelFork {
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    pub fn base_revision(&self) -> &WorkspaceRevision {
        &self.base.revision
    }

    pub fn overlay(&self) -> &KirOverlay {
        &self.overlay
    }

    pub fn package(
        &mut self,
        qualified_name: impl Into<String>,
        owner: Option<&ForkElement>,
    ) -> Result<ForkElement, SessionError> {
        let qualified_name = normalize_qname(&qualified_name.into())?;
        let id = format!("pkg.{qualified_name}");
        self.ensure_not_present(&id)?;
        let source_file = generated_source_file_for(&qualified_name);
        let mut properties = BTreeMap::from([
            (
                "declared_name".to_string(),
                Value::String(qualified_name.clone()),
            ),
            ("metadata".to_string(), source_file_metadata(&source_file)),
        ]);
        if let Some(owner) = owner {
            properties.insert("owner".to_string(), Value::String(owner.id.clone()));
            self.patch_member(owner.id.as_str(), id.as_str());
        }
        let element = KirElement {
            id: id.clone(),
            kind: "SysML::Package".to_string(),
            layer: 2,
            properties,
        };
        self.overlay.added_elements.insert(id.clone(), element);
        self.overlay.operations.push(ForkOperation::AddPackage {
            id: id.clone(),
            qualified_name: qualified_name.clone(),
            owner: owner.map(|owner| owner.id.clone()),
            source_file,
        });
        Ok(ForkElement { id, qualified_name })
    }

    pub fn requirement(
        &mut self,
        owner: &ForkElement,
        name: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<ForkElement, SessionError> {
        let name = normalize_name(&name.into())?;
        let qualified_name = format!("{}.{}", owner.qualified_name, name);
        let id = format!("requirement.{qualified_name}");
        self.ensure_not_present(&id)?;
        let source_file = source_file_for_owner(self.overlay.added_elements.get(&owner.id))
            .unwrap_or_else(|| generated_source_file_for(&owner.qualified_name));
        let element = KirElement {
            id: id.clone(),
            kind: "SysML::RequirementUsage".to_string(),
            layer: 2,
            properties: BTreeMap::from([
                ("declared_name".to_string(), Value::String(name)),
                ("owner".to_string(), Value::String(owner.id.clone())),
                ("doc".to_string(), doc_metadata(text.into())),
                ("metadata".to_string(), source_file_metadata(&source_file)),
            ]),
        };
        self.overlay.added_elements.insert(id.clone(), element);
        self.patch_member(owner.id.as_str(), id.as_str());
        self.overlay.operations.push(ForkOperation::AddRequirement {
            id: id.clone(),
            qualified_name: qualified_name.clone(),
            owner: owner.id.clone(),
            source_file,
        });
        Ok(ForkElement { id, qualified_name })
    }

    pub fn part(
        &mut self,
        owner: &ForkElement,
        name: impl Into<String>,
        ty: Option<impl Into<String>>,
    ) -> Result<ForkElement, SessionError> {
        let name = normalize_name(&name.into())?;
        let qualified_name = format!("{}.{}", owner.qualified_name, name);
        let id = format!("feature.{qualified_name}");
        self.ensure_not_present(&id)?;
        let source_file = source_file_for_owner(self.overlay.added_elements.get(&owner.id))
            .unwrap_or_else(|| generated_source_file_for(&owner.qualified_name));
        let mut properties = BTreeMap::from([
            ("declared_name".to_string(), Value::String(name)),
            ("owner".to_string(), Value::String(owner.id.clone())),
            ("metadata".to_string(), source_file_metadata(&source_file)),
        ]);
        if let Some(ty) = ty {
            properties.insert(
                "type".to_string(),
                Value::String(normalize_qname(&ty.into())?),
            );
        }
        let element = KirElement {
            id: id.clone(),
            kind: "SysML::PartUsage".to_string(),
            layer: 2,
            properties,
        };
        self.overlay.added_elements.insert(id.clone(), element);
        self.patch_member(owner.id.as_str(), id.as_str());
        Ok(ForkElement { id, qualified_name })
    }

    pub fn add_metadata(
        &mut self,
        owner: &ForkElement,
        metadata_type: impl Into<String>,
        properties: BTreeMap<String, String>,
    ) -> Result<ForkElement, SessionError> {
        let metadata_type = normalize_name(&metadata_type.into())?;
        let qualified_name = format!("{}.{}", owner.qualified_name, metadata_type);
        let id = format!("metadata.{qualified_name}");
        self.ensure_not_present(&id)?;
        let source_file = source_file_for_owner(self.overlay.added_elements.get(&owner.id))
            .unwrap_or_else(|| generated_source_file_for(&owner.qualified_name));
        let element = KirElement {
            id: id.clone(),
            kind: "SysML::MetadataUsage".to_string(),
            layer: 2,
            properties: BTreeMap::from([
                (
                    "declared_name".to_string(),
                    Value::String(metadata_type.clone()),
                ),
                ("owner".to_string(), Value::String(owner.id.clone())),
                ("metadata".to_string(), source_file_metadata(&source_file)),
                (
                    "doc".to_string(),
                    Value::Object(
                        properties
                            .into_iter()
                            .map(|(key, value)| (key, Value::String(value)))
                            .collect(),
                    ),
                ),
            ]),
        };
        self.overlay.added_elements.insert(id.clone(), element);
        self.patch_member(owner.id.as_str(), id.as_str());
        Ok(ForkElement { id, qualified_name })
    }

    pub fn satisfy(
        &mut self,
        owner: &ForkElement,
        target: &ForkElement,
    ) -> Result<ForkElement, SessionError> {
        self.relationship(owner, "satisfy", target)
    }

    pub fn verify(
        &mut self,
        owner: &ForkElement,
        target: &ForkElement,
    ) -> Result<ForkElement, SessionError> {
        self.relationship(owner, "verify", target)
    }

    pub fn requirements<I, N, T>(
        &mut self,
        owner: &ForkElement,
        items: I,
    ) -> Result<Vec<ForkElement>, SessionError>
    where
        I: IntoIterator<Item = (N, T)>,
        N: Into<String>,
        T: Into<String>,
    {
        items
            .into_iter()
            .map(|(name, text)| self.requirement(owner, name, text))
            .collect()
    }

    pub fn rename_declaration(
        &mut self,
        element: impl Into<String>,
        new_name: impl Into<String>,
    ) -> Result<(), SessionError> {
        let element = ElementRef::new(normalize_qname(&element.into())?);
        let new_name = normalize_name(&new_name.into())?;
        let element_id = self
            .base
            .kir
            .elements
            .iter()
            .find(|candidate| candidate.id.ends_with(&element.qualified_name))
            .map(|candidate| candidate.id.clone())
            .ok_or_else(|| SessionError::MissingElement(element.qualified_name.clone()))?;
        self.overlay
            .updated_properties
            .entry(element_id)
            .or_default()
            .insert("declared_name".to_string(), Value::String(new_name.clone()));
        self.overlay
            .operations
            .push(ForkOperation::RenameDeclaration { element, new_name });
        Ok(())
    }

    pub fn materialize(&self) -> Result<KirDocument, SessionError> {
        self.overlay.materialize(&self.base.kir)
    }

    pub fn validate(&self) -> Result<(), SessionError> {
        self.materialize()?.validate()?;
        Ok(())
    }

    pub fn diff(&self) -> Result<SemanticDiff, SessionError> {
        Ok(diff_kir_documents(&self.base.kir, &self.materialize()?))
    }

    pub fn commit(&self, mode: CommitMode) -> Result<CommitResult, SessionError> {
        self.ensure_current_workspace_revision()?;
        match mode {
            CommitMode::PreserveSource => self.commit_preserve_source(),
            CommitMode::RewriteSource => self.commit_rewrite_source(),
        }
    }

    fn commit_preserve_source(&self) -> Result<CommitResult, SessionError> {
        if self.overlay.is_empty() {
            return self.noop_commit(CommitMode::PreserveSource);
        }

        if self.can_use_mutator_plan() {
            return self.commit_with_mutator_plan();
        }

        let files = self.render_generated_companion_files()?;
        self.validate_generated_files(&files)?;
        self.finish_commit(
            CommitMode::PreserveSource,
            CommitStrategy::GeneratedCompanionFiles,
            files,
        )
    }

    fn commit_rewrite_source(&self) -> Result<CommitResult, SessionError> {
        if self.overlay.is_empty() {
            return self.noop_commit(CommitMode::RewriteSource);
        }

        let files = self.render_generated_companion_files()?;
        self.validate_generated_files(&files)?;
        self.finish_commit(
            CommitMode::RewriteSource,
            CommitStrategy::RewriteGeneratedSource,
            files,
        )
    }

    fn commit_with_mutator_plan(&self) -> Result<CommitResult, SessionError> {
        let Some(project) = self.base.source_project.clone() else {
            return Err(SessionError::Unsupported(
                "source-preserving mutator commits require source-backed snapshot".to_string(),
            ));
        };
        let context = MutationContext::from_project(project);
        let operations = self
            .overlay
            .operations
            .iter()
            .filter_map(|operation| match operation {
                ForkOperation::RenameDeclaration { element, new_name } => {
                    Some(SemanticMutation::RenameDeclaration {
                        element: element.clone(),
                        new_name: new_name.clone(),
                    })
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let proposal = MutationProposal {
            intent: self
                .label
                .clone()
                .unwrap_or_else(|| "Apply source-preserving session fork".to_string()),
            affected_elements: operations
                .iter()
                .filter_map(|operation| match operation {
                    SemanticMutation::RenameDeclaration { element, .. } => Some(element.clone()),
                    _ => None,
                })
                .collect(),
            operations,
            evidence: vec![MutationEvidence {
                element: None,
                summary: "Generated from ModelFork preserve-source commit.".to_string(),
            }],
            rationale: None,
            workspace_revision: context.workspace_revision.clone(),
        };
        let service = CoreMutationFeasibilityService::new();
        let report = service.check(&context, &proposal);
        if !matches!(
            report.status,
            FeasibilityStatus::Allowed | FeasibilityStatus::AllowedWithWarnings
        ) {
            return Err(SessionError::Feasibility(
                report
                    .blocking_reasons
                    .first()
                    .cloned()
                    .unwrap_or_else(|| FeasibilityIssue {
                        kind: FeasibilityIssueKind::ValidationFailure,
                        operation_index: None,
                        message: "mutator plan was not feasible".to_string(),
                    }),
            ));
        }
        let plan = report.normalized_plan.as_ref().ok_or_else(|| {
            SessionError::Unsupported("missing normalized mutation plan".to_string())
        })?;
        let application = service
            .apply_checked_plan(&context, plan)
            .map_err(SessionError::Feasibility)?;
        let semantic_diff = application.semantic_diff;
        let changed_files = application.changed_files;
        let edited_files = application.edited_files;
        let new_kir = self.materialize()?;
        let new_revision = workspace_revision_for_kir_document(&new_kir)?;
        self.publish_if_workspace(new_kir, new_revision.clone());
        Ok(CommitResult {
            mode: CommitMode::PreserveSource,
            strategy_used: CommitStrategy::MutatorPlan,
            base_revision: self.base.revision.clone(),
            new_revision,
            changed_files,
            edited_files,
            semantic_diff,
            generated_elements: 0,
        })
    }

    fn finish_commit(
        &self,
        mode: CommitMode,
        strategy_used: CommitStrategy,
        edited_files: BTreeMap<String, String>,
    ) -> Result<CommitResult, SessionError> {
        let new_kir = self.materialize()?;
        new_kir.validate()?;
        let new_revision = workspace_revision_for_kir_document(&new_kir)?;
        let semantic_diff = diff_kir_documents(&self.base.kir, &new_kir);
        let changed_files = edited_files.keys().cloned().collect::<BTreeSet<_>>();
        self.publish_if_workspace(new_kir, new_revision.clone());
        Ok(CommitResult {
            mode,
            strategy_used,
            base_revision: self.base.revision.clone(),
            new_revision,
            changed_files,
            edited_files,
            semantic_diff,
            generated_elements: self.overlay.added_elements.len(),
        })
    }

    fn noop_commit(&self, mode: CommitMode) -> Result<CommitResult, SessionError> {
        Ok(CommitResult {
            mode,
            strategy_used: CommitStrategy::NoOp,
            base_revision: self.base.revision.clone(),
            new_revision: self.base.revision.clone(),
            changed_files: BTreeSet::new(),
            edited_files: BTreeMap::new(),
            semantic_diff: SemanticDiff::default(),
            generated_elements: 0,
        })
    }

    fn can_use_mutator_plan(&self) -> bool {
        self.base.source_project.is_some()
            && self.overlay.added_elements.is_empty()
            && self.overlay.added_members.is_empty()
            && self.overlay.removed_elements.is_empty()
            && !self.overlay.operations.is_empty()
            && self
                .overlay
                .operations
                .iter()
                .all(|operation| matches!(operation, ForkOperation::RenameDeclaration { .. }))
            && self.overlay.operations.len() <= GENERATED_FILE_THRESHOLD
    }

    fn render_generated_companion_files(&self) -> Result<BTreeMap<String, String>, SessionError> {
        let overlay_document = KirDocument {
            metadata: BTreeMap::new(),
            elements: self.overlay.added_elements_with_member_patches(),
        };
        let project = AuthoringProject::from_kir_document(&overlay_document)?;
        let mut rendered = BTreeMap::new();
        for (path, _) in project.files() {
            rendered.insert(path.to_string(), project.render_new_file(path)?);
        }
        Ok(rendered)
    }

    fn validate_generated_files(
        &self,
        files: &BTreeMap<String, String>,
    ) -> Result<(), SessionError> {
        let stdlib = KirDocument::from_path(&default_stdlib_path())?;
        let mut documents = Vec::new();
        for (path, source) in files {
            documents.push(
                compile_sysml_text(source, path, &stdlib)
                    .map_err(|err| SessionError::Unsupported(err.to_string()))?,
            );
        }
        let emitted = KirDocument::merge(documents)?;
        for intended in self.overlay.added_elements.values() {
            let Some(actual) = emitted
                .elements
                .iter()
                .find(|candidate| candidate.id == intended.id)
            else {
                return Err(SessionError::Unsupported(format!(
                    "generated SysML did not round-trip intended element `{}`",
                    intended.id
                )));
            };
            if actual.kind != intended.kind
                && !is_compatible_round_trip_kind(&intended.kind, &actual.kind)
            {
                return Err(SessionError::Unsupported(format!(
                    "generated SysML round-tripped `{}` as `{}`, expected `{}`",
                    intended.id, actual.kind, intended.kind
                )));
            }
        }
        Ok(())
    }

    fn relationship(
        &mut self,
        owner: &ForkElement,
        kind: &str,
        target: &ForkElement,
    ) -> Result<ForkElement, SessionError> {
        let normalized = normalize_name(kind)?;
        let name = format!("{}_{}", normalized, target.qualified_name.replace('.', "_"));
        let qualified_name = format!("{}.{}", owner.qualified_name, name);
        let id = format!("relationship.{qualified_name}");
        self.ensure_not_present(&id)?;
        let source_file = source_file_for_owner(self.overlay.added_elements.get(&owner.id))
            .unwrap_or_else(|| generated_source_file_for(&owner.qualified_name));
        let element = KirElement {
            id: id.clone(),
            kind: format!("SysML::{}Relationship", pascal_case(&normalized)),
            layer: 2,
            properties: BTreeMap::from([
                ("declared_name".to_string(), Value::String(name)),
                ("owner".to_string(), Value::String(owner.id.clone())),
                ("source".to_string(), Value::String(owner.id.clone())),
                ("target".to_string(), Value::String(target.id.clone())),
                ("metadata".to_string(), source_file_metadata(&source_file)),
            ]),
        };
        self.overlay.added_elements.insert(id.clone(), element);
        self.patch_member(owner.id.as_str(), id.as_str());
        Ok(ForkElement { id, qualified_name })
    }

    fn publish_if_workspace(&self, kir: KirDocument, revision: WorkspaceRevision) {
        if let Some(workspace) = &self.workspace {
            workspace.publish_snapshot(WorkspaceSnapshot {
                revision,
                kir: Arc::new(kir),
                profile_id: self.base.profile_id.clone(),
                source_project: None,
            });
        }
    }

    fn ensure_current_workspace_revision(&self) -> Result<(), SessionError> {
        if let Some(workspace) = &self.workspace {
            let current = workspace.current_snapshot();
            if current.revision != self.base.revision {
                return Err(SessionError::StaleWorkspace {
                    base_revision: self.base.revision.clone(),
                    current_revision: current.revision.clone(),
                });
            }
        }
        Ok(())
    }

    fn ensure_not_present(&self, id: &str) -> Result<(), SessionError> {
        if self
            .base
            .kir
            .elements
            .iter()
            .any(|element| element.id == id)
            || self.overlay.added_elements.contains_key(id)
        {
            return Err(SessionError::DuplicateElement(id.to_string()));
        }
        Ok(())
    }

    fn patch_member(&mut self, owner_id: &str, member_id: &str) {
        let members = self
            .overlay
            .added_members
            .entry(owner_id.to_string())
            .or_default();
        members.push(member_id.to_string());
    }
}

impl KirOverlay {
    pub fn materialize(&self, base: &KirDocument) -> Result<KirDocument, SessionError> {
        let mut elements = Vec::new();
        for element in &base.elements {
            if self.removed_elements.contains(&element.id) {
                continue;
            }
            let mut element = element.clone();
            if let Some(updates) = self.updated_properties.get(&element.id) {
                for (key, value) in updates {
                    element.properties.insert(key.clone(), value.clone());
                }
            }
            self.apply_added_members(&mut element);
            elements.push(element);
        }
        for mut element in self.added_elements.values().cloned() {
            self.apply_added_members(&mut element);
            elements.push(element);
        }
        elements.sort_by(|left, right| left.id.cmp(&right.id));
        let document = KirDocument {
            metadata: base.metadata.clone(),
            elements,
        };
        document.validate()?;
        Ok(document)
    }

    pub fn is_empty(&self) -> bool {
        self.added_elements.is_empty()
            && self.updated_properties.is_empty()
            && self.added_members.is_empty()
            && self.removed_elements.is_empty()
            && self.operations.is_empty()
    }

    fn added_elements_with_member_patches(&self) -> Vec<KirElement> {
        self.added_elements
            .values()
            .cloned()
            .map(|mut element| {
                self.apply_added_members(&mut element);
                element
            })
            .collect()
    }

    fn apply_added_members(&self, element: &mut KirElement) {
        let Some(added) = self.added_members.get(&element.id) else {
            return;
        };
        let mut members = element
            .properties
            .get("members")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for member_id in added {
            if !members
                .iter()
                .any(|value| value.as_str() == Some(member_id.as_str()))
            {
                members.push(Value::String(member_id.clone()));
            }
        }
        element
            .properties
            .insert("members".to_string(), Value::Array(members));
    }
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleWorkspace {
                base_revision,
                current_revision,
            } => write!(
                f,
                "workspace changed after fork was created: base {}, current {}",
                base_revision.fingerprint, current_revision.fingerprint
            ),
            Self::DuplicateElement(id) => write!(f, "element `{id}` already exists"),
            Self::MissingElement(id) => write!(f, "element `{id}` does not exist"),
            Self::InvalidInput(message) => f.write_str(message),
            Self::Unsupported(message) => f.write_str(message),
            Self::Kir(err) => write!(f, "{err}"),
            Self::Authoring(err) => write!(f, "{err}"),
            Self::Feasibility(issue) => write!(f, "mutation feasibility failed: {}", issue.message),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<KirError> for SessionError {
    fn from(value: KirError) -> Self {
        Self::Kir(value)
    }
}

impl From<crate::authoring::AuthoringError> for SessionError {
    fn from(value: crate::authoring::AuthoringError) -> Self {
        Self::Authoring(value)
    }
}

fn normalize_qname(value: &str) -> Result<String, SessionError> {
    let normalized = value.trim().replace("::", ".");
    if normalized.is_empty()
        || normalized
            .split('.')
            .any(|segment| segment.trim().is_empty())
    {
        return Err(SessionError::InvalidInput(format!(
            "invalid qualified name `{value}`"
        )));
    }
    Ok(normalized)
}

fn normalize_name(value: &str) -> Result<String, SessionError> {
    let normalized = value.trim();
    if normalized.is_empty()
        || normalized.contains('.')
        || normalized.contains(':')
        || normalized.chars().any(char::is_whitespace)
    {
        return Err(SessionError::InvalidInput(format!(
            "invalid declaration name `{value}`"
        )));
    }
    Ok(normalized.to_string())
}

fn generated_source_file_for(qualified_name: &str) -> String {
    format!(
        "generated/{}.sysml",
        qualified_name
            .replace("::", ".")
            .split('.')
            .map(to_snake_segment)
            .collect::<Vec<_>>()
            .join("_")
    )
}

fn to_snake_segment(segment: &str) -> String {
    let mut output = String::new();
    for (index, ch) in segment.chars().enumerate() {
        if ch.is_ascii_uppercase() && index > 0 {
            output.push('_');
        }
        if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
        } else if !output.ends_with('_') {
            output.push('_');
        }
    }
    output.trim_matches('_').to_string()
}

fn pascal_case(value: &str) -> String {
    value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let mut chars = segment.chars();
            match chars.next() {
                Some(first) => format!(
                    "{}{}",
                    first.to_ascii_uppercase(),
                    chars.as_str().to_ascii_lowercase()
                ),
                None => String::new(),
            }
        })
        .collect()
}

fn is_compatible_round_trip_kind(intended: &str, actual: &str) -> bool {
    matches!(
        (intended, actual),
        ("SysML::RequirementUsage", "KerML::Core::Feature")
            | ("SysML::PartUsage", "KerML::Core::Feature")
            | ("SysML::MetadataUsage", "KerML::Core::Feature")
    )
}

fn source_file_for_owner(owner: Option<&KirElement>) -> Option<String> {
    owner
        .and_then(|element| element.properties.get("metadata"))
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("source_file"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn source_file_metadata(source_file: &str) -> Value {
    Value::Object(
        BTreeMap::from([(
            "source_file".to_string(),
            Value::String(source_file.to_string()),
        )])
        .into_iter()
        .collect(),
    )
}

fn doc_metadata(text: String) -> Value {
    Value::Object(
        BTreeMap::from([(
            "blocks".to_string(),
            Value::Array(vec![Value::Object(
                BTreeMap::from([("text".to_string(), Value::String(text))])
                    .into_iter()
                    .collect(),
            )]),
        )])
        .into_iter()
        .collect(),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use crate::authoring::load_authoring_project_from_sysml;

    use super::*;

    fn empty_document() -> KirDocument {
        KirDocument {
            metadata: BTreeMap::new(),
            elements: Vec::new(),
        }
    }

    #[test]
    fn session_and_fork_share_base_kir() {
        let snapshot = Arc::new(WorkspaceSnapshot::new(empty_document()).unwrap());
        let session = snapshot.session();
        let fork = session.fork("shared");

        assert!(Arc::ptr_eq(&session.snapshot().kir, &fork.base.kir));
        assert_eq!(fork.base_revision(), &snapshot.revision);
    }

    #[test]
    fn fork_overlay_adds_large_requirement_set_without_cloning_base() {
        let snapshot = Arc::new(WorkspaceSnapshot::new(empty_document()).unwrap());
        let session = snapshot.session();
        let mut fork = session.fork("bulk requirements");
        let package = fork.package("SyntheticRequirements", None).unwrap();
        for index in 0..10_000 {
            fork.requirement(
                &package,
                format!("Req{index:05}"),
                format!("Generated requirement {index:05}"),
            )
            .unwrap();
        }

        assert_eq!(fork.overlay().added_elements.len(), 10_001);
        assert_eq!(snapshot.kir.elements.len(), 0);
        assert_eq!(fork.materialize().unwrap().elements.len(), 10_001);
    }

    #[test]
    fn fork_overlay_bulk_requirement_addition_is_linear_enough() {
        let snapshot = Arc::new(WorkspaceSnapshot::new(empty_document()).unwrap());
        let session = snapshot.session();
        let mut fork = session.fork("bulk requirements timing");
        let package = fork.package("SyntheticRequirements", None).unwrap();

        let started = Instant::now();
        for index in 0..10_000 {
            fork.requirement(
                &package,
                format!("Req{index:05}"),
                format!("Generated requirement {index:05}"),
            )
            .unwrap();
        }
        let elapsed = started.elapsed();

        assert_eq!(fork.overlay().added_elements.len(), 10_001);
        assert!(
            elapsed.as_secs_f64() < 2.0,
            "adding 10k requirements to an overlay took {elapsed:?}"
        );
    }

    #[test]
    fn rewrite_source_commit_emits_generated_sysml() {
        let workspace = ModelWorkspace::new(WorkspaceSnapshot::new(empty_document()).unwrap());
        let session = workspace.session();
        let mut fork = session.fork("generated requirements");
        let package = fork.package("SyntheticRequirements", None).unwrap();
        fork.requirement(&package, "Req00001", "Generated requirement")
            .unwrap();
        fork.part(&package, "controller", Option::<String>::None)
            .unwrap();

        let result = fork.commit(CommitMode::RewriteSource).unwrap();

        assert_eq!(result.strategy_used, CommitStrategy::RewriteGeneratedSource);
        let source = result
            .edited_files
            .get("generated/synthetic_requirements.sysml")
            .unwrap();
        assert!(source.contains("package SyntheticRequirements"));
        assert!(source.contains("requirement Req00001"));
        assert!(source.contains("part controller"));
        assert_ne!(workspace.current_snapshot().revision, result.base_revision);
    }

    #[test]
    fn preserve_source_bulk_addition_uses_generated_companion_file() {
        let workspace = ModelWorkspace::new(WorkspaceSnapshot::new(empty_document()).unwrap());
        let session = workspace.session();
        let mut fork = session.fork("generated requirements");
        let package = fork.package("SyntheticRequirements", None).unwrap();
        for index in 0..101 {
            fork.requirement(&package, format!("Req{index:05}"), "Generated")
                .unwrap();
        }

        let result = fork.commit(CommitMode::PreserveSource).unwrap();

        assert_eq!(
            result.strategy_used,
            CommitStrategy::GeneratedCompanionFiles
        );
        assert_eq!(result.generated_elements, 102);
    }

    #[test]
    fn preserve_source_small_rename_uses_mutator_plan() {
        let project = load_authoring_project_from_sysml(BTreeMap::from([(
            "vehicle.sysml".to_string(),
            "package Vehicle { part engine; }".to_string(),
        )]))
        .unwrap();
        let workspace =
            ModelWorkspace::new(WorkspaceSnapshot::from_authoring_project(project).unwrap());
        let session = workspace.session();
        let mut fork = session.fork("rename engine");
        fork.rename_declaration("Vehicle.engine", "motor").unwrap();

        let result = fork.commit(CommitMode::PreserveSource).unwrap();

        assert_eq!(result.strategy_used, CommitStrategy::MutatorPlan);
        assert!(result.edited_files["vehicle.sysml"].contains("part motor;"));
    }

    #[test]
    fn stale_fork_commit_fails() {
        let workspace = ModelWorkspace::new(WorkspaceSnapshot::new(empty_document()).unwrap());
        let first = workspace.session();
        let second = workspace.session();

        let mut first_fork = first.fork("first");
        let first_package = first_fork.package("First", None).unwrap();
        first_fork
            .requirement(&first_package, "Req00001", "Generated")
            .unwrap();
        first_fork.commit(CommitMode::RewriteSource).unwrap();

        let mut stale_fork = second.fork("stale");
        stale_fork.package("Second", None).unwrap();
        let error = stale_fork.commit(CommitMode::RewriteSource).unwrap_err();

        assert!(matches!(error, SessionError::StaleWorkspace { .. }));
    }
}
