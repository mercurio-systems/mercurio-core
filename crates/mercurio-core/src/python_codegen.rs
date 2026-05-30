use std::collections::{BTreeMap, BTreeSet};

use crate::ir::KirDocument;
use crate::language::{LanguageProfile, SemanticConcept};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonWrapperGeneration {
    pub module_name: String,
    pub profile_id: String,
    pub stdlib_version: String,
    pub kir_schema_version: String,
    pub files: BTreeMap<String, String>,
}

pub fn generate_python_wrappers(
    document: &KirDocument,
    profile: &LanguageProfile,
    module_name: &str,
) -> PythonWrapperGeneration {
    let mut files = BTreeMap::new();
    files.insert(
        format!("{module_name}/__init__.py"),
        init_py(module_name, profile),
    );
    files.insert(format!("{module_name}/base.py"), base_py());
    files.insert(format!("{module_name}/concepts.py"), concepts_py(profile));
    files.insert(
        format!("{module_name}/generation_info.py"),
        generation_info_py(profile),
    );
    files.insert(
        format!("{module_name}/stdlib/__init__.py"),
        stdlib_init_py(),
    );
    files.insert(
        format!("{module_name}/stdlib/si.py"),
        catalog_py(document, "SI"),
    );
    files.insert(
        format!("{module_name}/stdlib/isq.py"),
        catalog_prefix_py(document, "ISQ"),
    );
    files.insert(format!("{module_name}/py.typed"), String::new());

    PythonWrapperGeneration {
        module_name: module_name.to_string(),
        profile_id: profile.id.clone(),
        stdlib_version: profile.stdlib_version.clone(),
        kir_schema_version: profile.kir_schema_version.clone(),
        files,
    }
}

fn init_py(module_name: &str, profile: &LanguageProfile) -> String {
    format!(
        r#""""Generated Mercurio wrappers for {profile_id}."""

from .base import ElementView, StdlibRef
from .concepts import (
    AttributeUsage,
    ConstraintUsage,
    MetadataUsage,
    Package,
    PartDefinition,
    PartUsage,
    RequirementUsage,
    SysML,
    VerificationCaseUsage,
)
from .generation_info import KIR_SCHEMA_VERSION, PROFILE_ID, STDLIB_VERSION

__all__ = [
    "AttributeUsage",
    "ConstraintUsage",
    "ElementView",
    "KIR_SCHEMA_VERSION",
    "MetadataUsage",
    "StdlibRef",
    "Package",
    "PartDefinition",
    "PartUsage",
    "PROFILE_ID",
    "RequirementUsage",
    "STDLIB_VERSION",
    "SysML",
    "VerificationCaseUsage",
    "register",
]


def register(registry):
    registry.register_profile("{profile_id}", "{module_name}")
    registry.register("package", Package)
    registry.register("part_definition", PartDefinition)
    registry.register("part_usage", PartUsage)
    registry.register("attribute_usage", AttributeUsage)
    registry.register("requirement_usage", RequirementUsage)
    registry.register("verification_case_usage", VerificationCaseUsage)
    registry.register("constraint_usage", ConstraintUsage)
    registry.register("metadata_usage", MetadataUsage)
"#,
        profile_id = profile.id,
        module_name = module_name,
    )
}

fn generation_info_py(profile: &LanguageProfile) -> String {
    format!(
        r#""""Version information for generated Mercurio wrappers."""

PROFILE_ID = {profile_id:?}
STDLIB_VERSION = {stdlib_version:?}
KIR_SCHEMA_VERSION = {kir_schema_version:?}
LANGUAGE_VERSION = {language_version:?}
METAMODEL_VERSION = {metamodel_version:?}
"#,
        profile_id = profile.id,
        stdlib_version = profile.stdlib_version,
        kir_schema_version = profile.kir_schema_version,
        language_version = profile.language_version,
        metamodel_version = profile.metamodel_version,
    )
}

fn base_py() -> String {
    r#"from __future__ import annotations

from dataclasses import dataclass
from typing import Any, ClassVar


class ElementView:
    concept: ClassVar[str | None] = None
    metatype_id: ClassVar[str | None] = None

    def __init__(self, element: Any):
        self._element = element

    @classmethod
    def wrap(cls, element: Any):
        if isinstance(element, cls):
            return element
        return cls(element)

    @classmethod
    def matches(cls, element: Any) -> bool:
        metatype_id = getattr(element, "metatype_id", None)
        if callable(metatype_id):
            metatype_id = metatype_id()
        return cls.metatype_id is None or metatype_id == cls.metatype_id

    @property
    def id(self) -> str:
        return self._element.id

    @property
    def kind(self) -> str:
        return self._element.kind

    def get(self, name: str) -> Any:
        return self._element.get(name)

    def effective(self, name: str) -> Any:
        return self._element.effective(name)

    def references(self, name: str) -> list[Any]:
        return self._element.references(name)

    def metadata(self) -> Any:
        metadata = getattr(self._element, "metadata", None)
        return metadata() if callable(metadata) else metadata

    def metadata_by_type(self, type_name: str) -> list[Any]:
        metadata_by_type = getattr(self._element, "metadata_by_type", None)
        if callable(metadata_by_type):
            return metadata_by_type(type_name)
        metadata = self.metadata() or []
        return [
            item
            for item in metadata
            if getattr(item, "type_name", None) == type_name
            or getattr(item, "type", None) == type_name
        ]

    def effective_str(self, name: str) -> str | None:
        value = self.effective(name)
        return value if isinstance(value, str) else None


@dataclass(frozen=True)
class StdlibRef:
    id: str

    def bind(self, model: Any) -> Any:
        return model.element(self.id)
"#
    .to_string()
}

fn concepts_py(profile: &LanguageProfile) -> String {
    let package = python_string_literal(concept_anchor(profile, SemanticConcept::Package));
    let part_definition =
        python_string_literal(concept_anchor(profile, SemanticConcept::PartDefinition));
    let part_usage = python_string_literal(concept_anchor(profile, SemanticConcept::PartUsage));
    let attribute_usage =
        python_string_literal(concept_anchor(profile, SemanticConcept::AttributeUsage));
    let requirement_usage =
        python_string_literal(concept_anchor(profile, SemanticConcept::RequirementUsage));
    let verification_case_usage = python_string_literal(concept_anchor(
        profile,
        SemanticConcept::VerificationCaseUsage,
    ));
    let constraint_usage =
        python_string_literal(concept_anchor(profile, SemanticConcept::ConstraintUsage));
    format!(
        r#"from __future__ import annotations

from .base import ElementView
from .stdlib.si import SINamespace
from .stdlib.isq import ISQNamespace


class Package(ElementView):
    concept = "package"
    metatype_id = {package}

    @property
    def qualified_name(self) -> str | None:
        return self.effective_str("qualified_name")

    def owned_members(self) -> list[ElementView]:
        return self.references("members") or self.references("features")


class PartDefinition(ElementView):
    concept = "part_definition"
    metatype_id = {part_definition}

    @property
    def name(self) -> str | None:
        return self.effective_str("name")

    @property
    def qualified_name(self) -> str | None:
        return self.effective_str("qualified_name")

    def features(self) -> list[ElementView]:
        return self.references("features")


class PartUsage(ElementView):
    concept = "part_usage"
    metatype_id = {part_usage}

    @property
    def name(self) -> str | None:
        return self.effective_str("name")

    @property
    def qualified_name(self) -> str | None:
        return self.effective_str("qualified_name")


class AttributeUsage(ElementView):
    concept = "attribute_usage"
    metatype_id = {attribute_usage}

    @property
    def name(self) -> str | None:
        return self.effective_str("name")


class RequirementUsage(ElementView):
    concept = "requirement_usage"
    metatype_id = {requirement_usage}

    @property
    def text(self) -> str | None:
        return self.effective_str("text") or self.effective_str("documentation")


class VerificationCaseUsage(ElementView):
    concept = "verification_case_usage"
    metatype_id = {verification_case_usage}

    @property
    def name(self) -> str | None:
        return self.effective_str("name")


class ConstraintUsage(ElementView):
    concept = "constraint_usage"
    metatype_id = {constraint_usage}

    @property
    def expression(self):
        return self.effective("expression")


class MetadataUsage(ElementView):
    concept = "metadata_usage"
    metatype_id = None

    @property
    def metadata_type(self) -> str | None:
        return self.effective_str("metadata_type") or self.effective_str("type")


class StdlibNamespace:
    def __init__(self, model):
        self.SI = SINamespace(model)
        self.ISQ = ISQNamespace(model)


class SysML:
    def __init__(self, model):
        self.model = model
        self.stdlib = StdlibNamespace(model)

    @classmethod
    def bind(cls, model):
        return cls(model)

    def elements_with_metadata(self, metadata_type: str) -> list[ElementView]:
        query = getattr(self.model, "elements_with_metadata", None)
        if callable(query):
            return query(metadata_type)
        return []
"#
    )
}

fn stdlib_init_py() -> String {
    "from .si import SINamespace\nfrom .isq import ISQNamespace\n".to_string()
}

fn catalog_py(document: &KirDocument, owner: &str) -> String {
    catalog_module_py(
        "SINamespace",
        entries_for_owner(document, owner).into_iter().take(600),
    )
}

fn catalog_prefix_py(document: &KirDocument, prefix: &str) -> String {
    catalog_module_py(
        "ISQNamespace",
        document
            .elements
            .iter()
            .filter(|element| element.id.starts_with(prefix))
            .filter_map(|element| {
                let leaf = element.id.rsplit("::").next()?;
                Some((python_identifier(leaf), element.id.clone()))
            })
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            .take(1200),
    )
}

fn catalog_module_py<I>(class_name: &str, entries: I) -> String
where
    I: IntoIterator<Item = (String, String)>,
{
    let entries = entries.into_iter().collect::<Vec<_>>();
    let mut output = String::from("from __future__ import annotations\n\n\n");
    output.push_str(&format!("class {class_name}:\n"));
    output.push_str("    def __init__(self, model):\n");
    output.push_str("        self._model = model\n\n");
    if entries.is_empty() {
        output.push_str("    pass\n");
        return output;
    }
    for (name, id) in entries {
        output.push_str("    @property\n");
        output.push_str(&format!("    def {name}(self):\n"));
        output.push_str(&format!("        return self._model.element({id:?})\n\n"));
    }
    output
}

fn entries_for_owner(document: &KirDocument, owner: &str) -> BTreeMap<String, String> {
    document
        .elements
        .iter()
        .filter(|element| {
            element
                .properties
                .get("owner")
                .and_then(|value| value.as_str())
                == Some(owner)
                || element.id.starts_with(&format!("{owner}::"))
        })
        .filter_map(|element| {
            let leaf = element.id.rsplit("::").next()?;
            Some((python_identifier(leaf), element.id.clone()))
        })
        .collect()
}

fn concept_anchor(profile: &LanguageProfile, concept: SemanticConcept) -> Option<&str> {
    profile.canonical_kinds.get(&concept).map(String::as_str)
}

fn python_string_literal(value: Option<&str>) -> String {
    value
        .map(|value| format!("{value:?}"))
        .unwrap_or_else(|| "None".to_string())
}

fn python_identifier(value: &str) -> String {
    let mut result = String::new();
    for (index, ch) in value.chars().enumerate() {
        let valid = ch == '_' || ch.is_ascii_alphanumeric();
        if index == 0 && ch.is_ascii_digit() {
            result.push('_');
        }
        result.push(if valid { ch } else { '_' });
    }
    while result.contains("__") {
        result = result.replace("__", "_");
    }
    result = result.trim_matches('_').to_string();
    if result.is_empty() {
        result = "element".to_string();
    }
    if python_keywords().contains(result.as_str()) {
        result.push('_');
    }
    result
}

fn python_keywords() -> BTreeSet<&'static str> {
    [
        "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class",
        "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global",
        "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return",
        "try", "while", "with", "yield",
    ]
    .into_iter()
    .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::{KirDocument, KirElement, LanguageProfile, language::SourceLanguage};

    use super::*;

    #[test]
    fn generates_initial_wrapper_files() {
        let document = KirDocument {
            metadata: BTreeMap::new(),
            elements: vec![KirElement {
                id: "SI::metre".to_string(),
                kind: "AttributeUsage".to_string(),
                layer: 1,
                properties: BTreeMap::new(),
            }],
        };
        let profile = LanguageProfile {
            id: "sysml-test".to_string(),
            language: SourceLanguage::Sysml,
            language_version: "2.0".to_string(),
            metamodel_version: "2.0".to_string(),
            stdlib_version: "test".to_string(),
            stdlib_path: "stdlib.kir.json".to_string(),
            kir_schema_version: "0.2".to_string(),
            canonical_kinds: BTreeMap::from([(
                SemanticConcept::Package,
                "SysML::Package".to_string(),
            )]),
            aliases: BTreeMap::new(),
        };

        let generated = generate_python_wrappers(&document, &profile, "mercurio_sysml_test");
        assert_eq!(generated.profile_id, "sysml-test");
        assert_eq!(generated.stdlib_version, "test");
        assert!(
            generated
                .files
                .contains_key("mercurio_sysml_test/__init__.py")
        );
        assert!(
            generated
                .files
                .contains_key("mercurio_sysml_test/generation_info.py")
        );
        assert!(generated.files["mercurio_sysml_test/stdlib/si.py"].contains("def metre(self)"));
        assert!(
            generated.files["mercurio_sysml_test/concepts.py"].contains("class PartDefinition")
        );
    }
}
