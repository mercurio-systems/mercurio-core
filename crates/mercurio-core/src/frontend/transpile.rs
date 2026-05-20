use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::frontend::ast::{BinaryOp, UnaryOp};
use crate::frontend::diagnostics::Diagnostic;
use crate::frontend::resolver::{
    ResolvedDefinition, ResolvedExpr, ResolvedImport, ResolvedModule, ResolvedPackage,
    ResolvedPathSegment, ResolvedUsage,
};
use crate::ir::{KirDocument, KirElement};
#[cfg(not(target_arch = "wasm32"))]
use crate::paths::repo_path;

#[derive(Debug, Deserialize)]
pub struct PilotConstructSeed {
    #[serde(default)]
    pub keyword_registry: KeywordRegistrySeed,
    #[serde(default)]
    pub default_specialization_anchors: DefaultSpecializationAnchorsSeed,
    #[serde(default)]
    pub semantic_specialization_defaults: SemanticSpecializationDefaultsSeed,
    #[serde(default)]
    pub usage_semantic_specialization_overrides: UsageSemanticSpecializationOverrideSeed,
    #[serde(default)]
    pub stdlib_aliases: StdlibAliasSeed,
    pub constructs: Vec<PilotConstructEntry>,
}

#[derive(Debug, Deserialize)]
pub struct PilotConstructEntry {
    pub construct: String,
    pub metaclass: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct KeywordRegistrySeed {
    #[serde(default)]
    pub definitions: BTreeMap<String, String>,
    #[serde(default)]
    pub usages: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct DefaultSpecializationAnchorsSeed {
    #[serde(default)]
    pub packages: BTreeMap<String, String>,
    #[serde(default)]
    pub definitions: BTreeMap<String, String>,
    #[serde(default)]
    pub usages: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SemanticSpecializationDefaultsSeed {
    #[serde(default)]
    pub definitions: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub usages: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct UsageSemanticSpecializationOverrideSeed {
    #[serde(default)]
    pub usages: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct StdlibAliasSeed {
    #[serde(default)]
    pub ids: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct KirEmissionSeed {
    pub metaclasses: BTreeMap<String, EmissionRule>,
}

#[derive(Debug, Deserialize)]
pub struct EmissionRule {
    pub kir_kind: String,
    pub id_template: String,
    pub emit: EmissionSpec,
}

#[derive(Debug, Deserialize)]
pub struct EmissionSpec {
    pub properties: BTreeMap<String, String>,
}

pub struct MappingBundle {
    construct_to_metaclass: HashMap<String, String>,
    package_default_specializations: HashMap<String, String>,
    definition_keyword_constructs: HashMap<String, String>,
    definition_default_specializations: HashMap<String, String>,
    definition_semantic_specializations: HashMap<String, Vec<String>>,
    stdlib_aliases: HashMap<String, String>,
    usage_keyword_constructs: HashMap<String, String>,
    usage_default_specializations: HashMap<String, String>,
    usage_semantic_specializations: HashMap<String, Vec<String>>,
    usage_semantic_specialization_overrides: HashMap<String, HashMap<String, Vec<String>>>,
    kir_emission: KirEmissionSeed,
}

#[derive(Debug, Clone, Default)]
struct ReferenceUsageSemantics {
    type_refs: Vec<String>,
    semantic_specializations: Vec<String>,
    subsetted_feature_refs: Vec<String>,
    specialized_feature_refs: Vec<String>,
    redefined_feature_refs: Vec<String>,
    direction: Option<String>,
}

impl MappingBundle {
    pub fn load() -> Result<&'static Self, Diagnostic> {
        static MAPPINGS: OnceLock<Result<MappingBundle, String>> = OnceLock::new();

        match MAPPINGS.get_or_init(|| Self::load_uncached().map_err(|err| err.message.clone())) {
            Ok(bundle) => Ok(bundle),
            Err(message) => Err(Diagnostic::new(message.clone(), None)),
        }
    }

    fn load_uncached() -> Result<Self, Diagnostic> {
        let construct_seed: PilotConstructSeed =
            serde_json::from_str(&load_pilot_constructs_seed()?).map_err(|err| {
                Diagnostic::new(format!("failed to parse mapping file: {err}"), None)
            })?;
        let kir_emission: KirEmissionSeed = serde_json::from_str(&load_kir_emission_seed()?)
            .map_err(|err| {
                Diagnostic::new(format!("failed to parse emission file: {err}"), None)
            })?;

        Ok(Self {
            package_default_specializations: construct_seed
                .default_specialization_anchors
                .packages
                .clone()
                .into_iter()
                .collect(),
            definition_keyword_constructs: construct_seed
                .keyword_registry
                .definitions
                .clone()
                .into_iter()
                .collect(),
            definition_default_specializations: construct_seed
                .default_specialization_anchors
                .definitions
                .clone()
                .into_iter()
                .collect(),
            definition_semantic_specializations: construct_seed
                .semantic_specialization_defaults
                .definitions
                .clone()
                .into_iter()
                .collect(),
            stdlib_aliases: construct_seed
                .stdlib_aliases
                .ids
                .clone()
                .into_iter()
                .collect(),
            usage_keyword_constructs: construct_seed
                .keyword_registry
                .usages
                .clone()
                .into_iter()
                .collect(),
            usage_default_specializations: construct_seed
                .default_specialization_anchors
                .usages
                .clone()
                .into_iter()
                .collect(),
            usage_semantic_specializations: construct_seed
                .semantic_specialization_defaults
                .usages
                .clone()
                .into_iter()
                .collect(),
            usage_semantic_specialization_overrides: construct_seed
                .usage_semantic_specialization_overrides
                .usages
                .clone()
                .into_iter()
                .map(|(construct, overrides)| (construct, overrides.into_iter().collect()))
                .collect(),
            construct_to_metaclass: construct_seed
                .constructs
                .into_iter()
                .map(|entry| (entry.construct, entry.metaclass))
                .collect(),
            kir_emission,
        })
    }

    pub fn metaclass_for(&self, construct: &str) -> Result<&str, Diagnostic> {
        if let Some(metaclass) = self.construct_to_metaclass.get(construct) {
            return Ok(metaclass);
        }
        if construct.ends_with("Usage") {
            return Ok("KerML::Feature");
        }
        if construct.ends_with("Definition") {
            return Ok("KerML::Classifier");
        }

        Err(Diagnostic::new(
            format!("missing construct mapping `{construct}`"),
            None,
        ))
    }

    pub fn emission_for(&self, metaclass: &str) -> Result<&EmissionRule, Diagnostic> {
        self.kir_emission
            .metaclasses
            .get(metaclass)
            .ok_or_else(|| Diagnostic::new(format!("missing emission mapping `{metaclass}`"), None))
    }

    pub fn definition_construct_for(&self, keyword: &str) -> String {
        self.definition_keyword_constructs
            .get(keyword)
            .cloned()
            .unwrap_or_else(|| format!("{}Definition", pascal_case(keyword)))
    }

    pub fn usage_construct_for(&self, keyword: &str) -> String {
        self.usage_keyword_constructs
            .get(keyword)
            .cloned()
            .unwrap_or_else(|| format!("{}Usage", pascal_case(keyword)))
    }

    pub fn default_specialization_for_definition(&self, construct: &str) -> Option<&str> {
        self.definition_default_specializations
            .get(construct)
            .map(String::as_str)
    }

    pub fn default_specialization_for_package(&self, construct: &str) -> Option<&str> {
        self.package_default_specializations
            .get(construct)
            .map(String::as_str)
    }

    pub fn default_specialization_for_usage(&self, construct: &str) -> Option<&str> {
        self.usage_default_specializations
            .get(construct)
            .map(String::as_str)
    }

    pub fn semantic_specializations_for_definition(&self, construct: &str) -> Vec<String> {
        self.definition_semantic_specializations
            .get(construct)
            .cloned()
            .unwrap_or_default()
    }

    pub fn semantic_specializations_for_usage(
        &self,
        construct: &str,
        modifiers: &[String],
    ) -> Vec<String> {
        if let Some(overrides) = self.usage_semantic_specialization_overrides.get(construct) {
            for modifier in modifiers {
                if let Some(targets) = overrides.get(modifier) {
                    return targets.clone();
                }
            }
        }

        self.usage_semantic_specializations
            .get(construct)
            .cloned()
            .unwrap_or_default()
    }

    pub fn stdlib_aliases(&self) -> &HashMap<String, String> {
        &self.stdlib_aliases
    }
}

#[cfg(target_arch = "wasm32")]
fn load_pilot_constructs_seed() -> Result<String, Diagnostic> {
    Ok(include_str!("../../../../mappings/l2/pilot_constructs.seed.json").to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn load_pilot_constructs_seed() -> Result<String, Diagnostic> {
    std::fs::read_to_string(repo_path("mappings/l2/pilot_constructs.seed.json"))
        .map_err(|err| Diagnostic::new(format!("failed to read mapping file: {err}"), None))
}

#[cfg(target_arch = "wasm32")]
fn load_kir_emission_seed() -> Result<String, Diagnostic> {
    Ok(include_str!("../../../../mappings/l2/kir_emission.seed.json").to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn load_kir_emission_seed() -> Result<String, Diagnostic> {
    std::fs::read_to_string(repo_path("mappings/l2/kir_emission.seed.json"))
        .map_err(|err| Diagnostic::new(format!("failed to read emission file: {err}"), None))
}

fn pascal_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => {
            let mut out = first.to_ascii_uppercase().to_string();
            out.push_str(chars.as_str());
            out
        }
        None => String::new(),
    }
}

pub fn transpile_module(
    module: &ResolvedModule,
    source_file: &str,
    mappings: &MappingBundle,
) -> Result<KirDocument, Diagnostic> {
    transpile_module_with_source(module, source_file, "sysml", mappings)
}

pub fn transpile_module_with_source(
    module: &ResolvedModule,
    source_file: &str,
    source_language: &str,
    mappings: &MappingBundle,
) -> Result<KirDocument, Diagnostic> {
    let mut elements = Vec::new();
    let definition_ids = module
        .definitions
        .iter()
        .map(|definition| {
            render_definition_id(definition, mappings)
                .map(|id| (definition.qualified_name.clone(), id))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let conjugated_port_ids = module
        .definitions
        .iter()
        .filter(|definition| definition.construct == "PortDefinition")
        .map(|definition| {
            render_conjugated_port_definition_id(definition, mappings)
                .map(|id| (definition.qualified_name.clone(), id))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let package_ids = module
        .packages
        .iter()
        .map(|package| {
            render_package_id(package, mappings).map(|id| (package.qualified_name.clone(), id))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let top_level_usage_ids = module
        .usages
        .iter()
        .map(|usage| {
            let owner_id = package_owner_id(usage, &package_ids);
            render_usage_id(usage, &owner_id, mappings).map(|id| (usage.qualified_name.clone(), id))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let package_member_ids =
        build_package_member_ids(module, &package_ids, &definition_ids, &top_level_usage_ids);

    for package in &module.packages {
        let package_id = package_ids
            .get(&package.qualified_name)
            .ok_or_else(|| Diagnostic::new("missing package id", None))?;
        let member_ids = package_member_ids
            .get(&package.qualified_name)
            .cloned()
            .unwrap_or_default();
        elements.push(transpile_package(
            package,
            package_id,
            &member_ids,
            &package_ids,
            &definition_ids,
            source_file,
            source_language,
            mappings,
        )?);
    }

    for import in &module.imports {
        let owner_id = import
            .owner_package_qualified_name
            .as_ref()
            .and_then(|qualified_name| package_ids.get(qualified_name))
            .cloned()
            .unwrap_or_else(|| "pkg.root".to_string());
        elements.push(transpile_import(
            import,
            &owner_id,
            source_file,
            source_language,
            mappings,
        )?);
    }

    for definition in &module.definitions {
        let definition_id = definition_ids
            .get(&definition.qualified_name)
            .cloned()
            .ok_or_else(|| Diagnostic::new("missing definition id", None))?;
        let feature_ids = render_owned_usage_tree_ids(
            &definition.members,
            &definition_id,
            mappings,
            source_language == "kerml",
        )?;
        let mut member_ids = feature_ids.clone();
        if let Some(conjugated_id) = conjugated_port_ids.get(&definition.qualified_name) {
            member_ids.push(conjugated_id.clone());
        }
        elements.push(transpile_definition(
            definition,
            &definition_id,
            &feature_ids,
            &member_ids,
            source_file,
            source_language,
            mappings,
        )?);
        transpile_usage_tree(
            &definition.members,
            &definition_id,
            source_file,
            source_language,
            mappings,
            &mut elements,
        )?;
        if let Some(conjugated_id) = conjugated_port_ids.get(&definition.qualified_name) {
            elements.push(transpile_conjugated_port_definition(
                definition,
                conjugated_id,
                &definition_id,
                source_file,
                source_language,
                mappings,
            )?);
        }
    }

    for usage in &module.usages {
        let owner_id = package_owner_id(usage, &package_ids);
        transpile_usage_tree(
            std::slice::from_ref(usage),
            &owner_id,
            source_file,
            source_language,
            mappings,
            &mut elements,
        )?;
    }

    if source_language == "kerml" {
        disambiguate_duplicate_element_ids(&mut elements);
    }
    disambiguate_duplicate_source_position_usage_ids(&mut elements);
    validate_unique_ids(&elements)?;

    Ok(KirDocument {
        metadata: [
            (
                "source".to_string(),
                Value::String(source_language.to_string()),
            ),
            (
                "parsed_from".to_string(),
                Value::String(source_file.to_string()),
            ),
        ]
        .into_iter()
        .collect(),
        elements,
    })
}

fn package_owner_id(usage: &ResolvedUsage, package_ids: &BTreeMap<String, String>) -> String {
    if usage.owner_qualified_name == "root" {
        "pkg.root".to_string()
    } else if let Some(package_id) = package_ids.get(&usage.owner_qualified_name) {
        package_id.clone()
    } else {
        format!("pkg.{}", usage.owner_qualified_name)
    }
}

fn transpile_package(
    package: &ResolvedPackage,
    package_id: &str,
    member_ids: &[String],
    package_ids: &BTreeMap<String, String>,
    definition_ids: &BTreeMap<String, String>,
    source_file: &str,
    source_language: &str,
    mappings: &MappingBundle,
) -> Result<KirElement, Diagnostic> {
    let metaclass = mappings.metaclass_for("Package")?;
    let emission = mappings.emission_for(metaclass)?;
    let metatype_ref = mappings
        .default_specialization_for_package("Package")
        .or(Some(metaclass))
        .map(|value| Value::String(value.to_string()))
        .unwrap_or(Value::Null);
    let context = BTreeMap::from([
        (
            "qualified_name".to_string(),
            Value::String(package.qualified_name.clone()),
        ),
        (
            "declared_name".to_string(),
            Value::String(package.declared_name.clone()),
        ),
        (
            "name".to_string(),
            Value::String(package.declared_name.clone()),
        ),
        (
            "member_ids".to_string(),
            Value::Array(member_ids.iter().cloned().map(Value::String).collect()),
        ),
        (
            "owner_id".to_string(),
            package
                .owner_package_qualified_name
                .as_ref()
                .and_then(|qualified_name| {
                    package_ids
                        .get(qualified_name)
                        .or_else(|| definition_ids.get(qualified_name))
                })
                .cloned()
                .map(Value::String)
                .unwrap_or(Value::Null),
        ),
        ("metatype_ref".to_string(), metatype_ref),
    ]);

    build_element(
        package_id,
        &package.docs,
        &package.span,
        source_file,
        source_language,
        emission,
        context,
    )
}

fn transpile_import(
    import: &ResolvedImport,
    owner_id: &str,
    source_file: &str,
    source_language: &str,
    mappings: &MappingBundle,
) -> Result<KirElement, Diagnostic> {
    let metaclass = mappings.metaclass_for("Import")?;
    let emission = mappings.emission_for(metaclass)?;
    let metatype_ref = Value::String(metaclass.to_string());
    let id = render_string(
        &emission.id_template,
        &BTreeMap::from([
            ("owner_id".to_string(), Value::String(owner_id.to_string())),
            ("ordinal".to_string(), json!(import.ordinal)),
        ]),
    )?;

    let context = BTreeMap::from([
        ("owner_id".to_string(), Value::String(owner_id.to_string())),
        (
            "target_ref".to_string(),
            Value::String(import.target_id.clone()),
        ),
        ("ordinal".to_string(), json!(import.ordinal)),
        ("metatype_ref".to_string(), metatype_ref),
    ]);

    build_element(
        &id,
        &import.docs,
        &import.span,
        source_file,
        source_language,
        emission,
        context,
    )
}

fn transpile_definition(
    definition: &ResolvedDefinition,
    definition_id: &str,
    feature_ids: &[String],
    member_ids: &[String],
    source_file: &str,
    source_language: &str,
    mappings: &MappingBundle,
) -> Result<KirElement, Diagnostic> {
    let metaclass = mappings.metaclass_for(&definition.construct)?;
    let emission = mappings.emission_for(metaclass)?;
    let specializes = semantic_specializations_for_definition(definition, mappings);
    let owner_id = definition_owner_id(definition, mappings)?;
    let metatype_ref = mappings
        .default_specialization_for_definition(&definition.construct)
        .or(Some(metaclass))
        .map(|value| Value::String(value.to_string()))
        .unwrap_or(Value::Null);
    let context = BTreeMap::from([
        (
            "qualified_name".to_string(),
            Value::String(definition.qualified_name.clone()),
        ),
        (
            "declared_name".to_string(),
            Value::String(definition.declared_name.clone()),
        ),
        (
            "name".to_string(),
            Value::String(definition.declared_name.clone()),
        ),
        (
            "owner_id".to_string(),
            owner_id.clone().map(Value::String).unwrap_or(Value::Null),
        ),
        (
            "specializes_refs".to_string(),
            Value::Array(specializes.iter().cloned().map(Value::String).collect()),
        ),
        (
            "owned_feature_ids".to_string(),
            Value::Array(feature_ids.iter().cloned().map(Value::String).collect()),
        ),
        (
            "member_ids".to_string(),
            Value::Array(member_ids.iter().cloned().map(Value::String).collect()),
        ),
        (
            "is_abstract".to_string(),
            Value::Bool(definition_is_abstract(definition)),
        ),
        ("metatype_ref".to_string(), metatype_ref),
    ]);

    build_element(
        definition_id,
        &definition.docs,
        &definition.span,
        source_file,
        source_language,
        emission,
        context,
    )
}

fn transpile_conjugated_port_definition(
    definition: &ResolvedDefinition,
    definition_id: &str,
    owner_id: &str,
    source_file: &str,
    source_language: &str,
    mappings: &MappingBundle,
) -> Result<KirElement, Diagnostic> {
    let metaclass = mappings.metaclass_for("ConjugatedPortDefinition")?;
    let emission = mappings.emission_for(metaclass)?;
    let metatype_ref = mappings
        .default_specialization_for_definition("ConjugatedPortDefinition")
        .or(Some(metaclass))
        .map(|value| Value::String(value.to_string()))
        .unwrap_or(Value::Null);
    let conjugated_name = format!("~{}", definition.declared_name);
    let span = crate::frontend::ast::SourceSpan {
        start_line: definition.span.end_line,
        start_col: definition.span.end_col,
        end_line: definition.span.end_line,
        end_col: definition.span.end_col,
    };
    let context = BTreeMap::from([
        (
            "qualified_name".to_string(),
            Value::String(definition.qualified_name.clone()),
        ),
        (
            "declared_name".to_string(),
            Value::String(conjugated_name.clone()),
        ),
        ("name".to_string(), Value::String(conjugated_name)),
        ("owner_id".to_string(), Value::String(owner_id.to_string())),
        ("is_abstract".to_string(), Value::Bool(false)),
        ("metatype_ref".to_string(), metatype_ref),
    ]);

    build_element(
        definition_id,
        &[],
        &span,
        source_file,
        source_language,
        emission,
        context,
    )
}

fn transpile_usage(
    usage: &ResolvedUsage,
    usage_id: &str,
    owner_id: &str,
    source_file: &str,
    source_language: &str,
    mappings: &MappingBundle,
) -> Result<KirElement, Diagnostic> {
    let metaclass = mappings.metaclass_for(&usage.construct)?;
    let emission = mappings.emission_for(metaclass)?;
    let reference_semantics = reference_usage_semantics(usage);
    let specializes =
        semantic_specializations_for_usage(usage, mappings, reference_semantics.as_ref());
    let subsetted_feature_refs =
        usage_subsetted_feature_refs(usage, mappings, reference_semantics.as_ref());
    let specialized_feature_refs =
        usage_specialized_feature_refs(usage, reference_semantics.as_ref());
    let redefined_feature_refs = usage_redefined_feature_refs(usage, reference_semantics.as_ref());
    let declared_name_is_synthetic = usage_has_synthetic_declared_name(usage);
    let usage_name = usage_display_name(usage);
    let metatype_ref = mappings
        .default_specialization_for_usage(&usage.construct)
        .or(Some(metaclass))
        .map(|value| Value::String(value.to_string()))
        .unwrap_or(Value::Null);
    let context = BTreeMap::from([
        ("owner_id".to_string(), Value::String(owner_id.to_string())),
        (
            "owner_path".to_string(),
            Value::String(usage.owner_qualified_name.clone()),
        ),
        (
            "qualified_name".to_string(),
            Value::String(usage.qualified_name.clone()),
        ),
        (
            "declared_name".to_string(),
            if usage.is_implicit_name || declared_name_is_synthetic {
                Value::Null
            } else {
                Value::String(usage.declared_name.clone())
            },
        ),
        (
            "name".to_string(),
            usage_name.map(Value::String).unwrap_or(Value::Null),
        ),
        (
            "type_ref".to_string(),
            usage_type_ref(usage, reference_semantics.as_ref())
                .map(Value::String)
                .unwrap_or(Value::Null),
        ),
        (
            "featuring_type_ref".to_string(),
            usage_featuring_type_ref(usage, owner_id)
                .map(Value::String)
                .unwrap_or(Value::Null),
        ),
        (
            "specializes_refs".to_string(),
            Value::Array(
                usage_specialization_refs(
                    usage,
                    specializes,
                    &specialized_feature_refs,
                    &subsetted_feature_refs,
                    &redefined_feature_refs,
                )
                .into_iter()
                .map(Value::String)
                .collect(),
            ),
        ),
        (
            "specialized_feature_refs".to_string(),
            Value::Array(
                specialized_feature_refs
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        ),
        (
            "subsetted_feature_refs".to_string(),
            Value::Array(
                subsetted_feature_refs
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        ),
        (
            "redefined_feature_refs".to_string(),
            Value::Array(
                redefined_feature_refs
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        ),
        (
            "owned_feature_ids".to_string(),
            Value::Array(
                render_owned_usage_tree_ids(
                    &usage.members,
                    usage_id,
                    mappings,
                    source_language == "kerml",
                )?
                .into_iter()
                .map(Value::String)
                .collect(),
            ),
        ),
        (
            "member_ids".to_string(),
            Value::Array(
                render_owned_usage_tree_ids(
                    &usage.members,
                    usage_id,
                    mappings,
                    source_language == "kerml",
                )?
                .into_iter()
                .map(Value::String)
                .collect(),
            ),
        ),
        (
            "direction".to_string(),
            usage_direction(usage, reference_semantics.as_ref())
                .map(|value| Value::String(value.to_string()))
                .unwrap_or(Value::Null),
        ),
        (
            "is_abstract".to_string(),
            Value::Bool(
                usage
                    .modifiers
                    .iter()
                    .any(|modifier| modifier == "abstract"),
            ),
        ),
        ("is_derived".to_string(), Value::Bool(usage.is_derived)),
        ("is_end".to_string(), Value::Bool(usage_is_end(usage))),
        ("is_ordered".to_string(), Value::Bool(false)),
        ("is_unique".to_string(), Value::Bool(true)),
        (
            "multiplicity".to_string(),
            usage
                .multiplicity
                .as_ref()
                .map(|multiplicity| Value::String(multiplicity.raw.clone()))
                .unwrap_or(Value::Null),
        ),
        (
            "multiplicity_lower".to_string(),
            usage
                .multiplicity
                .as_ref()
                .map(|multiplicity| Value::String(multiplicity.lower.clone()))
                .unwrap_or(Value::Null),
        ),
        (
            "multiplicity_upper".to_string(),
            usage
                .multiplicity
                .as_ref()
                .map(|multiplicity| Value::String(multiplicity.upper.clone()))
                .unwrap_or(Value::Null),
        ),
        (
            "is_variable".to_string(),
            Value::Bool(usage_is_variable(usage)),
        ),
        ("metatype_ref".to_string(), metatype_ref),
        ("start_line".to_string(), json!(usage.span.start_line)),
        ("start_col".to_string(), json!(usage.span.start_col)),
    ]);

    let mut element = build_element(
        usage_id,
        &usage.docs,
        &usage_source_span(usage),
        source_file,
        source_language,
        emission,
        context,
    )?;
    if let Some(expression) = &usage.expression {
        element.properties.insert(
            "expression_ir".to_string(),
            render_expression_ir(expression)?,
        );
    }
    if let Some(multiplicity) = &usage.multiplicity {
        element.properties.insert(
            "multiplicity".to_string(),
            Value::String(multiplicity.raw.clone()),
        );
        element.properties.insert(
            "multiplicity_lower".to_string(),
            Value::String(multiplicity.lower.clone()),
        );
        element.properties.insert(
            "multiplicity_upper".to_string(),
            Value::String(multiplicity.upper.clone()),
        );
    }
    if let Some(reference_semantics) = &reference_semantics {
        set_property_refs(
            &mut element.properties,
            "type",
            &reference_semantics.type_refs,
        );
        set_property_refs(
            &mut element.properties,
            "definition",
            &reference_semantics.type_refs,
        );
        if let Some(direction) = &reference_semantics.direction {
            element
                .properties
                .insert("direction".to_string(), Value::String(direction.clone()));
        }
    }
    enrich_usage_semantics(&mut element, usage, owner_id);
    enrich_trace_relationship_semantics(&mut element, usage, owner_id);
    Ok(element)
}

fn build_element(
    id: &str,
    docs: &[String],
    span: &crate::frontend::ast::SourceSpan,
    source_file: &str,
    source_language: &str,
    emission: &EmissionRule,
    context: BTreeMap<String, Value>,
) -> Result<KirElement, Diagnostic> {
    let mut properties = BTreeMap::new();
    for (key, template) in &emission.emit.properties {
        let value = render_value(template, &context)?;
        match &value {
            Value::Null => {}
            Value::Array(values) if values.is_empty() => {}
            Value::String(text) if text.is_empty() => {}
            _ => {
                properties.insert(key.clone(), value);
            }
        }
    }

    if !properties.contains_key("metatype") {
        if let Some(Value::String(metatype)) = context.get("metatype_ref") {
            if !metatype.is_empty() {
                properties.insert("metatype".to_string(), Value::String(metatype.clone()));
            }
        }
    }

    if !properties.contains_key("qualified_name") {
        if let Some(Value::String(qualified_name)) = context.get("qualified_name") {
            if !qualified_name.is_empty() {
                properties.insert(
                    "qualified_name".to_string(),
                    Value::String(qualified_name.clone()),
                );
            }
        }
    }

    if !docs.is_empty() {
        properties.insert(
            "doc".to_string(),
            json!({
                "source": source_language,
                "blocks": docs.iter().map(|doc| json!({"kind": "comment", "text": doc})).collect::<Vec<_>>()
            }),
        );
    }

    let mut metadata = Map::new();
    metadata.insert(
        "source_file".to_string(),
        Value::String(source_file.to_string()),
    );
    metadata.insert(
        "source_span".to_string(),
        json!({
            "start_line": span.start_line,
            "start_col": span.start_col,
            "end_line": span.end_line,
            "end_col": span.end_col
        }),
    );
    if !metadata.is_empty() {
        properties.insert("metadata".to_string(), Value::Object(metadata));
    }

    Ok(KirElement {
        id: id.to_string(),
        kind: emission.kir_kind.clone(),
        layer: 2,
        properties,
    })
}

fn render_value(template: &str, context: &BTreeMap<String, Value>) -> Result<Value, Diagnostic> {
    if let Some(key) = exact_placeholder(template) {
        return Ok(context.get(key).cloned().unwrap_or(Value::Null));
    }

    Ok(Value::String(render_string(template, context)?))
}

fn render_string(template: &str, context: &BTreeMap<String, Value>) -> Result<String, Diagnostic> {
    let mut rendered = template.to_string();
    for (key, value) in context {
        let placeholder = format!("{{{key}}}");
        let replacement = match value {
            Value::String(text) => text.clone(),
            Value::Number(number) => number.to_string(),
            Value::Bool(boolean) => boolean.to_string(),
            Value::Null => String::new(),
            _ => {
                return Err(Diagnostic::new(
                    format!("non-scalar template value for `{key}`"),
                    None,
                ));
            }
        };
        rendered = rendered.replace(&placeholder, &replacement);
    }
    Ok(rendered)
}

fn exact_placeholder(template: &str) -> Option<&str> {
    template
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
}

fn render_package_id(
    package: &ResolvedPackage,
    mappings: &MappingBundle,
) -> Result<String, Diagnostic> {
    let metaclass = mappings.metaclass_for("Package")?;
    let emission = mappings.emission_for(metaclass)?;
    render_string(
        &emission.id_template,
        &BTreeMap::from([(
            "qualified_name".to_string(),
            Value::String(package.qualified_name.clone()),
        )]),
    )
}

fn render_owned_usage_tree_ids(
    usages: &[ResolvedUsage],
    owner_id: &str,
    mappings: &MappingBundle,
    disambiguate_siblings: bool,
) -> Result<Vec<String>, Diagnostic> {
    let rendered_ids = if disambiguate_siblings {
        render_sibling_usage_ids(usages, owner_id, mappings)?
    } else {
        usages
            .iter()
            .map(|usage| render_usage_id(usage, owner_id, mappings))
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut ids = Vec::new();
    for (usage, usage_id) in usages.iter().zip(rendered_ids) {
        if usage_counts_as_owned_member(usage) {
            ids.push(usage_id);
        }
    }
    Ok(ids)
}

fn enrich_usage_semantics(element: &mut KirElement, usage: &ResolvedUsage, owner_id: &str) {
    if usage.is_implicit_name || usage_has_synthetic_declared_name(usage) {
        element.properties.remove("declared_name");
    }
    if let Some(name) = usage_display_name(usage) {
        element
            .properties
            .insert("name".to_string(), Value::String(name));
    }

    if let Some(defaults) = usage_family_defaults(usage) {
        set_property_refs(
            &mut element.properties,
            "type",
            &[defaults.type_ref.to_string()],
        );
        set_property_refs(
            &mut element.properties,
            "definition",
            &[defaults.type_ref.to_string()],
        );
        let family_refs = defaults
            .subsetted_feature_refs
            .iter()
            .map(|value| Value::String((*value).to_string()))
            .collect::<Vec<_>>();
        element.properties.insert(
            "subsetted_features".to_string(),
            Value::Array(family_refs.clone()),
        );
        element
            .properties
            .insert("specializes".to_string(), Value::Array(family_refs));
        element
            .properties
            .insert("is_unique".to_string(), Value::Bool(true));
        element
            .properties
            .insert("is_variable".to_string(), Value::Bool(defaults.is_variable));
    }

    if usage.construct == "PartUsage"
        && usage.owner_construct == "ItemDefinition"
        && !usage.modifiers.iter().any(|modifier| modifier == "ref")
    {
        append_unique_property_ref(&mut element.properties, "type", "Parts::Part");
    }
    if !element.properties.contains_key("definition")
        && let Some(type_ref) = element.properties.get("type").cloned()
    {
        element
            .properties
            .insert("definition".to_string(), type_ref);
    }
    if usage.construct == "PartUsage"
        && usage.owner_construct == "ItemDefinition"
        && !usage.modifiers.iter().any(|modifier| modifier == "ref")
    {
        append_unique_property_ref(&mut element.properties, "definition", "Parts::Part");
    }

    if !matches!(
        usage.owner_construct.as_str(),
        "EnumerationDefinition" | "Package"
    ) {
        if !element.properties.contains_key("featuring_type") {
            element.properties.insert(
                "featuring_type".to_string(),
                Value::String(owner_id.to_string()),
            );
        }
        element.properties.insert(
            "owning_type".to_string(),
            Value::String(owner_id.to_string()),
        );
        element.properties.insert(
            "owning_namespace".to_string(),
            Value::String(owner_id.to_string()),
        );
        if usage.owner_construct.ends_with("Definition") {
            element.properties.insert(
                "owning_definition".to_string(),
                Value::String(owner_id.to_string()),
            );
        }
    }
}

fn enrich_trace_relationship_semantics(
    element: &mut KirElement,
    usage: &ResolvedUsage,
    owner_id: &str,
) {
    match usage.construct.as_str() {
        "SatisfyUsage" => {
            element.kind = "SysML::Requirements::SatisfyRequirementUsage".to_string();
            element
                .properties
                .insert("source".to_string(), Value::String(owner_id.to_string()));
            if let Some(target) = &usage.reference_target {
                element
                    .properties
                    .insert("target".to_string(), Value::String(target.clone()));
            }
        }
        "VerifyUsage" => {
            element.kind = "SysML::Requirements::VerifyRequirementUsage".to_string();
            element
                .properties
                .insert("source".to_string(), Value::String(owner_id.to_string()));
            if let Some(target) = &usage.reference_target {
                element
                    .properties
                    .insert("target".to_string(), Value::String(target.clone()));
            }
        }
        _ => {}
    }
}

fn usage_has_synthetic_declared_name(usage: &ResolvedUsage) -> bool {
    usage.construct == "ReferenceUsage"
        && usage.modifiers.iter().any(|modifier| {
            matches!(
                modifier.as_str(),
                "payload" | "receiver" | "source-output" | "target-input"
            )
        })
}

fn usage_display_name(usage: &ResolvedUsage) -> Option<String> {
    if usage.is_implicit_name {
        return usage_family_defaults(usage)
            .and_then(|defaults| defaults.subsetted_feature_refs.last().copied())
            .map(display_name_for_ref)
            .or_else(|| (!usage.declared_name.is_empty()).then(|| usage.declared_name.clone()));
    }

    (!usage.declared_name.is_empty()).then(|| usage.declared_name.clone())
}

fn display_name_for_ref(value: &str) -> String {
    value
        .rsplit("::")
        .next()
        .unwrap_or(value)
        .rsplit('.')
        .next()
        .unwrap_or(value)
        .to_string()
}

fn append_unique_property_ref(properties: &mut BTreeMap<String, Value>, key: &str, value: &str) {
    let updated = match properties.get(key) {
        Some(Value::String(existing)) if existing == value => return,
        Some(Value::String(existing)) => Value::Array(vec![
            Value::String(existing.clone()),
            Value::String(value.to_string()),
        ]),
        Some(Value::Array(values)) => {
            if values.iter().any(|item| item.as_str() == Some(value)) {
                return;
            }
            let mut next = values.clone();
            next.push(Value::String(value.to_string()));
            Value::Array(next)
        }
        Some(Value::Null) | None => Value::String(value.to_string()),
        Some(other) => Value::Array(vec![other.clone(), Value::String(value.to_string())]),
    };

    properties.insert(key.to_string(), updated);
}

fn transpile_usage_tree(
    usages: &[ResolvedUsage],
    owner_id: &str,
    source_file: &str,
    source_language: &str,
    mappings: &MappingBundle,
    elements: &mut Vec<KirElement>,
) -> Result<(), Diagnostic> {
    let rendered_ids = if source_language == "kerml" {
        render_sibling_usage_ids(usages, owner_id, mappings)?
    } else {
        usages
            .iter()
            .map(|usage| render_usage_id(usage, owner_id, mappings))
            .collect::<Result<Vec<_>, _>>()?
    };
    for (usage, usage_id) in usages.iter().zip(rendered_ids) {
        elements.push(transpile_usage(
            usage,
            &usage_id,
            owner_id,
            source_file,
            source_language,
            mappings,
        )?);
        transpile_usage_tree(
            &usage.members,
            &usage_id,
            source_file,
            source_language,
            mappings,
            elements,
        )?;
    }
    Ok(())
}

fn render_sibling_usage_ids(
    usages: &[ResolvedUsage],
    owner_id: &str,
    mappings: &MappingBundle,
) -> Result<Vec<String>, Diagnostic> {
    let base_ids = usages
        .iter()
        .map(|usage| render_usage_id(usage, owner_id, mappings))
        .collect::<Result<Vec<_>, _>>()?;
    let mut counts = BTreeMap::<String, usize>::new();
    for id in &base_ids {
        *counts.entry(id.clone()).or_default() += 1;
    }

    Ok(usages
        .iter()
        .zip(base_ids)
        .map(|(usage, id)| {
            if counts.get(&id).copied().unwrap_or_default() <= 1 {
                id
            } else {
                format!("{}.{}_{}", id, usage.span.start_line, usage.span.start_col)
            }
        })
        .collect())
}

fn render_definition_id(
    definition: &ResolvedDefinition,
    mappings: &MappingBundle,
) -> Result<String, Diagnostic> {
    let metaclass = mappings.metaclass_for(&definition.construct)?;
    let emission = mappings.emission_for(metaclass)?;
    render_string(
        &emission.id_template,
        &BTreeMap::from([(
            "qualified_name".to_string(),
            Value::String(definition.qualified_name.clone()),
        )]),
    )
}

fn render_usage_id(
    usage: &ResolvedUsage,
    owner_id: &str,
    mappings: &MappingBundle,
) -> Result<String, Diagnostic> {
    let metaclass = mappings.metaclass_for(&usage.construct)?;
    let emission = mappings.emission_for(metaclass)?;
    let mut id = render_string(
        &emission.id_template,
        &BTreeMap::from([
            ("owner_id".to_string(), Value::String(owner_id.to_string())),
            (
                "owner_path".to_string(),
                Value::String(usage.owner_qualified_name.clone()),
            ),
            (
                "declared_name".to_string(),
                Value::String(usage.declared_name.clone()),
            ),
            ("start_line".to_string(), json!(usage.span.start_line)),
            ("start_col".to_string(), json!(usage.span.start_col)),
        ]),
    )?;
    if usage.construct == "EndUsage" && !id.ends_with(&format!(".{}", usage.span.start_col)) {
        id = format!("{}.{}_{}", id, usage.span.start_line, usage.span.start_col);
    }
    Ok(id)
}

fn render_conjugated_port_definition_id(
    definition: &ResolvedDefinition,
    mappings: &MappingBundle,
) -> Result<String, Diagnostic> {
    let metaclass = mappings.metaclass_for("ConjugatedPortDefinition")?;
    let emission = mappings.emission_for(metaclass)?;
    render_string(
        &emission.id_template,
        &BTreeMap::from([
            (
                "qualified_name".to_string(),
                Value::String(definition.qualified_name.clone()),
            ),
            (
                "declared_name".to_string(),
                Value::String(definition.declared_name.clone()),
            ),
        ]),
    )
}

fn build_package_member_ids(
    module: &ResolvedModule,
    package_ids: &BTreeMap<String, String>,
    definition_ids: &BTreeMap<String, String>,
    usage_ids: &BTreeMap<String, String>,
) -> BTreeMap<String, Vec<String>> {
    module
        .packages
        .iter()
        .map(|package| {
            let child_packages = module
                .packages
                .iter()
                .filter(|candidate| {
                    is_direct_child(&candidate.qualified_name, &package.qualified_name)
                })
                .filter_map(|candidate| package_ids.get(&candidate.qualified_name).cloned());
            let child_definitions = module
                .definitions
                .iter()
                .filter(|candidate| {
                    is_direct_child(&candidate.qualified_name, &package.qualified_name)
                })
                .filter_map(|candidate| definition_ids.get(&candidate.qualified_name).cloned());
            let child_usages = module
                .usages
                .iter()
                .filter(|candidate| usage_counts_as_owned_member(candidate))
                .filter(|candidate| {
                    is_direct_child(&candidate.qualified_name, &package.qualified_name)
                })
                .filter_map(|candidate| usage_ids.get(&candidate.qualified_name).cloned());
            (
                package.qualified_name.clone(),
                child_packages
                    .chain(child_definitions)
                    .chain(child_usages)
                    .collect(),
            )
        })
        .collect()
}

fn is_direct_child(candidate: &str, parent: &str) -> bool {
    let Some(remainder) = candidate.strip_prefix(parent) else {
        return false;
    };
    let Some(remainder) = remainder.strip_prefix('.') else {
        return false;
    };
    !remainder.contains('.')
}

fn definition_owner_id(
    definition: &ResolvedDefinition,
    mappings: &MappingBundle,
) -> Result<Option<String>, Diagnostic> {
    let Some((owner, _)) = definition.qualified_name.rsplit_once('.') else {
        return Ok(None);
    };

    let package = ResolvedPackage {
        owner_package_qualified_name: None,
        qualified_name: owner.to_string(),
        declared_name: owner.rsplit('.').next().unwrap_or(owner).to_string(),
        docs: Vec::new(),
        span: definition.span.clone(),
    };
    render_package_id(&package, mappings).map(Some)
}

fn semantic_specializations_for_definition(
    definition: &ResolvedDefinition,
    mappings: &MappingBundle,
) -> Vec<String> {
    if definition.specializes.is_empty() {
        mappings.semantic_specializations_for_definition(&definition.construct)
    } else {
        definition.specializes.clone()
    }
}

fn definition_is_abstract(definition: &ResolvedDefinition) -> bool {
    definition.is_abstract || definition.construct == "EnumerationDefinition"
}

fn semantic_specializations_for_usage(
    usage: &ResolvedUsage,
    _mappings: &MappingBundle,
    reference_semantics: Option<&ReferenceUsageSemantics>,
) -> Vec<String> {
    if let Some(reference_semantics) = reference_semantics {
        return reference_semantics.semantic_specializations.clone();
    }

    if !usage.specializes.is_empty() {
        return usage.specializes.clone();
    }

    if usage.is_implicit_name
        && !usage.redefined_features.is_empty()
        && usage.specialized_features.is_empty()
        && usage.subsetted_features.is_empty()
    {
        return Vec::new();
    }

    if !usage.has_explicit_type
        && (!usage.specialized_features.is_empty()
            || !usage.subsetted_features.is_empty()
            || !usage.redefined_features.is_empty())
    {
        return Vec::new();
    }

    let mut specializes = Vec::new();
    if usage.construct != "ReferenceUsage" {
        if let Some(type_ref) = usage.type_ref.clone() {
            specializes.push(type_ref);
        } else if usage.construct == "EnumerationUsage"
            && usage.owner_construct == "EnumerationDefinition"
        {
            specializes.push(usage.owner_qualified_name.clone());
        }
    }
    specializes
}

fn usage_specialization_refs(
    usage: &ResolvedUsage,
    mut semantic_specializations: Vec<String>,
    specialized_feature_refs: &[String],
    subsetted_feature_refs: &[String],
    redefined_feature_refs: &[String],
) -> Vec<String> {
    if usage.construct == "ReferenceUsage" {
        semantic_specializations.extend(specialized_feature_refs.iter().cloned());
        semantic_specializations.extend(subsetted_feature_refs.iter().cloned());
        semantic_specializations.extend(redefined_feature_refs.iter().cloned());
        return dedupe_refs(semantic_specializations);
    }

    if usage.construct == "PartUsage"
        && usage.has_explicit_type
        && !usage.specialized_features.is_empty()
        && redefined_feature_refs.is_empty()
    {
        semantic_specializations.extend(redefined_feature_refs.iter().cloned());
        return dedupe_refs(semantic_specializations);
    }

    semantic_specializations.extend(specialized_feature_refs.iter().cloned());
    semantic_specializations.extend(subsetted_feature_refs.iter().cloned());
    semantic_specializations.extend(redefined_feature_refs.iter().cloned());
    dedupe_refs(semantic_specializations)
}

fn usage_is_variable(usage: &ResolvedUsage) -> bool {
    !matches!(
        usage.owner_construct.as_str(),
        "AttributeDefinition" | "EnumerationDefinition" | "Package"
    )
}

fn usage_is_end(usage: &ResolvedUsage) -> bool {
    usage
        .modifiers
        .iter()
        .any(|modifier| modifier == "end" || modifier.starts_with("end-"))
}

fn usage_counts_as_owned_member(usage: &ResolvedUsage) -> bool {
    usage.construct != "ConnectionUsage"
}

fn usage_featuring_type_ref(usage: &ResolvedUsage, owner_id: &str) -> Option<String> {
    (!matches!(
        usage.owner_construct.as_str(),
        "EnumerationDefinition" | "Package"
    ))
    .then(|| owner_id.to_string())
}

fn usage_direction<'a>(
    usage: &'a ResolvedUsage,
    reference_semantics: Option<&'a ReferenceUsageSemantics>,
) -> Option<&'a str> {
    if let Some(reference_semantics) = reference_semantics
        && let Some(direction) = reference_semantics.direction.as_deref()
    {
        return Some(direction);
    }

    if usage.modifiers.iter().any(|modifier| modifier == "inout") {
        Some("inout")
    } else if usage.modifiers.iter().any(|modifier| modifier == "in") {
        Some("in")
    } else if usage.modifiers.iter().any(|modifier| modifier == "out") {
        Some("out")
    } else {
        None
    }
}

fn usage_source_span(usage: &ResolvedUsage) -> crate::frontend::ast::SourceSpan {
    let mut span = usage.span.clone();
    if !usage.members.is_empty() && span.end_line > span.start_line {
        span.end_line -= 1;
    }
    span
}

fn render_expression_ir(expr: &ResolvedExpr) -> Result<Value, Diagnostic> {
    match expr {
        ResolvedExpr::Literal(value) => Ok(json!({
            "kind": "literal",
            "value": value,
        })),
        ResolvedExpr::Tuple { items } => Ok(json!({
            "kind": "tuple",
            "items": items
                .iter()
                .map(render_expression_ir)
                .collect::<Result<Vec<_>, _>>()?,
        })),
        ResolvedExpr::SelfRef => Ok(json!({
            "kind": "self",
        })),
        ResolvedExpr::Unary { op, expr } => Ok(json!({
            "kind": "unary",
            "op": unary_op_name(op),
            "expr": render_expression_ir(expr)?,
        })),
        ResolvedExpr::Binary { left, op, right } => Ok(json!({
            "kind": "binary",
            "op": binary_op_name(op),
            "left": render_expression_ir(left)?,
            "right": render_expression_ir(right)?,
        })),
        ResolvedExpr::FeaturePath { segments } => Ok(json!({
            "kind": "path",
            "root": "self",
            "segments": segments.iter().map(render_path_segment).collect::<Vec<_>>(),
        })),
        ResolvedExpr::Call { function, args } => Ok(json!({
            "kind": "call",
            "function": function,
            "args": args
                .iter()
                .map(render_expression_ir)
                .collect::<Result<Vec<_>, _>>()?,
        })),
    }
}

fn render_path_segment(segment: &ResolvedPathSegment) -> Value {
    json!({
        "name": segment.name,
        "feature": segment.feature_id,
    })
}

fn unary_op_name(op: &UnaryOp) -> &'static str {
    match op {
        UnaryOp::Negate => "negate",
        UnaryOp::Not => "not",
    }
}

fn binary_op_name(op: &BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "add",
        BinaryOp::Subtract => "subtract",
        BinaryOp::Multiply => "multiply",
        BinaryOp::Divide => "divide",
        BinaryOp::Power => "power",
        BinaryOp::Equal => "equal",
        BinaryOp::NotEqual => "not_equal",
        BinaryOp::Less => "less",
        BinaryOp::LessEqual => "less_equal",
        BinaryOp::Greater => "greater",
        BinaryOp::GreaterEqual => "greater_equal",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
    }
}

fn usage_type_ref(
    usage: &ResolvedUsage,
    reference_semantics: Option<&ReferenceUsageSemantics>,
) -> Option<String> {
    if let Some(reference_semantics) = reference_semantics {
        return reference_semantics.type_refs.first().cloned();
    }

    resolved_usage_type_ref(usage)
}

fn resolved_usage_type_ref(usage: &ResolvedUsage) -> Option<String> {
    usage.type_ref.clone().or_else(|| {
        if usage.construct == "PartUsage" {
            Some("Parts::Part".to_string())
        } else if usage.construct == "PortUsage" {
            Some("Ports::Port".to_string())
        } else if usage.construct == "AttributeUsage" {
            Some("Base::DataValue".to_string())
        } else if usage.construct == "EnumerationUsage"
            && usage.owner_construct == "EnumerationDefinition"
        {
            Some(usage.owner_qualified_name.clone())
        } else {
            None
        }
    })
}

fn usage_subsetted_feature_refs(
    usage: &ResolvedUsage,
    mappings: &MappingBundle,
    reference_semantics: Option<&ReferenceUsageSemantics>,
) -> Vec<String> {
    if let Some(reference_semantics) = reference_semantics {
        return dedupe_refs(reference_semantics.subsetted_feature_refs.clone());
    }

    let mut subsetted_feature_refs = usage.subsetted_features.clone();
    if !subsetted_feature_refs.is_empty() {
        return dedupe_refs(subsetted_feature_refs);
    }

    if !usage.redefined_features.is_empty() {
        return Vec::new();
    }

    if usage.construct == "PartUsage" && usage.modifiers.iter().any(|modifier| modifier == "end") {
        return Vec::new();
    }

    if usage.construct == "PartUsage"
        && usage
            .specialized_features
            .iter()
            .any(|feature| feature.starts_with("feature."))
    {
        if usage.has_explicit_type {
            return dedupe_refs(usage.specialized_features.clone());
        }
        let mut specialized_feature_refs = usage.specialized_features.clone();
        specialized_feature_refs.push(if usage.owner_construct == "Package" {
            "Parts::parts".to_string()
        } else {
            "Items::Item::subparts".to_string()
        });
        return dedupe_refs(specialized_feature_refs);
    }

    if usage.construct == "AttributeUsage"
        && usage
            .specialized_features
            .iter()
            .any(|feature| feature.starts_with("feature."))
    {
        return dedupe_refs(usage.specialized_features.clone());
    }

    if usage.construct == "ReferenceUsage" {
        subsetted_feature_refs.extend(
            mappings.semantic_specializations_for_usage(&usage.construct, &usage.modifiers),
        );
        return dedupe_refs(subsetted_feature_refs);
    }

    if usage.construct == "PortUsage"
        && matches!(
            usage.owner_construct.as_str(),
            "PortDefinition" | "PortUsage"
        )
    {
        subsetted_feature_refs.push(
            if usage.modifiers.iter().any(|modifier| modifier == "ref") {
                "Ports::ports"
            } else {
                "Ports::Port::subports"
            }
            .to_string(),
        );
        return dedupe_refs(subsetted_feature_refs);
    }

    if usage.construct == "PartUsage" && usage.owner_construct == "Package" {
        subsetted_feature_refs.push("Parts::parts".to_string());
        return dedupe_refs(subsetted_feature_refs);
    }

    if usage.construct == "ItemUsage"
        && matches!(
            usage.owner_construct.as_str(),
            "PartUsage" | "ItemDefinition"
        )
    {
        subsetted_feature_refs.push("Items::Item::subitems".to_string());
        return dedupe_refs(subsetted_feature_refs);
    }

    subsetted_feature_refs
        .extend(mappings.semantic_specializations_for_usage(&usage.construct, &usage.modifiers));
    dedupe_refs(subsetted_feature_refs)
}

fn usage_specialized_feature_refs(
    usage: &ResolvedUsage,
    reference_semantics: Option<&ReferenceUsageSemantics>,
) -> Vec<String> {
    let mut specialized_feature_refs = usage.specialized_features.clone();
    if let Some(reference_semantics) = reference_semantics {
        specialized_feature_refs.extend(reference_semantics.specialized_feature_refs.clone());
    }
    dedupe_refs(specialized_feature_refs)
}

fn usage_redefined_feature_refs(
    usage: &ResolvedUsage,
    reference_semantics: Option<&ReferenceUsageSemantics>,
) -> Vec<String> {
    let mut redefined_feature_refs = usage.redefined_features.clone();
    if let Some(reference_semantics) = reference_semantics {
        redefined_feature_refs.extend(reference_semantics.redefined_feature_refs.clone());
    }
    dedupe_refs(redefined_feature_refs)
}

fn reference_usage_semantics(usage: &ResolvedUsage) -> Option<ReferenceUsageSemantics> {
    if usage.construct != "ReferenceUsage" {
        return None;
    }

    let type_refs = usage_all_type_refs(usage);
    let mut semantics = ReferenceUsageSemantics {
        type_refs: type_refs.clone(),
        semantic_specializations: type_refs.clone(),
        ..ReferenceUsageSemantics::default()
    };

    if usage.modifiers.iter().any(|modifier| modifier == "payload") {
        semantics
            .subsetted_feature_refs
            .push("Objects::objects".to_string());
        semantics.redefined_feature_refs.push("payload".to_string());
        semantics.direction = Some("in".to_string());
        return Some(semantics);
    }

    if usage
        .modifiers
        .iter()
        .any(|modifier| modifier == "source-output")
    {
        if semantics.type_refs.is_empty() {
            semantics.type_refs.push("Ports::Port".to_string());
        }
        semantics.semantic_specializations.clear();
        semantics
            .redefined_feature_refs
            .push(usage.declared_name.clone());
        semantics
            .redefined_feature_refs
            .push("Transfers::sourceOutput".to_string());
        return Some(semantics);
    }

    if usage
        .modifiers
        .iter()
        .any(|modifier| modifier == "target-input")
    {
        if semantics.type_refs.is_empty() {
            semantics
                .type_refs
                .push("Occurrences::Occurrence".to_string());
        }
        semantics.semantic_specializations.clear();
        semantics
            .redefined_feature_refs
            .push(usage.declared_name.clone());
        semantics
            .redefined_feature_refs
            .push("Transfers::targetInput".to_string());
        if usage.modifiers.iter().any(|modifier| modifier == "in") {
            semantics.direction = Some("in".to_string());
        }
        return Some(semantics);
    }

    if usage
        .modifiers
        .iter()
        .any(|modifier| modifier == "receiver")
    {
        if semantics.type_refs.is_empty() {
            semantics
                .type_refs
                .push("Occurrences::Occurrence".to_string());
        }
        semantics.semantic_specializations.clear();
        semantics
            .redefined_feature_refs
            .push("receiver".to_string());
        if usage.modifiers.iter().any(|modifier| modifier == "in") {
            semantics.direction = Some("in".to_string());
        }
        return Some(semantics);
    }

    if !semantics.type_refs.is_empty() && all_data_value_like_refs(&semantics.type_refs) {
        semantics
            .subsetted_feature_refs
            .push("Base::dataValues".to_string());
        return Some(semantics);
    }

    if !semantics.type_refs.is_empty() {
        semantics
            .subsetted_feature_refs
            .push("Objects::objects".to_string());
        if usage.modifiers.iter().any(|modifier| modifier == "in") {
            semantics.direction = Some("in".to_string());
        } else if usage.modifiers.iter().any(|modifier| modifier == "out") {
            semantics.direction = Some("out".to_string());
        } else if usage.modifiers.iter().any(|modifier| modifier == "inout") {
            semantics.direction = Some("inout".to_string());
        }
        return Some(semantics);
    }

    None
}

fn usage_all_type_refs(usage: &ResolvedUsage) -> Vec<String> {
    let mut refs = Vec::new();
    if let Some(type_ref) = &usage.type_ref {
        refs.push(type_ref.clone());
    }
    refs.extend(usage.additional_type_refs.clone());
    dedupe_refs(refs)
}

fn all_data_value_like_refs(type_refs: &[String]) -> bool {
    !type_refs.is_empty()
        && type_refs
            .iter()
            .all(|type_ref| is_data_value_like_ref(type_ref))
}

fn is_data_value_like_ref(type_ref: &str) -> bool {
    let tail = type_ref
        .rsplit("::")
        .next()
        .unwrap_or(type_ref)
        .rsplit('.')
        .next()
        .unwrap_or(type_ref);
    matches!(
        tail,
        "Boolean" | "Integer" | "Natural" | "Real" | "Rational" | "String" | "UnlimitedNatural"
    ) || tail.ends_with("Value")
}

fn set_property_refs(properties: &mut BTreeMap<String, Value>, key: &str, refs: &[String]) {
    match refs {
        [] => {
            properties.remove(key);
        }
        [only] => {
            properties.insert(key.to_string(), Value::String(only.clone()));
        }
        _ => {
            properties.insert(
                key.to_string(),
                Value::Array(refs.iter().cloned().map(Value::String).collect()),
            );
        }
    }
}

struct UsageFamilyDefaults {
    type_ref: &'static str,
    subsetted_feature_refs: &'static [&'static str],
    is_variable: bool,
}

fn usage_family_defaults(usage: &ResolvedUsage) -> Option<UsageFamilyDefaults> {
    match usage.construct.as_str() {
        "ActionUsage" => Some(UsageFamilyDefaults {
            type_ref: "Actions::Action",
            subsetted_feature_refs: &["Actions::ownedActions"],
            is_variable: false,
        }),
        "PerformActionUsage" => Some(UsageFamilyDefaults {
            type_ref: "Actions::Action",
            subsetted_feature_refs: &["Actions::performedActions"],
            is_variable: true,
        }),
        "AcceptActionUsage" => Some(UsageFamilyDefaults {
            type_ref: "Actions::AcceptAction",
            subsetted_feature_refs: &["Actions::acceptSubactions"],
            is_variable: false,
        }),
        "StateUsage" => Some(UsageFamilyDefaults {
            type_ref: "States::StateAction",
            subsetted_feature_refs: &["States::ownedStates"],
            is_variable: false,
        }),
        "ExhibitStateUsage" => Some(UsageFamilyDefaults {
            type_ref: "States::StateAction",
            subsetted_feature_refs: &["States::exhibitedStates"],
            is_variable: true,
        }),
        "SuccessionUsage" => Some(UsageFamilyDefaults {
            type_ref: "Flows::SuccessionFlow",
            subsetted_feature_refs: &["Actions::ownedActions", "Flows::successionFlows"],
            is_variable: false,
        }),
        _ => None,
    }
}

fn dedupe_refs(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn validate_unique_ids(elements: &[KirElement]) -> Result<(), Diagnostic> {
    let mut seen = BTreeSet::new();
    for element in elements {
        if !seen.insert(element.id.clone()) {
            return Err(Diagnostic::new(
                format!("duplicate emitted KIR id `{}`", element.id),
                None,
            ));
        }
    }
    Ok(())
}

fn disambiguate_duplicate_element_ids(elements: &mut [KirElement]) {
    let mut seen = BTreeSet::new();
    for element in elements {
        if seen.insert(element.id.clone()) {
            continue;
        }

        let base = element.id.clone();
        let suffix = element
            .properties
            .get("source_span")
            .and_then(Value::as_object)
            .and_then(|span| {
                Some(format!(
                    "{}_{}",
                    span.get("start_line")?.as_u64()?,
                    span.get("start_col")?.as_u64()?
                ))
            })
            .unwrap_or_else(|| "duplicate".to_string());
        let mut candidate = format!("{base}.{suffix}");
        let mut ordinal = 2;
        while !seen.insert(candidate.clone()) {
            candidate = format!("{base}.{suffix}_{ordinal}");
            ordinal += 1;
        }
        element.id = candidate;
    }
}

fn disambiguate_duplicate_source_position_usage_ids(elements: &mut [KirElement]) {
    let mut seen = BTreeSet::new();
    for element in elements {
        if seen.insert(element.id.clone()) {
            continue;
        }
        let disambiguate_by_source_position = element.id.ends_with(".end")
            || element.kind == "AcceptActionUsage"
            || element.id.ends_with(".AcceptActionUsage")
            || element.id.starts_with("assert.")
            || element.id.starts_with("assume.")
            || element.id.starts_with("require.")
            || element.id.starts_with("transition.");
        if !disambiguate_by_source_position {
            continue;
        }

        let base = element.id.clone();
        let suffix = element
            .properties
            .get("source_span")
            .and_then(Value::as_object)
            .and_then(|span| {
                Some(format!(
                    "{}_{}",
                    span.get("start_line")?.as_u64()?,
                    span.get("start_col")?.as_u64()?
                ))
            })
            .unwrap_or_else(|| "duplicate".to_string());
        let mut candidate = format!("{base}.{suffix}");
        let mut ordinal = 2;
        while !seen.insert(candidate.clone()) {
            candidate = format!("{base}.{suffix}_{ordinal}");
            ordinal += 1;
        }
        element.id = candidate;
    }
}
