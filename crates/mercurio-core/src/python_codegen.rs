use std::collections::{BTreeMap, BTreeSet};

use crate::ir::KirDocument;
use crate::language::{LanguageProfile, SemanticConcept};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonWrapperGeneration {
    pub module_name: String,
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
    files.insert(format!("{module_name}/stdlib/__init__.py"), stdlib_init_py());
    files.insert(format!("{module_name}/stdlib/si.py"), catalog_py(document, "SI"));
    files.insert(
        format!("{module_name}/stdlib/isq.py"),
        catalog_prefix_py(document, "ISQ"),
    );
    files.insert(format!("{module_name}/py.typed"), String::new());

    PythonWrapperGeneration {
        module_name: module_name.to_string(),
        files,
    }
}

fn init_py(module_name: &str, profile: &LanguageProfile) -> String {
    format!(
        r#""""Generated Mercurio wrappers for {profile_id}."""

from .base import ElementView, StdlibRef
from .concepts import Package, PartUsage, RequirementUsage, SysML

__all__ = [
    "ElementView",
    "StdlibRef",
    "Package",
    "PartUsage",
    "RequirementUsage",
    "SysML",
    "register",
]


def register(registry):
    registry.register_profile("{profile_id}", "{module_name}")
    registry.register("package", Package)
    registry.register("part_usage", PartUsage)
    registry.register("requirement_usage", RequirementUsage)
"#,
        profile_id = profile.id,
        module_name = module_name,
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
    let part_usage = python_string_literal(concept_anchor(profile, SemanticConcept::PartUsage));
    let requirement_usage =
        python_string_literal(concept_anchor(profile, SemanticConcept::RequirementUsage));
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


class PartUsage(ElementView):
    concept = "part_usage"
    metatype_id = {part_usage}

    @property
    def name(self) -> str | None:
        return self.effective_str("name")

    @property
    def qualified_name(self) -> str | None:
        return self.effective_str("qualified_name")


class RequirementUsage(ElementView):
    concept = "requirement_usage"
    metatype_id = {requirement_usage}

    @property
    def text(self) -> str | None:
        return self.effective_str("text") or self.effective_str("documentation")


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
        "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise",
        "return", "try", "while", "with", "yield",
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
        assert!(generated.files.contains_key("mercurio_sysml_test/__init__.py"));
        assert!(
            generated.files["mercurio_sysml_test/stdlib/si.py"].contains("def metre(self)")
        );
    }
}
