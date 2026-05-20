use std::collections::{BTreeMap, BTreeSet, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;

use crate::frontend::ast::{
    AliasDecl, BinaryOp, Declaration, Expr, GenericDefinitionDecl, GenericUsageDecl, ImportDecl,
    LiteralExpr, MultiplicityRange, PackageDecl, PartDefinitionDecl, PartUsageDecl, QualifiedName,
    SourceSpan, SysmlModule, UnaryOp,
};
use crate::frontend::diagnostics::Diagnostic;
use crate::frontend::transpile::MappingBundle;
use crate::ir::KirDocument;
use crate::logging::{compile_timer_start, log_compile_timed_event};

#[derive(Debug, Clone)]
pub struct ResolvedModule {
    pub packages: Vec<ResolvedPackage>,
    pub imports: Vec<ResolvedImport>,
    pub definitions: Vec<ResolvedDefinition>,
    pub usages: Vec<ResolvedUsage>,
}

#[derive(Debug, Clone)]
pub struct ResolvedPackage {
    pub owner_package_qualified_name: Option<String>,
    pub qualified_name: String,
    pub declared_name: String,
    pub docs: Vec<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone)]
pub struct ResolvedImport {
    pub owner_package_qualified_name: Option<String>,
    pub target_id: String,
    pub imported_name: Option<String>,
    pub docs: Vec<String>,
    pub span: SourceSpan,
    pub ordinal: usize,
}

#[derive(Debug, Clone)]
pub struct ResolvedDefinition {
    pub construct: String,
    pub qualified_name: String,
    pub declared_name: String,
    pub is_abstract: bool,
    pub specializes: Vec<String>,
    pub members: Vec<ResolvedUsage>,
    pub docs: Vec<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone)]
pub struct ResolvedUsage {
    pub construct: String,
    pub owner_construct: String,
    pub owner_qualified_name: String,
    pub qualified_name: String,
    pub declared_name: String,
    pub is_implicit_name: bool,
    pub has_explicit_type: bool,
    pub type_ref: Option<String>,
    pub additional_type_refs: Vec<String>,
    pub reference_target: Option<String>,
    pub multiplicity: Option<MultiplicityRange>,
    pub expression: Option<ResolvedExpr>,
    pub is_derived: bool,
    pub specializes: Vec<String>,
    pub specialized_features: Vec<String>,
    pub subsetted_features: Vec<String>,
    pub redefined_features: Vec<String>,
    pub members: Vec<ResolvedUsage>,
    pub modifiers: Vec<String>,
    pub docs: Vec<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedExpr {
    Literal(Value),
    SelfRef,
    Tuple {
        items: Vec<ResolvedExpr>,
    },
    FeaturePath {
        segments: Vec<ResolvedPathSegment>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<ResolvedExpr>,
    },
    Binary {
        left: Box<ResolvedExpr>,
        op: BinaryOp,
        right: Box<ResolvedExpr>,
    },
    Call {
        function: String,
        args: Vec<ResolvedExpr>,
    },
}

fn expression_span(expr: &Expr) -> SourceSpan {
    match expr {
        Expr::Literal(_) => SourceSpan {
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 0,
        },
        Expr::Name(name) => name.span.clone(),
        Expr::SelfRef(span) => span.clone(),
        Expr::Tuple { span, .. }
        | Expr::Unary { span, .. }
        | Expr::Binary { span, .. }
        | Expr::Path { span, .. }
        | Expr::Call { span, .. } => span.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPathSegment {
    pub name: String,
    pub feature_id: String,
}

#[derive(Debug, Clone)]
struct CollectedImport {
    owner_package_qualified_name: Option<String>,
    decl: ImportDecl,
}

#[derive(Debug, Clone)]
struct CollectedDefinition {
    construct: String,
    qualified_name: String,
    declared_name: String,
    is_abstract: bool,
    specializes: Vec<QualifiedName>,
    members: Vec<CollectedUsage>,
    docs: Vec<String>,
    span: SourceSpan,
}

#[derive(Debug, Clone)]
struct CollectedUsage {
    construct: String,
    owner_construct: String,
    owner_qualified_name: String,
    qualified_name: String,
    declared_name: String,
    is_implicit_name: bool,
    ty: Option<QualifiedName>,
    additional_types: Vec<QualifiedName>,
    reference_target: Option<QualifiedName>,
    multiplicity: Option<MultiplicityRange>,
    expression: Option<Expr>,
    specializes: Vec<QualifiedName>,
    subsets: Vec<QualifiedName>,
    redefines: Vec<QualifiedName>,
    members: Vec<CollectedUsage>,
    modifiers: Vec<String>,
    docs: Vec<String>,
    span: SourceSpan,
}

#[derive(Debug, Clone)]
struct CollectedAlias {
    qualified_name: String,
    declared_name: String,
    target: QualifiedName,
}

#[derive(Debug, Clone, Default)]
struct ImportAliases {
    value_aliases: BTreeMap<String, String>,
    namespace_aliases: BTreeMap<String, QualifiedName>,
    ambiguous_value_aliases: BTreeSet<String>,
    ambiguous_namespace_aliases: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct StdlibIndexes {
    ids: Vec<String>,
    feature_index: BTreeMap<String, BTreeMap<String, String>>,
    aliases: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ResolverContext {
    module_count: usize,
    packages: Vec<ResolvedPackage>,
    definitions: Vec<CollectedDefinition>,
    local_definitions: BTreeMap<String, String>,
    definition_index: BTreeMap<String, CollectedDefinition>,
    local_feature_index: BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: BTreeMap<String, CollectedUsage>,
    stdlib_indexes: Arc<StdlibIndexes>,
}

impl ResolverContext {
    pub fn module_count(&self) -> usize {
        self.module_count
    }

    pub fn from_modules(
        context_modules: &[SysmlModule],
        stdlib: &KirDocument,
        mappings: &MappingBundle,
    ) -> Result<Self, Diagnostic> {
        let collect_context_start = compile_timer_start();
        let (packages, _, definitions, usages, _) = collect_modules(context_modules, mappings)?;
        log_compile_timed_event(
            "resolver.collect_context_modules",
            collect_context_start,
            "ok",
            format!(
                "context_modules={} packages={} definitions={} usages={}",
                context_modules.len(),
                packages.len(),
                definitions.len(),
                usages.len()
            ),
        );

        let local_index_start = compile_timer_start();
        let local_definitions = build_local_definition_map(&definitions)?;
        let definition_index = definitions
            .iter()
            .cloned()
            .map(|definition| (definition.qualified_name.clone(), definition))
            .collect::<BTreeMap<_, _>>();
        let local_feature_index = build_local_feature_index(&definitions, &usages);
        let local_usage_map = build_local_usage_map(&definitions, &usages);
        log_compile_timed_event(
            "resolver.build_local_indexes",
            local_index_start,
            "ok",
            format!("definitions={} usages={}", definitions.len(), usages.len()),
        );

        let stdlib_index_start = compile_timer_start();
        let stdlib_indexes = cached_stdlib_indexes(stdlib, mappings);
        log_compile_timed_event(
            "resolver.build_stdlib_indexes",
            stdlib_index_start,
            "ok",
            format!(
                "stdlib_elements={} aliases={} cache=instance_keyed",
                stdlib.elements.len(),
                stdlib_indexes.aliases.len()
            ),
        );

        Ok(Self {
            module_count: context_modules.len(),
            packages,
            definitions,
            local_definitions,
            definition_index,
            local_feature_index,
            local_usage_map,
            stdlib_indexes,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct ResolvePolicy {
    preserve_unresolved_references: bool,
}

const STRICT_RESOLVE_POLICY: ResolvePolicy = ResolvePolicy {
    preserve_unresolved_references: false,
};

const KERML_RESOLVE_POLICY: ResolvePolicy = ResolvePolicy {
    preserve_unresolved_references: true,
};

pub fn resolve_module(
    module: &SysmlModule,
    stdlib: &KirDocument,
    mappings: &MappingBundle,
) -> Result<ResolvedModule, Diagnostic> {
    resolve_module_with_context(module, std::slice::from_ref(module), stdlib, mappings)
}

pub fn resolve_module_with_context(
    module: &SysmlModule,
    context_modules: &[SysmlModule],
    stdlib: &KirDocument,
    mappings: &MappingBundle,
) -> Result<ResolvedModule, Diagnostic> {
    resolve_module_with_policy(
        module,
        context_modules,
        stdlib,
        mappings,
        STRICT_RESOLVE_POLICY,
    )
}

pub(crate) fn resolve_module_with_resolver_context(
    module: &SysmlModule,
    context: &ResolverContext,
    mappings: &MappingBundle,
) -> Result<ResolvedModule, Diagnostic> {
    resolve_module_with_policy_context(module, context, mappings, STRICT_RESOLVE_POLICY)
}

pub fn resolve_kerml_module_with_context(
    module: &SysmlModule,
    context_modules: &[SysmlModule],
    stdlib: &KirDocument,
    mappings: &MappingBundle,
) -> Result<ResolvedModule, Diagnostic> {
    resolve_module_with_policy(
        module,
        context_modules,
        stdlib,
        mappings,
        KERML_RESOLVE_POLICY,
    )
}

pub(crate) fn resolve_kerml_module_with_resolver_context(
    module: &SysmlModule,
    context: &ResolverContext,
    mappings: &MappingBundle,
) -> Result<ResolvedModule, Diagnostic> {
    resolve_module_with_policy_context(module, context, mappings, KERML_RESOLVE_POLICY)
}

fn resolve_module_with_policy(
    module: &SysmlModule,
    context_modules: &[SysmlModule],
    stdlib: &KirDocument,
    mappings: &MappingBundle,
    policy: ResolvePolicy,
) -> Result<ResolvedModule, Diagnostic> {
    let context = ResolverContext::from_modules(context_modules, stdlib, mappings)?;
    resolve_module_with_policy_context(module, &context, mappings, policy)
}

fn resolve_module_with_policy_context(
    module: &SysmlModule,
    context: &ResolverContext,
    mappings: &MappingBundle,
    policy: ResolvePolicy,
) -> Result<ResolvedModule, Diagnostic> {
    let collect_module_start = compile_timer_start();
    let (packages, imports, definitions, usages, aliases) = collect_module(module, mappings)?;
    log_compile_timed_event(
        "resolver.collect_module",
        collect_module_start,
        "ok",
        format!(
            "packages={} imports={} definitions={} usages={} aliases={}",
            packages.len(),
            imports.len(),
            definitions.len(),
            usages.len(),
            aliases.len()
        ),
    );

    let local_aliases = build_local_alias_map(&aliases);

    let resolve_import_start = compile_timer_start();
    let resolved_imports = resolve_imports(
        &imports,
        &context.stdlib_indexes.ids,
        &context.stdlib_indexes.aliases,
        &context.local_definitions,
        &local_aliases,
    )?;
    log_compile_timed_event(
        "resolver.resolve_imports",
        resolve_import_start,
        "ok",
        format!("imports={}", resolved_imports.len()),
    );

    let import_alias_start = compile_timer_start();
    let import_aliases = build_import_alias_map(
        &resolved_imports,
        &context.packages,
        &context.definitions,
        &context.local_usage_map,
        &context.stdlib_indexes.ids,
        &context.stdlib_indexes.aliases,
        policy,
    )?;
    log_compile_timed_event(
        "resolver.build_import_aliases",
        import_alias_start,
        "ok",
        format!(
            "namespace_aliases={} value_aliases={} ambiguous_namespace_aliases={} ambiguous_value_aliases={}",
            import_aliases.namespace_aliases.len(),
            import_aliases.value_aliases.len(),
            import_aliases.ambiguous_namespace_aliases.len(),
            import_aliases.ambiguous_value_aliases.len()
        ),
    );

    let resolve_definition_start = compile_timer_start();
    let resolved_definitions = definitions
        .into_iter()
        .map(|definition| {
            resolve_definition(
                definition,
                &context.stdlib_indexes.ids,
                &context.stdlib_indexes.feature_index,
                &context.stdlib_indexes.aliases,
                &context.local_definitions,
                &local_aliases,
                &import_aliases,
                &context.definition_index,
                &context.local_feature_index,
                &context.local_usage_map,
                policy,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    log_compile_timed_event(
        "resolver.resolve_definitions",
        resolve_definition_start,
        "ok",
        format!("definitions={}", resolved_definitions.len()),
    );

    let resolve_usage_start = compile_timer_start();
    let resolved_usages = usages
        .into_iter()
        .map(|usage| {
            resolve_usage(
                usage,
                &context.stdlib_indexes.ids,
                &context.stdlib_indexes.feature_index,
                &context.stdlib_indexes.aliases,
                &context.local_definitions,
                &local_aliases,
                &import_aliases,
                &context.definition_index,
                &context.local_feature_index,
                &context.local_usage_map,
                policy,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    log_compile_timed_event(
        "resolver.resolve_usages",
        resolve_usage_start,
        "ok",
        format!("usages={}", resolved_usages.len()),
    );

    Ok(ResolvedModule {
        packages,
        imports: resolved_imports,
        definitions: resolved_definitions,
        usages: resolved_usages,
    })
}

fn collect_module(
    module: &SysmlModule,
    mappings: &MappingBundle,
) -> Result<
    (
        Vec<ResolvedPackage>,
        Vec<CollectedImport>,
        Vec<CollectedDefinition>,
        Vec<CollectedUsage>,
        Vec<CollectedAlias>,
    ),
    Diagnostic,
> {
    let mut packages = Vec::new();
    let mut imports = Vec::new();
    let mut definitions = Vec::new();
    let mut usages = Vec::new();
    let mut aliases = Vec::new();

    let root_members = if !module.members.is_empty() {
        module.members.clone()
    } else if let Some(package) = &module.package {
        vec![Declaration::Package(package.clone())]
    } else {
        Vec::new()
    };

    collect_declarations(
        &root_members,
        &[],
        None,
        &mut packages,
        &mut imports,
        &mut definitions,
        &mut usages,
        &mut aliases,
        mappings,
    )?;
    collect_nested_aliases(
        &root_members,
        &[],
        None,
        &mut aliases,
    );

    Ok((packages, imports, definitions, usages, aliases))
}

fn collect_modules(
    modules: &[SysmlModule],
    mappings: &MappingBundle,
) -> Result<
    (
        Vec<ResolvedPackage>,
        Vec<CollectedImport>,
        Vec<CollectedDefinition>,
        Vec<CollectedUsage>,
        Vec<CollectedAlias>,
    ),
    Diagnostic,
> {
    let mut packages = Vec::new();
    let mut imports = Vec::new();
    let mut definitions = Vec::new();
    let mut usages = Vec::new();
    let mut aliases = Vec::new();

    for module in modules {
        let (module_packages, module_imports, module_definitions, module_usages, module_aliases) =
            collect_module(module, mappings)?;
        packages.extend(module_packages);
        imports.extend(module_imports);
        definitions.extend(module_definitions);
        usages.extend(module_usages);
        aliases.extend(module_aliases);
    }

    Ok((packages, imports, definitions, usages, aliases))
}

#[allow(clippy::too_many_arguments)]
fn collect_declarations(
    declarations: &[Declaration],
    owner_package_segments: &[String],
    owner_package_qualified_name: Option<&str>,
    packages: &mut Vec<ResolvedPackage>,
    imports: &mut Vec<CollectedImport>,
    definitions: &mut Vec<CollectedDefinition>,
    usages: &mut Vec<CollectedUsage>,
    aliases: &mut Vec<CollectedAlias>,
    mappings: &MappingBundle,
) -> Result<(), Diagnostic> {
    for declaration in declarations {
        match declaration {
            Declaration::Package(package) => collect_package(
                package,
                owner_package_segments,
                packages,
                imports,
                definitions,
                usages,
                aliases,
                mappings,
            )?,
            Declaration::Import(import_decl) => imports.push(CollectedImport {
                owner_package_qualified_name: owner_package_qualified_name.map(str::to_string),
                decl: import_decl.clone(),
            }),
            Declaration::PartDefinition(definition) => {
                let qualified_segments =
                    qualify_segments(owner_package_segments, &[definition.name.clone()]);
                definitions.push(collect_part_definition(
                    definition,
                    owner_package_segments,
                    mappings,
                )?);
                collect_nested_owned_definitions(
                    &definition.members,
                    &qualified_segments,
                    definitions,
                    mappings,
                )?;
                collect_nested_member_imports(
                    &definition.members,
                    &qualified_segments.join("."),
                    imports,
                );
                collect_nested_owned_packages(
                    &definition.members,
                    &qualified_segments,
                    packages,
                    imports,
                    definitions,
                    usages,
                    aliases,
                    mappings,
                )?;
            }
            Declaration::GenericDefinition(definition) => {
                let qualified_segments =
                    qualify_segments(owner_package_segments, &[definition.name.clone()]);
                definitions.push(collect_generic_definition(
                    definition,
                    owner_package_segments,
                    mappings,
                )?);
                collect_nested_owned_definitions(
                    &definition.members,
                    &qualified_segments,
                    definitions,
                    mappings,
                )?;
                collect_nested_member_imports(
                    &definition.members,
                    &qualified_segments.join("."),
                    imports,
                );
                collect_nested_owned_packages(
                    &definition.members,
                    &qualified_segments,
                    packages,
                    imports,
                    definitions,
                    usages,
                    aliases,
                    mappings,
                )?;
            }
            Declaration::PartUsage(usage) => {
                let owner = owner_package_qualified_name.unwrap_or("root");
                usages.push(collect_part_usage(usage, owner, "Package", mappings));
                let qualified_name = usage_qualified_name(owner, &usage.name);
                collect_nested_member_imports(&usage.body_members, &qualified_name, imports);
            }
            Declaration::GenericUsage(usage) => {
                let owner = owner_package_qualified_name.unwrap_or("root");
                usages.push(collect_generic_usage(usage, owner, "Package", mappings));
                let qualified_name = usage_qualified_name(owner, &usage.name);
                collect_nested_member_imports(&usage.body_members, &qualified_name, imports);
            }
            Declaration::Alias(alias) => aliases.push(collect_alias(alias, owner_package_segments)),
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn collect_nested_owned_packages(
    declarations: &[Declaration],
    owner_package_segments: &[String],
    packages: &mut Vec<ResolvedPackage>,
    imports: &mut Vec<CollectedImport>,
    definitions: &mut Vec<CollectedDefinition>,
    usages: &mut Vec<CollectedUsage>,
    aliases: &mut Vec<CollectedAlias>,
    mappings: &MappingBundle,
) -> Result<(), Diagnostic> {
    for declaration in declarations {
        if let Declaration::Package(package) = declaration {
            collect_package(
                package,
                owner_package_segments,
                packages,
                imports,
                definitions,
                usages,
                aliases,
                mappings,
            )?;
        }
    }

    Ok(())
}

fn collect_nested_owned_definitions(
    declarations: &[Declaration],
    owner_package_segments: &[String],
    definitions: &mut Vec<CollectedDefinition>,
    mappings: &MappingBundle,
) -> Result<(), Diagnostic> {
    for declaration in declarations {
        match declaration {
            Declaration::PartDefinition(definition) => {
                definitions.push(collect_part_definition(
                    definition,
                    owner_package_segments,
                    mappings,
                )?);
                collect_nested_owned_definitions(
                    &definition.members,
                    &qualify_segments(owner_package_segments, &[definition.name.clone()]),
                    definitions,
                    mappings,
                )?;
            }
            Declaration::GenericDefinition(definition) => {
                definitions.push(collect_generic_definition(
                    definition,
                    owner_package_segments,
                    mappings,
                )?);
                collect_nested_owned_definitions(
                    &definition.members,
                    &qualify_segments(owner_package_segments, &[definition.name.clone()]),
                    definitions,
                    mappings,
                )?;
            }
            Declaration::Package(_) => {}
            _ => {}
        }
    }

    Ok(())
}

fn collect_nested_member_imports(
    declarations: &[Declaration],
    owner_qualified_name: &str,
    imports: &mut Vec<CollectedImport>,
) {
    for declaration in declarations {
        match declaration {
            Declaration::Import(import_decl) => imports.push(CollectedImport {
                owner_package_qualified_name: Some(owner_qualified_name.to_string()),
                decl: import_decl.clone(),
            }),
            Declaration::PartUsage(usage) => {
                let qualified_name = usage_qualified_name(owner_qualified_name, &usage.name);
                collect_nested_member_imports(&usage.body_members, &qualified_name, imports);
            }
            Declaration::GenericUsage(usage) => {
                let qualified_name = usage_qualified_name(owner_qualified_name, &usage.name);
                collect_nested_member_imports(&usage.body_members, &qualified_name, imports);
            }
            Declaration::PartDefinition(definition) => {
                let qualified_name = usage_qualified_name(owner_qualified_name, &definition.name);
                collect_nested_member_imports(&definition.members, &qualified_name, imports);
            }
            Declaration::GenericDefinition(definition) => {
                let qualified_name = usage_qualified_name(owner_qualified_name, &definition.name);
                collect_nested_member_imports(&definition.members, &qualified_name, imports);
            }
            Declaration::Package(package) => {
                let qualified_name =
                    usage_qualified_name(owner_qualified_name, &package.name.as_dot_string());
                collect_nested_member_imports(&package.members, &qualified_name, imports);
            }
            Declaration::Alias(_) => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_package(
    package: &PackageDecl,
    owner_package_segments: &[String],
    packages: &mut Vec<ResolvedPackage>,
    imports: &mut Vec<CollectedImport>,
    definitions: &mut Vec<CollectedDefinition>,
    usages: &mut Vec<CollectedUsage>,
    aliases: &mut Vec<CollectedAlias>,
    mappings: &MappingBundle,
) -> Result<(), Diagnostic> {
    let package_segments = qualify_segments(owner_package_segments, &package.name.segments);
    let qualified_name = package_segments.join(".");

    packages.push(ResolvedPackage {
        owner_package_qualified_name: (!owner_package_segments.is_empty())
            .then(|| owner_package_segments.join(".")),
        qualified_name: qualified_name.clone(),
        declared_name: package
            .name
            .segments
            .last()
            .cloned()
            .unwrap_or_else(|| qualified_name.clone()),
        docs: package.docs.clone(),
        span: package.span.clone(),
    });

    collect_declarations(
        &package.members,
        &package_segments,
        Some(&qualified_name),
        packages,
        imports,
        definitions,
        usages,
        aliases,
        mappings,
    )
}

fn collect_part_definition(
    definition: &PartDefinitionDecl,
    owner_package_segments: &[String],
    mappings: &MappingBundle,
) -> Result<CollectedDefinition, Diagnostic> {
    let qualified_name = qualify_name(owner_package_segments, &definition.name);
    let members = definition
        .members
        .iter()
        .filter_map(|member| match member {
            Declaration::PartUsage(usage) => Some(collect_part_usage(
                usage,
                &qualified_name,
                "PartDefinition",
                mappings,
            )),
            Declaration::GenericUsage(usage) => Some(collect_generic_usage(
                usage,
                &qualified_name,
                "PartDefinition",
                mappings,
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    let specializes = definition_specializations_with_default(
        "PartDefinition",
        &definition.specializes,
        mappings,
    );

    Ok(CollectedDefinition {
        construct: "PartDefinition".to_string(),
        qualified_name,
        declared_name: definition.name.clone(),
        is_abstract: definition
            .modifiers
            .iter()
            .any(|modifier| modifier == "abstract"),
        specializes,
        members,
        docs: definition.docs.clone(),
        span: definition.span.clone(),
    })
}

fn collect_generic_definition(
    definition: &GenericDefinitionDecl,
    owner_package_segments: &[String],
    mappings: &MappingBundle,
) -> Result<CollectedDefinition, Diagnostic> {
    let qualified_name = qualify_name(owner_package_segments, &definition.name);
    let construct = mappings.definition_construct_for(&definition.keyword);
    let mut members = definition
        .members
        .iter()
        .filter_map(|member| match member {
            Declaration::PartUsage(usage) => Some(collect_part_usage(
                usage,
                &qualified_name,
                &construct,
                mappings,
            )),
            Declaration::GenericUsage(usage) => Some(collect_generic_usage(
                usage,
                &qualified_name,
                &construct,
                mappings,
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    annotate_connection_definition_members(&construct, &mut members);
    let specializes =
        definition_specializations_with_default(&construct, &definition.specializes, mappings);

    Ok(CollectedDefinition {
        construct,
        qualified_name,
        declared_name: definition.name.clone(),
        is_abstract: definition
            .modifiers
            .iter()
            .any(|modifier| modifier == "abstract"),
        specializes,
        members,
        docs: definition.docs.clone(),
        span: definition.span.clone(),
    })
}

fn definition_specializations_with_default(
    construct: &str,
    explicit: &[QualifiedName],
    mappings: &MappingBundle,
) -> Vec<QualifiedName> {
    if !explicit.is_empty() {
        return explicit.to_vec();
    }

    let zero_span = SourceSpan {
        start_line: 0,
        start_col: 0,
        end_line: 0,
        end_col: 0,
    };
    let mut specializations = Vec::new();
    for semantic_specialization in mappings.semantic_specializations_for_definition(construct) {
        specializations.push(QualifiedName {
            segments: semantic_specialization
                .split("::")
                .map(str::to_string)
                .collect(),
            span: zero_span.clone(),
        });
    }
    specializations
}

fn collect_part_usage(
    usage: &PartUsageDecl,
    owner_qualified_name: &str,
    owner_construct: &str,
    mappings: &MappingBundle,
) -> CollectedUsage {
    let qualified_name = usage_qualified_name(owner_qualified_name, &usage.name);
    let members = usage
        .body_members
        .iter()
        .filter_map(|member| match member {
            Declaration::PartUsage(usage) => Some(collect_part_usage(
                usage,
                &qualified_name,
                "PartUsage",
                mappings,
            )),
            Declaration::GenericUsage(usage) => Some(collect_generic_usage(
                usage,
                &qualified_name,
                "PartUsage",
                mappings,
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    CollectedUsage {
        construct: "PartUsage".to_string(),
        owner_construct: owner_construct.to_string(),
        owner_qualified_name: owner_qualified_name.to_string(),
        qualified_name,
        declared_name: usage.name.clone(),
        is_implicit_name: usage.is_implicit_name,
        ty: usage.ty.clone(),
        additional_types: usage.additional_types.clone(),
        reference_target: None,
        multiplicity: usage.multiplicity.clone(),
        expression: usage.expression.clone(),
        specializes: usage.specializes.clone(),
        subsets: usage.subsets.clone(),
        redefines: usage.redefines.clone(),
        members,
        modifiers: usage.modifiers.clone(),
        docs: usage.docs.clone(),
        span: usage.span.clone(),
    }
}

fn collect_generic_usage(
    usage: &GenericUsageDecl,
    owner_qualified_name: &str,
    owner_construct: &str,
    mappings: &MappingBundle,
) -> CollectedUsage {
    let construct = mappings.usage_construct_for(&usage.keyword);
    let qualified_name = usage_qualified_name(owner_qualified_name, &usage.name);
    let members = usage
        .body_members
        .iter()
        .filter_map(|member| match member {
            Declaration::PartUsage(usage) => Some(collect_part_usage(
                usage,
                &qualified_name,
                &construct,
                mappings,
            )),
            Declaration::GenericUsage(usage) => Some(collect_generic_usage(
                usage,
                &qualified_name,
                &construct,
                mappings,
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    CollectedUsage {
        construct,
        owner_construct: owner_construct.to_string(),
        owner_qualified_name: owner_qualified_name.to_string(),
        qualified_name,
        declared_name: usage.name.clone(),
        is_implicit_name: usage.is_implicit_name,
        ty: usage.ty.clone(),
        additional_types: usage.additional_types.clone(),
        reference_target: usage.reference_target.clone(),
        multiplicity: usage.multiplicity.clone(),
        expression: usage.expression.clone(),
        specializes: usage.specializes.clone(),
        subsets: usage.subsets.clone(),
        redefines: usage.redefines.clone(),
        members,
        modifiers: usage.modifiers.clone(),
        docs: usage.docs.clone(),
        span: usage.span.clone(),
    }
}

fn annotate_connection_definition_members(
    definition_construct: &str,
    members: &mut [CollectedUsage],
) {
    if definition_construct != "ConnectionDefinition" {
        return;
    }

    let mut end_index = 0usize;
    for member in members {
        if member.construct == "PartUsage"
            && member.modifiers.iter().any(|modifier| modifier == "end")
        {
            let directional_modifier = if end_index == 0 {
                "end-source"
            } else {
                "end-target"
            };
            member.modifiers.push(directional_modifier.to_string());
            end_index += 1;
        }
    }
}

fn collect_alias(alias: &AliasDecl, owner_package_segments: &[String]) -> CollectedAlias {
    let target = if alias.target.segments.len() == 1 && !owner_package_segments.is_empty() {
        QualifiedName {
            segments: qualify_segments(owner_package_segments, &alias.target.segments),
            span: alias.target.span.clone(),
        }
    } else {
        alias.target.clone()
    };
    CollectedAlias {
        qualified_name: qualify_name(owner_package_segments, &alias.name),
        declared_name: alias.name.clone(),
        target,
    }
}

fn collect_alias_in_owner(alias: &AliasDecl, owner_qualified_name: &str) -> CollectedAlias {
    let target = if alias.target.segments.len() == 1 && owner_qualified_name != "root" {
        let mut segments = owner_qualified_name
            .split('.')
            .map(str::to_string)
            .collect::<Vec<_>>();
        segments.extend(alias.target.segments.clone());
        QualifiedName {
            segments,
            span: alias.target.span.clone(),
        }
    } else {
        alias.target.clone()
    };
    CollectedAlias {
        qualified_name: usage_qualified_name(owner_qualified_name, &alias.name),
        declared_name: alias.name.clone(),
        target,
    }
}

fn collect_nested_aliases(
    declarations: &[Declaration],
    owner_package_segments: &[String],
    owner_qualified_name: Option<&str>,
    aliases: &mut Vec<CollectedAlias>,
) {
    for declaration in declarations {
        match declaration {
            Declaration::Package(package) => {
                let package_segments = qualify_segments(owner_package_segments, &package.name.segments);
                let package_qualified_name = package_segments.join(".");
                collect_nested_aliases(
                    &package.members,
                    &package_segments,
                    Some(&package_qualified_name),
                    aliases,
                );
            }
            Declaration::PartDefinition(definition) => {
                let qualified_name = qualify_name(owner_package_segments, &definition.name);
                collect_nested_member_aliases(&definition.members, &qualified_name, aliases);
            }
            Declaration::GenericDefinition(definition) => {
                let qualified_name = qualify_name(owner_package_segments, &definition.name);
                collect_nested_member_aliases(&definition.members, &qualified_name, aliases);
            }
            Declaration::PartUsage(usage) => {
                let qualified_name =
                    usage_qualified_name(owner_qualified_name.unwrap_or("root"), &usage.name);
                collect_nested_member_aliases(&usage.body_members, &qualified_name, aliases);
            }
            Declaration::GenericUsage(usage) => {
                let qualified_name =
                    usage_qualified_name(owner_qualified_name.unwrap_or("root"), &usage.name);
                collect_nested_member_aliases(&usage.body_members, &qualified_name, aliases);
            }
            Declaration::Import(_) | Declaration::Alias(_) => {}
        }
    }
}

fn collect_nested_member_aliases(
    declarations: &[Declaration],
    owner_qualified_name: &str,
    aliases: &mut Vec<CollectedAlias>,
) {
    for declaration in declarations {
        match declaration {
            Declaration::Alias(alias) => aliases.push(collect_alias_in_owner(alias, owner_qualified_name)),
            Declaration::PartUsage(usage) => {
                let qualified_name = usage_qualified_name(owner_qualified_name, &usage.name);
                collect_nested_member_aliases(&usage.body_members, &qualified_name, aliases);
            }
            Declaration::GenericUsage(usage) => {
                let qualified_name = usage_qualified_name(owner_qualified_name, &usage.name);
                collect_nested_member_aliases(&usage.body_members, &qualified_name, aliases);
            }
            Declaration::PartDefinition(definition) => {
                let qualified_name = usage_qualified_name(owner_qualified_name, &definition.name);
                collect_nested_member_aliases(&definition.members, &qualified_name, aliases);
            }
            Declaration::GenericDefinition(definition) => {
                let qualified_name = usage_qualified_name(owner_qualified_name, &definition.name);
                collect_nested_member_aliases(&definition.members, &qualified_name, aliases);
            }
            Declaration::Package(package) => {
                let qualified_name = usage_qualified_name(owner_qualified_name, &package.name.as_dot_string());
                collect_nested_member_aliases(&package.members, &qualified_name, aliases);
            }
            Declaration::Import(_) => {}
        }
    }
}

fn build_local_definition_map(
    definitions: &[CollectedDefinition],
) -> Result<BTreeMap<String, String>, Diagnostic> {
    let mut simple_names = BTreeMap::<String, String>::new();
    let mut duplicates = BTreeSet::new();
    let mut resolved = BTreeMap::new();

    for definition in definitions {
        let id = format!("type.{}", definition.qualified_name);
        resolved.insert(definition.qualified_name.clone(), id.clone());
        if definition.construct == "PortDefinition" {
            let conjugated_name = format!("~{}", definition.declared_name);
            let conjugated_id = format!("type.{}.{}", definition.qualified_name, conjugated_name);
            resolved.insert(conjugated_name.clone(), conjugated_id.clone());
            if let Some((owner, _)) = definition.qualified_name.rsplit_once('.') {
                resolved.insert(format!("{owner}.{conjugated_name}"), conjugated_id);
            }
        }
        match simple_names.entry(definition.declared_name.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(id);
            }
            std::collections::btree_map::Entry::Occupied(_) => {
                duplicates.insert(definition.declared_name.clone());
            }
        }
    }

    for duplicate in duplicates {
        simple_names.remove(&duplicate);
    }

    for (simple, id) in simple_names {
        resolved.entry(simple).or_insert(id);
    }

    Ok(resolved)
}

fn build_local_alias_map(aliases: &[CollectedAlias]) -> BTreeMap<String, QualifiedName> {
    let mut simple_aliases = BTreeMap::<String, QualifiedName>::new();
    let mut duplicates = BTreeSet::new();
    let mut resolved = BTreeMap::new();

    for alias in aliases {
        resolved.insert(alias.qualified_name.clone(), alias.target.clone());
        match simple_aliases.entry(alias.declared_name.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(alias.target.clone());
            }
            std::collections::btree_map::Entry::Occupied(_) => {
                duplicates.insert(alias.declared_name.clone());
            }
        }
    }

    for duplicate in duplicates {
        simple_aliases.remove(&duplicate);
    }

    for (simple, target) in simple_aliases {
        resolved.entry(simple).or_insert(target);
    }

    resolved
}

fn build_stdlib_feature_index(stdlib: &KirDocument) -> BTreeMap<String, BTreeMap<String, String>> {
    let direct_features = stdlib.elements.iter().fold(
        BTreeMap::<String, BTreeMap<String, String>>::new(),
        |mut acc, element| {
            if let Some((owner, feature_name)) = element.id.rsplit_once("::") {
                acc.entry(owner.to_string())
                    .or_default()
                    .entry(feature_name.to_string())
                    .or_insert_with(|| element.id.clone());
            }
            acc
        },
    );
    let specializations = stdlib
        .elements
        .iter()
        .map(|element| {
            let parents = element
                .properties
                .get("specializes")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            (element.id.clone(), parents)
        })
        .collect::<BTreeMap<_, _>>();

    let mut resolved = BTreeMap::new();
    let mut resolving = BTreeSet::new();
    for owner in specializations.keys() {
        collect_stdlib_owner_features(
            owner,
            &direct_features,
            &specializations,
            &mut resolved,
            &mut resolving,
        );
    }
    resolved
}

fn cached_stdlib_indexes(stdlib: &KirDocument, mappings: &MappingBundle) -> Arc<StdlibIndexes> {
    static CACHE: OnceLock<Mutex<BTreeMap<(usize, usize, u64), Arc<StdlibIndexes>>>> =
        OnceLock::new();

    let key = stdlib_instance_key(stdlib);
    let cache = CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    {
        let guard = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(indexes) = guard.get(&key) {
            return indexes.clone();
        }
    }

    let indexes = Arc::new(StdlibIndexes {
        ids: stdlib
            .elements
            .iter()
            .map(|element| element.id.clone())
            .collect::<Vec<_>>(),
        feature_index: build_stdlib_feature_index(stdlib),
        aliases: build_stdlib_alias_map(stdlib, mappings),
    });

    let mut guard = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.entry(key).or_insert_with(|| indexes.clone()).clone()
}

fn stdlib_instance_key(stdlib: &KirDocument) -> (usize, usize, u64) {
    let mut hasher = DefaultHasher::new();
    for element in &stdlib.elements {
        element.id.hash(&mut hasher);
    }
    (
        stdlib.elements.as_ptr() as usize,
        stdlib.elements.len(),
        hasher.finish(),
    )
}

fn collect_stdlib_owner_features(
    owner: &str,
    direct_features: &BTreeMap<String, BTreeMap<String, String>>,
    specializations: &BTreeMap<String, Vec<String>>,
    resolved: &mut BTreeMap<String, BTreeMap<String, String>>,
    resolving: &mut BTreeSet<String>,
) -> BTreeMap<String, String> {
    if let Some(existing) = resolved.get(owner) {
        return existing.clone();
    }
    if !resolving.insert(owner.to_string()) {
        return direct_features.get(owner).cloned().unwrap_or_default();
    }

    let mut features = BTreeMap::new();
    if let Some(parents) = specializations.get(owner) {
        for parent in parents {
            for (name, feature_id) in collect_stdlib_owner_features(
                parent,
                direct_features,
                specializations,
                resolved,
                resolving,
            ) {
                features.entry(name).or_insert(feature_id);
            }
        }
    }
    if let Some(local) = direct_features.get(owner) {
        for (name, feature_id) in local {
            features.insert(name.clone(), feature_id.clone());
        }
    }

    resolving.remove(owner);
    resolved.insert(owner.to_string(), features.clone());
    features
}

fn resolve_imports(
    imports: &[CollectedImport],
    stdlib_ids: &[String],
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
) -> Result<Vec<ResolvedImport>, Diagnostic> {
    let mut resolved = Vec::new();

    for import in imports {
        let target_id = resolve_import_target(
            &import.decl.path,
            stdlib_ids,
            stdlib_aliases,
            local_definitions,
            local_aliases,
        )
        .ok_or_else(|| {
            Diagnostic::new(
                format!("unresolved import `{}`", import.decl.path.as_colon_string()),
                Some(import.decl.span.clone()),
            )
        })?;

        resolved.push(ResolvedImport {
            owner_package_qualified_name: import.owner_package_qualified_name.clone(),
            target_id,
            imported_name: import
                .decl
                .path
                .segments
                .last()
                .cloned()
                .filter(|name| name != "*" && name != "**"),
            docs: import.decl.docs.clone(),
            span: import.decl.span.clone(),
            ordinal: resolved.len() + 1,
        });
    }

    Ok(resolved)
}

fn build_import_alias_map(
    imports: &[ResolvedImport],
    packages: &[ResolvedPackage],
    definitions: &[CollectedDefinition],
    usages: &BTreeMap<String, CollectedUsage>,
    stdlib_ids: &[String],
    stdlib_aliases: &BTreeMap<String, String>,
    policy: ResolvePolicy,
) -> Result<ImportAliases, Diagnostic> {
    let mut aliases = ImportAliases::default();
    let root_package = packages
        .iter()
        .find(|package| package.owner_package_qualified_name.is_none())
        .or_else(|| packages.first())
        .map(|package| package.qualified_name.clone());

    for import in imports {
        if import.target_id.ends_with("::*") || import.target_id.ends_with("::**") {
            let namespace = import_namespace_prefix(&import.target_id);
            add_wildcard_import_aliases(
                &namespace,
                import.owner_package_qualified_name.as_deref().unwrap_or(""),
                &root_package,
                packages,
                definitions,
                usages,
                stdlib_ids,
                stdlib_aliases,
                &mut aliases,
                &import.span,
                policy,
            )?;
            continue;
        }

        if let Some(alias) = import
            .imported_name
            .as_deref()
            .or_else(|| import.target_id.rsplit("::").next())
        {
            bind_value_alias(
                &mut aliases,
                alias,
                import.target_id.clone(),
                &import.span,
                policy,
            )?;
            bind_owner_qualified_value_aliases(
                &mut aliases,
                import.owner_package_qualified_name.as_deref().unwrap_or(""),
                alias,
                import.target_id.clone(),
                &import.span,
                policy,
            )?;
        }
    }
    Ok(aliases)
}

#[allow(clippy::too_many_arguments)]
fn add_wildcard_import_aliases(
    namespace: &str,
    owner_package_qualified_name: &str,
    root_package: &Option<String>,
    packages: &[ResolvedPackage],
    definitions: &[CollectedDefinition],
    usages: &BTreeMap<String, CollectedUsage>,
    stdlib_ids: &[String],
    stdlib_aliases: &BTreeMap<String, String>,
    aliases: &mut ImportAliases,
    span: &SourceSpan,
    policy: ResolvePolicy,
) -> Result<(), Diagnostic> {
    let local_namespace = resolve_local_namespace_dot(
        namespace,
        owner_package_qualified_name,
        root_package,
        packages,
    );

    if let Some(namespace_dot) = local_namespace {
        if let Some(alias) = namespace_dot.rsplit('.').next() {
            bind_namespace_alias(
                aliases,
                alias,
                dotted_name_to_qualified_name(&namespace_dot, span),
                span,
                policy,
            )?;
        }

        let namespace_prefix = format!("{namespace_dot}.");

        for package in packages {
            if let Some(child) = direct_child_name(&package.qualified_name, &namespace_prefix) {
                bind_namespace_alias(
                    aliases,
                    child,
                    dotted_name_to_qualified_name(&package.qualified_name, span),
                    span,
                    policy,
                )?;
            }
        }

        for definition in definitions {
            if let Some(child) = direct_child_name(&definition.qualified_name, &namespace_prefix) {
                let target = format!("type.{}", definition.qualified_name);
                bind_value_alias(aliases, child, target.clone(), span, policy)?;
                bind_owner_qualified_value_aliases(
                    aliases,
                    owner_package_qualified_name,
                    child,
                    target,
                    span,
                    policy,
                )?;
            }
        }

        for usage in usages.values() {
            if let Some(child) = direct_child_name(&usage.qualified_name, &namespace_prefix) {
                let target = format!("feature.{}", usage.qualified_name);
                bind_value_alias(aliases, child, target.clone(), span, policy)?;
                bind_owner_qualified_value_aliases(
                    aliases,
                    owner_package_qualified_name,
                    child,
                    target,
                    span,
                    policy,
                )?;
            }
        }
    } else {
        if let Some(alias) = namespace.rsplit("::").next() {
            bind_namespace_alias(
                aliases,
                alias,
                dotted_name_to_qualified_name(&namespace.replace("::", "."), span),
                span,
                policy,
            )?;
        }

        let namespace_prefix = format!("{namespace}::");
        for id in stdlib_ids {
            if let Some(child) = direct_child_name(id, &namespace_prefix) {
                bind_value_alias(aliases, child, id.clone(), span, policy)?;
                bind_owner_qualified_value_aliases(
                    aliases,
                    owner_package_qualified_name,
                    child,
                    id.clone(),
                    span,
                    policy,
                )?;
            }
        }
        let namespace_alias_prefix = format!("{namespace_prefix}");
        for (alias, target) in stdlib_aliases {
            let Some(short_alias) = alias.strip_prefix(&namespace_alias_prefix) else {
                continue;
            };
            if short_alias.contains("::") {
                continue;
            }
            bind_value_alias(aliases, short_alias, target.clone(), span, policy)?;
            bind_owner_qualified_value_aliases(
                aliases,
                owner_package_qualified_name,
                short_alias,
                target.clone(),
                span,
                policy,
            )?;
        }
    }

    Ok(())
}

fn bind_value_alias(
    aliases: &mut ImportAliases,
    alias: &str,
    target: String,
    _span: &SourceSpan,
    _policy: ResolvePolicy,
) -> Result<(), Diagnostic> {
    if aliases.ambiguous_value_aliases.contains(alias) {
        return Ok(());
    }

    match aliases.value_aliases.get(alias) {
        Some(existing) if existing != &target => {
            aliases.value_aliases.remove(alias);
            aliases.ambiguous_value_aliases.insert(alias.to_string());
            Ok(())
        }
        _ => {
            aliases.value_aliases.insert(alias.to_string(), target);
            Ok(())
        }
    }
}

fn bind_namespace_alias(
    aliases: &mut ImportAliases,
    alias: &str,
    target: QualifiedName,
    _span: &SourceSpan,
    _policy: ResolvePolicy,
) -> Result<(), Diagnostic> {
    if aliases.ambiguous_namespace_aliases.contains(alias) {
        return Ok(());
    }

    match aliases.namespace_aliases.get(alias) {
        Some(existing) if !qualified_names_match(existing, &target) => {
            aliases.namespace_aliases.remove(alias);
            aliases
                .ambiguous_namespace_aliases
                .insert(alias.to_string());
            Ok(())
        }
        _ => {
            aliases.namespace_aliases.insert(alias.to_string(), target);
            Ok(())
        }
    }
}

fn bind_owner_qualified_value_aliases(
    aliases: &mut ImportAliases,
    owner_package_qualified_name: &str,
    alias: &str,
    target: String,
    span: &SourceSpan,
    policy: ResolvePolicy,
) -> Result<(), Diagnostic> {
    if owner_package_qualified_name.is_empty() {
        return Ok(());
    }

    let segments = owner_package_qualified_name.split('.').collect::<Vec<_>>();
    for start in 0..segments.len() {
        let qualified_alias = format!("{}.{}", segments[start..].join("."), alias);
        bind_value_alias(aliases, &qualified_alias, target.clone(), span, policy)?;
    }
    Ok(())
}

fn qualified_names_match(left: &QualifiedName, right: &QualifiedName) -> bool {
    left.segments == right.segments
        || qualified_name_suffix_matches(&left.segments, &right.segments)
        || qualified_name_suffix_matches(&right.segments, &left.segments)
}

fn qualified_name_suffix_matches(longer: &[String], shorter: &[String]) -> bool {
    longer.len() >= shorter.len() && longer[longer.len() - shorter.len()..] == *shorter
}

fn resolve_definition(
    definition: CollectedDefinition,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
    policy: ResolvePolicy,
) -> Result<ResolvedDefinition, Diagnostic> {
    let specializes = definition
        .specializes
        .iter()
        .map(|name| {
            unresolved_or_error(
                resolve_type_reference_in_scope(
                    name,
                    &definition.qualified_name,
                    stdlib_ids,
                    stdlib_aliases,
                    local_definitions,
                    local_aliases,
                    import_aliases,
                ),
                name,
                "specialization",
                policy,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let members = definition
        .members
        .into_iter()
        .map(|usage| {
            resolve_usage(
                usage,
                stdlib_ids,
                stdlib_feature_index,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
                definition_index,
                local_feature_index,
                local_usage_map,
                policy,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ResolvedDefinition {
        construct: definition.construct,
        qualified_name: definition.qualified_name,
        declared_name: definition.declared_name,
        is_abstract: definition.is_abstract,
        specializes,
        members,
        docs: definition.docs,
        span: definition.span,
    })
}

fn resolve_usage(
    usage: CollectedUsage,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
    policy: ResolvePolicy,
) -> Result<ResolvedUsage, Diagnostic> {
    let mut effective_reference_target = usage.reference_target.clone();
    let mut effective_redefines = usage.redefines.clone();
    if usage.construct == "ReferenceUsage"
        && usage.is_implicit_name
        && usage.declared_name == "ref"
        && usage.ty.is_none()
        && effective_reference_target.is_none()
        && effective_redefines.len() == 1
    {
        effective_reference_target = effective_redefines.first().cloned();
        effective_redefines.clear();
    }
    if matches!(usage.construct.as_str(), "SatisfyUsage" | "VerifyUsage")
        && effective_reference_target.is_none()
        && !usage.declared_name.is_empty()
    {
        effective_reference_target = Some(QualifiedName {
            segments: vec![usage.declared_name.clone()],
            span: usage.span.clone(),
        });
    }
    let expression = usage
        .expression
        .as_ref()
        .map(|expr| {
            resolve_expression(
                &usage,
                expr,
                stdlib_ids,
                stdlib_feature_index,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
                definition_index,
                local_feature_index,
                local_usage_map,
            )
        })
        .transpose()?;
    let mut specialized_features = Vec::new();
    let mut type_ref = match &usage.ty {
        Some(name) => {
            if let Some(target) = resolve_type_reference_in_scope(
                name,
                &usage.owner_qualified_name,
                stdlib_ids,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
            ) {
                Some(target)
            } else if let Some(target) = resolve_feature_reference(
                &usage,
                name,
                stdlib_ids,
                stdlib_feature_index,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
                definition_index,
                local_feature_index,
                local_usage_map,
            ) {
                specialized_features.push(target);
                None
            } else {
                Some(unresolved_or_error(None, name, "type", policy)?)
            }
        }
        None => None,
    };
    let reference_target = effective_reference_target
        .as_ref()
        .map(|name| {
            if matches!(usage.construct.as_str(), "SatisfyUsage" | "VerifyUsage") {
                resolve_type_reference_in_scope(
                    name,
                    &usage.owner_qualified_name,
                    stdlib_ids,
                    stdlib_aliases,
                    local_definitions,
                    local_aliases,
                    import_aliases,
                )
            } else {
                resolve_reference_usage_target(
                    &usage,
                    name,
                    stdlib_ids,
                    stdlib_feature_index,
                    stdlib_aliases,
                    local_definitions,
                    local_aliases,
                    import_aliases,
                    definition_index,
                    local_feature_index,
                    local_usage_map,
                )
            }
            .ok_or_else(|| {
                Diagnostic::new(
                    format!("unresolved reference target `{}`", name.as_colon_string()),
                    Some(name.span.clone()),
                )
            })
        })
        .transpose()?;
    let additional_type_refs = usage
        .additional_types
        .iter()
        .map(|name| {
            unresolved_or_error(
                resolve_type_reference_in_scope(
                    name,
                    &usage.owner_qualified_name,
                    stdlib_ids,
                    stdlib_aliases,
                    local_definitions,
                    local_aliases,
                    import_aliases,
                ),
                name,
                "type",
                policy,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut specializes = Vec::new();
    for name in &usage.specializes {
        if let Some(target) = resolve_feature_reference(
            &usage,
            name,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            local_usage_map,
        ) {
            specialized_features.push(target);
        } else if is_self_feature_reference(&usage, name) {
            specialized_features.push(feature_id_from_qualified_name(&usage.qualified_name));
        } else if let Some(target) = resolve_type_reference_in_scope(
            name,
            &usage.owner_qualified_name,
            stdlib_ids,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
        ) {
            specializes.push(target);
        } else {
            specializes.push(unresolved_or_error(None, name, "specialization", policy)?);
        }
    }
    let subsetted_features = usage
        .subsets
        .iter()
        .map(|name| {
            unresolved_or_error(
                resolve_feature_reference(
                    &usage,
                    name,
                    stdlib_ids,
                    stdlib_feature_index,
                    stdlib_aliases,
                    local_definitions,
                    local_aliases,
                    import_aliases,
                    definition_index,
                    local_feature_index,
                    local_usage_map,
                ),
                name,
                "subset target",
                policy,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let redefined_features = effective_redefines
        .iter()
        .map(|name| {
            unresolved_or_error(
                resolve_redefinition_feature_reference(
                    &usage,
                    name,
                    stdlib_ids,
                    stdlib_feature_index,
                    stdlib_aliases,
                    local_definitions,
                    local_aliases,
                    import_aliases,
                    definition_index,
                    local_feature_index,
                    local_usage_map,
                ),
                name,
                "redefinition target",
                policy,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    if usage.construct == "ReferenceUsage" {
        if let Some(parent_feature) = resolve_connection_end_specialization(
            &usage,
            stdlib_ids,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_usage_map,
        ) {
            specialized_features.push(parent_feature);
        }
        if type_ref.is_none() {
            type_ref = reference_target.as_deref().and_then(|target| {
                infer_usage_type_from_feature_id(
                    target,
                    stdlib_ids,
                    stdlib_aliases,
                    local_definitions,
                    local_aliases,
                    import_aliases,
                    local_usage_map,
                )
            });
        }
    }
    let type_ref = type_ref
        .or_else(|| {
            inferred_usage_type_ref(
                &usage,
                &redefined_features,
                &subsetted_features,
                &specialized_features,
                stdlib_ids,
                stdlib_feature_index,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
                definition_index,
                local_feature_index,
                local_usage_map,
            )
        })
        .or_else(|| infer_named_definition_type_ref(&usage, local_definitions));
    let members = usage
        .members
        .into_iter()
        .map(|member| {
            resolve_usage(
                member,
                stdlib_ids,
                stdlib_feature_index,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
                definition_index,
                local_feature_index,
                local_usage_map,
                policy,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ResolvedUsage {
        construct: usage.construct,
        owner_construct: usage.owner_construct,
        owner_qualified_name: usage.owner_qualified_name,
        qualified_name: usage.qualified_name,
        declared_name: usage.declared_name,
        is_implicit_name: usage.is_implicit_name,
        has_explicit_type: usage.ty.is_some() || !usage.additional_types.is_empty(),
        type_ref,
        additional_type_refs,
        reference_target,
        multiplicity: usage.multiplicity,
        expression,
        is_derived: usage.modifiers.iter().any(|modifier| modifier == "derived")
            || (usage.expression.is_some() && effective_redefines.is_empty()),
        specializes,
        specialized_features,
        subsetted_features,
        redefined_features,
        members,
        modifiers: usage.modifiers,
        docs: usage.docs,
        span: usage.span,
    })
}

#[allow(clippy::too_many_arguments)]
fn resolve_expression(
    usage: &CollectedUsage,
    expr: &Expr,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
) -> Result<ResolvedExpr, Diagnostic> {
    match expr {
        Expr::Literal(LiteralExpr::Integer(value)) => {
            Ok(ResolvedExpr::Literal(Value::from(*value)))
        }
        Expr::Literal(LiteralExpr::Real(value)) => {
            let parsed = value.parse::<f64>().map_err(|_| {
                Diagnostic::new("invalid real literal", Some(expression_span(expr)))
            })?;
            Ok(ResolvedExpr::Literal(Value::from(parsed)))
        }
        Expr::Literal(LiteralExpr::Boolean(value)) => {
            Ok(ResolvedExpr::Literal(Value::from(*value)))
        }
        Expr::Literal(LiteralExpr::String(value)) => {
            Ok(ResolvedExpr::Literal(Value::from(value.clone())))
        }
        Expr::SelfRef(_) => Ok(ResolvedExpr::SelfRef),
        Expr::Tuple { items, .. } => Ok(ResolvedExpr::Tuple {
            items: items
                .iter()
                .map(|item| {
                    resolve_expression(
                        usage,
                        item,
                        stdlib_ids,
                        stdlib_feature_index,
                        stdlib_aliases,
                        local_definitions,
                        local_aliases,
                        import_aliases,
                        definition_index,
                        local_feature_index,
                        local_usage_map,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
        }),
        Expr::Name(name) => resolve_expression_name(
            usage,
            name,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            local_usage_map,
        ),
        Expr::Path { .. } => resolve_expression_path(
            usage,
            expr,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            local_usage_map,
        ),
        Expr::Unary { op, expr, .. } => Ok(ResolvedExpr::Unary {
            op: op.clone(),
            expr: Box::new(resolve_expression(
                usage,
                expr,
                stdlib_ids,
                stdlib_feature_index,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
                definition_index,
                local_feature_index,
                local_usage_map,
            )?),
        }),
        Expr::Binary {
            left, op, right, ..
        } => Ok(ResolvedExpr::Binary {
            left: Box::new(resolve_expression(
                usage,
                left,
                stdlib_ids,
                stdlib_feature_index,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
                definition_index,
                local_feature_index,
                local_usage_map,
            )?),
            op: op.clone(),
            right: Box::new(resolve_expression(
                usage,
                right,
                stdlib_ids,
                stdlib_feature_index,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
                definition_index,
                local_feature_index,
                local_usage_map,
            )?),
        }),
        Expr::Call { function, args, .. } => Ok(ResolvedExpr::Call {
            function: function.clone(),
            args: args
                .iter()
                .map(|arg| {
                    resolve_expression(
                        usage,
                        arg,
                        stdlib_ids,
                        stdlib_feature_index,
                        stdlib_aliases,
                        local_definitions,
                        local_aliases,
                        import_aliases,
                        definition_index,
                        local_feature_index,
                        local_usage_map,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
        }),
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_expression_name(
    usage: &CollectedUsage,
    name: &QualifiedName,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
) -> Result<ResolvedExpr, Diagnostic> {
    if let Some(feature_id) = resolve_feature_reference(
        usage,
        name,
        stdlib_ids,
        stdlib_feature_index,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
        definition_index,
        local_feature_index,
        local_usage_map,
    ) {
        let first = name
            .segments
            .last()
            .cloned()
            .unwrap_or_else(|| name.as_dot_string());
        return Ok(ResolvedExpr::FeaturePath {
            segments: vec![ResolvedPathSegment {
                name: first,
                feature_id,
            }],
        });
    }

    if let Some(feature_id) = resolve_qualified_reference(
        name,
        stdlib_ids,
        stdlib_aliases,
        local_definitions,
        local_aliases,
    ) {
        let first = name
            .segments
            .last()
            .cloned()
            .unwrap_or_else(|| name.as_dot_string());
        return Ok(ResolvedExpr::FeaturePath {
            segments: vec![ResolvedPathSegment {
                name: first,
                feature_id,
            }],
        });
    }

    if let Some(path) = resolve_qualified_expression_name_as_path(
        usage,
        name,
        stdlib_ids,
        stdlib_feature_index,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
        definition_index,
        local_feature_index,
        local_usage_map,
    ) {
        return Ok(path);
    }

    if name.segments.len() > 1 {
        let tail = QualifiedName {
            segments: vec![name.segments.last().cloned().unwrap_or_default()],
            span: name.span.clone(),
        };
        if let Some(feature_id) = resolve_feature_reference(
            usage,
            &tail,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            local_usage_map,
        ) {
            let first = tail
                .segments
                .last()
                .cloned()
                .unwrap_or_else(|| tail.as_dot_string());
            return Ok(ResolvedExpr::FeaturePath {
                segments: vec![ResolvedPathSegment {
                    name: first,
                    feature_id,
                }],
            });
        }
    }

    if let Some(feature_id) = resolve_type_reference(
        name,
        stdlib_ids,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
    ) {
        let first = name
            .segments
            .last()
            .cloned()
            .unwrap_or_else(|| name.as_dot_string());
        return Ok(ResolvedExpr::FeaturePath {
            segments: vec![ResolvedPathSegment {
                name: first,
                feature_id,
            }],
        });
    }

    Err(Diagnostic::new(
        format!("unresolved expression name `{}`", name.as_colon_string()),
        Some(name.span.clone()),
    ))
}

#[allow(clippy::too_many_arguments)]
fn resolve_qualified_expression_name_as_path(
    usage: &CollectedUsage,
    name: &QualifiedName,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
) -> Option<ResolvedExpr> {
    if name.segments.len() <= 1 {
        return None;
    }

    let root_name = QualifiedName {
        segments: vec![name.segments.first()?.clone()],
        span: name.span.clone(),
    };
    let mut current_feature_id = resolve_feature_reference(
        usage,
        &root_name,
        stdlib_ids,
        stdlib_feature_index,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
        definition_index,
        local_feature_index,
        local_usage_map,
    )
    .or_else(|| {
        resolve_type_reference(
            &root_name,
            stdlib_ids,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
        )
    })?;

    let mut bound_segments = vec![ResolvedPathSegment {
        name: root_name
            .segments
            .first()
            .cloned()
            .unwrap_or_else(|| root_name.as_dot_string()),
        feature_id: current_feature_id.clone(),
    }];

    for segment in name.segments.iter().skip(1) {
        let feature_id = resolve_feature_reference_from_feature_type(
            &current_feature_id,
            segment,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            local_usage_map,
        )?;
        bound_segments.push(ResolvedPathSegment {
            name: segment.clone(),
            feature_id: feature_id.clone(),
        });
        current_feature_id = feature_id;
    }

    Some(ResolvedExpr::FeaturePath {
        segments: bound_segments,
    })
}

#[allow(clippy::too_many_arguments)]
fn resolve_expression_path(
    usage: &CollectedUsage,
    expr: &Expr,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
) -> Result<ResolvedExpr, Diagnostic> {
    let (root, segments, span) = flatten_expression_path(expr).ok_or_else(|| {
        Diagnostic::new(
            "expression path must be rooted in `self` or a feature name",
            None,
        )
    })?;

    let mut bound_segments = Vec::new();
    let (mut current_feature_id, mut current_type_id) = match root {
        ExpressionPathRoot::SelfRef => (None, None),
        ExpressionPathRoot::Name(name) => {
            let feature_id = resolve_feature_reference(
                usage,
                &name,
                stdlib_ids,
                stdlib_feature_index,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
                definition_index,
                local_feature_index,
                local_usage_map,
            )
            .ok_or_else(|| {
                Diagnostic::new(
                    format!("unresolved expression root `{}`", name.as_colon_string()),
                    Some(name.span.clone()),
                )
            })?;
            let first_name = name
                .segments
                .last()
                .cloned()
                .unwrap_or_else(|| name.as_dot_string());
            bound_segments.push(ResolvedPathSegment {
                name: first_name,
                feature_id: feature_id.clone(),
            });
            (Some(feature_id), None)
        }
        ExpressionPathRoot::CastType(name) => {
            let type_id = resolve_type_reference(
                &name,
                stdlib_ids,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
            )
            .or_else(|| {
                resolve_feature_reference(
                    usage,
                    &name,
                    stdlib_ids,
                    stdlib_feature_index,
                    stdlib_aliases,
                    local_definitions,
                    local_aliases,
                    import_aliases,
                    definition_index,
                    local_feature_index,
                    local_usage_map,
                )
                .and_then(|feature_id| {
                    infer_usage_type_from_feature_id(
                        &feature_id,
                        stdlib_ids,
                        stdlib_aliases,
                        local_definitions,
                        local_aliases,
                        import_aliases,
                        local_usage_map,
                    )
                })
            })
            .ok_or_else(|| {
                Diagnostic::new(
                    format!("unresolved expression cast type `{}`", name.as_colon_string()),
                    Some(name.span.clone()),
                )
            })?;
            (None, Some(type_id))
        }
    };

    for segment in segments {
        let feature_id = if let Some(current_type_id) = &current_type_id {
            resolve_feature_reference_from_type_id(
                current_type_id,
                &segment,
                stdlib_ids,
                stdlib_feature_index,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
                definition_index,
                local_feature_index,
            )
        } else if let Some(current_feature_id) = &current_feature_id {
            resolve_feature_reference_from_feature_type(
                current_feature_id,
                &segment,
                stdlib_ids,
                stdlib_feature_index,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
                definition_index,
                local_feature_index,
                local_usage_map,
            )
        } else {
            let qualified = QualifiedName {
                segments: vec![segment.clone()],
                span: span.clone(),
            };
            resolve_feature_reference(
                usage,
                &qualified,
                stdlib_ids,
                stdlib_feature_index,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
                definition_index,
                local_feature_index,
                local_usage_map,
            )
        }
        .ok_or_else(|| {
            Diagnostic::new(
                format!("unresolved expression path segment `{segment}`"),
                Some(span.clone()),
            )
        })?;

        bound_segments.push(ResolvedPathSegment {
            name: segment.clone(),
            feature_id: feature_id.clone(),
        });
        current_feature_id = Some(feature_id);
        current_type_id = None;
    }

    Ok(ResolvedExpr::FeaturePath {
        segments: bound_segments,
    })
}

#[derive(Debug, Clone)]
enum ExpressionPathRoot {
    SelfRef,
    Name(QualifiedName),
    CastType(QualifiedName),
}

fn flatten_expression_path(expr: &Expr) -> Option<(ExpressionPathRoot, Vec<String>, SourceSpan)> {
    match expr {
        Expr::SelfRef(span) => Some((ExpressionPathRoot::SelfRef, Vec::new(), span.clone())),
        Expr::Name(name) => Some((
            ExpressionPathRoot::Name(name.clone()),
            Vec::new(),
            name.span.clone(),
        )),
        Expr::Path {
            root,
            segment,
            span,
        } => {
            let (base, mut segments, _) = flatten_expression_path(root)?;
            segments.push(segment.clone());
            Some((base, segments, span.clone()))
        }
        Expr::Call {
            function,
            args,
            span,
        } if args.len() == 1 && function.starts_with("as ") => {
            let type_name = function.strip_prefix("as ")?.trim();
            let segments = if type_name.contains("::") {
                type_name.split("::").map(str::to_string).collect()
            } else {
                type_name.split('.').map(str::to_string).collect()
            };
            Some((
                ExpressionPathRoot::CastType(QualifiedName {
                    segments,
                    span: span.clone(),
                }),
                Vec::new(),
                span.clone(),
            ))
        }
        _ => None,
    }
}

fn build_local_feature_index(
    definitions: &[CollectedDefinition],
    usages: &[CollectedUsage],
) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut index = BTreeMap::new();
    for definition in definitions {
        collect_feature_scope(&definition.members, &mut index);
    }
    collect_feature_scope(usages, &mut index);
    index
}

fn build_local_usage_map(
    definitions: &[CollectedDefinition],
    usages: &[CollectedUsage],
) -> BTreeMap<String, CollectedUsage> {
    let mut map = BTreeMap::new();
    for definition in definitions {
        collect_usage_map(&definition.members, &mut map);
    }
    collect_usage_map(usages, &mut map);
    map
}

fn collect_feature_scope(
    usages: &[CollectedUsage],
    index: &mut BTreeMap<String, BTreeMap<String, String>>,
) {
    for usage in usages {
        index
            .entry(usage.owner_qualified_name.clone())
            .or_default()
            .insert(usage.declared_name.clone(), usage.qualified_name.clone());
        collect_feature_scope(&usage.members, index);
    }
}

fn collect_usage_map(usages: &[CollectedUsage], map: &mut BTreeMap<String, CollectedUsage>) {
    for usage in usages {
        map.insert(usage.qualified_name.clone(), usage.clone());
        collect_usage_map(&usage.members, map);
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_feature_reference(
    usage: &CollectedUsage,
    name: &QualifiedName,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
) -> Option<String> {
    let mut seen_usages = BTreeSet::new();
    let mut seen_definitions = BTreeSet::new();
    resolve_feature_reference_with_seen(
        usage,
        name,
        stdlib_ids,
        stdlib_feature_index,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
        definition_index,
        local_feature_index,
        local_usage_map,
        &mut seen_usages,
        &mut seen_definitions,
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_feature_reference_with_seen(
    usage: &CollectedUsage,
    name: &QualifiedName,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
    seen_usages: &mut BTreeSet<String>,
    seen_definitions: &mut BTreeSet<String>,
) -> Option<String> {
    if let Some(scoped_local) = resolve_local_scoped_feature_reference(
        usage,
        name,
        stdlib_ids,
        stdlib_feature_index,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
        definition_index,
        local_feature_index,
        local_usage_map,
        seen_usages,
        seen_definitions,
    ) {
        return Some(scoped_local);
    }

    if let Some(scoped) = resolve_ancestor_scoped_feature_reference(
        usage,
        name,
        stdlib_ids,
        stdlib_feature_index,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
        definition_index,
        local_feature_index,
        local_usage_map,
        seen_usages,
        seen_definitions,
    ) {
        return Some(scoped);
    }

    if let Some(scoped) = resolve_enclosing_scope_sibling_feature_reference(
        usage,
        name,
        stdlib_ids,
        stdlib_feature_index,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
        definition_index,
        local_feature_index,
        local_usage_map,
        seen_usages,
        seen_definitions,
    ) {
        return Some(scoped);
    }

    if let Some(exact) =
        resolve_local_feature_name(name, local_aliases, import_aliases, local_feature_index)
    {
        if exact != usage.qualified_name {
            return Some(feature_id_from_qualified_name(&exact));
        }
    }

    if let Some(local) = resolve_owner_feature_name(
        &usage.owner_qualified_name,
        name,
        local_aliases,
        import_aliases,
        local_feature_index,
    ) {
        if local != usage.qualified_name {
            return Some(feature_id_from_qualified_name(&local));
        }
    }

    if let Some(ancestor_local) = resolve_enclosing_usage_feature_reference(
        &usage.owner_qualified_name,
        name,
        local_aliases,
        import_aliases,
        local_feature_index,
        local_usage_map,
    ) {
        if ancestor_local != usage.qualified_name {
            return Some(feature_id_from_qualified_name(&ancestor_local));
        }
    }

    if let Some(ancestor_inherited) = resolve_enclosing_usage_inherited_feature_reference(
        &usage.owner_qualified_name,
        name,
        stdlib_ids,
        stdlib_feature_index,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
        definition_index,
        local_feature_index,
        local_usage_map,
        seen_usages,
        seen_definitions,
    ) {
        if ancestor_inherited != usage.qualified_name {
            return Some(feature_id_from_qualified_name(&ancestor_inherited));
        }
    }

    if usage.owner_construct.ends_with("Definition") {
        if let Some(inherited) = resolve_inherited_definition_feature_reference(
            &usage.owner_qualified_name,
            name,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            seen_definitions,
        ) {
            return Some(feature_id_from_qualified_name(&inherited));
        }
    }

    if let Some(type_name) = &usage.ty
        && let Some(type_id) = resolve_type_reference(
            type_name,
            stdlib_ids,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
        )
        && let Some(definition_qualified_name) = type_id.strip_prefix("type.")
        && let Some(local) = resolve_owner_feature_name(
            definition_qualified_name,
            name,
            local_aliases,
            import_aliases,
            local_feature_index,
        )
        && local != usage.qualified_name
    {
        return Some(feature_id_from_qualified_name(&local));
    }

    if let Some(inherited) = resolve_owner_usage_feature_reference(
        &usage.owner_qualified_name,
        name,
        stdlib_ids,
        stdlib_feature_index,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
        definition_index,
        local_feature_index,
        local_usage_map,
        seen_usages,
        seen_definitions,
    ) {
        if inherited != usage.qualified_name {
            return Some(normalize_feature_target_id(&inherited));
        }
    }

    None
}

#[allow(clippy::too_many_arguments)]
fn resolve_local_scoped_feature_reference(
    usage: &CollectedUsage,
    name: &QualifiedName,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
    seen_usages: &mut BTreeSet<String>,
    seen_definitions: &mut BTreeSet<String>,
) -> Option<String> {
    if name.segments.len() < 2 {
        return None;
    }

    let scope = local_feature_index.get(&usage.owner_qualified_name)?;
    let head = name.segments.first()?;
    let scoped_target = scope.get(head)?;
    let scoped_usage = local_usage_map.get(scoped_target)?;
    let tail = QualifiedName {
        segments: name.segments[1..].to_vec(),
        span: name.span.clone(),
    };
    if let Some(local) = resolve_owner_feature_name(
        &scoped_usage.qualified_name,
        &tail,
        local_aliases,
        import_aliases,
        local_feature_index,
    ) {
        return Some(feature_id_from_qualified_name(&local));
    }
    if let Some(inherited) = resolve_owner_usage_feature_reference(
        &scoped_usage.qualified_name,
        &tail,
        stdlib_ids,
        stdlib_feature_index,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
        definition_index,
        local_feature_index,
        local_usage_map,
        seen_usages,
        seen_definitions,
    ) {
        return Some(feature_id_from_qualified_name(&inherited));
    }
    resolve_feature_reference_with_seen(
        scoped_usage,
        &tail,
        stdlib_ids,
        stdlib_feature_index,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
        definition_index,
        local_feature_index,
        local_usage_map,
        seen_usages,
        seen_definitions,
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_ancestor_scoped_feature_reference(
    usage: &CollectedUsage,
    name: &QualifiedName,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
    seen_usages: &mut BTreeSet<String>,
    seen_definitions: &mut BTreeSet<String>,
) -> Option<String> {
    if name.segments.len() < 2 {
        return None;
    }

    let mut owner_cursor = usage.owner_qualified_name.clone();
    while let Some(owner_usage) = local_usage_map.get(&owner_cursor) {
        if name.segments.first() == Some(&owner_usage.declared_name) {
            let tail = QualifiedName {
                segments: name.segments[1..].to_vec(),
                span: name.span.clone(),
            };
            return resolve_feature_reference_with_seen(
                owner_usage,
                &tail,
                stdlib_ids,
                stdlib_feature_index,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
                definition_index,
                local_feature_index,
                local_usage_map,
                seen_usages,
                seen_definitions,
            );
        }
        owner_cursor = owner_usage.owner_qualified_name.clone();
    }

    None
}

#[allow(clippy::too_many_arguments)]
fn resolve_enclosing_scope_sibling_feature_reference(
    usage: &CollectedUsage,
    name: &QualifiedName,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
    seen_usages: &mut BTreeSet<String>,
    seen_definitions: &mut BTreeSet<String>,
) -> Option<String> {
    if name.segments.len() < 2 {
        return None;
    }

    let head = QualifiedName {
        segments: vec![name.segments.first()?.clone()],
        span: name.span.clone(),
    };
    let tail = QualifiedName {
        segments: name.segments[1..].to_vec(),
        span: name.span.clone(),
    };

    let mut scope_cursor = usage.owner_qualified_name.clone();
    loop {
        let mut scoped_target = resolve_owner_feature_name(
            &scope_cursor,
            &head,
            local_aliases,
            import_aliases,
            local_feature_index,
        );
        if scoped_target.is_none()
            && let Some(scope_usage) = local_usage_map.get(&scope_cursor)
            && let Some(inherited) = resolve_owner_usage_feature_reference(
                &scope_usage.qualified_name,
                &head,
                stdlib_ids,
                stdlib_feature_index,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
                definition_index,
                local_feature_index,
                local_usage_map,
                seen_usages,
                seen_definitions,
            )
        {
            scoped_target = inherited
                .strip_prefix("feature.")
                .map(str::to_string)
                .or_else(|| (!inherited.contains("::")).then_some(inherited));
        }

        if let Some(scoped_target) = scoped_target {
            let scoped_usage = local_usage_map.get(&scoped_target)?;
            if let Some(local) = resolve_owner_feature_name(
                &scoped_usage.qualified_name,
                &tail,
                local_aliases,
                import_aliases,
                local_feature_index,
            ) {
                return Some(feature_id_from_qualified_name(&local));
            }
            if let Some(inherited) = resolve_owner_usage_feature_reference(
                &scoped_usage.qualified_name,
                &tail,
                stdlib_ids,
                stdlib_feature_index,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
                definition_index,
                local_feature_index,
                local_usage_map,
                seen_usages,
                seen_definitions,
            ) {
                return Some(normalize_feature_target_id(&inherited));
            }
            if let Some(resolved) = resolve_feature_reference_with_seen(
                scoped_usage,
                &tail,
                stdlib_ids,
                stdlib_feature_index,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
                definition_index,
                local_feature_index,
                local_usage_map,
                seen_usages,
                seen_definitions,
            ) {
                return Some(resolved);
            }
        }

        let Some(owner_usage) = local_usage_map.get(&scope_cursor) else {
            break;
        };
        scope_cursor = owner_usage.owner_qualified_name.clone();
    }

    None
}

#[allow(clippy::too_many_arguments)]
fn resolve_redefinition_feature_reference(
    usage: &CollectedUsage,
    name: &QualifiedName,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
) -> Option<String> {
    let mut seen_usages = BTreeSet::new();
    let mut seen_definitions = BTreeSet::new();
    resolve_redefinition_feature_reference_with_seen(
        usage,
        name,
        stdlib_ids,
        stdlib_feature_index,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
        definition_index,
        local_feature_index,
        local_usage_map,
        &mut seen_usages,
        &mut seen_definitions,
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_redefinition_feature_reference_with_seen(
    usage: &CollectedUsage,
    name: &QualifiedName,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
    seen_usages: &mut BTreeSet<String>,
    seen_definitions: &mut BTreeSet<String>,
) -> Option<String> {
    if usage.owner_construct.ends_with("Definition") {
        if let Some(inherited) = resolve_inherited_definition_feature_reference(
            &usage.owner_qualified_name,
            name,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            seen_definitions,
        ) {
            return Some(feature_id_from_qualified_name(&inherited));
        }
    }

    if let Some(target) = resolve_feature_reference_with_seen(
        usage,
        name,
        stdlib_ids,
        stdlib_feature_index,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
        definition_index,
        local_feature_index,
        local_usage_map,
        seen_usages,
        seen_definitions,
    ) {
        return Some(target);
    }

    let mut owner_cursor = usage.owner_qualified_name.clone();
    let mut owner_seen_usages = BTreeSet::new();
    let mut owner_seen_definitions = BTreeSet::new();
    while let Some(owner_usage) = local_usage_map.get(&owner_cursor) {
        if let Some(type_name) = &owner_usage.ty
            && let Some(type_id) = resolve_type_reference_in_scope(
                type_name,
                &owner_usage.owner_qualified_name,
                stdlib_ids,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
            )
            && let Some(definition_qualified_name) = type_id.strip_prefix("type.")
            && let Some(local) = resolve_owner_feature_name(
                definition_qualified_name,
                name,
                local_aliases,
                import_aliases,
                local_feature_index,
            )
        {
            return Some(feature_id_from_qualified_name(&local));
        }
        if let Some(inherited) = resolve_owner_usage_feature_reference(
            &owner_usage.qualified_name,
            name,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            local_usage_map,
            &mut owner_seen_usages,
            &mut owner_seen_definitions,
        ) {
            return Some(normalize_feature_target_id(&inherited));
        }
        owner_cursor = owner_usage.owner_qualified_name.clone();
    }

    if name.segments.len() == 1 {
        if let Some(local) = unique_definition_owned_feature_match_excluding(
            name.segments.first()?,
            local_feature_index,
            definition_index,
            &usage.qualified_name,
        ) {
            return Some(feature_id_from_qualified_name(&local));
        }
        if let Some(local) = unique_feature_match_excluding(
            name.segments.first()?,
            local_feature_index,
            &usage.qualified_name,
        ) {
            return Some(feature_id_from_qualified_name(&local));
        }
        unique_suffix_match(name.segments.first()?, stdlib_ids)
    } else {
        resolve_qualified_reference(
            name,
            stdlib_ids,
            stdlib_aliases,
            local_definitions,
            local_aliases,
        )
    }
}

fn resolve_local_feature_name(
    name: &QualifiedName,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
) -> Option<String> {
    if let Some(expanded) = expand_import_namespace_prefix(name, local_aliases, import_aliases) {
        return resolve_local_feature_name(
            &expanded,
            local_aliases,
            import_aliases,
            local_feature_index,
        );
    }

    let dotted = name.as_dot_string();
    if let Some(imported) = import_aliases.value_aliases.get(&dotted)
        && imported.starts_with("feature.")
    {
        return imported.strip_prefix("feature.").map(str::to_string);
    }
    if let Some(exact) = unique_feature_match(&dotted, local_feature_index) {
        return Some(exact);
    }

    if name.segments.len() == 1 {
        let simple = name.segments.first()?;
        if let Some(imported) = import_aliases.value_aliases.get(simple)
            && imported.starts_with("feature.")
        {
            return imported.strip_prefix("feature.").map(str::to_string);
        }
        unique_feature_match(simple, local_feature_index)
    } else {
        None
    }
}

fn resolve_owner_feature_name(
    owner_qualified_name: &str,
    name: &QualifiedName,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
) -> Option<String> {
    let expanded = expand_import_namespace_prefix(name, local_aliases, import_aliases);
    let resolved = expanded.as_ref().unwrap_or(name);
    let scope = local_feature_index.get(owner_qualified_name)?;
    if resolved.segments.len() == 1 {
        scope.get(resolved.segments.first()?).cloned()
    } else {
        let dotted = resolved.as_dot_string();
        scope
            .values()
            .find(|qualified| *qualified == &dotted)
            .cloned()
    }
}

fn resolve_enclosing_usage_feature_reference(
    owner_qualified_name: &str,
    name: &QualifiedName,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
) -> Option<String> {
    let mut cursor = owner_qualified_name.to_string();
    while let Some(owner_usage) = local_usage_map.get(&cursor) {
        if let Some(local) = resolve_owner_feature_name(
            &owner_usage.qualified_name,
            name,
            local_aliases,
            import_aliases,
            local_feature_index,
        ) {
            return Some(local);
        }
        cursor = owner_usage.owner_qualified_name.clone();
    }
    resolve_owner_feature_name(
        &cursor,
        name,
        local_aliases,
        import_aliases,
        local_feature_index,
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_enclosing_usage_inherited_feature_reference(
    owner_qualified_name: &str,
    name: &QualifiedName,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
    seen_usages: &mut BTreeSet<String>,
    seen_definitions: &mut BTreeSet<String>,
) -> Option<String> {
    let mut cursor = owner_qualified_name.to_string();
    while let Some(owner_usage) = local_usage_map.get(&cursor) {
        if let Some(inherited) = resolve_owner_usage_feature_reference(
            &owner_usage.qualified_name,
            name,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            local_usage_map,
            seen_usages,
            seen_definitions,
        ) {
            return Some(inherited);
        }
        cursor = owner_usage.owner_qualified_name.clone();
    }
    resolve_inherited_definition_feature_reference(
        &cursor,
        name,
        stdlib_ids,
        stdlib_feature_index,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
        definition_index,
        local_feature_index,
        seen_definitions,
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_inherited_definition_feature_reference(
    owner_qualified_name: &str,
    name: &QualifiedName,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    seen: &mut BTreeSet<String>,
) -> Option<String> {
    if !seen.insert(owner_qualified_name.to_string()) {
        return None;
    }

    let definition = definition_index.get(owner_qualified_name)?;
    for parent in &definition.specializes {
        let Some(parent_id) = resolve_type_reference(
            parent,
            stdlib_ids,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
        ) else {
            continue;
        };
        let Some(parent_qualified_name) = parent_id.strip_prefix("type.") else {
            if let Some(inherited) =
                resolve_stdlib_owned_feature_reference(&parent_id, name, stdlib_feature_index)
            {
                return Some(inherited);
            }
            continue;
        };
        if let Some(local) = resolve_owner_feature_name(
            parent_qualified_name,
            name,
            local_aliases,
            import_aliases,
            local_feature_index,
        ) {
            return Some(local);
        }
        if let Some(inherited) = resolve_inherited_definition_feature_reference(
            parent_qualified_name,
            name,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            seen,
        ) {
            return Some(inherited);
        }
    }

    None
}

#[allow(clippy::too_many_arguments)]
fn resolve_owner_usage_feature_reference(
    owner_qualified_name: &str,
    name: &QualifiedName,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
    seen_usages: &mut BTreeSet<String>,
    seen_definitions: &mut BTreeSet<String>,
) -> Option<String> {
    if !seen_usages.insert(owner_qualified_name.to_string()) {
        return None;
    }

    let owner_usage = local_usage_map.get(owner_qualified_name)?;
    let mut candidate_definitions = BTreeSet::new();
    let mut stdlib_owner_ids = BTreeSet::new();
    if let Some(stdlib_owner) = usage_construct_stdlib_owner(&owner_usage.construct) {
        stdlib_owner_ids.insert(stdlib_owner.to_string());
    }

    if let Some(type_name) = &owner_usage.ty
        && let Some(type_id) = resolve_type_reference(
            type_name,
            stdlib_ids,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
        )
    {
        if let Some(local_definition) = type_id.strip_prefix("type.") {
            candidate_definitions.insert(local_definition.to_string());
        } else {
            stdlib_owner_ids.insert(type_id);
        }
    }

    if let Some(type_id) = infer_named_definition_type_ref(owner_usage, local_definitions)
        && let Some(local_definition) = type_id.strip_prefix("type.")
    {
        candidate_definitions.insert(local_definition.to_string());
    }

    if let Some(type_qualified_name) = resolve_collected_usage_type_qualified_name(
        owner_usage,
        stdlib_ids,
        stdlib_feature_index,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
        definition_index,
        local_feature_index,
        local_usage_map,
        seen_usages,
        seen_definitions,
    ) {
        candidate_definitions.insert(type_qualified_name);
    }

    for target_name in owner_usage
        .redefines
        .iter()
        .chain(owner_usage.subsets.iter())
        .chain(owner_usage.specializes.iter())
    {
        let Some(target_id) = resolve_feature_reference_with_seen(
            owner_usage,
            target_name,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            local_usage_map,
            seen_usages,
            seen_definitions,
        ) else {
            continue;
        };
        let Some(target_usage) = target_id
            .strip_prefix("feature.")
            .and_then(|qualified_name| local_usage_map.get(qualified_name))
        else {
            continue;
        };
        if let Some(local) = resolve_owner_feature_name(
            &target_usage.qualified_name,
            name,
            local_aliases,
            import_aliases,
            local_feature_index,
        ) {
            return Some(local);
        }
        if let Some(inherited) = resolve_owner_usage_feature_reference(
            &target_usage.qualified_name,
            name,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            local_usage_map,
            seen_usages,
            seen_definitions,
        ) {
            return Some(inherited);
        }
    }

    for definition_qualified_name in candidate_definitions {
        if let Some(local) = resolve_owner_feature_name(
            &definition_qualified_name,
            name,
            local_aliases,
            import_aliases,
            local_feature_index,
        ) {
            return Some(local);
        }
        if let Some(inherited) = resolve_inherited_definition_feature_reference(
            &definition_qualified_name,
            name,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            seen_definitions,
        ) {
            return Some(inherited);
        }
        let stdlib_owner = format!("type.{definition_qualified_name}");
        if let Some(inherited) =
            resolve_stdlib_owned_feature_reference(&stdlib_owner, name, stdlib_feature_index)
        {
            return Some(inherited);
        }
    }

    for owner_type_id in stdlib_owner_ids {
        if let Some(inherited) =
            resolve_stdlib_owned_feature_reference(&owner_type_id, name, stdlib_feature_index)
        {
            return Some(inherited);
        }
    }

    None
}

fn resolve_stdlib_owned_feature_reference(
    owner_type_id: &str,
    name: &QualifiedName,
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
) -> Option<String> {
    if name.segments.len() != 1 {
        return None;
    }
    let feature_name = name.segments.first()?;
    stdlib_feature_index
        .get(owner_type_id)
        .and_then(|features| features.get(feature_name))
        .cloned()
}

fn unique_feature_match(
    dotted_name: &str,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
) -> Option<String> {
    let matches = local_feature_index
        .values()
        .flat_map(BTreeMap::values)
        .filter(|qualified_name| {
            *qualified_name == dotted_name || qualified_name.ends_with(&format!(".{dotted_name}"))
        })
        .cloned()
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

fn unique_feature_match_excluding(
    dotted_name: &str,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    excluded_qualified_name: &str,
) -> Option<String> {
    let matches = local_feature_index
        .values()
        .flat_map(BTreeMap::values)
        .filter(|qualified_name| {
            qualified_name.as_str() != excluded_qualified_name
                && (*qualified_name == dotted_name
                    || qualified_name.ends_with(&format!(".{dotted_name}")))
        })
        .cloned()
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

fn unique_definition_owned_feature_match_excluding(
    dotted_name: &str,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    excluded_qualified_name: &str,
) -> Option<String> {
    let matches = local_feature_index
        .values()
        .flat_map(BTreeMap::values)
        .filter(|qualified_name| {
            qualified_name.as_str() != excluded_qualified_name
                && (*qualified_name == dotted_name
                    || qualified_name.ends_with(&format!(".{dotted_name}")))
                && qualified_name
                    .rsplit_once('.')
                    .is_some_and(|(owner, _)| definition_index.contains_key(owner))
        })
        .cloned()
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

fn usage_construct_stdlib_owner(construct: &str) -> Option<&'static str> {
    match construct {
        "SendUsage" => Some("Actions::SendAction"),
        "AcceptActionUsage" => Some("Actions::AcceptAction"),
        _ => None,
    }
}

fn feature_id_from_qualified_name(qualified_name: &str) -> String {
    format!("feature.{qualified_name}")
}

fn normalize_feature_target_id(target: &str) -> String {
    if target.starts_with("feature.") || target.contains("::") {
        target.to_string()
    } else {
        feature_id_from_qualified_name(target)
    }
}

fn infer_named_definition_type_ref(
    usage: &CollectedUsage,
    local_definitions: &BTreeMap<String, String>,
) -> Option<String> {
    if usage.ty.is_some() || usage.declared_name.is_empty() {
        return None;
    }

    let action_like = matches!(
        usage.construct.as_str(),
        "ActionUsage" | "PerformActionUsage" | "AcceptActionUsage"
    );
    if !action_like {
        return None;
    }

    let mut candidates = vec![usage.declared_name.clone()];
    let mut chars = usage.declared_name.chars();
    if let Some(first) = chars.next() {
        let mut pascal = String::new();
        pascal.extend(first.to_uppercase());
        pascal.push_str(chars.as_str());
        if pascal != usage.declared_name {
            candidates.push(pascal);
        }
    }

    candidates
        .into_iter()
        .find_map(|candidate| local_definitions.get(&candidate).cloned())
}

fn is_self_feature_reference(usage: &CollectedUsage, name: &QualifiedName) -> bool {
    let dotted = name.as_dot_string();
    dotted == usage.declared_name || dotted == usage.qualified_name
}

#[allow(clippy::too_many_arguments)]
fn resolve_collected_usage_type_qualified_name(
    usage: &CollectedUsage,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
    seen_usages: &mut BTreeSet<String>,
    seen_definitions: &mut BTreeSet<String>,
) -> Option<String> {
    if let Some(type_name) = &usage.ty {
        return resolve_type_reference(
            type_name,
            stdlib_ids,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
        )
        .and_then(|target| target.strip_prefix("type.").map(str::to_string));
    }

    if let Some(target_name) = &usage.reference_target
        && let Some(target_id) = resolve_reference_usage_target(
            usage,
            target_name,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            local_usage_map,
        )
        && let Some(target_type_id) = infer_usage_type_from_feature_id(
            &target_id,
            stdlib_ids,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            local_usage_map,
        )
    {
        return target_type_id.strip_prefix("type.").map(str::to_string);
    }

    if !usage.declared_name.is_empty() {
        let inferred_name = QualifiedName {
            segments: vec![usage.declared_name.clone()],
            span: usage.span.clone(),
        };
        if let Some(target_id) = resolve_feature_reference(
            usage,
            &inferred_name,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            local_usage_map,
        ) && let Some(target_type_id) = infer_usage_type_from_feature_id(
            &target_id,
            stdlib_ids,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            local_usage_map,
        ) {
            return target_type_id.strip_prefix("type.").map(str::to_string);
        }
    }

    for name in &usage.redefines {
        let target = resolve_redefinition_feature_reference_with_seen(
            usage,
            name,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            local_usage_map,
            seen_usages,
            seen_definitions,
        )?;
        if let Some(target_usage) = target
            .strip_prefix("feature.")
            .and_then(|qualified_name| local_usage_map.get(qualified_name))
        {
            if !seen_usages.insert(target_usage.qualified_name.clone()) {
                continue;
            }
            if let Some(type_qualified_name) = resolve_collected_usage_type_qualified_name(
                target_usage,
                stdlib_ids,
                stdlib_feature_index,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
                definition_index,
                local_feature_index,
                local_usage_map,
                seen_usages,
                seen_definitions,
            ) {
                return Some(type_qualified_name);
            }
        }
    }

    for name in usage.subsets.iter().chain(usage.specializes.iter()) {
        let Some(target) = resolve_feature_reference_with_seen(
            usage,
            name,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            local_usage_map,
            seen_usages,
            seen_definitions,
        ) else {
            continue;
        };
        let Some(target_usage) = target
            .strip_prefix("feature.")
            .and_then(|qualified_name| local_usage_map.get(qualified_name))
        else {
            continue;
        };
        if !seen_usages.insert(target_usage.qualified_name.clone()) {
            continue;
        }
        if let Some(type_qualified_name) = resolve_collected_usage_type_qualified_name(
            target_usage,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            local_usage_map,
            seen_usages,
            seen_definitions,
        ) {
            return Some(type_qualified_name);
        }
    }

    None
}

#[allow(clippy::too_many_arguments)]
fn inferred_usage_type_ref(
    usage: &CollectedUsage,
    redefined_features: &[String],
    subsetted_features: &[String],
    specialized_features: &[String],
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
) -> Option<String> {
    redefined_features
        .iter()
        .chain(subsetted_features.iter())
        .chain(specialized_features.iter())
        .find_map(|feature_id| {
            let target = feature_id.strip_prefix("feature.")?;
            let target_usage = local_usage_map.get(target)?;
            if let Some(target_type) = target_usage.ty.as_ref() {
                return resolve_type_reference(
                    target_type,
                    stdlib_ids,
                    stdlib_aliases,
                    local_definitions,
                    local_aliases,
                    import_aliases,
                );
            }

            let mut seen_usages = BTreeSet::from([usage.qualified_name.clone()]);
            let mut seen_definitions = BTreeSet::new();
            resolve_collected_usage_type_qualified_name(
                target_usage,
                stdlib_ids,
                stdlib_feature_index,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
                definition_index,
                local_feature_index,
                local_usage_map,
                &mut seen_usages,
                &mut seen_definitions,
            )
            .map(|qualified_name| format!("type.{qualified_name}"))
        })
}

fn infer_usage_type_from_feature_id(
    feature_id: &str,
    stdlib_ids: &[String],
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
) -> Option<String> {
    let target = feature_id.strip_prefix("feature.")?;
    let usage = local_usage_map.get(target)?;
    let ty = usage.ty.as_ref()?;
    resolve_type_reference_in_scope(
        ty,
        &usage.owner_qualified_name,
        stdlib_ids,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_feature_reference_from_type_id(
    type_id: &str,
    segment: &str,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
) -> Option<String> {
    let name = QualifiedName {
        segments: vec![segment.to_string()],
        span: SourceSpan {
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 0,
        },
    };

    if let Some(qualified_name) = type_id.strip_prefix("type.") {
        if let Some(local) = resolve_owner_feature_name(
            qualified_name,
            &name,
            local_aliases,
            import_aliases,
            local_feature_index,
        ) {
            return Some(feature_id_from_qualified_name(&local));
        }

        let mut seen = BTreeSet::new();
        return resolve_inherited_definition_feature_reference(
            qualified_name,
            &name,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            &mut seen,
        )
        .map(|qualified| feature_id_from_qualified_name(&qualified));
    }

    resolve_stdlib_owned_feature_reference(type_id, &name, stdlib_feature_index)
}

#[allow(clippy::too_many_arguments)]
fn resolve_feature_reference_from_feature_type(
    feature_id: &str,
    segment: &str,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
) -> Option<String> {
    if let Some(target_qualified_name) = feature_id.strip_prefix("feature.") {
        let name = QualifiedName {
            segments: vec![segment.to_string()],
            span: SourceSpan {
                start_line: 0,
                start_col: 0,
                end_line: 0,
                end_col: 0,
            },
        };
        if let Some(local) = resolve_owner_feature_name(
            target_qualified_name,
            &name,
            local_aliases,
            import_aliases,
            local_feature_index,
        ) {
            return Some(feature_id_from_qualified_name(&local));
        }
    }

    let type_id = infer_usage_type_from_feature_id(
        feature_id,
        stdlib_ids,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
        local_usage_map,
    );
    if type_id.is_none()
        && let Some(target_id) = resolve_usage_feature_type_from_feature_id(
            feature_id,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            local_usage_map,
        )
        && let Some(target_qualified_name) = target_id.strip_prefix("feature.")
    {
        let name = QualifiedName {
            segments: vec![segment.to_string()],
            span: SourceSpan {
                start_line: 0,
                start_col: 0,
                end_line: 0,
                end_col: 0,
            },
        };

        if let Some(local) = resolve_owner_feature_name(
            target_qualified_name,
            &name,
            local_aliases,
            import_aliases,
            local_feature_index,
        ) {
            return Some(feature_id_from_qualified_name(&local));
        }

        let mut seen_usages = BTreeSet::new();
        let mut seen_definitions = BTreeSet::new();
        if let Some(inherited) = resolve_owner_usage_feature_reference(
            target_qualified_name,
            &name,
            stdlib_ids,
            stdlib_feature_index,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
            definition_index,
            local_feature_index,
            local_usage_map,
            &mut seen_usages,
            &mut seen_definitions,
        ) {
            return Some(normalize_feature_target_id(&inherited));
        }
    }

    resolve_feature_reference_from_type_id(
        &type_id?,
        segment,
        stdlib_ids,
        stdlib_feature_index,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
        definition_index,
        local_feature_index,
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_usage_feature_type_from_feature_id(
    feature_id: &str,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
) -> Option<String> {
    let target = feature_id.strip_prefix("feature.")?;
    let usage = local_usage_map.get(target)?;
    let ty = usage.ty.as_ref()?;
    resolve_feature_reference(
        usage,
        ty,
        stdlib_ids,
        stdlib_feature_index,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
        definition_index,
        local_feature_index,
        local_usage_map,
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_reference_usage_target(
    usage: &CollectedUsage,
    name: &QualifiedName,
    stdlib_ids: &[String],
    stdlib_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_feature_index: &BTreeMap<String, BTreeMap<String, String>>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
) -> Option<String> {
    let mut scoped_usage = usage.clone();
    while !matches!(
        scoped_usage.owner_construct.as_str(),
        "Package" | "PartDefinition"
    ) && !scoped_usage.owner_construct.ends_with("Definition")
    {
        let Some(owner_usage) = local_usage_map.get(&scoped_usage.owner_qualified_name) else {
            break;
        };
        scoped_usage.owner_construct = owner_usage.owner_construct.clone();
        scoped_usage.owner_qualified_name = owner_usage.owner_qualified_name.clone();
    }

    if let Some(target) = resolve_feature_reference(
        &scoped_usage,
        name,
        stdlib_ids,
        stdlib_feature_index,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
        definition_index,
        local_feature_index,
        local_usage_map,
    ) {
        return Some(target);
    }

    let mut namespace_cursor = scoped_usage.owner_qualified_name.clone();
    while let Some((parent, _)) = namespace_cursor.rsplit_once('.') {
        namespace_cursor = parent.to_string();
        if let Some(local) = resolve_owner_feature_name(
            &namespace_cursor,
            name,
            local_aliases,
            import_aliases,
            local_feature_index,
        ) {
            return Some(feature_id_from_qualified_name(&local));
        }
    }

    None
}

#[allow(clippy::too_many_arguments)]
fn resolve_connection_end_specialization(
    usage: &CollectedUsage,
    stdlib_ids: &[String],
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
    definition_index: &BTreeMap<String, CollectedDefinition>,
    local_usage_map: &BTreeMap<String, CollectedUsage>,
) -> Option<String> {
    if usage.construct != "ReferenceUsage" {
        return None;
    }

    let parent_usage = local_usage_map.get(&usage.owner_qualified_name)?;
    if parent_usage.construct != "ConnectionUsage" {
        return None;
    }

    let parent_type_name = parent_usage.ty.as_ref()?;
    let parent_type_id = resolve_type_reference(
        parent_type_name,
        stdlib_ids,
        stdlib_aliases,
        local_definitions,
        local_aliases,
        import_aliases,
    )?;
    let parent_definition = definition_index.get(parent_type_id.strip_prefix("type.")?)?;
    let member = parent_definition
        .members
        .iter()
        .find(|member| member.declared_name == usage.declared_name)?;
    Some(format!("feature.{}", member.qualified_name))
}

fn resolve_import_target(
    name: &QualifiedName,
    stdlib_ids: &[String],
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
) -> Option<String> {
    let as_colon = name.as_colon_string();
    if as_colon.contains('*') {
        return Some(as_colon);
    }

    resolve_qualified_reference(
        name,
        stdlib_ids,
        stdlib_aliases,
        local_definitions,
        local_aliases,
    )
    .or_else(|| {
        if name.segments.len() > 1 {
            unique_suffix_match(name.segments.last()?, stdlib_ids)
        } else {
            None
        }
    })
}

fn resolve_type_reference(
    name: &QualifiedName,
    stdlib_ids: &[String],
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
) -> Option<String> {
    if let Some(unconjugated) = unconjugated_type_name(name) {
        return resolve_type_reference(
            &unconjugated,
            stdlib_ids,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
        );
    }

    if name.segments.len() == 1 {
        let simple = &name.segments[0];
        if let Some(local) = local_definitions.get(simple) {
            return Some(local.clone());
        }
        if let Some(alias_target) = local_aliases.get(simple) {
            return resolve_type_reference(
                alias_target,
                stdlib_ids,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
            );
        }
        if let Some(imported) = import_aliases.value_aliases.get(simple) {
            return Some(imported.clone());
        }
        if let Some(alias) = stdlib_aliases.get(simple) {
            return Some(alias.clone());
        }
        return unique_suffix_match(simple, stdlib_ids);
    }

    if let Some(expanded) = expand_import_namespace_prefix(name, local_aliases, import_aliases) {
        return resolve_type_reference(
            &expanded,
            stdlib_ids,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
        );
    }

    if let Some(alias_target) = local_aliases.get(&name.as_dot_string()) {
        return resolve_type_reference(
            alias_target,
            stdlib_ids,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
        );
    }

    if let Some(alias_target) =
        unique_local_alias_suffix_match(&name.as_dot_string(), local_aliases)
    {
        return resolve_type_reference(
            &alias_target,
            stdlib_ids,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
        );
    }

    if let Some(imported) = import_aliases.value_aliases.get(&name.as_dot_string()) {
        return Some(imported.clone());
    }

    resolve_qualified_reference(
        name,
        stdlib_ids,
        stdlib_aliases,
        local_definitions,
        local_aliases,
    )
}

fn resolve_type_reference_in_scope(
    name: &QualifiedName,
    owner_qualified_name: &str,
    stdlib_ids: &[String],
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
) -> Option<String> {
    if let Some(unconjugated) = unconjugated_type_name(name) {
        return resolve_type_reference_in_scope(
            &unconjugated,
            owner_qualified_name,
            stdlib_ids,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
        );
    }

    resolve_scoped_local_type_reference(name, owner_qualified_name, local_definitions)
        .or_else(|| resolve_scoped_import_value_alias(name, owner_qualified_name, import_aliases))
        .or_else(|| {
            resolve_visible_type_reference(
                name,
                stdlib_ids,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
            )
        })
}

fn resolve_scoped_import_value_alias(
    name: &QualifiedName,
    owner_qualified_name: &str,
    import_aliases: &ImportAliases,
) -> Option<String> {
    if name.segments.len() != 1 {
        return None;
    }

    let simple = name.segments.first()?;
    let mut cursor = owner_qualified_name;
    loop {
        let key = format!("{cursor}.{simple}");
        if let Some(imported) = import_aliases.value_aliases.get(&key) {
            return Some(imported.clone());
        }
        let Some((parent, _)) = cursor.rsplit_once('.') else {
            break;
        };
        cursor = parent;
    }
    None
}

fn resolve_visible_type_reference(
    name: &QualifiedName,
    stdlib_ids: &[String],
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
) -> Option<String> {
    if name.segments.len() == 1 {
        let simple = &name.segments[0];
        if let Some(alias_target) = local_aliases.get(simple) {
            return resolve_visible_type_reference(
                alias_target,
                stdlib_ids,
                stdlib_aliases,
                local_definitions,
                local_aliases,
                import_aliases,
            );
        }
        if let Some(imported) = import_aliases.value_aliases.get(simple) {
            return Some(imported.clone());
        }
        if let Some(alias) = stdlib_aliases.get(simple) {
            return Some(alias.clone());
        }
        return unique_suffix_match(simple, stdlib_ids);
    }

    if let Some(expanded) = expand_import_namespace_prefix(name, local_aliases, import_aliases) {
        return resolve_visible_type_reference(
            &expanded,
            stdlib_ids,
            stdlib_aliases,
            local_definitions,
            local_aliases,
            import_aliases,
        );
    }

    if let Some(imported) = import_aliases.value_aliases.get(&name.as_dot_string()) {
        return Some(imported.clone());
    }

    resolve_explicit_type_reference(name, stdlib_ids, stdlib_aliases, local_definitions)
}

fn unconjugated_type_name(name: &QualifiedName) -> Option<QualifiedName> {
    let first = name.segments.first()?;
    let stripped = first.strip_prefix('~')?;
    if stripped.is_empty() {
        return None;
    }
    let mut segments = name.segments.clone();
    segments[0] = stripped.to_string();
    Some(QualifiedName {
        segments,
        span: name.span.clone(),
    })
}

fn resolve_explicit_type_reference(
    name: &QualifiedName,
    stdlib_ids: &[String],
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
) -> Option<String> {
    let colon = name.as_colon_string();
    if let Some(alias) = stdlib_aliases.get(&colon) {
        return Some(alias.clone());
    }
    if stdlib_ids.iter().any(|id| id == &colon) {
        return Some(colon);
    }

    local_definitions.get(&name.as_dot_string()).cloned()
}

fn resolve_scoped_local_type_reference(
    name: &QualifiedName,
    owner_qualified_name: &str,
    local_definitions: &BTreeMap<String, String>,
) -> Option<String> {
    let dotted_name = name.as_dot_string();
    let mut cursor = owner_qualified_name;
    loop {
        let candidate = format!("{cursor}.{dotted_name}");
        if let Some(local) = local_definitions.get(&candidate) {
            return Some(local.clone());
        }
        let Some((parent, _)) = cursor.rsplit_once('.') else {
            break;
        };
        cursor = parent;
    }
    None
}

fn resolve_qualified_reference(
    name: &QualifiedName,
    stdlib_ids: &[String],
    stdlib_aliases: &BTreeMap<String, String>,
    local_definitions: &BTreeMap<String, String>,
    local_aliases: &BTreeMap<String, QualifiedName>,
) -> Option<String> {
    let colon = name.as_colon_string();
    if let Some(alias) = stdlib_aliases.get(&colon) {
        return Some(alias.clone());
    }
    if stdlib_ids.iter().any(|id| id == &colon) {
        return Some(colon);
    }

    if let Some(local) = local_definitions.get(&name.as_dot_string()) {
        return Some(local.clone());
    }

    if let Some(local) = unique_local_suffix_match(&name.as_dot_string(), local_definitions) {
        return Some(local);
    }

    if let Some(alias_target) = local_aliases.get(&name.as_dot_string()) {
        return resolve_qualified_reference(
            alias_target,
            stdlib_ids,
            stdlib_aliases,
            local_definitions,
            local_aliases,
        );
    }

    if let Some(alias_target) =
        unique_local_alias_suffix_match(&name.as_dot_string(), local_aliases)
    {
        return resolve_qualified_reference(
            &alias_target,
            stdlib_ids,
            stdlib_aliases,
            local_definitions,
            local_aliases,
        );
    }

    if name.segments.len() == 1 {
        unique_suffix_match(name.segments.last()?, stdlib_ids)
    } else {
        None
    }
}

fn unique_local_alias_suffix_match(
    dotted_name: &str,
    local_aliases: &BTreeMap<String, QualifiedName>,
) -> Option<QualifiedName> {
    let matches = local_aliases
        .iter()
        .filter(|(qualified_name, _)| {
            *qualified_name == dotted_name || qualified_name.ends_with(&format!(".{dotted_name}"))
        })
        .map(|(_, target)| target.clone())
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

fn build_stdlib_alias_map(
    stdlib: &KirDocument,
    mappings: &MappingBundle,
) -> BTreeMap<String, String> {
    let mut aliases = mappings
        .stdlib_aliases()
        .iter()
        .map(|(alias, target)| (alias.clone(), target.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut bare_short_name_targets = BTreeMap::<String, String>::new();
    let mut duplicate_bare_short_names = BTreeSet::<String>::new();

    for element in &stdlib.elements {
        let Some((namespace, _)) = element.id.rsplit_once("::") else {
            continue;
        };
        let Some(metadata) = element
            .properties
            .get("metadata")
            .and_then(Value::as_object)
        else {
            continue;
        };
        let Some(short_name) = metadata.get("declared_short_name").and_then(Value::as_str) else {
            continue;
        };
        aliases
            .entry(format!("{namespace}::{short_name}"))
            .or_insert_with(|| element.id.clone());
        match bare_short_name_targets.entry(short_name.to_string()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(element.id.clone());
            }
            std::collections::btree_map::Entry::Occupied(existing)
                if existing.get() != &element.id =>
            {
                duplicate_bare_short_names.insert(short_name.to_string());
            }
            std::collections::btree_map::Entry::Occupied(_) => {}
        }
    }

    for duplicate in duplicate_bare_short_names {
        bare_short_name_targets.remove(&duplicate);
    }

    for (short_name, target) in bare_short_name_targets {
        aliases.entry(short_name).or_insert(target);
    }

    add_compat_stdlib_alias(&mut aliases, stdlib, "Items::Item", "sysml.Item");
    add_compat_stdlib_alias(&mut aliases, stdlib, "Base::DataValue", "sysml.DataValue");
    add_compat_stdlib_alias(&mut aliases, stdlib, "Parts::Part", "sysml.Part");
    add_compat_stdlib_alias(&mut aliases, stdlib, "Ports::Port", "sysml.Port");
    add_compat_stdlib_alias(
        &mut aliases,
        stdlib,
        "Interfaces::Interface",
        "sysml.Interface",
    );
    add_compat_stdlib_alias(
        &mut aliases,
        stdlib,
        "ISQSpaceTime::breadth",
        "ISQSpaceTime::width",
    );
    add_compat_stdlib_alias(&mut aliases, stdlib, "breadth", "ISQSpaceTime::width");

    aliases
}

fn add_compat_stdlib_alias(
    aliases: &mut BTreeMap<String, String>,
    stdlib: &KirDocument,
    alias: &str,
    target: &str,
) {
    if stdlib.elements.iter().any(|element| element.id == target) {
        aliases
            .entry(alias.to_string())
            .or_insert(target.to_string());
    }
}

fn unique_suffix_match(name: &str, stdlib_ids: &[String]) -> Option<String> {
    let matches = stdlib_ids
        .iter()
        .filter(|id| id.rsplit("::").next() == Some(name))
        .cloned()
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

fn unique_local_suffix_match(
    dotted_name: &str,
    local_definitions: &BTreeMap<String, String>,
) -> Option<String> {
    let matches = local_definitions
        .iter()
        .filter(|(key, _)| *key == dotted_name || key.ends_with(&format!(".{dotted_name}")))
        .map(|(_, value)| value.clone())
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

fn expand_import_namespace_prefix(
    name: &QualifiedName,
    local_aliases: &BTreeMap<String, QualifiedName>,
    import_aliases: &ImportAliases,
) -> Option<QualifiedName> {
    let first = name.segments.first()?;
    let prefix = import_aliases
        .namespace_aliases
        .get(first)
        .or_else(|| local_aliases.get(first))?;
    let mut segments = prefix.segments.clone();
    segments.extend(name.segments.iter().skip(1).cloned());
    let expanded = QualifiedName {
        segments,
        span: name.span.clone(),
    };
    (expanded != *name).then_some(expanded)
}

fn import_namespace_prefix(target_id: &str) -> String {
    target_id
        .split("::*")
        .next()
        .unwrap_or(target_id)
        .trim_end_matches("::")
        .to_string()
}

fn resolve_local_namespace_dot(
    namespace: &str,
    owner_package_qualified_name: &str,
    root_package: &Option<String>,
    packages: &[ResolvedPackage],
) -> Option<String> {
    let dotted = namespace.replace("::", ".");
    let mut candidates = vec![dotted.clone()];
    let mut cursor = owner_package_qualified_name;
    while !cursor.is_empty() {
        let candidate = format!("{cursor}.{dotted}");
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
        let Some((parent, _)) = cursor.rsplit_once('.') else {
            break;
        };
        cursor = parent;
    }
    if let Some(root) = root_package {
        if dotted != *root && !dotted.starts_with(&format!("{root}.")) {
            candidates.push(format!("{root}.{dotted}"));
        }
    }

    for candidate in candidates {
        let matches = packages
            .iter()
            .filter(|package| {
                package.qualified_name == candidate
                    || package.qualified_name.ends_with(&format!(".{candidate}"))
            })
            .map(|package| package.qualified_name.clone())
            .collect::<Vec<_>>();
        if matches.len() == 1 {
            return matches.into_iter().next();
        }
    }

    None
}

fn direct_child_name<'a>(qualified_name: &'a str, prefix: &str) -> Option<&'a str> {
    let remainder = qualified_name.strip_prefix(prefix)?;
    if remainder.is_empty() || remainder.contains('.') || remainder.contains("::") {
        None
    } else {
        Some(remainder)
    }
}

fn dotted_name_to_qualified_name(value: &str, span: &SourceSpan) -> QualifiedName {
    QualifiedName {
        segments: value.split('.').map(str::to_string).collect(),
        span: span.clone(),
    }
}

fn qualify_name(owner_package_segments: &[String], name: &str) -> String {
    let mut segments = owner_package_segments.to_vec();
    segments.push(name.to_string());
    segments.join(".")
}

fn usage_qualified_name(owner_qualified_name: &str, declared_name: &str) -> String {
    if owner_qualified_name == "root" {
        declared_name.to_string()
    } else {
        format!("{owner_qualified_name}.{declared_name}")
    }
}

fn unresolved_or_error(
    resolved: Option<String>,
    name: &QualifiedName,
    reference_kind: &str,
    policy: ResolvePolicy,
) -> Result<String, Diagnostic> {
    if let Some(resolved) = resolved {
        return Ok(resolved);
    }
    if policy.preserve_unresolved_references {
        return Ok(name.as_colon_string());
    }
    Err(Diagnostic::new(
        format!("unresolved {reference_kind} `{}`", name.as_colon_string()),
        Some(name.span.clone()),
    ))
}

fn qualify_segments(
    owner_package_segments: &[String],
    declared_segments: &[String],
) -> Vec<String> {
    let mut segments = owner_package_segments.to_vec();
    segments.extend(declared_segments.iter().cloned());
    segments
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::ast::SourceSpan;
    use crate::frontend::sysml::parse_sysml;
    use crate::frontend::transpile::MappingBundle;
    use crate::ir::KirElement;

    #[test]
    fn expand_import_namespace_prefix_ignores_noop_expansion() {
        let span = SourceSpan {
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 0,
        };
        let name = QualifiedName {
            segments: vec!["Packets".to_string(), "packet data field".to_string()],
            span: span.clone(),
        };
        let import_aliases = ImportAliases {
            value_aliases: BTreeMap::new(),
            namespace_aliases: BTreeMap::from([(
                "Packets".to_string(),
                QualifiedName {
                    segments: vec!["Packets".to_string()],
                    span,
                },
            )]),
            ambiguous_value_aliases: BTreeSet::new(),
            ambiguous_namespace_aliases: BTreeSet::new(),
        };

        assert_eq!(
            expand_import_namespace_prefix(&name, &BTreeMap::new(), &import_aliases),
            None
        );
    }

    #[test]
    fn expand_import_namespace_prefix_still_expands_real_aliases() {
        let span = SourceSpan {
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 0,
        };
        let name = QualifiedName {
            segments: vec!["P".to_string(), "packet data field".to_string()],
            span: span.clone(),
        };
        let import_aliases = ImportAliases {
            value_aliases: BTreeMap::new(),
            namespace_aliases: BTreeMap::from([(
                "P".to_string(),
                QualifiedName {
                    segments: vec!["Packets".to_string()],
                    span: span.clone(),
                },
            )]),
            ambiguous_value_aliases: BTreeSet::new(),
            ambiguous_namespace_aliases: BTreeSet::new(),
        };

        assert_eq!(
            expand_import_namespace_prefix(&name, &BTreeMap::new(), &import_aliases),
            Some(QualifiedName {
                segments: vec!["Packets".to_string(), "packet data field".to_string()],
                span,
            })
        );
    }

    #[test]
    fn definition_defaults_use_semantic_specialization_without_metatype_anchor() {
        let mappings = MappingBundle::load().unwrap();
        let specializations =
            definition_specializations_with_default("ItemDefinition", &[], mappings);

        assert_eq!(specializations.len(), 1);
        assert_eq!(specializations[0].as_colon_string(), "Items::Item");
    }

    #[test]
    fn resolve_type_reference_prefers_local_definition_over_stdlib_alias() {
        let span = SourceSpan {
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 0,
        };
        let name = QualifiedName {
            segments: vec!["A".to_string()],
            span,
        };
        let stdlib_aliases = BTreeMap::from([("A".to_string(), "ISQBase::ampere".to_string())]);
        let local_definitions = BTreeMap::from([("A".to_string(), "type.ItemTest.A".to_string())]);

        let resolved = resolve_type_reference(
            &name,
            &["ISQBase::ampere".to_string()],
            &stdlib_aliases,
            &local_definitions,
            &BTreeMap::new(),
            &ImportAliases::default(),
        );

        assert_eq!(resolved.as_deref(), Some("type.ItemTest.A"));
    }

    #[test]
    fn expression_path_resolves_cast_target_features() {
        let module = parse_sysml(
            r#"
            package Demo {
                part def VehiclePart {
                    attribute m;
                }

                part def Vehicle :> VehiclePart;
                part vehicle : Vehicle;
                part vehicles[*] = (vehicle, vehicle);
                attribute masses1[*] = (vehicles as VehiclePart).m;
                attribute masses2[*] = (vehicles as vehicle).m;
            }
            "#,
        )
        .unwrap();
        let stdlib = fake_stdlib([]);
        let mappings = MappingBundle::load().unwrap();

        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        let expressions = resolved
            .usages
            .iter()
            .filter(|usage| usage.declared_name == "masses1" || usage.declared_name == "masses2")
            .map(|usage| usage.expression.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(expressions.len(), 2);
        assert!(expressions.iter().all(|expression| expression.is_some()));
    }

    #[test]
    fn expression_path_resolves_calculation_return_feature() {
        let module = parse_sysml(
            r#"
            package Demo {
                calc def Acceleration {
                    return a;
                }

                action dyn {
                    calc acc : Acceleration;
                    bind out = acc.a;
                }
            }
            "#,
        )
        .unwrap();
        let stdlib = fake_stdlib([]);
        let mappings = MappingBundle::load().unwrap();

        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        let bind = resolved
            .usages
            .iter()
            .find_map(|usage| find_resolved_usage_by_declared_name(usage, "out"))
            .expect("expected bind usage");
        assert!(bind.expression.is_some());
    }

    fn find_resolved_usage_by_declared_name<'a>(
        usage: &'a ResolvedUsage,
        declared_name: &str,
    ) -> Option<&'a ResolvedUsage> {
        if usage.declared_name == declared_name {
            return Some(usage);
        }
        usage
            .members
            .iter()
            .find_map(|member| find_resolved_usage_by_declared_name(member, declared_name))
    }

    fn fake_stdlib<const N: usize>(ids: [&str; N]) -> KirDocument {
        let default_ids = [
            "Actions::Action",
            "Base::DataValue",
            "BinaryConnection",
            "Items::Item",
            "Parts::Part",
            "Ports::Port",
        ];
        KirDocument {
            metadata: BTreeMap::new(),
            elements: default_ids
                .into_iter()
                .chain(ids)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(|id| KirElement {
                    id: id.to_string(),
                    kind: id.to_string(),
                    layer: 1,
                    properties: BTreeMap::new(),
                })
                .collect(),
        }
    }
}
