use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::datalog::materialize_core_indexes;
use crate::graph::{Element, Graph};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RequirementTableViewDto {
    pub title: String,
    pub columns: Vec<RequirementTableColumnDto>,
    pub rows: Vec<RequirementTableRowDto>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RequirementTableColumnDto {
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RequirementTableRowDto {
    pub id: String,
    pub name: Option<String>,
    pub text: Option<String>,
    pub owner: Option<String>,
    pub satisfied_by: Vec<String>,
    pub verified_by: Vec<String>,
    pub source: Option<RequirementSourceDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RequirementSourceDto {
    pub file: Option<String>,
    pub start_line: Option<u64>,
    pub end_line: Option<u64>,
}

pub fn requirements_table_view(graph: &Graph) -> RequirementTableViewDto {
    let derived = materialize_core_indexes(graph, &[]).ok();
    let mut rows = graph
        .elements()
        .iter()
        .filter(|element| {
            derived
                .as_ref()
                .is_some_and(|derived| derived.requirements.contains(&element.element_id))
                || is_requirement(element)
        })
        .map(|requirement| RequirementTableRowDto {
            id: requirement.element_id.clone(),
            name: string_property(requirement, "declared_name")
                .or_else(|| string_property(requirement, "name")),
            text: string_property(requirement, "text")
                .or_else(|| string_property(requirement, "documentation")),
            owner: string_property(requirement, "owner"),
            satisfied_by: derived_sources(&derived, &requirement.element_id, "satisfies")
                .unwrap_or_else(|| related_sources(graph, requirement, &["satisfy", "satisfies"])),
            verified_by: derived_sources(&derived, &requirement.element_id, "verifies")
                .unwrap_or_else(|| related_sources(graph, requirement, &["verify", "verifies"])),
            source: source_for(requirement),
        })
        .collect::<Vec<_>>();

    rows.sort_by(|left, right| left.id.cmp(&right.id));

    let mut warnings = Vec::new();
    if rows.is_empty() {
        warnings.push("No requirement elements were found in the semantic graph.".to_string());
    }

    RequirementTableViewDto {
        title: "Requirements".to_string(),
        columns: vec![
            column("id", "ID"),
            column("name", "Name"),
            column("text", "Text"),
            column("owner", "Owner"),
            column("satisfied_by", "Satisfied By"),
            column("verified_by", "Verified By"),
            column("source", "Source"),
        ],
        rows,
        warnings,
    }
}

fn derived_sources(
    derived: &Option<crate::datalog::DerivedIndexes>,
    requirement_id: &str,
    relation: &str,
) -> Option<Vec<String>> {
    let derived = derived.as_ref()?;
    let sources = match relation {
        "satisfies" => derived.satisfied_by.get(requirement_id),
        "verifies" => derived.verified_by.get(requirement_id),
        _ => None,
    }?;
    Some(sources.iter().cloned().collect())
}

fn column(key: &str, label: &str) -> RequirementTableColumnDto {
    RequirementTableColumnDto {
        key: key.to_string(),
        label: label.to_string(),
    }
}

fn is_requirement(element: &Element) -> bool {
    if is_requirement_relationship(element) {
        return false;
    }

    element.layer == 2
        && (element.kind.contains("Requirement")
            || element
                .properties
                .get("specializes")
                .and_then(Value::as_array)
                .is_some_and(|specializations| {
                    specializations
                        .iter()
                        .filter_map(Value::as_str)
                        .any(|target| target.contains("Requirement"))
                }))
}

fn is_requirement_relationship(element: &Element) -> bool {
    let kind = element.kind.to_ascii_lowercase();
    ["satisfy", "verify", "derive", "refine"]
        .iter()
        .any(|relationship| kind.contains(relationship))
}

fn related_sources(graph: &Graph, requirement: &Element, relations: &[&str]) -> Vec<String> {
    let mut sources = Vec::new();

    for relation in relations {
        for edge in graph.incoming(requirement.id, relation) {
            if let Some(source) = graph.element_id(edge.source) {
                push_unique(&mut sources, source.to_string());
            }
        }
    }

    for element in graph.elements() {
        if !is_relationship_element(element, relations) {
            continue;
        }
        let Some(target) = string_property(element, "target") else {
            continue;
        };
        if target != requirement.element_id {
            continue;
        }
        if let Some(source) = string_property(element, "source") {
            push_unique(&mut sources, source);
        }
    }

    sources.sort();
    sources
}

fn is_relationship_element(element: &Element, relations: &[&str]) -> bool {
    let kind = element.kind.to_ascii_lowercase();
    relations
        .iter()
        .any(|relation| kind.contains(&relation.to_ascii_lowercase()))
}

fn source_for(element: &Element) -> Option<RequirementSourceDto> {
    let metadata = element.properties.get("metadata")?;
    let file = metadata
        .get("source_file")
        .and_then(Value::as_str)
        .map(str::to_string);
    let span = metadata.get("source_span");
    let start_line = span
        .and_then(|span| span.get("start_line"))
        .and_then(Value::as_u64);
    let end_line = span
        .and_then(|span| span.get("end_line"))
        .and_then(Value::as_u64);

    Some(RequirementSourceDto {
        file,
        start_line,
        end_line,
    })
}

fn string_property(element: &Element, key: &str) -> Option<String> {
    element
        .properties
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

#[cfg(test)]
mod tests {
    use crate::{Graph, KirDocument, Runtime, repo_path};

    use super::requirements_table_view;

    #[test]
    fn extracts_requirement_rows_from_example_model() {
        let document =
            KirDocument::from_path(&repo_path("examples/requirements_table_model.json")).unwrap();
        let runtime = Runtime::from_document(document).unwrap();

        let view = requirements_table_view(runtime.graph());

        assert_eq!(view.rows.len(), 3);
        assert_eq!(view.rows[0].id, "req.VehicleSafety.BrakingDistance");
        assert_eq!(
            view.rows[0].satisfied_by,
            vec!["part.VehicleSafety.BrakeController"]
        );
        assert_eq!(
            view.rows[0].verified_by,
            vec!["case.VehicleSafety.BrakingDistanceTest"]
        );
    }

    #[test]
    fn reports_empty_requirement_view() {
        let graph = Graph::from_document(KirDocument {
            metadata: Default::default(),
            elements: vec![],
        })
        .unwrap();

        let view = requirements_table_view(&graph);

        assert!(view.rows.is_empty());
        assert_eq!(
            view.warnings,
            vec!["No requirement elements were found in the semantic graph."]
        );
    }
}
