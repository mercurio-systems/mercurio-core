use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::path::Path;
use std::sync::OnceLock;

use serde_json::Value;

use crate::frontend::ast::{
    BinaryOp, Declaration as AstDeclaration, Expr, LiteralExpr, MultiplicityRange, PackageDecl,
    SourceSpan, SysmlModule, UnaryOp,
};
use crate::frontend::diagnostics::Diagnostic;
use crate::frontend::sysml::{compile_sysml_text, parse_sysml};
use crate::ir::{KirDocument, KirElement, KirError};
use crate::paths::default_stdlib_path;

static DEFAULT_STDLIB_DOCUMENT: OnceLock<Result<KirDocument, String>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuthoringProject {
    files: BTreeMap<String, FileModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileModel {
    path: String,
    module: AuthoringModule,
    original_text: Option<String>,
    source_map: Option<FileSourceMap>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuthoringModule {
    pub package: Option<Package>,
    pub members: Vec<Declaration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub name: QualifiedName,
    pub members: Vec<Declaration>,
    pub docs: Vec<String>,
    pub modifiers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    pub path: QualifiedName,
    pub docs: Vec<String>,
    pub modifiers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub keyword: String,
    pub name: String,
    pub specializes: Vec<QualifiedName>,
    pub members: Vec<Declaration>,
    pub docs: Vec<String>,
    pub modifiers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    pub keyword: String,
    pub name: String,
    pub is_implicit_name: bool,
    pub ty: Option<QualifiedName>,
    pub reference_target: Option<QualifiedName>,
    pub metadata_properties: BTreeMap<String, String>,
    pub multiplicity: Option<MultiplicityRange>,
    pub expression: Option<String>,
    pub additional_types: Vec<QualifiedName>,
    pub specializes: Vec<QualifiedName>,
    pub subsets: Vec<QualifiedName>,
    pub redefines: Vec<QualifiedName>,
    pub members: Vec<Declaration>,
    pub docs: Vec<String>,
    pub modifiers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    pub name: String,
    pub target: QualifiedName,
    pub docs: Vec<String>,
    pub modifiers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Declaration {
    Package(Package),
    Import(Import),
    Definition(Definition),
    Usage(Usage),
    Alias(Alias),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QualifiedName(pub Vec<String>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mutation {
    AddPackage {
        target_file: String,
        package_name: QualifiedName,
    },
    AddImport {
        target_file: String,
        package_name: Option<QualifiedName>,
        path: QualifiedName,
    },
    RemoveImport {
        target_file: String,
        package_name: Option<QualifiedName>,
        path: QualifiedName,
    },
    AddDefinition {
        container: ContainerSelector,
        keyword: String,
        name: String,
        specializes: Vec<QualifiedName>,
    },
    AddUsage {
        container: ContainerSelector,
        keyword: String,
        name: String,
        ty: Option<QualifiedName>,
        specializes: Vec<QualifiedName>,
    },
    AddRelationship {
        container: ContainerSelector,
        kind: String,
        source: QualifiedName,
        target: QualifiedName,
    },
    AddMetadataAnnotation {
        element: QualifiedName,
        metadata_type: String,
        properties: BTreeMap<String, String>,
    },
    RemoveDeclaration {
        qualified_name: QualifiedName,
    },
    RenameDeclaration {
        qualified_name: QualifiedName,
        new_name: String,
    },
    UpdateSpecializations {
        qualified_name: QualifiedName,
        specializes: Vec<QualifiedName>,
    },
    UpdateUsageType {
        qualified_name: QualifiedName,
        ty: Option<QualifiedName>,
    },
    SetExpression {
        qualified_name: QualifiedName,
        expression: Option<String>,
    },
    MoveDeclaration {
        qualified_name: QualifiedName,
        destination: ContainerSelector,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum SemanticEdit {
    SetAttribute {
        element: QualifiedName,
        attribute: String,
        value: Value,
        policy: AttributeWritePolicy,
    },
    ClearAttribute {
        element: QualifiedName,
        attribute: String,
        policy: AttributeWritePolicy,
    },
    AddAttributeValue {
        element: QualifiedName,
        attribute: String,
        value: Value,
        policy: AttributeWritePolicy,
    },
    RemoveAttributeValue {
        element: QualifiedName,
        attribute: String,
        value: Value,
        policy: AttributeWritePolicy,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeWritePolicy {
    DirectOnly,
    UpsertDirect,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticAttribute {
    pub name: String,
    pub origin_kind: String,
    pub direct_value: Option<Value>,
    pub effective_value: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerSelector {
    File { target_file: String },
    Package { qualified_name: QualifiedName },
    Declaration { qualified_name: QualifiedName },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationResult {
    pub changed_files: BTreeSet<String>,
    pub changed_declarations: BTreeSet<String>,
    pub affected_element_ids: BTreeSet<String>,
    rewrite_plan: Vec<RewriteInstruction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteBackResult {
    pub edited_files: BTreeMap<String, String>,
    pub mode: WriteBackMode,
    pub changed_spans: BTreeMap<String, Vec<RenderedSpan>>,
    pub validation: ValidationReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteBackMode {
    LocalizedPatch,
    CanonicalRewrite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedSpan {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    pub ok: bool,
    pub expected_element_count: usize,
    pub actual_element_count: usize,
    pub message: Option<String>,
}

#[derive(Debug)]
pub enum AuthoringError {
    Parse(Diagnostic),
    Kir(KirError),
    MissingFile(String),
    MissingPackage(String),
    MissingDeclaration(String),
    InvalidMutation(String),
    Unsupported(String),
    Validation(String),
    Io(std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileSourceMap {
    package: Option<SourceNode>,
    declarations: BTreeMap<String, SourceNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceNode {
    span: SourceSpan,
    indent: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RewriteInstruction {
    FullFile {
        file: String,
    },
    ReplaceNode {
        file: String,
        anchor_qname: String,
        render_qname: String,
    },
    ReplaceContainer {
        file: String,
        anchor_qname: Option<String>,
        render_qname: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DeclarationKind {
    Package,
    Definition,
    Usage,
    Alias,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocatedDeclaration {
    file: String,
    kind: DeclarationKind,
    parent_qname: Option<String>,
}

impl fmt::Display for AuthoringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(err) => write!(f, "{err}"),
            Self::Kir(err) => write!(f, "{err}"),
            Self::MissingFile(path) => write!(f, "missing authoring file: {path}"),
            Self::MissingPackage(name) => write!(f, "missing package: {name}"),
            Self::MissingDeclaration(name) => write!(f, "missing declaration: {name}"),
            Self::InvalidMutation(message) => write!(f, "{message}"),
            Self::Unsupported(message) => write!(f, "{message}"),
            Self::Validation(message) => write!(f, "{message}"),
            Self::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for AuthoringError {}

impl From<Diagnostic> for AuthoringError {
    fn from(value: Diagnostic) -> Self {
        Self::Parse(value)
    }
}

impl From<KirError> for AuthoringError {
    fn from(value: KirError) -> Self {
        Self::Kir(value)
    }
}

impl From<std::io::Error> for AuthoringError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl QualifiedName {
    pub fn new(segments: Vec<String>) -> Self {
        Self(segments)
    }

    pub fn parse(value: &str) -> Self {
        let segments = value
            .split(['.', ':'])
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>();
        if value.contains("::") {
            return Self(
                value
                    .split("::")
                    .filter(|segment| !segment.is_empty())
                    .map(str::to_string)
                    .collect(),
            );
        }
        Self(segments.into_iter().map(str::to_string).collect())
    }

    pub fn as_dot_string(&self) -> String {
        self.0.join(".")
    }

    pub fn as_colon_string(&self) -> String {
        self.0.join("::")
    }

    fn tail(&self) -> Option<&str> {
        self.0.last().map(String::as_str)
    }
}

pub fn create_empty_model() -> AuthoringProject {
    AuthoringProject::default()
}

pub fn load_authoring_project_from_sysml(
    files: BTreeMap<String, String>,
) -> Result<AuthoringProject, AuthoringError> {
    AuthoringProject::from_sysml_files(files)
}

pub fn load_authoring_project_from_kir(
    document: &KirDocument,
) -> Result<AuthoringProject, AuthoringError> {
    AuthoringProject::from_kir_document(document)
}

impl AuthoringProject {
    pub fn from_sysml_files(files: BTreeMap<String, String>) -> Result<Self, AuthoringError> {
        let mut project = Self::default();
        for (path, content) in files {
            let parsed = parse_sysml(&content)?;
            let module = AuthoringModule::from_ast(&parsed);
            let source_map = FileSourceMap::from_ast(&parsed);
            project.files.insert(
                path.clone(),
                FileModel {
                    path,
                    module,
                    original_text: Some(content),
                    source_map: Some(source_map),
                },
            );
        }
        Ok(project)
    }

    pub fn from_kir_document(document: &KirDocument) -> Result<Self, AuthoringError> {
        let grouped = group_kir_by_source_file(document);
        let mut project = Self::default();

        for (path, elements) in grouped {
            let module = module_from_kir_elements(&elements)?;
            project.files.insert(
                path.clone(),
                FileModel {
                    path,
                    module,
                    original_text: None,
                    source_map: None,
                },
            );
        }

        Ok(project)
    }

    pub fn files(&self) -> impl Iterator<Item = (&str, &AuthoringModule)> {
        self.files
            .iter()
            .map(|(path, file)| (path.as_str(), &file.module))
    }

    pub fn render_new_file(&self, path: &str) -> Result<String, AuthoringError> {
        let file = self
            .files
            .get(path)
            .ok_or_else(|| AuthoringError::MissingFile(path.to_string()))?;
        Ok(file.module.render())
    }

    pub fn semantic_attributes(
        &self,
        element: &QualifiedName,
    ) -> Result<Vec<SemanticAttribute>, AuthoringError> {
        let located = self.locate_declaration(element)?;
        let file = self
            .files
            .get(&located.file)
            .ok_or_else(|| AuthoringError::MissingFile(located.file.clone()))?;

        if matches!(located.kind, DeclarationKind::Package) {
            let package = locate_package_ref(&file.module, element)
                .ok_or_else(|| AuthoringError::MissingPackage(element.as_dot_string()))?;
            return Ok(semantic_attributes_for_package(package));
        }

        let declaration = locate_declaration_ref(&file.module, element)
            .ok_or_else(|| AuthoringError::MissingDeclaration(element.as_dot_string()))?;
        Ok(semantic_attributes_for_declaration(declaration))
    }

    pub fn apply_semantic_edit(
        &mut self,
        edit: SemanticEdit,
    ) -> Result<MutationResult, AuthoringError> {
        match edit {
            SemanticEdit::SetAttribute {
                element,
                attribute,
                value,
                policy,
            } => {
                let attribute = normalize_attribute_name(&attribute);
                self.ensure_attribute_policy(&element, &attribute, policy)?;
                match attribute.as_str() {
                    "declared_name" => self.apply_mutation(Mutation::RenameDeclaration {
                        qualified_name: element,
                        new_name: value_as_string(&value, "declared_name")?,
                    }),
                    "specializes" => self.apply_mutation(Mutation::UpdateSpecializations {
                        qualified_name: element,
                        specializes: value_as_qname_list(&value, "specializes")?,
                    }),
                    "type" => self.apply_mutation(Mutation::UpdateUsageType {
                        qualified_name: element,
                        ty: Some(value_as_qname(&value, "type")?),
                    }),
                    "is_abstract" => self.apply_modifier_flag_edit(
                        &element,
                        &attribute,
                        "abstract",
                        value_as_bool(&value, "is_abstract")?,
                    ),
                    "is_end" => self.apply_modifier_flag_edit(
                        &element,
                        &attribute,
                        "end",
                        value_as_bool(&value, "is_end")?,
                    ),
                    "direction" => {
                        self.apply_direction_edit(&element, Some(value_as_direction(&value)?))
                    }
                    "target" => {
                        self.apply_target_edit(&element, Some(value_as_qname(&value, "target")?))
                    }
                    "imports" => self
                        .apply_imports_replace(&element, value_as_qname_list(&value, "imports")?),
                    "doc" | "text" => self.apply_doc_edit(
                        &element,
                        DocEdit::Text(value_as_string(&value, &attribute)?),
                    ),
                    "id" | "requirement_id" => self.apply_doc_edit(
                        &element,
                        DocEdit::Id(value_as_string(&value, &attribute)?),
                    ),
                    other => Err(AuthoringError::Unsupported(format!(
                        "semantic set is not supported for attribute `{other}`"
                    ))),
                }
            }
            SemanticEdit::ClearAttribute {
                element,
                attribute,
                policy,
            } => {
                let attribute = normalize_attribute_name(&attribute);
                self.ensure_attribute_policy(&element, &attribute, policy)?;
                match attribute.as_str() {
                    "specializes" => self.apply_mutation(Mutation::UpdateSpecializations {
                        qualified_name: element,
                        specializes: Vec::new(),
                    }),
                    "type" => self.apply_mutation(Mutation::UpdateUsageType {
                        qualified_name: element,
                        ty: None,
                    }),
                    "is_abstract" => {
                        self.apply_modifier_flag_edit(&element, &attribute, "abstract", false)
                    }
                    "is_end" => self.apply_modifier_flag_edit(&element, &attribute, "end", false),
                    "direction" => self.apply_direction_edit(&element, None),
                    "target" => self.apply_target_edit(&element, None),
                    "imports" => self.apply_imports_replace(&element, Vec::new()),
                    "doc" | "text" => self.apply_doc_edit(&element, DocEdit::ClearText),
                    "id" | "requirement_id" => self.apply_doc_edit(&element, DocEdit::ClearId),
                    other => Err(AuthoringError::Unsupported(format!(
                        "semantic clear is not supported for attribute `{other}`"
                    ))),
                }
            }
            SemanticEdit::AddAttributeValue {
                element,
                attribute,
                value,
                policy,
            } => {
                let attribute = normalize_attribute_name(&attribute);
                self.ensure_attribute_policy(&element, &attribute, policy)?;
                match attribute.as_str() {
                    "specializes" => {
                        let mut values = self.qname_list_attribute_values(&element, &attribute)?;
                        for item in value_as_qname_list(&value, "specializes")? {
                            if !values.contains(&item) {
                                values.push(item);
                            }
                        }
                        self.apply_mutation(Mutation::UpdateSpecializations {
                            qualified_name: element,
                            specializes: values,
                        })
                    }
                    "imports" => {
                        let mut values = self.qname_list_attribute_values(&element, &attribute)?;
                        for item in value_as_qname_list(&value, "imports")? {
                            if !values.contains(&item) {
                                values.push(item);
                            }
                        }
                        self.apply_imports_replace(&element, values)
                    }
                    other => Err(AuthoringError::Unsupported(format!(
                        "semantic add is not supported for attribute `{other}`"
                    ))),
                }
            }
            SemanticEdit::RemoveAttributeValue {
                element,
                attribute,
                value,
                policy,
            } => {
                let attribute = normalize_attribute_name(&attribute);
                self.ensure_attribute_policy(&element, &attribute, policy)?;
                match attribute.as_str() {
                    "specializes" => {
                        let mut values = self.qname_list_attribute_values(&element, &attribute)?;
                        let removals = value_as_qname_list(&value, "specializes")?;
                        values.retain(|item| !removals.contains(item));
                        self.apply_mutation(Mutation::UpdateSpecializations {
                            qualified_name: element,
                            specializes: values,
                        })
                    }
                    "imports" => {
                        let mut values = self.qname_list_attribute_values(&element, &attribute)?;
                        let removals = value_as_qname_list(&value, "imports")?;
                        values.retain(|item| !removals.contains(item));
                        self.apply_imports_replace(&element, values)
                    }
                    other => Err(AuthoringError::Unsupported(format!(
                        "semantic remove is not supported for attribute `{other}`"
                    ))),
                }
            }
        }
    }

    pub fn apply_mutation(&mut self, mutation: Mutation) -> Result<MutationResult, AuthoringError> {
        let before = self.compile_user_kir()?;
        let mut changed_files = BTreeSet::new();
        let mut changed_declarations = BTreeSet::new();
        let rewrite_plan = match mutation {
            Mutation::AddPackage {
                target_file,
                package_name,
            } => {
                let file = self.ensure_file_mut(&target_file);
                if file.module.package.is_some() {
                    return Err(AuthoringError::InvalidMutation(format!(
                        "file `{target_file}` already has a package"
                    )));
                }
                file.module.package = Some(Package {
                    name: package_name.clone(),
                    members: Vec::new(),
                    docs: Vec::new(),
                    modifiers: Vec::new(),
                });
                changed_files.insert(target_file.clone());
                changed_declarations.insert(package_name.as_dot_string());
                vec![RewriteInstruction::FullFile { file: target_file }]
            }
            Mutation::AddImport {
                target_file,
                package_name,
                path,
            } => {
                let file = self.ensure_file_mut(&target_file);
                let import = Declaration::Import(Import {
                    path: path.clone(),
                    docs: Vec::new(),
                    modifiers: Vec::new(),
                });
                let instruction = if let Some(package_name) = package_name {
                    let package =
                        locate_package_mut(&mut file.module, &package_name).ok_or_else(|| {
                            AuthoringError::MissingPackage(package_name.as_dot_string())
                        })?;
                    package.members.push(import);
                    RewriteInstruction::ReplaceContainer {
                        file: target_file.clone(),
                        anchor_qname: Some(package_name.as_dot_string()),
                        render_qname: Some(package_name.as_dot_string()),
                    }
                } else {
                    file.module.members.push(import);
                    RewriteInstruction::FullFile {
                        file: target_file.clone(),
                    }
                };
                changed_files.insert(target_file);
                vec![instruction]
            }
            Mutation::RemoveImport {
                target_file,
                package_name,
                path,
            } => {
                let file = self
                    .files
                    .get_mut(&target_file)
                    .ok_or_else(|| AuthoringError::MissingFile(target_file.clone()))?;
                let removed = if let Some(package_name) = &package_name {
                    let package =
                        locate_package_mut(&mut file.module, package_name).ok_or_else(|| {
                            AuthoringError::MissingPackage(package_name.as_dot_string())
                        })?;
                    remove_import(&mut package.members, &path)
                } else {
                    remove_import(&mut file.module.members, &path)
                };
                if !removed {
                    return Err(AuthoringError::InvalidMutation(format!(
                        "missing import `{}` in `{target_file}`",
                        path.as_colon_string()
                    )));
                }
                changed_files.insert(target_file.clone());
                vec![if let Some(package_name) = package_name {
                    RewriteInstruction::ReplaceContainer {
                        file: target_file,
                        anchor_qname: Some(package_name.as_dot_string()),
                        render_qname: Some(package_name.as_dot_string()),
                    }
                } else {
                    RewriteInstruction::FullFile { file: target_file }
                }]
            }
            Mutation::AddDefinition {
                container,
                keyword,
                name,
                specializes,
            } => {
                let definition = Declaration::Definition(Definition {
                    keyword,
                    name: name.clone(),
                    specializes,
                    members: Vec::new(),
                    docs: Vec::new(),
                    modifiers: Vec::new(),
                });
                let (file, owner_qname, instruction) =
                    self.push_into_container(container, definition)?;
                changed_files.insert(file.clone());
                let owner = owner_qname.unwrap_or_default();
                changed_declarations.insert(join_qname(&owner, &name));
                vec![instruction]
            }
            Mutation::AddUsage {
                container,
                keyword,
                name,
                ty,
                specializes,
            } => {
                let usage = Declaration::Usage(Usage {
                    keyword,
                    name: name.clone(),
                    is_implicit_name: false,
                    ty,
                    reference_target: None,
                    metadata_properties: BTreeMap::new(),
                    multiplicity: None,
                    expression: None,
                    additional_types: Vec::new(),
                    specializes,
                    subsets: Vec::new(),
                    redefines: Vec::new(),
                    members: Vec::new(),
                    docs: Vec::new(),
                    modifiers: Vec::new(),
                });
                let (file, owner_qname, instruction) =
                    self.push_into_container(container, usage)?;
                changed_files.insert(file.clone());
                let owner = owner_qname.unwrap_or_default();
                changed_declarations.insert(join_qname(&owner, &name));
                vec![instruction]
            }
            Mutation::AddRelationship {
                container,
                kind,
                source,
                target,
            } => {
                let usage = relationship_usage(&kind, &source, &target)?;
                let relationship_name = usage.name.clone();
                let (file, owner_qname, instruction) =
                    self.push_into_container(container, Declaration::Usage(usage))?;
                changed_files.insert(file.clone());
                let owner = owner_qname.unwrap_or_default();
                changed_declarations.insert(join_qname(&owner, &relationship_name));
                vec![instruction]
            }
            Mutation::AddMetadataAnnotation {
                element,
                metadata_type,
                properties,
            } => {
                if metadata_type.trim().is_empty() {
                    return Err(AuthoringError::InvalidMutation(
                        "metadata annotation type must not be empty".to_string(),
                    ));
                }
                let usage = Declaration::Usage(Usage {
                    keyword: "metadata".to_string(),
                    name: metadata_type.clone(),
                    is_implicit_name: false,
                    ty: None,
                    reference_target: None,
                    metadata_properties: properties,
                    multiplicity: None,
                    expression: None,
                    additional_types: Vec::new(),
                    specializes: Vec::new(),
                    subsets: Vec::new(),
                    redefines: Vec::new(),
                    members: Vec::new(),
                    docs: Vec::new(),
                    modifiers: Vec::new(),
                });
                let (file, _, instruction) = self.push_into_container(
                    ContainerSelector::Declaration {
                        qualified_name: element.clone(),
                    },
                    usage,
                )?;
                changed_files.insert(file);
                changed_declarations.insert(element.as_dot_string());
                vec![instruction]
            }
            Mutation::RemoveDeclaration { qualified_name } => {
                let located = self.locate_declaration(&qualified_name)?;
                let file = self
                    .files
                    .get_mut(&located.file)
                    .ok_or_else(|| AuthoringError::MissingFile(located.file.clone()))?;
                match located.parent_qname.clone() {
                    Some(parent_qname) => {
                        let parent = locate_members_mut(
                            &mut file.module,
                            &QualifiedName::parse(&parent_qname),
                        )
                        .ok_or_else(|| AuthoringError::MissingDeclaration(parent_qname.clone()))?;
                        remove_declaration(parent, &qualified_name).ok_or_else(|| {
                            AuthoringError::MissingDeclaration(qualified_name.as_dot_string())
                        })?;
                        changed_files.insert(located.file.clone());
                        vec![RewriteInstruction::ReplaceContainer {
                            file: located.file,
                            anchor_qname: Some(parent_qname.clone()),
                            render_qname: Some(parent_qname),
                        }]
                    }
                    None => {
                        match located.kind {
                            DeclarationKind::Package => {
                                file.module.package = None;
                            }
                            _ => {
                                remove_declaration(&mut file.module.members, &qualified_name)
                                    .ok_or_else(|| {
                                        AuthoringError::MissingDeclaration(
                                            qualified_name.as_dot_string(),
                                        )
                                    })?;
                            }
                        }
                        changed_files.insert(located.file.clone());
                        vec![RewriteInstruction::FullFile { file: located.file }]
                    }
                }
            }
            Mutation::RenameDeclaration {
                qualified_name,
                new_name,
            } => {
                let located = self.locate_declaration(&qualified_name)?;
                let old_qname = qualified_name.as_dot_string();
                let new_qname = if let Some(parent) = &located.parent_qname {
                    join_qname(parent, &new_name)
                } else {
                    new_name.clone()
                };
                let file = self
                    .files
                    .get_mut(&located.file)
                    .ok_or_else(|| AuthoringError::MissingFile(located.file.clone()))?;
                rename_declaration(&mut file.module, &qualified_name, &new_name)?;
                changed_files.insert(located.file.clone());
                changed_declarations.insert(new_qname.clone());
                vec![RewriteInstruction::ReplaceNode {
                    file: located.file,
                    anchor_qname: old_qname,
                    render_qname: new_qname,
                }]
            }
            Mutation::UpdateSpecializations {
                qualified_name,
                specializes,
            } => {
                let located = self.locate_declaration(&qualified_name)?;
                let file = self
                    .files
                    .get_mut(&located.file)
                    .ok_or_else(|| AuthoringError::MissingFile(located.file.clone()))?;
                update_specializations(&mut file.module, &qualified_name, specializes)?;
                changed_files.insert(located.file.clone());
                changed_declarations.insert(qualified_name.as_dot_string());
                vec![RewriteInstruction::ReplaceNode {
                    file: located.file,
                    anchor_qname: qualified_name.as_dot_string(),
                    render_qname: qualified_name.as_dot_string(),
                }]
            }
            Mutation::UpdateUsageType { qualified_name, ty } => {
                let located = self.locate_declaration(&qualified_name)?;
                let file = self
                    .files
                    .get_mut(&located.file)
                    .ok_or_else(|| AuthoringError::MissingFile(located.file.clone()))?;
                update_usage_type(&mut file.module, &qualified_name, ty)?;
                changed_files.insert(located.file.clone());
                changed_declarations.insert(qualified_name.as_dot_string());
                vec![RewriteInstruction::ReplaceNode {
                    file: located.file,
                    anchor_qname: qualified_name.as_dot_string(),
                    render_qname: qualified_name.as_dot_string(),
                }]
            }
            Mutation::SetExpression {
                qualified_name,
                expression,
            } => {
                let located = self.locate_declaration(&qualified_name)?;
                let file = self
                    .files
                    .get_mut(&located.file)
                    .ok_or_else(|| AuthoringError::MissingFile(located.file.clone()))?;
                set_usage_expression(&mut file.module, &qualified_name, expression)?;
                changed_files.insert(located.file.clone());
                changed_declarations.insert(qualified_name.as_dot_string());
                vec![RewriteInstruction::ReplaceNode {
                    file: located.file,
                    anchor_qname: qualified_name.as_dot_string(),
                    render_qname: qualified_name.as_dot_string(),
                }]
            }
            Mutation::MoveDeclaration {
                qualified_name,
                destination,
            } => {
                let located = self.locate_declaration(&qualified_name)?;
                let source_file_path = located.file.clone();
                let moved = {
                    let source_file = self
                        .files
                        .get_mut(&source_file_path)
                        .ok_or_else(|| AuthoringError::MissingFile(source_file_path.clone()))?;
                    extract_declaration(&mut source_file.module, &qualified_name)?
                };
                let source_instruction = match located.parent_qname.clone() {
                    Some(parent_qname) => RewriteInstruction::ReplaceContainer {
                        file: source_file_path.clone(),
                        anchor_qname: Some(parent_qname.clone()),
                        render_qname: Some(parent_qname),
                    },
                    None => RewriteInstruction::FullFile {
                        file: source_file_path.clone(),
                    },
                };
                let (dest_file, _, dest_instruction) =
                    self.push_into_container(destination, moved)?;
                changed_files.insert(source_file_path.clone());
                changed_files.insert(dest_file.clone());
                vec![source_instruction, dest_instruction]
            }
        };
        self.finalize_change(before, changed_files, changed_declarations, rewrite_plan)
    }

    pub fn write_back_mutation(
        &mut self,
        mutation: &MutationResult,
    ) -> Result<WriteBackResult, AuthoringError> {
        let localized = self.try_localized_writeback(mutation);
        let (edited_files, mode, changed_spans, validation) = match localized {
            Ok((files, spans, report)) if report.ok => {
                (files, WriteBackMode::LocalizedPatch, spans, report)
            }
            Ok((_, _, _report)) => {
                let (files, spans) = self.canonical_rewrite(&mutation.changed_files)?;
                let report = self.validate_rendered_files(&files)?;
                if !report.ok {
                    return Err(AuthoringError::Validation(
                        report
                            .message
                            .clone()
                            .unwrap_or_else(|| "write-back validation failed".to_string()),
                    ));
                }
                (files, WriteBackMode::CanonicalRewrite, spans, report)
            }
            Err(_) => {
                let (files, spans) = self.canonical_rewrite(&mutation.changed_files)?;
                let report = self.validate_rendered_files(&files)?;
                if !report.ok {
                    return Err(AuthoringError::Validation(
                        report
                            .message
                            .clone()
                            .unwrap_or_else(|| "write-back validation failed".to_string()),
                    ));
                }
                (files, WriteBackMode::CanonicalRewrite, spans, report)
            }
        };

        self.accept_write_back_files(&edited_files)?;

        Ok(WriteBackResult {
            edited_files,
            mode,
            changed_spans,
            validation,
        })
    }

    pub fn write_back_changed_files(
        &self,
        changed_files: &BTreeSet<String>,
    ) -> Result<WriteBackResult, AuthoringError> {
        let (edited_files, changed_spans) = self.canonical_rewrite(changed_files)?;
        let validation = self.validate_rendered_files(&edited_files)?;
        Ok(WriteBackResult {
            edited_files,
            mode: WriteBackMode::CanonicalRewrite,
            changed_spans,
            validation,
        })
    }

    pub fn write_back_changed_files_and_update(
        &mut self,
        changed_files: &BTreeSet<String>,
    ) -> Result<WriteBackResult, AuthoringError> {
        let write_back = self.write_back_changed_files(changed_files)?;
        self.accept_write_back_files(&write_back.edited_files)?;
        Ok(write_back)
    }

    pub fn validate_rendered_sysml(
        &self,
        rendered: &BTreeMap<String, String>,
    ) -> Result<ValidationReport, AuthoringError> {
        self.validate_rendered_files(rendered)
    }

    fn accept_write_back_files(
        &mut self,
        edited_files: &BTreeMap<String, String>,
    ) -> Result<(), AuthoringError> {
        for (path, content) in edited_files {
            let parsed = parse_sysml(content)?;
            let source_map = FileSourceMap::from_ast(&parsed);
            let file = self.ensure_file_mut(path);
            file.original_text = Some(content.clone());
            file.source_map = Some(source_map);
        }
        Ok(())
    }

    fn try_localized_writeback(
        &self,
        mutation: &MutationResult,
    ) -> Result<
        (
            BTreeMap<String, String>,
            BTreeMap<String, Vec<RenderedSpan>>,
            ValidationReport,
        ),
        AuthoringError,
    > {
        let mut edited = BTreeMap::new();
        let mut changed_spans = BTreeMap::new();
        let instructions_by_file = group_rewrites_by_file(&mutation.rewrite_plan);

        for (file_path, instructions) in instructions_by_file {
            let file = self
                .files
                .get(&file_path)
                .ok_or_else(|| AuthoringError::MissingFile(file_path.clone()))?;
            let original = file.original_text.as_ref().ok_or_else(|| {
                AuthoringError::Unsupported(format!(
                    "localized write-back requires original source text for `{file_path}`"
                ))
            })?;
            let source_map = file.source_map.as_ref().ok_or_else(|| {
                AuthoringError::Unsupported(format!(
                    "localized write-back requires source provenance for `{file_path}`"
                ))
            })?;

            let mut patches = Vec::new();
            let mut spans = Vec::new();
            for instruction in instructions {
                let (span, replacement) = match instruction {
                    RewriteInstruction::FullFile { .. } => {
                        return Err(AuthoringError::Unsupported(
                            "full-file rewrite is not localized".to_string(),
                        ));
                    }
                    RewriteInstruction::ReplaceNode {
                        anchor_qname,
                        render_qname,
                        ..
                    } => {
                        let node = source_map.declarations.get(&anchor_qname).ok_or_else(|| {
                            AuthoringError::Unsupported(format!(
                                "missing source span for `{anchor_qname}`"
                            ))
                        })?;
                        let declaration = render_declaration_at_qname(
                            &file.module,
                            &QualifiedName::parse(&render_qname),
                        )
                        .ok_or_else(|| AuthoringError::MissingDeclaration(render_qname.clone()))?;
                        (
                            node.span.clone(),
                            render_with_indent(&declaration, node.indent),
                        )
                    }
                    RewriteInstruction::ReplaceContainer {
                        anchor_qname,
                        render_qname,
                        ..
                    } => {
                        if let Some(anchor_qname) = anchor_qname {
                            let node = if let Some(package) = &source_map.package {
                                if render_qname.as_deref() == Some(&anchor_qname)
                                    && file.module.package.as_ref().is_some_and(|package_model| {
                                        package_model.name.as_dot_string() == anchor_qname
                                    })
                                {
                                    package
                                } else {
                                    source_map.declarations.get(&anchor_qname).ok_or_else(|| {
                                        AuthoringError::Unsupported(format!(
                                            "missing source span for `{anchor_qname}`"
                                        ))
                                    })?
                                }
                            } else {
                                source_map.declarations.get(&anchor_qname).ok_or_else(|| {
                                    AuthoringError::Unsupported(format!(
                                        "missing source span for `{anchor_qname}`"
                                    ))
                                })?
                            };
                            let replacement = if let Some(render_qname) = render_qname {
                                if file.module.package.as_ref().is_some_and(|package_model| {
                                    package_model.name.as_dot_string() == render_qname
                                }) {
                                    render_with_indent(
                                        &file
                                            .module
                                            .package
                                            .as_ref()
                                            .expect("package exists")
                                            .render(0),
                                        node.indent,
                                    )
                                } else if let Some(declaration) = render_declaration_at_qname(
                                    &file.module,
                                    &QualifiedName::parse(&render_qname),
                                ) {
                                    render_with_indent(&declaration, node.indent)
                                } else {
                                    return Err(AuthoringError::MissingDeclaration(render_qname));
                                }
                            } else {
                                String::new()
                            };
                            (node.span.clone(), replacement)
                        } else {
                            return Err(AuthoringError::Unsupported(
                                "module-level localized replacement is unsupported".to_string(),
                            ));
                        }
                    }
                };
                spans.push(RenderedSpan {
                    start_line: span.start_line,
                    start_col: span.start_col,
                    end_line: span.end_line,
                    end_col: span.end_col,
                });
                patches.push((span_to_offsets(original, &span)?, replacement));
            }

            patches.sort_by(|left, right| right.0.0.cmp(&left.0.0));
            validate_non_overlapping_patches(&patches)?;
            let mut updated = original.clone();
            for ((start, end), replacement) in patches {
                updated.replace_range(start..end, &replacement);
            }
            edited.insert(file_path.clone(), updated);
            changed_spans.insert(file_path, spans);
        }

        let validation = self.validate_rendered_files(&edited)?;
        Ok((edited, changed_spans, validation))
    }

    fn canonical_rewrite(
        &self,
        changed_files: &BTreeSet<String>,
    ) -> Result<
        (
            BTreeMap<String, String>,
            BTreeMap<String, Vec<RenderedSpan>>,
        ),
        AuthoringError,
    > {
        let mut edited = BTreeMap::new();
        let mut spans = BTreeMap::new();
        for file_path in changed_files {
            let file = self
                .files
                .get(file_path)
                .ok_or_else(|| AuthoringError::MissingFile(file_path.clone()))?;
            let rendered = file.module.render();
            let span = rendered_span_for_text(&rendered);
            edited.insert(file_path.clone(), rendered);
            spans.insert(file_path.clone(), vec![span]);
        }
        Ok((edited, spans))
    }

    fn validate_rendered_files(
        &self,
        edited_files: &BTreeMap<String, String>,
    ) -> Result<ValidationReport, AuthoringError> {
        let mut final_texts = self.current_texts();
        for (path, content) in edited_files {
            final_texts.insert(path.clone(), content.clone());
        }

        let expected = self.compile_user_kir()?;
        let actual = compile_user_kir_from_texts(&final_texts)?;
        let expected_norm = normalize_kir(&expected);
        let actual_norm = normalize_kir(&actual);
        let ok = expected_norm == actual_norm;
        Ok(ValidationReport {
            ok,
            expected_element_count: expected.elements.len(),
            actual_element_count: actual.elements.len(),
            message: (!ok)
                .then(|| "rendered SysML does not round-trip to the intended KIR".to_string()),
        })
    }

    fn compile_user_kir(&self) -> Result<KirDocument, AuthoringError> {
        compile_user_kir_from_texts(&self.render_all_files())
    }

    fn render_all_files(&self) -> BTreeMap<String, String> {
        self.files
            .iter()
            .map(|(path, file)| (path.clone(), file.module.render()))
            .collect()
    }

    fn current_texts(&self) -> BTreeMap<String, String> {
        self.files
            .iter()
            .map(|(path, file)| {
                (
                    path.clone(),
                    file.original_text
                        .clone()
                        .unwrap_or_else(|| file.module.render()),
                )
            })
            .collect()
    }

    fn ensure_file_mut(&mut self, path: &str) -> &mut FileModel {
        self.files
            .entry(path.to_string())
            .or_insert_with(|| FileModel {
                path: path.to_string(),
                module: AuthoringModule::default(),
                original_text: None,
                source_map: None,
            })
    }

    fn finalize_change(
        &self,
        before: KirDocument,
        changed_files: BTreeSet<String>,
        changed_declarations: BTreeSet<String>,
        rewrite_plan: Vec<RewriteInstruction>,
    ) -> Result<MutationResult, AuthoringError> {
        let after = self.compile_user_kir()?;
        let affected_element_ids = diff_element_ids(&before, &after);

        Ok(MutationResult {
            changed_files,
            changed_declarations,
            affected_element_ids,
            rewrite_plan,
        })
    }

    fn ensure_attribute_policy(
        &self,
        element: &QualifiedName,
        attribute: &str,
        policy: AttributeWritePolicy,
    ) -> Result<(), AuthoringError> {
        let attributes = self.semantic_attributes(element)?;
        let Some(row) = attributes.iter().find(|row| row.name == attribute) else {
            return Err(AuthoringError::Unsupported(format!(
                "attribute `{attribute}` is not supported on `{}`",
                element.as_dot_string()
            )));
        };
        if matches!(policy, AttributeWritePolicy::DirectOnly) && row.origin_kind != "direct" {
            return Err(AuthoringError::InvalidMutation(format!(
                "attribute `{attribute}` on `{}` is `{}`; use UpsertDirect to create a direct value",
                element.as_dot_string(),
                row.origin_kind
            )));
        }
        Ok(())
    }

    fn qname_list_attribute_values(
        &self,
        element: &QualifiedName,
        attribute: &str,
    ) -> Result<Vec<QualifiedName>, AuthoringError> {
        let attributes = self.semantic_attributes(element)?;
        let row = attributes
            .into_iter()
            .find(|row| row.name == attribute)
            .ok_or_else(|| {
                AuthoringError::Unsupported(format!(
                    "attribute `{attribute}` is not supported on `{}`",
                    element.as_dot_string()
                ))
            })?;
        let values = row
            .direct_value
            .or(row.effective_value)
            .unwrap_or_else(|| Value::Array(Vec::new()));
        value_as_qname_list(&values, attribute)
    }

    fn apply_modifier_flag_edit(
        &mut self,
        element: &QualifiedName,
        attribute: &str,
        modifier: &str,
        enabled: bool,
    ) -> Result<MutationResult, AuthoringError> {
        let before = self.compile_user_kir()?;
        let located = self.locate_declaration(element)?;
        let file = self
            .files
            .get_mut(&located.file)
            .ok_or_else(|| AuthoringError::MissingFile(located.file.clone()))?;
        let declaration = locate_declaration_mut(&mut file.module, element)
            .ok_or_else(|| AuthoringError::MissingDeclaration(element.as_dot_string()))?;
        let changed = match declaration {
            Declaration::Definition(definition) => {
                set_modifier_flag(&mut definition.modifiers, modifier, enabled)
            }
            Declaration::Usage(usage) => set_modifier_flag(&mut usage.modifiers, modifier, enabled),
            other => {
                return Err(AuthoringError::Unsupported(format!(
                    "attribute `{attribute}` is not supported on `{}`",
                    declaration_kind_label(other)
                )));
            }
        };
        if !changed {
            return Err(AuthoringError::InvalidMutation(format!(
                "attribute `{attribute}` on `{}` is already set to `{enabled}`",
                element.as_dot_string()
            )));
        }
        let mut changed_files = BTreeSet::new();
        changed_files.insert(located.file.clone());
        let mut changed_declarations = BTreeSet::new();
        changed_declarations.insert(element.as_dot_string());
        self.finalize_change(
            before,
            changed_files,
            changed_declarations,
            vec![RewriteInstruction::ReplaceNode {
                file: located.file,
                anchor_qname: element.as_dot_string(),
                render_qname: element.as_dot_string(),
            }],
        )
    }

    fn apply_direction_edit(
        &mut self,
        element: &QualifiedName,
        direction: Option<String>,
    ) -> Result<MutationResult, AuthoringError> {
        let before = self.compile_user_kir()?;
        let located = self.locate_declaration(element)?;
        let file = self
            .files
            .get_mut(&located.file)
            .ok_or_else(|| AuthoringError::MissingFile(located.file.clone()))?;
        let declaration = locate_declaration_mut(&mut file.module, element)
            .ok_or_else(|| AuthoringError::MissingDeclaration(element.as_dot_string()))?;
        let usage = match declaration {
            Declaration::Usage(usage) => usage,
            _ => {
                return Err(AuthoringError::Unsupported(format!(
                    "attribute `direction` is only supported on usages"
                )));
            }
        };
        let changed = set_direction(&mut usage.modifiers, direction.as_deref());
        if !changed {
            return Err(AuthoringError::InvalidMutation(format!(
                "direction on `{}` is already `{}`",
                element.as_dot_string(),
                direction.as_deref().unwrap_or("<none>")
            )));
        }
        let mut changed_files = BTreeSet::new();
        changed_files.insert(located.file.clone());
        let mut changed_declarations = BTreeSet::new();
        changed_declarations.insert(element.as_dot_string());
        self.finalize_change(
            before,
            changed_files,
            changed_declarations,
            vec![RewriteInstruction::ReplaceNode {
                file: located.file,
                anchor_qname: element.as_dot_string(),
                render_qname: element.as_dot_string(),
            }],
        )
    }

    fn apply_target_edit(
        &mut self,
        element: &QualifiedName,
        target: Option<QualifiedName>,
    ) -> Result<MutationResult, AuthoringError> {
        let before = self.compile_user_kir()?;
        let located = self.locate_declaration(element)?;
        let file = self
            .files
            .get_mut(&located.file)
            .ok_or_else(|| AuthoringError::MissingFile(located.file.clone()))?;
        let declaration = locate_declaration_mut(&mut file.module, element)
            .ok_or_else(|| AuthoringError::MissingDeclaration(element.as_dot_string()))?;
        let alias = match declaration {
            Declaration::Alias(alias) => alias,
            _ => {
                return Err(AuthoringError::Unsupported(format!(
                    "attribute `target` is only supported on aliases"
                )));
            }
        };
        if alias.target
            == target
                .clone()
                .unwrap_or_else(|| QualifiedName::new(Vec::new()))
        {
            return Err(AuthoringError::InvalidMutation(format!(
                "target on `{}` is already unchanged",
                element.as_dot_string()
            )));
        }
        alias.target = target.ok_or_else(|| {
            AuthoringError::InvalidMutation("alias target cannot be cleared".to_string())
        })?;
        let mut changed_files = BTreeSet::new();
        changed_files.insert(located.file.clone());
        let mut changed_declarations = BTreeSet::new();
        changed_declarations.insert(element.as_dot_string());
        self.finalize_change(
            before,
            changed_files,
            changed_declarations,
            vec![RewriteInstruction::ReplaceNode {
                file: located.file,
                anchor_qname: element.as_dot_string(),
                render_qname: element.as_dot_string(),
            }],
        )
    }

    fn apply_imports_replace(
        &mut self,
        element: &QualifiedName,
        imports: Vec<QualifiedName>,
    ) -> Result<MutationResult, AuthoringError> {
        let before = self.compile_user_kir()?;
        let located = self.locate_declaration(element)?;
        if !matches!(located.kind, DeclarationKind::Package) {
            return Err(AuthoringError::Unsupported(
                "attribute `imports` is only supported on packages".to_string(),
            ));
        }
        let file = self
            .files
            .get_mut(&located.file)
            .ok_or_else(|| AuthoringError::MissingFile(located.file.clone()))?;
        let package = locate_package_mut(&mut file.module, element)
            .ok_or_else(|| AuthoringError::MissingPackage(element.as_dot_string()))?;
        let mut non_imports = package
            .members
            .iter()
            .filter(|member| !matches!(member, Declaration::Import(_)))
            .cloned()
            .collect::<Vec<_>>();
        let mut new_members = imports
            .into_iter()
            .map(|path| {
                Declaration::Import(Import {
                    path,
                    docs: Vec::new(),
                    modifiers: Vec::new(),
                })
            })
            .collect::<Vec<_>>();
        new_members.append(&mut non_imports);
        package.members = new_members;

        let mut changed_files = BTreeSet::new();
        changed_files.insert(located.file.clone());
        let mut changed_declarations = BTreeSet::new();
        changed_declarations.insert(element.as_dot_string());
        self.finalize_change(
            before,
            changed_files,
            changed_declarations,
            vec![RewriteInstruction::ReplaceContainer {
                file: located.file,
                anchor_qname: Some(element.as_dot_string()),
                render_qname: Some(element.as_dot_string()),
            }],
        )
    }

    fn apply_doc_edit(
        &mut self,
        element: &QualifiedName,
        edit: DocEdit,
    ) -> Result<MutationResult, AuthoringError> {
        let before = self.compile_user_kir()?;
        let located = self.locate_declaration(element)?;
        let changed_file = located.file.clone();
        let file = self
            .files
            .get_mut(&located.file)
            .ok_or_else(|| AuthoringError::MissingFile(located.file.clone()))?;

        if matches!(located.kind, DeclarationKind::Package) {
            let package = locate_package_mut(&mut file.module, element)
                .ok_or_else(|| AuthoringError::MissingPackage(element.as_dot_string()))?;
            apply_doc_value_edit(&mut package.docs, edit);
        } else {
            let declaration = locate_declaration_mut(&mut file.module, element)
                .ok_or_else(|| AuthoringError::MissingDeclaration(element.as_dot_string()))?;
            apply_doc_value_edit(declaration_docs_mut(declaration), edit);
        }

        self.finalize_change(
            before,
            BTreeSet::from([changed_file.clone()]),
            BTreeSet::from([element.as_dot_string()]),
            vec![RewriteInstruction::ReplaceContainer {
                file: changed_file,
                anchor_qname: Some(element.as_dot_string()),
                render_qname: Some(element.as_dot_string()),
            }],
        )
    }

    fn locate_declaration(
        &self,
        qualified_name: &QualifiedName,
    ) -> Result<LocatedDeclaration, AuthoringError> {
        let key = qualified_name.as_dot_string();
        let mut found = Vec::new();
        for (file_path, file) in &self.files {
            if let Some((kind, parent)) = locate_declaration_in_module(&file.module, qualified_name)
            {
                found.push(LocatedDeclaration {
                    file: file_path.clone(),
                    kind,
                    parent_qname: parent,
                });
            }
        }
        match found.len() {
            0 => Err(AuthoringError::MissingDeclaration(key)),
            1 => Ok(found.remove(0)),
            _ => Err(AuthoringError::InvalidMutation(format!(
                "ambiguous declaration ownership for `{key}`"
            ))),
        }
    }

    fn push_into_container(
        &mut self,
        container: ContainerSelector,
        declaration: Declaration,
    ) -> Result<(String, Option<String>, RewriteInstruction), AuthoringError> {
        match container {
            ContainerSelector::File { target_file } => {
                let file = self.ensure_file_mut(&target_file);
                file.module.members.push(declaration);
                Ok((
                    target_file.clone(),
                    None,
                    RewriteInstruction::FullFile { file: target_file },
                ))
            }
            ContainerSelector::Package { qualified_name } => {
                let located = self.locate_declaration(&qualified_name).or_else(|_| {
                    self.file_for_package(&qualified_name)
                        .map(|file| LocatedDeclaration {
                            file,
                            kind: DeclarationKind::Package,
                            parent_qname: None,
                        })
                        .ok_or_else(|| {
                            AuthoringError::MissingPackage(qualified_name.as_dot_string())
                        })
                })?;
                let file = self
                    .files
                    .get_mut(&located.file)
                    .ok_or_else(|| AuthoringError::MissingFile(located.file.clone()))?;
                let package =
                    locate_package_mut(&mut file.module, &qualified_name).ok_or_else(|| {
                        AuthoringError::MissingPackage(qualified_name.as_dot_string())
                    })?;
                package.members.push(declaration);
                Ok((
                    located.file.clone(),
                    Some(qualified_name.as_dot_string()),
                    RewriteInstruction::ReplaceContainer {
                        file: located.file,
                        anchor_qname: Some(qualified_name.as_dot_string()),
                        render_qname: Some(qualified_name.as_dot_string()),
                    },
                ))
            }
            ContainerSelector::Declaration { qualified_name } => {
                let located = self.locate_declaration(&qualified_name)?;
                let file = self
                    .files
                    .get_mut(&located.file)
                    .ok_or_else(|| AuthoringError::MissingFile(located.file.clone()))?;
                let members =
                    locate_members_mut(&mut file.module, &qualified_name).ok_or_else(|| {
                        AuthoringError::MissingDeclaration(qualified_name.as_dot_string())
                    })?;
                members.push(declaration);
                Ok((
                    located.file.clone(),
                    Some(qualified_name.as_dot_string()),
                    RewriteInstruction::ReplaceContainer {
                        file: located.file,
                        anchor_qname: Some(qualified_name.as_dot_string()),
                        render_qname: Some(qualified_name.as_dot_string()),
                    },
                ))
            }
        }
    }

    fn file_for_package(&self, package_name: &QualifiedName) -> Option<String> {
        self.files.iter().find_map(|(path, file)| {
            file.module
                .package
                .as_ref()
                .filter(|package| package.name == *package_name)
                .map(|_| path.clone())
        })
    }
}

impl AuthoringModule {
    fn from_ast(module: &SysmlModule) -> Self {
        let members = if module.package.is_some() {
            module
                .members
                .iter()
                .filter(|member| !matches!(member, AstDeclaration::Package(_)))
                .map(Declaration::from_ast)
                .collect()
        } else {
            module.members.iter().map(Declaration::from_ast).collect()
        };
        Self {
            package: module.package.as_ref().map(Package::from_ast),
            members,
        }
    }

    fn render(&self) -> String {
        let mut sections = Vec::new();
        if let Some(package) = &self.package {
            sections.push(package.render(0));
        }
        sections.extend(self.members.iter().map(|member| member.render(0)));
        if sections.is_empty() {
            String::new()
        } else {
            format!("{}\n", sections.join("\n\n"))
        }
    }
}

impl Package {
    fn from_ast(package: &PackageDecl) -> Self {
        Self {
            name: QualifiedName(package.name.segments.clone()),
            members: package.members.iter().map(Declaration::from_ast).collect(),
            docs: package.docs.clone(),
            modifiers: package.modifiers.clone(),
        }
    }

    fn render(&self, indent: usize) -> String {
        let prefix = " ".repeat(indent);
        let mut lines = render_docs(&self.docs, indent);
        let mut header = String::new();
        if !self.modifiers.is_empty() {
            header.push_str(&self.modifiers.join(" "));
            header.push(' ');
        }
        header.push_str("package ");
        header.push_str(&self.name.as_colon_string());
        header.push_str(" {");
        lines.push(format!("{prefix}{header}"));
        if !self.members.is_empty() {
            let body = self
                .members
                .iter()
                .map(|member| member.render(indent + 2))
                .collect::<Vec<_>>()
                .join("\n\n");
            lines.push(body);
        }
        lines.push(format!("{prefix}}}"));
        lines.join("\n")
    }
}

impl Definition {
    fn render(&self, indent: usize) -> String {
        let prefix = " ".repeat(indent);
        let mut lines = render_docs(&self.docs, indent);
        let mut header = render_modifier_prefix(&self.modifiers);
        header.push_str(&self.keyword);
        header.push_str(" def ");
        header.push_str(&self.name);
        if !self.specializes.is_empty() {
            header.push_str(" specializes ");
            header.push_str(
                &self
                    .specializes
                    .iter()
                    .map(QualifiedName::as_colon_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if self.members.is_empty() {
            header.push(';');
            lines.push(format!("{prefix}{header}"));
            return lines.join("\n");
        }

        header.push_str(" {");
        lines.push(format!("{prefix}{header}"));
        let body = self
            .members
            .iter()
            .map(|member| member.render(indent + 2))
            .collect::<Vec<_>>()
            .join("\n\n");
        lines.push(body);
        lines.push(format!("{prefix}}}"));
        lines.join("\n")
    }
}

impl Usage {
    fn render(&self, indent: usize) -> String {
        let prefix = " ".repeat(indent);
        let mut lines = render_docs(&self.docs, indent);
        let mut header = render_modifier_prefix(&self.modifiers);
        if self.keyword == "satisfy" {
            header.push_str("satisfy requirement ");
            header.push_str(&self.name);
            if self.members.is_empty() {
                header.push(';');
                lines.push(format!("{prefix}{header}"));
                return lines.join("\n");
            }
            header.push_str(" {");
            lines.push(format!("{prefix}{header}"));
            let body = self
                .members
                .iter()
                .map(|member| member.render(indent + 2))
                .collect::<Vec<_>>()
                .join("\n\n");
            lines.push(body);
            lines.push(format!("{prefix}}}"));
            return lines.join("\n");
        }
        if self.keyword == "metadata" {
            header.push('@');
            header.push_str(&self.name.replace('.', "::"));
            if self.metadata_properties.is_empty() {
                header.push(';');
                lines.push(format!("{prefix}{header}"));
                return lines.join("\n");
            }
            header.push_str(" {");
            lines.push(format!("{prefix}{header}"));
            for (key, value) in &self.metadata_properties {
                lines.push(format!(
                    "{}  {key} = {};",
                    prefix,
                    render_metadata_property_value(value)
                ));
            }
            lines.push(format!("{prefix}}}"));
            return lines.join("\n");
        }
        header.push_str(&self.keyword);
        header.push(' ');
        if !self.is_implicit_name {
            header.push_str(&self.name);
        }
        if let Some(ty) = &self.ty {
            if !self.is_implicit_name {
                header.push_str(": ");
            } else {
                header.push_str(": ");
            }
            header.push_str(&ty.as_colon_string());
        }
        if let Some(multiplicity) = &self.multiplicity {
            header.push('[');
            header.push_str(&multiplicity.raw);
            header.push(']');
        }
        if !self.additional_types.is_empty() {
            header.push_str(" :> ");
            header.push_str(
                &self
                    .additional_types
                    .iter()
                    .map(QualifiedName::as_colon_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if !self.specializes.is_empty() {
            header.push_str(" specializes ");
            header.push_str(
                &self
                    .specializes
                    .iter()
                    .map(QualifiedName::as_colon_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if !self.subsets.is_empty() {
            header.push_str(" subsets ");
            header.push_str(
                &self
                    .subsets
                    .iter()
                    .map(QualifiedName::as_colon_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if !self.redefines.is_empty() {
            header.push_str(" redefines ");
            header.push_str(
                &self
                    .redefines
                    .iter()
                    .map(QualifiedName::as_colon_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if let Some(reference_target) = &self.reference_target {
            header.push_str(" references ");
            header.push_str(&reference_target.as_dot_string());
        }
        if let Some(expression) = &self.expression {
            header.push_str(" = ");
            header.push_str(expression);
        }
        if self.members.is_empty() {
            header.push(';');
            lines.push(format!("{prefix}{header}"));
            return lines.join("\n");
        }

        header.push_str(" {");
        lines.push(format!("{prefix}{header}"));
        let body = self
            .members
            .iter()
            .map(|member| member.render(indent + 2))
            .collect::<Vec<_>>()
            .join("\n\n");
        lines.push(body);
        lines.push(format!("{prefix}}}"));
        lines.join("\n")
    }
}

impl Alias {
    fn render(&self, indent: usize) -> String {
        let prefix = " ".repeat(indent);
        let mut lines = render_docs(&self.docs, indent);
        let mut header = render_modifier_prefix(&self.modifiers);
        header.push_str("alias ");
        header.push_str(&self.name);
        header.push_str(" = ");
        header.push_str(&self.target.as_colon_string());
        header.push(';');
        lines.push(format!("{prefix}{header}"));
        lines.join("\n")
    }
}

impl Declaration {
    fn from_ast(declaration: &AstDeclaration) -> Self {
        match declaration {
            AstDeclaration::Package(package) => Self::Package(Package::from_ast(package)),
            AstDeclaration::Import(import) => Self::Import(Import {
                path: QualifiedName(import.path.segments.clone()),
                docs: import.docs.clone(),
                modifiers: import.modifiers.clone(),
            }),
            AstDeclaration::PartDefinition(definition) => Self::Definition(Definition {
                keyword: "part".to_string(),
                name: definition.name.clone(),
                specializes: definition
                    .specializes
                    .iter()
                    .map(|name| QualifiedName(name.segments.clone()))
                    .collect(),
                members: definition
                    .members
                    .iter()
                    .map(Declaration::from_ast)
                    .collect(),
                docs: definition.docs.clone(),
                modifiers: definition.modifiers.clone(),
            }),
            AstDeclaration::PartUsage(usage) => Self::Usage(Usage {
                keyword: "part".to_string(),
                name: usage.name.clone(),
                is_implicit_name: usage.is_implicit_name,
                ty: usage
                    .ty
                    .as_ref()
                    .map(|ty| QualifiedName(ty.segments.clone())),
                reference_target: None,
                metadata_properties: BTreeMap::new(),
                multiplicity: usage.multiplicity.clone(),
                expression: usage.expression.as_ref().map(render_expr),
                additional_types: usage
                    .additional_types
                    .iter()
                    .map(|name| QualifiedName(name.segments.clone()))
                    .collect(),
                specializes: usage
                    .specializes
                    .iter()
                    .map(|name| QualifiedName(name.segments.clone()))
                    .collect(),
                subsets: usage
                    .subsets
                    .iter()
                    .map(|name| QualifiedName(name.segments.clone()))
                    .collect(),
                redefines: usage
                    .redefines
                    .iter()
                    .map(|name| QualifiedName(name.segments.clone()))
                    .collect(),
                members: usage
                    .body_members
                    .iter()
                    .map(Declaration::from_ast)
                    .collect(),
                docs: usage.docs.clone(),
                modifiers: usage.modifiers.clone(),
            }),
            AstDeclaration::GenericDefinition(definition) => Self::Definition(Definition {
                keyword: definition.keyword.clone(),
                name: definition.name.clone(),
                specializes: definition
                    .specializes
                    .iter()
                    .map(|name| QualifiedName(name.segments.clone()))
                    .collect(),
                members: definition
                    .members
                    .iter()
                    .map(Declaration::from_ast)
                    .collect(),
                docs: definition.docs.clone(),
                modifiers: definition.modifiers.clone(),
            }),
            AstDeclaration::GenericUsage(usage) => Self::Usage(Usage {
                keyword: usage.keyword.clone(),
                name: usage.name.clone(),
                is_implicit_name: usage.is_implicit_name,
                ty: usage
                    .ty
                    .as_ref()
                    .map(|ty| QualifiedName(ty.segments.clone())),
                reference_target: usage
                    .reference_target
                    .as_ref()
                    .map(|target| QualifiedName(target.segments.clone())),
                metadata_properties: usage.metadata_properties.clone(),
                multiplicity: usage.multiplicity.clone(),
                expression: usage.expression.as_ref().map(render_expr),
                additional_types: usage
                    .additional_types
                    .iter()
                    .map(|name| QualifiedName(name.segments.clone()))
                    .collect(),
                specializes: usage
                    .specializes
                    .iter()
                    .map(|name| QualifiedName(name.segments.clone()))
                    .collect(),
                subsets: usage
                    .subsets
                    .iter()
                    .map(|name| QualifiedName(name.segments.clone()))
                    .collect(),
                redefines: usage
                    .redefines
                    .iter()
                    .map(|name| QualifiedName(name.segments.clone()))
                    .collect(),
                members: usage
                    .body_members
                    .iter()
                    .map(Declaration::from_ast)
                    .collect(),
                docs: usage.docs.clone(),
                modifiers: usage.modifiers.clone(),
            }),
            AstDeclaration::Alias(alias) => Self::Alias(Alias {
                name: alias.name.clone(),
                target: QualifiedName(alias.target.segments.clone()),
                docs: alias.docs.clone(),
                modifiers: alias.modifiers.clone(),
            }),
        }
    }

    fn render(&self, indent: usize) -> String {
        match self {
            Self::Package(package) => package.render(indent),
            Self::Import(import) => {
                let prefix = " ".repeat(indent);
                let mut lines = render_docs(&import.docs, indent);
                let mut header = render_modifier_prefix(&import.modifiers);
                header.push_str("import ");
                header.push_str(&import.path.as_colon_string());
                header.push(';');
                lines.push(format!("{prefix}{header}"));
                lines.join("\n")
            }
            Self::Definition(definition) => definition.render(indent),
            Self::Usage(usage) => usage.render(indent),
            Self::Alias(alias) => alias.render(indent),
        }
    }
}

impl FileSourceMap {
    fn from_ast(module: &SysmlModule) -> Self {
        let mut map = Self {
            package: module.package.as_ref().map(|package| SourceNode {
                span: package.span.clone(),
                indent: package.span.start_col.saturating_sub(1),
            }),
            declarations: BTreeMap::new(),
        };
        if let Some(package) = &module.package {
            collect_source_nodes(
                &package.members,
                &package.name.segments.join("."),
                Some(&package.name.segments.join(".")),
                &mut map.declarations,
            );
        }
        if module.package.is_none() {
            collect_source_nodes(&module.members, "", None, &mut map.declarations);
        }
        map
    }
}

fn collect_source_nodes(
    declarations: &[AstDeclaration],
    owner: &str,
    parent_qname: Option<&str>,
    nodes: &mut BTreeMap<String, SourceNode>,
) {
    for declaration in declarations {
        match declaration {
            AstDeclaration::Package(package) => {
                let qname = qualify_name(owner, &package.name.segments.join("."));
                nodes.insert(
                    qname.clone(),
                    SourceNode {
                        span: package.span.clone(),
                        indent: package.span.start_col.saturating_sub(1),
                    },
                );
                collect_source_nodes(&package.members, &qname, Some(&qname), nodes);
            }
            AstDeclaration::Import(_) => {}
            AstDeclaration::PartDefinition(definition) => {
                let qname = qualify_name(owner, &definition.name);
                nodes.insert(
                    qname.clone(),
                    SourceNode {
                        span: definition.span.clone(),
                        indent: definition.span.start_col.saturating_sub(1),
                    },
                );
                collect_source_nodes(
                    &definition.members,
                    &qname,
                    Some(parent_or_self(parent_qname, &qname)),
                    nodes,
                );
            }
            AstDeclaration::PartUsage(usage) => {
                let qname = qualify_name(owner, &usage.name);
                nodes.insert(
                    qname.clone(),
                    SourceNode {
                        span: usage.span.clone(),
                        indent: usage.span.start_col.saturating_sub(1),
                    },
                );
                collect_source_nodes(
                    &usage.body_members,
                    &qname,
                    Some(parent_or_self(parent_qname, &qname)),
                    nodes,
                );
            }
            AstDeclaration::GenericDefinition(definition) => {
                let qname = qualify_name(owner, &definition.name);
                nodes.insert(
                    qname.clone(),
                    SourceNode {
                        span: definition.span.clone(),
                        indent: definition.span.start_col.saturating_sub(1),
                    },
                );
                collect_source_nodes(
                    &definition.members,
                    &qname,
                    Some(parent_or_self(parent_qname, &qname)),
                    nodes,
                );
            }
            AstDeclaration::GenericUsage(usage) => {
                let qname = qualify_name(owner, &usage.name);
                nodes.insert(
                    qname.clone(),
                    SourceNode {
                        span: usage.span.clone(),
                        indent: usage.span.start_col.saturating_sub(1),
                    },
                );
                collect_source_nodes(
                    &usage.body_members,
                    &qname,
                    Some(parent_or_self(parent_qname, &qname)),
                    nodes,
                );
            }
            AstDeclaration::Alias(alias) => {
                let qname = qualify_name(owner, &alias.name);
                nodes.insert(
                    qname,
                    SourceNode {
                        span: alias.span.clone(),
                        indent: alias.span.start_col.saturating_sub(1),
                    },
                );
            }
        }
    }
}

fn parent_or_self<'a>(parent_qname: Option<&'a str>, fallback: &'a str) -> &'a str {
    parent_qname.unwrap_or(fallback)
}

fn locate_declaration_in_module(
    module: &AuthoringModule,
    qualified_name: &QualifiedName,
) -> Option<(DeclarationKind, Option<String>)> {
    if module
        .package
        .as_ref()
        .is_some_and(|package| package.name == *qualified_name)
    {
        return Some((DeclarationKind::Package, None));
    }
    if let Some(package) = &module.package
        && let Some(result) = locate_declaration_in_members(
            &package.members,
            &package.name.as_dot_string(),
            qualified_name,
        )
    {
        return Some(result);
    }
    locate_declaration_in_members(&module.members, "", qualified_name)
}

fn locate_declaration_in_members(
    declarations: &[Declaration],
    owner: &str,
    qualified_name: &QualifiedName,
) -> Option<(DeclarationKind, Option<String>)> {
    for declaration in declarations {
        match declaration {
            Declaration::Package(package) => {
                let qname = qualify_name(owner, &package.name.as_dot_string());
                if qname == qualified_name.as_dot_string() {
                    return Some((
                        DeclarationKind::Package,
                        (!owner.is_empty()).then(|| owner.to_string()),
                    ));
                }
                if let Some(result) =
                    locate_declaration_in_members(&package.members, &qname, qualified_name)
                {
                    return Some(result);
                }
            }
            Declaration::Definition(definition) => {
                let qname = qualify_name(owner, &definition.name);
                if qname == qualified_name.as_dot_string() {
                    return Some((
                        DeclarationKind::Definition,
                        (!owner.is_empty()).then(|| owner.to_string()),
                    ));
                }
                if let Some(result) =
                    locate_declaration_in_members(&definition.members, &qname, qualified_name)
                {
                    return Some(result);
                }
            }
            Declaration::Usage(usage) => {
                let qname = qualify_name(owner, &usage.name);
                if qname == qualified_name.as_dot_string() {
                    return Some((
                        DeclarationKind::Usage,
                        (!owner.is_empty()).then(|| owner.to_string()),
                    ));
                }
                if let Some(result) =
                    locate_declaration_in_members(&usage.members, &qname, qualified_name)
                {
                    return Some(result);
                }
            }
            Declaration::Alias(alias) => {
                let qname = qualify_name(owner, &alias.name);
                if qname == qualified_name.as_dot_string() {
                    return Some((
                        DeclarationKind::Alias,
                        (!owner.is_empty()).then(|| owner.to_string()),
                    ));
                }
            }
            Declaration::Import(_) => {}
        }
    }
    None
}

fn locate_package_mut<'a>(
    module: &'a mut AuthoringModule,
    qualified_name: &QualifiedName,
) -> Option<&'a mut Package> {
    if module
        .package
        .as_ref()
        .is_some_and(|package| package.name == *qualified_name)
    {
        return module.package.as_mut();
    }
    if let Some(package) = &mut module.package {
        return locate_package_in_members_mut(
            &mut package.members,
            &package.name.as_dot_string(),
            qualified_name,
        );
    }
    locate_package_in_members_mut(&mut module.members, "", qualified_name)
}

fn locate_package_ref<'a>(
    module: &'a AuthoringModule,
    qualified_name: &QualifiedName,
) -> Option<&'a Package> {
    if module
        .package
        .as_ref()
        .is_some_and(|package| package.name == *qualified_name)
    {
        return module.package.as_ref();
    }
    if let Some(package) = &module.package
        && let Some(found) = locate_package_in_members_ref(
            &package.members,
            &package.name.as_dot_string(),
            qualified_name,
        )
    {
        return Some(found);
    }
    locate_package_in_members_ref(&module.members, "", qualified_name)
}

fn locate_package_in_members_mut<'a>(
    declarations: &'a mut [Declaration],
    owner: &str,
    qualified_name: &QualifiedName,
) -> Option<&'a mut Package> {
    for declaration in declarations {
        if let Declaration::Package(package) = declaration {
            let qname = qualify_name(owner, &package.name.as_dot_string());
            if qname == qualified_name.as_dot_string() {
                return Some(package);
            }
            if let Some(found) =
                locate_package_in_members_mut(&mut package.members, &qname, qualified_name)
            {
                return Some(found);
            }
        }
    }
    None
}

fn locate_package_in_members_ref<'a>(
    declarations: &'a [Declaration],
    owner: &str,
    qualified_name: &QualifiedName,
) -> Option<&'a Package> {
    for declaration in declarations {
        if let Declaration::Package(package) = declaration {
            let qname = qualify_name(owner, &package.name.as_dot_string());
            if qname == qualified_name.as_dot_string() {
                return Some(package);
            }
            if let Some(found) =
                locate_package_in_members_ref(&package.members, &qname, qualified_name)
            {
                return Some(found);
            }
        }
    }
    None
}

fn locate_members_mut<'a>(
    module: &'a mut AuthoringModule,
    qualified_name: &QualifiedName,
) -> Option<&'a mut Vec<Declaration>> {
    if module
        .package
        .as_ref()
        .is_some_and(|package| package.name == *qualified_name)
    {
        return module.package.as_mut().map(|package| &mut package.members);
    }
    if let Some(package) = &mut module.package
        && let Some(found) = locate_members_in_declarations_mut(
            &mut package.members,
            &package.name.as_dot_string(),
            qualified_name,
        )
    {
        return Some(found);
    }
    locate_members_in_declarations_mut(&mut module.members, "", qualified_name)
}

fn locate_members_in_declarations_mut<'a>(
    declarations: &'a mut [Declaration],
    owner: &str,
    qualified_name: &QualifiedName,
) -> Option<&'a mut Vec<Declaration>> {
    for declaration in declarations {
        match declaration {
            Declaration::Package(package) => {
                let qname = qualify_name(owner, &package.name.as_dot_string());
                if qname == qualified_name.as_dot_string() {
                    return Some(&mut package.members);
                }
                if let Some(found) =
                    locate_members_in_declarations_mut(&mut package.members, &qname, qualified_name)
                {
                    return Some(found);
                }
            }
            Declaration::Definition(definition) => {
                let qname = qualify_name(owner, &definition.name);
                if qname == qualified_name.as_dot_string() {
                    return Some(&mut definition.members);
                }
                if let Some(found) = locate_members_in_declarations_mut(
                    &mut definition.members,
                    &qname,
                    qualified_name,
                ) {
                    return Some(found);
                }
            }
            Declaration::Usage(usage) => {
                let qname = qualify_name(owner, &usage.name);
                if qname == qualified_name.as_dot_string() {
                    return Some(&mut usage.members);
                }
                if let Some(found) =
                    locate_members_in_declarations_mut(&mut usage.members, &qname, qualified_name)
                {
                    return Some(found);
                }
            }
            Declaration::Import(_) | Declaration::Alias(_) => {}
        }
    }
    None
}

fn locate_declaration_ref<'a>(
    module: &'a AuthoringModule,
    qualified_name: &QualifiedName,
) -> Option<&'a Declaration> {
    if let Some(package) = &module.package
        && let Some(found) = locate_declaration_in_members_ref(
            &package.members,
            &package.name.as_dot_string(),
            qualified_name,
        )
    {
        return Some(found);
    }
    locate_declaration_in_members_ref(&module.members, "", qualified_name)
}

fn locate_declaration_in_members_ref<'a>(
    declarations: &'a [Declaration],
    owner: &str,
    qualified_name: &QualifiedName,
) -> Option<&'a Declaration> {
    for declaration in declarations {
        let qname = match declaration {
            Declaration::Package(package) => qualify_name(owner, &package.name.as_dot_string()),
            Declaration::Definition(definition) => qualify_name(owner, &definition.name),
            Declaration::Usage(usage) => qualify_name(owner, &usage.name),
            Declaration::Alias(alias) => qualify_name(owner, &alias.name),
            Declaration::Import(_) => continue,
        };
        if qname == qualified_name.as_dot_string() {
            return Some(declaration);
        }
        let nested = match declaration {
            Declaration::Package(package) => {
                locate_declaration_in_members_ref(&package.members, &qname, qualified_name)
            }
            Declaration::Definition(definition) => {
                locate_declaration_in_members_ref(&definition.members, &qname, qualified_name)
            }
            Declaration::Usage(usage) => {
                locate_declaration_in_members_ref(&usage.members, &qname, qualified_name)
            }
            Declaration::Alias(_) | Declaration::Import(_) => None,
        };
        if let Some(found) = nested {
            return Some(found);
        }
    }
    None
}

fn locate_declaration_mut<'a>(
    module: &'a mut AuthoringModule,
    qualified_name: &QualifiedName,
) -> Option<&'a mut Declaration> {
    if let Some(package_owner) = module
        .package
        .as_ref()
        .map(|package| package.name.as_dot_string())
        && let Some(package) = module.package.as_mut()
        && let Some(found) =
            locate_declaration_in_members_mut(&mut package.members, &package_owner, qualified_name)
    {
        return Some(found);
    }
    locate_declaration_in_members_mut(&mut module.members, "", qualified_name)
}

fn locate_declaration_in_members_mut<'a>(
    declarations: &'a mut [Declaration],
    owner: &str,
    qualified_name: &QualifiedName,
) -> Option<&'a mut Declaration> {
    for declaration in declarations {
        let qname = match declaration {
            Declaration::Package(package) => qualify_name(owner, &package.name.as_dot_string()),
            Declaration::Definition(definition) => qualify_name(owner, &definition.name),
            Declaration::Usage(usage) => qualify_name(owner, &usage.name),
            Declaration::Alias(alias) => qualify_name(owner, &alias.name),
            Declaration::Import(_) => continue,
        };
        if qname == qualified_name.as_dot_string() {
            return Some(declaration);
        }
        let nested = match declaration {
            Declaration::Package(package) => {
                locate_declaration_in_members_mut(&mut package.members, &qname, qualified_name)
            }
            Declaration::Definition(definition) => {
                locate_declaration_in_members_mut(&mut definition.members, &qname, qualified_name)
            }
            Declaration::Usage(usage) => {
                locate_declaration_in_members_mut(&mut usage.members, &qname, qualified_name)
            }
            Declaration::Alias(_) | Declaration::Import(_) => None,
        };
        if nested.is_some() {
            return nested;
        }
    }
    None
}

fn semantic_attributes_for_package(package: &Package) -> Vec<SemanticAttribute> {
    vec![
        SemanticAttribute {
            name: "declared_name".to_string(),
            origin_kind: "direct".to_string(),
            direct_value: Some(Value::String(
                package.name.tail().unwrap_or_default().to_string(),
            )),
            effective_value: Some(Value::String(
                package.name.tail().unwrap_or_default().to_string(),
            )),
        },
        SemanticAttribute {
            name: "imports".to_string(),
            origin_kind: if package
                .members
                .iter()
                .any(|member| matches!(member, Declaration::Import(_)))
            {
                "direct".to_string()
            } else {
                "declared".to_string()
            },
            direct_value: Some(Value::Array(
                package
                    .members
                    .iter()
                    .filter_map(|member| match member {
                        Declaration::Import(import) => {
                            Some(Value::String(import.path.as_colon_string()))
                        }
                        _ => None,
                    })
                    .collect(),
            )),
            effective_value: Some(Value::Array(
                package
                    .members
                    .iter()
                    .filter_map(|member| match member {
                        Declaration::Import(import) => {
                            Some(Value::String(import.path.as_colon_string()))
                        }
                        _ => None,
                    })
                    .collect(),
            )),
        },
    ]
}

fn semantic_attributes_for_declaration(declaration: &Declaration) -> Vec<SemanticAttribute> {
    match declaration {
        Declaration::Package(package) => semantic_attributes_for_package(package),
        Declaration::Definition(definition) => vec![
            semantic_scalar_attribute(
                "declared_name",
                Some(Value::String(definition.name.clone())),
                true,
            ),
            semantic_doc_attribute("doc", &definition.docs),
            semantic_doc_attribute(
                "text",
                &requirement_text_from_docs(&definition.docs)
                    .into_iter()
                    .collect::<Vec<_>>(),
            ),
            semantic_doc_attribute(
                "id",
                &requirement_id_from_docs(&definition.docs)
                    .into_iter()
                    .collect::<Vec<_>>(),
            ),
            semantic_list_attribute("specializes", &definition.specializes),
            semantic_scalar_attribute(
                "is_abstract",
                Some(Value::Bool(
                    definition
                        .modifiers
                        .iter()
                        .any(|modifier| modifier == "abstract"),
                )),
                definition
                    .modifiers
                    .iter()
                    .any(|modifier| modifier == "abstract"),
            ),
        ],
        Declaration::Usage(usage) => vec![
            semantic_scalar_attribute(
                "declared_name",
                Some(Value::String(usage.name.clone())),
                true,
            ),
            semantic_doc_attribute("doc", &usage.docs),
            semantic_doc_attribute(
                "text",
                &requirement_text_from_docs(&usage.docs)
                    .into_iter()
                    .collect::<Vec<_>>(),
            ),
            semantic_doc_attribute(
                "id",
                &requirement_id_from_docs(&usage.docs)
                    .into_iter()
                    .collect::<Vec<_>>(),
            ),
            SemanticAttribute {
                name: "type".to_string(),
                origin_kind: if usage.ty.is_some() {
                    "direct".to_string()
                } else {
                    "declared".to_string()
                },
                direct_value: usage
                    .ty
                    .as_ref()
                    .map(|value| Value::String(value.as_colon_string())),
                effective_value: usage
                    .ty
                    .as_ref()
                    .map(|value| Value::String(value.as_colon_string())),
            },
            semantic_scalar_attribute(
                "multiplicity",
                usage
                    .multiplicity
                    .as_ref()
                    .map(|multiplicity| Value::String(multiplicity.raw.clone())),
                usage.multiplicity.is_some(),
            ),
            semantic_list_attribute("specializes", &usage.specializes),
            semantic_scalar_attribute(
                "is_abstract",
                Some(Value::Bool(
                    usage
                        .modifiers
                        .iter()
                        .any(|modifier| modifier == "abstract"),
                )),
                usage
                    .modifiers
                    .iter()
                    .any(|modifier| modifier == "abstract"),
            ),
            semantic_scalar_attribute(
                "is_end",
                Some(Value::Bool(
                    usage.modifiers.iter().any(|modifier| modifier == "end"),
                )),
                usage.modifiers.iter().any(|modifier| modifier == "end"),
            ),
            SemanticAttribute {
                name: "direction".to_string(),
                origin_kind: usage
                    .modifiers
                    .iter()
                    .find(|modifier| matches!(modifier.as_str(), "in" | "out" | "inout"))
                    .map(|_| "direct".to_string())
                    .unwrap_or_else(|| "declared".to_string()),
                direct_value: usage
                    .modifiers
                    .iter()
                    .find(|modifier| matches!(modifier.as_str(), "in" | "out" | "inout"))
                    .map(|value| Value::String(value.clone())),
                effective_value: usage
                    .modifiers
                    .iter()
                    .find(|modifier| matches!(modifier.as_str(), "in" | "out" | "inout"))
                    .map(|value| Value::String(value.clone())),
            },
        ],
        Declaration::Alias(alias) => vec![
            semantic_scalar_attribute(
                "declared_name",
                Some(Value::String(alias.name.clone())),
                true,
            ),
            semantic_scalar_attribute(
                "target",
                Some(Value::String(alias.target.as_colon_string())),
                true,
            ),
        ],
        Declaration::Import(import) => vec![semantic_scalar_attribute(
            "imports",
            Some(Value::String(import.path.as_colon_string())),
            true,
        )],
    }
}

fn semantic_doc_attribute(name: &str, docs: &[String]) -> SemanticAttribute {
    let value = match docs {
        [] => None,
        [single] => Some(Value::String(single.clone())),
        many => Some(Value::Array(
            many.iter().map(|doc| Value::String(doc.clone())).collect(),
        )),
    };
    SemanticAttribute {
        name: name.to_string(),
        origin_kind: if value.is_some() {
            "direct".to_string()
        } else {
            "declared".to_string()
        },
        direct_value: value.clone(),
        effective_value: value,
    }
}

fn semantic_scalar_attribute(
    name: &str,
    value: Option<Value>,
    is_direct: bool,
) -> SemanticAttribute {
    SemanticAttribute {
        name: name.to_string(),
        origin_kind: if is_direct {
            "direct".to_string()
        } else {
            "declared".to_string()
        },
        direct_value: value.clone().filter(|_| is_direct),
        effective_value: value,
    }
}

fn semantic_list_attribute(name: &str, values: &[QualifiedName]) -> SemanticAttribute {
    SemanticAttribute {
        name: name.to_string(),
        origin_kind: if values.is_empty() {
            "declared".to_string()
        } else {
            "direct".to_string()
        },
        direct_value: Some(Value::Array(
            values
                .iter()
                .map(|value| Value::String(value.as_colon_string()))
                .collect(),
        )),
        effective_value: Some(Value::Array(
            values
                .iter()
                .map(|value| Value::String(value.as_colon_string()))
                .collect(),
        )),
    }
}

fn multiplicity_range_from_raw(raw: &str) -> MultiplicityRange {
    let (lower, upper) = raw
        .split_once("..")
        .map(|(lower, upper)| (lower.to_string(), upper.to_string()))
        .unwrap_or_else(|| (raw.to_string(), raw.to_string()));
    MultiplicityRange {
        lower,
        upper,
        raw: raw.to_string(),
        span: SourceSpan {
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 0,
        },
    }
}

fn normalize_attribute_name(name: &str) -> String {
    match name {
        "ownedFeature" => "features".to_string(),
        "ownedMember" => "members".to_string(),
        "ownedSpecialization" => "specializes".to_string(),
        "documentation" => "doc".to_string(),
        "requirementId" => "requirement_id".to_string(),
        "requirement_id" => "requirement_id".to_string(),
        "id" => "id".to_string(),
        "text" => "text".to_string(),
        "declaredName" => "declared_name".to_string(),
        "declaredShortName" => "declared_short_name".to_string(),
        "shortName" => "short_name".to_string(),
        "isAbstract" => "is_abstract".to_string(),
        "isDerived" => "is_derived".to_string(),
        "isEnd" => "is_end".to_string(),
        "isOrdered" => "is_ordered".to_string(),
        "isUnique" => "is_unique".to_string(),
        "isVariable" => "is_variable".to_string(),
        "featuringType" => "featuring_type".to_string(),
        "imports" => "imports".to_string(),
        "type" => "type".to_string(),
        "target" => "target".to_string(),
        "direction" => "direction".to_string(),
        "name" => "declared_name".to_string(),
        other => {
            let mut result = String::with_capacity(other.len());
            for (index, ch) in other.chars().enumerate() {
                if ch.is_ascii_uppercase() && index > 0 {
                    result.push('_');
                }
                result.push(ch.to_ascii_lowercase());
            }
            result
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DocEdit {
    Id(String),
    Text(String),
    ClearId,
    ClearText,
}

fn declaration_docs_mut(declaration: &mut Declaration) -> &mut Vec<String> {
    match declaration {
        Declaration::Package(package) => &mut package.docs,
        Declaration::Import(import) => &mut import.docs,
        Declaration::Definition(definition) => &mut definition.docs,
        Declaration::Usage(usage) => &mut usage.docs,
        Declaration::Alias(alias) => &mut alias.docs,
    }
}

fn apply_doc_value_edit(docs: &mut Vec<String>, edit: DocEdit) {
    match edit {
        DocEdit::Id(value) => {
            docs.retain(|doc| !is_requirement_id_doc(doc));
            let value = value.trim();
            if !value.is_empty() {
                docs.insert(0, format!("id: {value}"));
            }
        }
        DocEdit::Text(value) => {
            docs.retain(|doc| is_requirement_id_doc(doc));
            let value = value.trim();
            if !value.is_empty() {
                docs.push(value.to_string());
            }
        }
        DocEdit::ClearId => docs.retain(|doc| !is_requirement_id_doc(doc)),
        DocEdit::ClearText => docs.retain(|doc| is_requirement_id_doc(doc)),
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

fn value_as_string(value: &Value, attribute: &str) -> Result<String, AuthoringError> {
    value.as_str().map(str::to_string).ok_or_else(|| {
        AuthoringError::InvalidMutation(format!("attribute `{attribute}` expects a string value"))
    })
}

fn value_as_qname(value: &Value, attribute: &str) -> Result<QualifiedName, AuthoringError> {
    Ok(QualifiedName::parse(&value_as_string(value, attribute)?))
}

fn value_as_qname_list(
    value: &Value,
    attribute: &str,
) -> Result<Vec<QualifiedName>, AuthoringError> {
    match value {
        Value::String(_) => Ok(vec![value_as_qname(value, attribute)?]),
        Value::Array(items) => items
            .iter()
            .map(|item| value_as_qname(item, attribute))
            .collect(),
        _ => Err(AuthoringError::InvalidMutation(format!(
            "attribute `{attribute}` expects a string or string array"
        ))),
    }
}

fn value_as_bool(value: &Value, attribute: &str) -> Result<bool, AuthoringError> {
    value.as_bool().ok_or_else(|| {
        AuthoringError::InvalidMutation(format!("attribute `{attribute}` expects a boolean value"))
    })
}

fn value_as_direction(value: &Value) -> Result<String, AuthoringError> {
    let value = value_as_string(value, "direction")?;
    match value.as_str() {
        "in" | "out" | "inout" => Ok(value),
        _ => Err(AuthoringError::InvalidMutation(
            "direction must be one of `in`, `out`, or `inout`".to_string(),
        )),
    }
}

fn set_modifier_flag(modifiers: &mut Vec<String>, modifier: &str, enabled: bool) -> bool {
    let had_modifier = modifiers.iter().any(|existing| existing == modifier);
    if enabled {
        if had_modifier {
            return false;
        }
        modifiers.push(modifier.to_string());
        true
    } else if had_modifier {
        modifiers.retain(|existing| existing != modifier);
        true
    } else {
        false
    }
}

fn set_direction(modifiers: &mut Vec<String>, direction: Option<&str>) -> bool {
    let current = modifiers
        .iter()
        .find(|modifier| matches!(modifier.as_str(), "in" | "out" | "inout"))
        .cloned();
    if current.as_deref() == direction {
        return false;
    }
    modifiers.retain(|modifier| !matches!(modifier.as_str(), "in" | "out" | "inout"));
    if let Some(direction) = direction {
        modifiers.push(direction.to_string());
    }
    true
}

fn declaration_kind_label(declaration: &Declaration) -> &'static str {
    match declaration {
        Declaration::Package(_) => "package",
        Declaration::Import(_) => "import",
        Declaration::Definition(_) => "definition",
        Declaration::Usage(_) => "usage",
        Declaration::Alias(_) => "alias",
    }
}

fn remove_import(declarations: &mut Vec<Declaration>, path: &QualifiedName) -> bool {
    let original = declarations.len();
    declarations.retain(
        |declaration| !matches!(declaration, Declaration::Import(import) if import.path == *path),
    );
    original != declarations.len()
}

fn remove_declaration(
    declarations: &mut Vec<Declaration>,
    qualified_name: &QualifiedName,
) -> Option<Declaration> {
    let mut index = None;
    for (idx, declaration) in declarations.iter().enumerate() {
        if declaration_name(declaration) == Some(qualified_name.tail().unwrap_or_default())
            && declaration_matches_qname(declaration, "", qualified_name)
        {
            index = Some(idx);
            break;
        }
    }
    index.map(|idx| declarations.remove(idx))
}

fn declaration_name(declaration: &Declaration) -> Option<&str> {
    match declaration {
        Declaration::Package(package) => package.name.tail(),
        Declaration::Definition(definition) => Some(definition.name.as_str()),
        Declaration::Usage(usage) => Some(usage.name.as_str()),
        Declaration::Alias(alias) => Some(alias.name.as_str()),
        Declaration::Import(_) => None,
    }
}

fn declaration_matches_qname(
    declaration: &Declaration,
    owner: &str,
    qualified_name: &QualifiedName,
) -> bool {
    match declaration {
        Declaration::Package(package) => {
            qualify_name(owner, &package.name.as_dot_string()) == qualified_name.as_dot_string()
        }
        Declaration::Definition(definition) => {
            qualify_name(owner, &definition.name) == qualified_name.as_dot_string()
        }
        Declaration::Usage(usage) => {
            qualify_name(owner, &usage.name) == qualified_name.as_dot_string()
        }
        Declaration::Alias(alias) => {
            qualify_name(owner, &alias.name) == qualified_name.as_dot_string()
        }
        Declaration::Import(_) => false,
    }
}

fn rename_declaration(
    module: &mut AuthoringModule,
    qualified_name: &QualifiedName,
    new_name: &str,
) -> Result<(), AuthoringError> {
    if module
        .package
        .as_ref()
        .is_some_and(|package| package.name == *qualified_name)
    {
        if let Some(package) = &mut module.package {
            if let Some(last) = package.name.0.last_mut() {
                *last = new_name.to_string();
                return Ok(());
            }
        }
    }
    if let Some(package_owner) = module
        .package
        .as_ref()
        .map(|package| package.name.as_dot_string())
        && let Some(package) = module.package.as_mut()
        && rename_in_members(
            &mut package.members,
            &package_owner,
            qualified_name,
            new_name,
        )
        .is_some()
    {
        return Ok(());
    }
    rename_in_members(&mut module.members, "", qualified_name, new_name)
        .ok_or_else(|| AuthoringError::MissingDeclaration(qualified_name.as_dot_string()))
}

fn rename_in_members(
    declarations: &mut [Declaration],
    owner: &str,
    qualified_name: &QualifiedName,
    new_name: &str,
) -> Option<()> {
    for declaration in declarations {
        match declaration {
            Declaration::Package(package) => {
                let qname = qualify_name(owner, &package.name.as_dot_string());
                if qname == qualified_name.as_dot_string() {
                    if let Some(last) = package.name.0.last_mut() {
                        *last = new_name.to_string();
                        return Some(());
                    }
                }
                if rename_in_members(&mut package.members, &qname, qualified_name, new_name)
                    .is_some()
                {
                    return Some(());
                }
            }
            Declaration::Definition(definition) => {
                let qname = qualify_name(owner, &definition.name);
                if qname == qualified_name.as_dot_string() {
                    definition.name = new_name.to_string();
                    return Some(());
                }
                if rename_in_members(&mut definition.members, &qname, qualified_name, new_name)
                    .is_some()
                {
                    return Some(());
                }
            }
            Declaration::Usage(usage) => {
                let qname = qualify_name(owner, &usage.name);
                if qname == qualified_name.as_dot_string() {
                    usage.name = new_name.to_string();
                    usage.is_implicit_name = false;
                    return Some(());
                }
                if rename_in_members(&mut usage.members, &qname, qualified_name, new_name).is_some()
                {
                    return Some(());
                }
            }
            Declaration::Alias(alias) => {
                let qname = qualify_name(owner, &alias.name);
                if qname == qualified_name.as_dot_string() {
                    alias.name = new_name.to_string();
                    return Some(());
                }
            }
            Declaration::Import(_) => {}
        }
    }
    None
}

fn update_specializations(
    module: &mut AuthoringModule,
    qualified_name: &QualifiedName,
    specializes: Vec<QualifiedName>,
) -> Result<(), AuthoringError> {
    if let Some(package_owner) = module
        .package
        .as_ref()
        .map(|package| package.name.as_dot_string())
        && let Some(package) = module.package.as_mut()
        && update_specializations_in_members(
            &mut package.members,
            &package_owner,
            qualified_name,
            &specializes,
        )
        .is_some()
    {
        return Ok(());
    }
    update_specializations_in_members(&mut module.members, "", qualified_name, &specializes)
        .ok_or_else(|| AuthoringError::MissingDeclaration(qualified_name.as_dot_string()))
}

fn update_specializations_in_members(
    declarations: &mut [Declaration],
    owner: &str,
    qualified_name: &QualifiedName,
    specializes: &[QualifiedName],
) -> Option<()> {
    for declaration in declarations {
        match declaration {
            Declaration::Definition(definition) => {
                let qname = qualify_name(owner, &definition.name);
                if qname == qualified_name.as_dot_string() {
                    definition.specializes = specializes.to_vec();
                    return Some(());
                }
                if update_specializations_in_members(
                    &mut definition.members,
                    &qname,
                    qualified_name,
                    specializes,
                )
                .is_some()
                {
                    return Some(());
                }
            }
            Declaration::Usage(usage) => {
                let qname = qualify_name(owner, &usage.name);
                if qname == qualified_name.as_dot_string() {
                    usage.specializes = specializes.to_vec();
                    return Some(());
                }
                if update_specializations_in_members(
                    &mut usage.members,
                    &qname,
                    qualified_name,
                    specializes,
                )
                .is_some()
                {
                    return Some(());
                }
            }
            Declaration::Package(package) => {
                let qname = qualify_name(owner, &package.name.as_dot_string());
                if update_specializations_in_members(
                    &mut package.members,
                    &qname,
                    qualified_name,
                    specializes,
                )
                .is_some()
                {
                    return Some(());
                }
            }
            Declaration::Import(_) | Declaration::Alias(_) => {}
        }
    }
    None
}

fn update_usage_type(
    module: &mut AuthoringModule,
    qualified_name: &QualifiedName,
    ty: Option<QualifiedName>,
) -> Result<(), AuthoringError> {
    if let Some(package_owner) = module
        .package
        .as_ref()
        .map(|package| package.name.as_dot_string())
        && let Some(package) = module.package.as_mut()
        && update_usage_type_in_members(
            &mut package.members,
            &package_owner,
            qualified_name,
            ty.as_ref(),
        )
        .is_some()
    {
        return Ok(());
    }
    update_usage_type_in_members(&mut module.members, "", qualified_name, ty.as_ref())
        .ok_or_else(|| AuthoringError::MissingDeclaration(qualified_name.as_dot_string()))
}

fn update_usage_type_in_members(
    declarations: &mut [Declaration],
    owner: &str,
    qualified_name: &QualifiedName,
    ty: Option<&QualifiedName>,
) -> Option<()> {
    for declaration in declarations {
        match declaration {
            Declaration::Usage(usage) => {
                let qname = qualify_name(owner, &usage.name);
                if qname == qualified_name.as_dot_string() {
                    usage.ty = ty.cloned();
                    return Some(());
                }
                if update_usage_type_in_members(&mut usage.members, &qname, qualified_name, ty)
                    .is_some()
                {
                    return Some(());
                }
            }
            Declaration::Definition(definition) => {
                let qname = qualify_name(owner, &definition.name);
                if update_usage_type_in_members(&mut definition.members, &qname, qualified_name, ty)
                    .is_some()
                {
                    return Some(());
                }
            }
            Declaration::Package(package) => {
                let qname = qualify_name(owner, &package.name.as_dot_string());
                if update_usage_type_in_members(&mut package.members, &qname, qualified_name, ty)
                    .is_some()
                {
                    return Some(());
                }
            }
            Declaration::Import(_) | Declaration::Alias(_) => {}
        }
    }
    None
}

fn set_usage_expression(
    module: &mut AuthoringModule,
    qualified_name: &QualifiedName,
    expression: Option<String>,
) -> Result<(), AuthoringError> {
    if let Some(package_owner) = module
        .package
        .as_ref()
        .map(|package| package.name.as_dot_string())
        && let Some(package) = module.package.as_mut()
        && set_usage_expression_in_members(
            &mut package.members,
            &package_owner,
            qualified_name,
            expression.as_deref(),
        )
        .is_some()
    {
        return Ok(());
    }
    set_usage_expression_in_members(
        &mut module.members,
        "",
        qualified_name,
        expression.as_deref(),
    )
    .ok_or_else(|| AuthoringError::MissingDeclaration(qualified_name.as_dot_string()))
}

fn set_usage_expression_in_members(
    declarations: &mut [Declaration],
    owner: &str,
    qualified_name: &QualifiedName,
    expression: Option<&str>,
) -> Option<()> {
    for declaration in declarations {
        match declaration {
            Declaration::Usage(usage) => {
                let qname = qualify_name(owner, &usage.name);
                if qname == qualified_name.as_dot_string() {
                    usage.expression = expression.map(str::to_string);
                    return Some(());
                }
                if set_usage_expression_in_members(
                    &mut usage.members,
                    &qname,
                    qualified_name,
                    expression,
                )
                .is_some()
                {
                    return Some(());
                }
            }
            Declaration::Definition(definition) => {
                let qname = qualify_name(owner, &definition.name);
                if set_usage_expression_in_members(
                    &mut definition.members,
                    &qname,
                    qualified_name,
                    expression,
                )
                .is_some()
                {
                    return Some(());
                }
            }
            Declaration::Package(package) => {
                let qname = qualify_name(owner, &package.name.as_dot_string());
                if set_usage_expression_in_members(
                    &mut package.members,
                    &qname,
                    qualified_name,
                    expression,
                )
                .is_some()
                {
                    return Some(());
                }
            }
            Declaration::Import(_) | Declaration::Alias(_) => {}
        }
    }
    None
}

fn relationship_usage(
    kind: &str,
    _source: &QualifiedName,
    target: &QualifiedName,
) -> Result<Usage, AuthoringError> {
    let normalized = kind.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "satisfy" | "satisfies" => Ok(Usage {
            keyword: "satisfy".to_string(),
            name: target.tail().unwrap_or("requirement").to_string(),
            is_implicit_name: false,
            ty: None,
            reference_target: Some(target.clone()),
            metadata_properties: BTreeMap::new(),
            multiplicity: None,
            expression: None,
            additional_types: Vec::new(),
            specializes: Vec::new(),
            subsets: Vec::new(),
            redefines: Vec::new(),
            members: Vec::new(),
            docs: Vec::new(),
            modifiers: Vec::new(),
        }),
        other => Err(AuthoringError::Unsupported(format!(
            "relationship kind `{other}` is not supported by authoring write-back"
        ))),
    }
}

fn extract_declaration(
    module: &mut AuthoringModule,
    qualified_name: &QualifiedName,
) -> Result<Declaration, AuthoringError> {
    if module
        .package
        .as_ref()
        .is_some_and(|package| package.name == *qualified_name)
    {
        return module
            .package
            .take()
            .map(Declaration::Package)
            .ok_or_else(|| AuthoringError::MissingDeclaration(qualified_name.as_dot_string()));
    }
    if let Some(package) = &mut module.package {
        if let Some(removed) = extract_from_members(
            &mut package.members,
            &package.name.as_dot_string(),
            qualified_name,
        ) {
            return Ok(removed);
        }
    }
    extract_from_members(&mut module.members, "", qualified_name)
        .ok_or_else(|| AuthoringError::MissingDeclaration(qualified_name.as_dot_string()))
}

fn extract_from_members(
    declarations: &mut Vec<Declaration>,
    owner: &str,
    qualified_name: &QualifiedName,
) -> Option<Declaration> {
    let mut index = None;
    for (idx, declaration) in declarations.iter_mut().enumerate() {
        let qname = match declaration {
            Declaration::Package(package) => qualify_name(owner, &package.name.as_dot_string()),
            Declaration::Definition(definition) => qualify_name(owner, &definition.name),
            Declaration::Usage(usage) => qualify_name(owner, &usage.name),
            Declaration::Alias(alias) => qualify_name(owner, &alias.name),
            Declaration::Import(_) => continue,
        };
        if qname == qualified_name.as_dot_string() {
            index = Some(idx);
            break;
        }
        let nested = match declaration {
            Declaration::Package(package) => {
                extract_from_members(&mut package.members, &qname, qualified_name)
            }
            Declaration::Definition(definition) => {
                extract_from_members(&mut definition.members, &qname, qualified_name)
            }
            Declaration::Usage(usage) => {
                extract_from_members(&mut usage.members, &qname, qualified_name)
            }
            Declaration::Alias(_) | Declaration::Import(_) => None,
        };
        if nested.is_some() {
            return nested;
        }
    }
    index.map(|idx| declarations.remove(idx))
}

fn render_declaration_at_qname(
    module: &AuthoringModule,
    qualified_name: &QualifiedName,
) -> Option<String> {
    if module
        .package
        .as_ref()
        .is_some_and(|package| package.name == *qualified_name)
    {
        return module.package.as_ref().map(|package| package.render(0));
    }
    if let Some(package) = &module.package
        && let Some(rendered) = render_decl_in_members(
            &package.members,
            &package.name.as_dot_string(),
            qualified_name,
        )
    {
        return Some(rendered);
    }
    render_decl_in_members(&module.members, "", qualified_name)
}

fn render_decl_in_members(
    declarations: &[Declaration],
    owner: &str,
    qualified_name: &QualifiedName,
) -> Option<String> {
    for declaration in declarations {
        let qname = match declaration {
            Declaration::Package(package) => qualify_name(owner, &package.name.as_dot_string()),
            Declaration::Definition(definition) => qualify_name(owner, &definition.name),
            Declaration::Usage(usage) => qualify_name(owner, &usage.name),
            Declaration::Alias(alias) => qualify_name(owner, &alias.name),
            Declaration::Import(_) => continue,
        };
        if qname == qualified_name.as_dot_string() {
            return Some(declaration.render(0));
        }
        let nested = match declaration {
            Declaration::Package(package) => {
                render_decl_in_members(&package.members, &qname, qualified_name)
            }
            Declaration::Definition(definition) => {
                render_decl_in_members(&definition.members, &qname, qualified_name)
            }
            Declaration::Usage(usage) => {
                render_decl_in_members(&usage.members, &qname, qualified_name)
            }
            Declaration::Alias(_) | Declaration::Import(_) => None,
        };
        if nested.is_some() {
            return nested;
        }
    }
    None
}

fn render_docs(docs: &[String], indent: usize) -> Vec<String> {
    let prefix = " ".repeat(indent);
    docs.iter()
        .map(|doc| format!("{prefix}doc /* {} */", doc.replace("*/", "* /")))
        .collect()
}

fn render_modifier_prefix(modifiers: &[String]) -> String {
    if modifiers.is_empty() {
        String::new()
    } else {
        format!("{} ", modifiers.join(" "))
    }
}

fn render_expr(expr: &Expr) -> String {
    match expr {
        Expr::Literal(literal) => match literal {
            LiteralExpr::Integer(value) => value.to_string(),
            LiteralExpr::Real(value) => value.clone(),
            LiteralExpr::Boolean(value) => value.to_string(),
            LiteralExpr::String(value) => format!("{value:?}"),
        },
        Expr::Name(name) => name.as_colon_string(),
        Expr::SelfRef(_) => "self".to_string(),
        Expr::Tuple { items, .. } => format!(
            "({})",
            items.iter().map(render_expr).collect::<Vec<_>>().join(", ")
        ),
        Expr::Unary { op, expr, .. } => match op {
            UnaryOp::Negate => format!("-{}", render_expr(expr)),
            UnaryOp::Not => format!("not {}", render_expr(expr)),
        },
        Expr::Binary {
            left, op, right, ..
        } => format!(
            "{} {} {}",
            render_expr(left),
            render_binary_op(op),
            render_expr(right)
        ),
        Expr::Path { root, segment, .. } => format!("{}.{}", render_expr(root), segment),
        Expr::Call { function, args, .. } => format!(
            "{function}({})",
            args.iter().map(render_expr).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn render_binary_op(op: &BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Subtract => "-",
        BinaryOp::Multiply => "*",
        BinaryOp::Divide => "/",
        BinaryOp::Power => "**",
        BinaryOp::Equal => "==",
        BinaryOp::NotEqual => "!=",
        BinaryOp::Less => "<",
        BinaryOp::LessEqual => "<=",
        BinaryOp::Greater => ">",
        BinaryOp::GreaterEqual => ">=",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
    }
}

fn render_metadata_property_value(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "\"\"".to_string();
    }
    if is_unquoted_metadata_value(trimmed) {
        trimmed.to_string()
    } else {
        format!("{trimmed:?}")
    }
}

fn is_unquoted_metadata_value(value: &str) -> bool {
    value.split("::").all(|segment| {
        let mut chars = segment.chars();
        chars
            .next()
            .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
            && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    })
}

fn render_with_indent(rendered: &str, indent: usize) -> String {
    if rendered.is_empty() {
        return String::new();
    }
    let prefix = " ".repeat(indent);
    rendered
        .lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{prefix}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn qualify_name(owner: &str, name: &str) -> String {
    if owner.is_empty() {
        name.to_string()
    } else {
        format!("{owner}.{name}")
    }
}

fn join_qname(owner: &str, name: &str) -> String {
    if owner.is_empty() {
        name.to_string()
    } else {
        format!("{owner}.{name}")
    }
}

fn group_rewrites_by_file(
    rewrites: &[RewriteInstruction],
) -> BTreeMap<String, Vec<RewriteInstruction>> {
    let mut grouped = BTreeMap::new();
    for rewrite in rewrites {
        let file = match rewrite {
            RewriteInstruction::FullFile { file }
            | RewriteInstruction::ReplaceNode { file, .. }
            | RewriteInstruction::ReplaceContainer { file, .. } => file.clone(),
        };
        grouped
            .entry(file)
            .or_insert_with(Vec::new)
            .push(rewrite.clone());
    }
    grouped
}

fn validate_non_overlapping_patches(
    patches: &[((usize, usize), String)],
) -> Result<(), AuthoringError> {
    let mut previous_start = usize::MAX;
    for ((start, end), _) in patches {
        if *end > previous_start {
            return Err(AuthoringError::Unsupported(
                "localized rewrite produced overlapping source patches".to_string(),
            ));
        }
        previous_start = *start;
    }
    Ok(())
}

fn span_to_offsets(text: &str, span: &SourceSpan) -> Result<(usize, usize), AuthoringError> {
    let starts = line_start_offsets(text);
    let start_line = span
        .start_line
        .checked_sub(1)
        .ok_or_else(|| AuthoringError::Validation("invalid source span start".to_string()))?;
    let end_line = span
        .end_line
        .checked_sub(1)
        .ok_or_else(|| AuthoringError::Validation("invalid source span end".to_string()))?;
    let start = starts
        .get(start_line)
        .copied()
        .unwrap_or(text.len())
        .saturating_add(span.start_col.saturating_sub(1));
    let end = starts
        .get(end_line)
        .copied()
        .unwrap_or(text.len())
        .saturating_add(span.end_col);
    Ok((start.min(text.len()), end.min(text.len())))
}

fn line_start_offsets(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, ch) in text.char_indices() {
        if ch == '\n' {
            starts.push(index + 1);
        }
    }
    starts
}

fn rendered_span_for_text(text: &str) -> RenderedSpan {
    let lines = text.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return RenderedSpan {
            start_line: 1,
            start_col: 1,
            end_line: 1,
            end_col: 1,
        };
    }
    let end_line = lines.len();
    let end_col = lines.last().map_or(1, |line| line.len() + 1);
    RenderedSpan {
        start_line: 1,
        start_col: 1,
        end_line,
        end_col,
    }
}

fn compile_user_kir_from_texts(
    texts: &BTreeMap<String, String>,
) -> Result<KirDocument, AuthoringError> {
    let stdlib = default_stdlib_document()?;
    let mut documents = Vec::new();
    for (path, content) in texts {
        documents.push(compile_sysml_text(content, path, stdlib)?);
    }
    KirDocument::merge(documents).map_err(Into::into)
}

fn default_stdlib_document() -> Result<&'static KirDocument, AuthoringError> {
    DEFAULT_STDLIB_DOCUMENT
        .get_or_init(|| {
            KirDocument::from_path(Path::new(&default_stdlib_path())).map_err(|err| err.to_string())
        })
        .as_ref()
        .map_err(|err| AuthoringError::Kir(KirError::Frontend(err.clone())))
}

fn diff_element_ids(before: &KirDocument, after: &KirDocument) -> BTreeSet<String> {
    let before_ids = before
        .elements
        .iter()
        .map(|element| element.id.clone())
        .collect::<BTreeSet<_>>();
    let after_ids = after
        .elements
        .iter()
        .map(|element| element.id.clone())
        .collect::<BTreeSet<_>>();
    before_ids
        .symmetric_difference(&after_ids)
        .cloned()
        .collect()
}

fn normalize_kir(document: &KirDocument) -> Vec<(String, String, u8, BTreeMap<String, Value>)> {
    let mut elements = document
        .elements
        .iter()
        .map(|element| {
            (
                element.id.clone(),
                element.kind.clone(),
                element.layer,
                normalize_properties(&element.properties),
            )
        })
        .collect::<Vec<_>>();
    elements.sort_by(|left, right| left.0.cmp(&right.0));
    elements
}

fn normalize_properties(properties: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    properties
        .iter()
        .filter(|(key, _)| *key != "metadata")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn group_kir_by_source_file(document: &KirDocument) -> BTreeMap<String, Vec<KirElement>> {
    let mut grouped = BTreeMap::new();
    for element in &document.elements {
        let source_file = element
            .properties
            .get("metadata")
            .and_then(Value::as_object)
            .and_then(|metadata| metadata.get("source_file"))
            .and_then(Value::as_str);
        if let Some(source_file) = source_file {
            grouped
                .entry(source_file.replace('\\', "/"))
                .or_insert_with(Vec::new)
                .push(element.clone());
        }
    }
    grouped
}

fn module_from_kir_elements(elements: &[KirElement]) -> Result<AuthoringModule, AuthoringError> {
    let mut by_id = HashMap::new();
    for element in elements {
        by_id.insert(element.id.clone(), element.clone());
    }

    let mut module = AuthoringModule::default();
    let mut consumed = BTreeSet::new();
    let package_id = elements
        .iter()
        .filter(|element| element.kind.contains("Package") && element.id.starts_with("pkg."))
        .min_by_key(|element| element.id.matches('.').count())
        .map(|element| element.id.clone());

    if let Some(package_id) = package_id {
        if let Some(package) = build_package_from_kir(&package_id, &by_id, &mut consumed)? {
            module.package = Some(package);
        }
    }

    let mut top_level = Vec::new();
    for element in elements {
        if consumed.contains(&element.id) {
            continue;
        }
        if let Some(declaration) = build_declaration_from_kir(&element.id, &by_id, &mut consumed)? {
            top_level.push(declaration);
        }
    }
    top_level.sort_by_key(|declaration| declaration_name_for_sort(declaration));
    module.members = top_level;
    Ok(module)
}

fn build_package_from_kir(
    id: &str,
    by_id: &HashMap<String, KirElement>,
    consumed: &mut BTreeSet<String>,
) -> Result<Option<Package>, AuthoringError> {
    let Some(element) = by_id.get(id) else {
        return Ok(None);
    };
    consumed.insert(id.to_string());
    let name = declared_name_from_properties(&element.properties)
        .or_else(|| id.strip_prefix("pkg.").map(QualifiedName::parse))
        .unwrap_or_else(|| QualifiedName::new(vec!["Package".to_string()]));
    let member_ids = element
        .properties
        .get("members")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut members = Vec::new();
    for member_id in member_ids {
        if let Some(member) = build_declaration_from_kir(&member_id, by_id, consumed)? {
            members.push(member);
        }
    }
    Ok(Some(Package {
        name,
        members,
        docs: docs_from_properties(&element.properties),
        modifiers: Vec::new(),
    }))
}

fn build_declaration_from_kir(
    id: &str,
    by_id: &HashMap<String, KirElement>,
    consumed: &mut BTreeSet<String>,
) -> Result<Option<Declaration>, AuthoringError> {
    let Some(element) = by_id.get(id) else {
        return Ok(None);
    };
    if !consumed.insert(id.to_string()) {
        return Ok(None);
    }

    if element.kind.contains("Import") || id.starts_with("import.") {
        let path = element
            .properties
            .get("imports")
            .and_then(Value::as_str)
            .map(QualifiedName::parse)
            .ok_or_else(|| {
                AuthoringError::Unsupported(format!("cannot reconstruct import `{id}` from KIR"))
            })?;
        return Ok(Some(Declaration::Import(Import {
            path,
            docs: docs_from_properties(&element.properties),
            modifiers: Vec::new(),
        })));
    }

    if element.kind.contains("Package") && id.starts_with("pkg.") {
        return Ok(build_package_from_kir(id, by_id, consumed)?.map(Declaration::Package));
    }

    let members = element
        .properties
        .get("members")
        .or_else(|| element.properties.get("features"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    let built_members = members
        .iter()
        .filter_map(|member_id| build_declaration_from_kir(member_id, by_id, consumed).transpose())
        .collect::<Result<Vec<_>, _>>()?;
    if id.starts_with("type.") {
        let keyword = keyword_from_kind(&element.kind, true);
        let name = declared_name_from_properties(&element.properties)
            .and_then(|name| name.tail().map(str::to_string))
            .unwrap_or_else(|| tail_from_id(id));
        return Ok(Some(Declaration::Definition(Definition {
            keyword,
            name,
            specializes: specializations_from_properties(&element.properties, None),
            members: built_members,
            docs: docs_from_properties(&element.properties),
            modifiers: Vec::new(),
        })));
    }

    if id.starts_with("feature.") || element.properties.contains_key("owner") {
        let name = element
            .properties
            .get("declared_name")
            .and_then(Value::as_str)
            .or_else(|| element.properties.get("name").and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| tail_from_id(id));
        let ty = element
            .properties
            .get("type")
            .and_then(Value::as_str)
            .map(QualifiedName::parse);
        return Ok(Some(Declaration::Usage(Usage {
            keyword: keyword_from_kind(&element.kind, false),
            name,
            is_implicit_name: element.properties.get("declared_name").is_none(),
            ty: ty.clone(),
            reference_target: None,
            metadata_properties: BTreeMap::new(),
            multiplicity: element
                .properties
                .get("multiplicity")
                .and_then(Value::as_str)
                .map(multiplicity_range_from_raw),
            expression: element
                .properties
                .get("expression_ir")
                .and_then(Value::as_str)
                .map(str::to_string),
            additional_types: Vec::new(),
            specializes: specializations_from_properties(&element.properties, ty.as_ref()),
            subsets: property_qnames(&element.properties, "subsetted_features"),
            redefines: property_qnames(&element.properties, "redefined_features"),
            members: built_members,
            docs: docs_from_properties(&element.properties),
            modifiers: usage_modifiers_from_properties(&element.properties),
        })));
    }

    Ok(None)
}

fn declared_name_from_properties(properties: &BTreeMap<String, Value>) -> Option<QualifiedName> {
    properties
        .get("declared_name")
        .and_then(Value::as_str)
        .or_else(|| properties.get("name").and_then(Value::as_str))
        .map(QualifiedName::parse)
}

fn docs_from_properties(properties: &BTreeMap<String, Value>) -> Vec<String> {
    properties
        .get("doc")
        .and_then(Value::as_object)
        .and_then(|doc| doc.get("blocks"))
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn property_qnames(properties: &BTreeMap<String, Value>, key: &str) -> Vec<QualifiedName> {
    properties
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(QualifiedName::parse)
        .collect()
}

fn specializations_from_properties(
    properties: &BTreeMap<String, Value>,
    ty: Option<&QualifiedName>,
) -> Vec<QualifiedName> {
    property_qnames(properties, "specializes")
        .into_iter()
        .filter(|name| Some(name) != ty)
        .collect()
}

fn usage_modifiers_from_properties(properties: &BTreeMap<String, Value>) -> Vec<String> {
    let mut modifiers = Vec::new();
    if properties
        .get("is_end")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        modifiers.push("end".to_string());
    }
    if let Some(direction) = properties.get("direction").and_then(Value::as_str) {
        modifiers.push(direction.to_string());
    }
    modifiers
}

fn keyword_from_kind(kind: &str, is_definition: bool) -> String {
    let tail = kind.rsplit("::").next().unwrap_or(kind);
    let suffix = if is_definition { "Definition" } else { "Usage" };
    let keyword = tail.strip_suffix(suffix).unwrap_or(tail);
    keyword
        .chars()
        .enumerate()
        .fold(String::new(), |mut acc, (index, ch)| {
            if ch.is_ascii_uppercase() && index > 0 {
                acc.push('-');
            }
            acc.push(ch.to_ascii_lowercase());
            acc
        })
}

fn declaration_name_for_sort(declaration: &Declaration) -> String {
    match declaration {
        Declaration::Package(package) => package.name.as_dot_string(),
        Declaration::Import(import) => import.path.as_colon_string(),
        Declaration::Definition(definition) => definition.name.clone(),
        Declaration::Usage(usage) => usage.name.clone(),
        Declaration::Alias(alias) => alias.name.clone(),
    }
}

fn tail_from_id(id: &str) -> String {
    id.rsplit('.').next().unwrap_or(id).to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        AuthoringProject, ContainerSelector, Mutation, QualifiedName, WriteBackMode,
        create_empty_model, load_authoring_project_from_sysml,
    };

    fn qname(value: &str) -> QualifiedName {
        QualifiedName::parse(value)
    }

    #[test]
    fn empty_project_can_emit_new_sysml_file_after_mutation() {
        let mut project = create_empty_model();
        let package_result = project
            .apply_mutation(Mutation::AddPackage {
                target_file: "model.sysml".to_string(),
                package_name: qname("Demo"),
            })
            .unwrap();
        project.write_back_mutation(&package_result).unwrap();

        let definition_result = project
            .apply_mutation(Mutation::AddDefinition {
                container: ContainerSelector::Package {
                    qualified_name: qname("Demo"),
                },
                keyword: "part".to_string(),
                name: "Vehicle".to_string(),
                specializes: Vec::new(),
            })
            .unwrap();
        let write_back = project.write_back_mutation(&definition_result).unwrap();
        let text = write_back.edited_files.get("model.sysml").unwrap();

        assert_eq!(write_back.mode, WriteBackMode::LocalizedPatch);
        assert!(text.contains("package Demo {"));
        assert!(text.contains("part def Vehicle;"));
        assert!(write_back.validation.ok);
    }

    #[test]
    fn rename_rewrites_existing_definition_locally() {
        let source = "package Demo {\n  // untouched\n  part def Vehicle;\n}\n";
        let mut project = load_authoring_project_from_sysml(
            [("model.sysml".to_string(), source.to_string())]
                .into_iter()
                .collect(),
        )
        .unwrap();

        let mutation = project
            .apply_mutation(Mutation::RenameDeclaration {
                qualified_name: qname("Demo.Vehicle"),
                new_name: "Car".to_string(),
            })
            .unwrap();
        let write_back = project.write_back_mutation(&mutation).unwrap();
        let text = write_back.edited_files.get("model.sysml").unwrap();

        assert_eq!(write_back.mode, WriteBackMode::LocalizedPatch);
        assert!(text.contains("// untouched"));
        assert!(text.contains("part def Car;"));
        assert!(!text.contains("part def Vehicle;"));
        assert!(write_back.validation.ok);
    }

    #[test]
    fn adding_nested_usage_rewrites_only_owner_declaration() {
        let source =
            "package Demo {\n  // file comment\n  part def Engine;\n  part def Vehicle {\n  }\n}\n";
        let mut project = load_authoring_project_from_sysml(
            [("model.sysml".to_string(), source.to_string())]
                .into_iter()
                .collect(),
        )
        .unwrap();

        let mutation = project
            .apply_mutation(Mutation::AddUsage {
                container: ContainerSelector::Declaration {
                    qualified_name: qname("Demo.Vehicle"),
                },
                keyword: "part".to_string(),
                name: "engine".to_string(),
                ty: Some(qname("Engine")),
                specializes: Vec::new(),
            })
            .unwrap();
        let write_back = project.write_back_mutation(&mutation).unwrap();
        let text = write_back.edited_files.get("model.sysml").unwrap();

        assert_eq!(write_back.mode, WriteBackMode::LocalizedPatch);
        assert!(text.contains("// file comment"));
        assert!(text.contains("part engine: Engine;"));
        assert!(write_back.validation.ok);
    }

    #[test]
    fn adding_metadata_annotation_round_trips_through_source() {
        let source = r#"package Demo {
  metadata def ReviewTag {
    attribute status : String;
    attribute owner : String;
  }
  requirement safeStart;
}
"#;
        let mut project = load_authoring_project_from_sysml(
            [("model.sysml".to_string(), source.to_string())]
                .into_iter()
                .collect(),
        )
        .unwrap();

        let mutation = project
            .apply_mutation(Mutation::AddMetadataAnnotation {
                element: qname("Demo.safeStart"),
                metadata_type: "ReviewTag".to_string(),
                properties: [
                    ("owner".to_string(), "Safety Team".to_string()),
                    ("status".to_string(), "draft".to_string()),
                ]
                .into_iter()
                .collect(),
            })
            .unwrap();
        let write_back = project.write_back_mutation(&mutation).unwrap();
        let text = write_back.edited_files.get("model.sysml").unwrap();

        assert!(text.contains("@ReviewTag"));
        assert!(text.contains("owner = \"Safety Team\";"));
        assert!(text.contains("status = draft;"));
        assert!(write_back.validation.ok);
    }

    #[test]
    fn multi_file_top_level_addition_requires_target_file_and_edits_only_that_file() {
        let files = [
            (
                "a.sysml".to_string(),
                "package A {\n  part def Vehicle;\n}\n".to_string(),
            ),
            (
                "b.sysml".to_string(),
                "package B {\n  part def Engine;\n}\n".to_string(),
            ),
        ]
        .into_iter()
        .collect();
        let mut project = AuthoringProject::from_sysml_files(files).unwrap();

        let mutation = project
            .apply_mutation(Mutation::AddDefinition {
                container: ContainerSelector::Package {
                    qualified_name: qname("B"),
                },
                keyword: "part".to_string(),
                name: "Brake".to_string(),
                specializes: Vec::new(),
            })
            .unwrap();
        let write_back = project.write_back_mutation(&mutation).unwrap();

        assert!(write_back.edited_files.contains_key("b.sysml"));
        assert!(!write_back.edited_files.contains_key("a.sysml"));
        assert!(write_back.edited_files["b.sysml"].contains("part def Brake;"));
        assert!(write_back.validation.ok);
    }

    #[test]
    fn moving_declaration_between_files_updates_source_and_destination() {
        let files = [
            (
                "a.sysml".to_string(),
                "package A {\n  part def Vehicle;\n}\n".to_string(),
            ),
            ("b.sysml".to_string(), "package B {\n}\n".to_string()),
        ]
        .into_iter()
        .collect();
        let mut project = AuthoringProject::from_sysml_files(files).unwrap();

        let mutation = project
            .apply_mutation(Mutation::MoveDeclaration {
                qualified_name: qname("A.Vehicle"),
                destination: ContainerSelector::Package {
                    qualified_name: qname("B"),
                },
            })
            .unwrap();
        let write_back = project.write_back_mutation(&mutation).unwrap();

        assert!(write_back.edited_files["a.sysml"].contains("package A {\n}\n"));
        assert!(write_back.edited_files["b.sysml"].contains("part def Vehicle;"));
        assert!(write_back.validation.ok);
    }
}
