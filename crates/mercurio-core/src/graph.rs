use std::collections::{BTreeMap, HashMap};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ir::{KirDocument, KirElement, KirFieldRegistry};

pub type NodeId = u32;

#[derive(Debug, Clone)]
pub struct Graph {
    elements: Vec<Element>,
    by_element_id: HashMap<String, NodeId>,
    edges: Vec<Edge>,
    outgoing: HashMap<NodeId, Vec<Edge>>,
    incoming: HashMap<NodeId, Vec<Edge>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Element {
    pub id: NodeId,
    pub element_id: String,
    pub kind: String,
    pub layer: u8,
    pub properties: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub source: NodeId,
    pub target: NodeId,
    pub relation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphArtifact {
    pub elements: Vec<Element>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    DuplicateId(String),
    UnknownElement(String),
    NodeOverflow,
    InvalidArtifact(String),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateId(id) => write!(f, "duplicate element id: {id}"),
            Self::UnknownElement(id) => write!(f, "unknown element id: {id}"),
            Self::NodeOverflow => write!(f, "too many elements for u32 node ids"),
            Self::InvalidArtifact(message) => write!(f, "invalid graph artifact: {message}"),
        }
    }
}

impl std::error::Error for GraphError {}

impl Graph {
    pub fn from_document(document: KirDocument) -> Result<Self, GraphError> {
        let mut by_element_id = HashMap::new();
        let mut elements = Vec::with_capacity(document.elements.len());

        for raw in document.elements {
            if by_element_id.contains_key(&raw.id) {
                return Err(GraphError::DuplicateId(raw.id));
            }

            let id = NodeId::try_from(elements.len()).map_err(|_| GraphError::NodeOverflow)?;
            by_element_id.insert(raw.id.clone(), id);
            elements.push(Element::from_raw(id, raw));
        }

        let mut graph = Self {
            elements,
            by_element_id,
            edges: Vec::new(),
            outgoing: HashMap::new(),
            incoming: HashMap::new(),
        };
        graph.build_edges()?;
        Ok(graph)
    }

    pub fn from_artifact(artifact: GraphArtifact) -> Result<Self, GraphError> {
        let mut by_element_id = HashMap::new();
        let mut elements = Vec::with_capacity(artifact.elements.len());

        for raw in artifact.elements {
            if by_element_id.contains_key(&raw.element_id) {
                return Err(GraphError::DuplicateId(raw.element_id));
            }

            let id = NodeId::try_from(elements.len()).map_err(|_| GraphError::NodeOverflow)?;
            by_element_id.insert(raw.element_id.clone(), id);
            elements.push(Element {
                id,
                element_id: raw.element_id,
                kind: raw.kind,
                layer: raw.layer,
                properties: raw.properties,
            });
        }

        let mut graph = Self {
            elements,
            by_element_id,
            edges: Vec::new(),
            outgoing: HashMap::new(),
            incoming: HashMap::new(),
        };

        for edge in artifact.edges {
            if graph.element(edge.source).is_none() || graph.element(edge.target).is_none() {
                return Err(GraphError::InvalidArtifact(format!(
                    "edge references missing node {} -> {}",
                    edge.source, edge.target
                )));
            }
            graph
                .outgoing
                .entry(edge.source)
                .or_default()
                .push(edge.clone());
            graph
                .incoming
                .entry(edge.target)
                .or_default()
                .push(edge.clone());
            graph.edges.push(edge);
        }

        Ok(graph)
    }

    pub fn artifact(&self) -> GraphArtifact {
        GraphArtifact {
            elements: self.elements.clone(),
            edges: self.edges.clone(),
        }
    }

    fn build_edges(&mut self) -> Result<(), GraphError> {
        let field_registry = KirFieldRegistry::standard();

        for element in &self.elements {
            for (property, value) in &element.properties {
                if property == "element_id" {
                    continue;
                }
                for external_target in field_registry.reference_ids(property, value) {
                    let Some(&target) = self.by_element_id.get(external_target) else {
                        continue;
                    };
                    let edge = Edge {
                        source: element.id,
                        target,
                        relation: property.clone(),
                    };
                    self.outgoing
                        .entry(element.id)
                        .or_default()
                        .push(edge.clone());
                    self.incoming.entry(target).or_default().push(edge);
                    self.edges.push(Edge {
                        source: element.id,
                        target,
                        relation: property.clone(),
                    });
                }
            }
        }

        Ok(())
    }

    pub fn element(&self, id: NodeId) -> Option<&Element> {
        self.elements.get(id as usize)
    }

    pub fn element_by_element_id(&self, element_id: &str) -> Option<&Element> {
        self.node_id(element_id).and_then(|id| self.element(id))
    }

    pub fn node_id(&self, element_id: &str) -> Option<NodeId> {
        self.by_element_id.get(element_id).copied()
    }

    pub fn element_id(&self, id: NodeId) -> Option<&str> {
        self.element(id).map(|element| element.element_id.as_str())
    }

    pub fn elements(&self) -> &[Element] {
        &self.elements
    }

    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn outgoing_edges(&self, id: NodeId) -> impl Iterator<Item = &Edge> {
        self.outgoing
            .get(&id)
            .into_iter()
            .flat_map(|edges| edges.iter())
    }

    pub fn incoming_edges(&self, id: NodeId) -> impl Iterator<Item = &Edge> {
        self.incoming
            .get(&id)
            .into_iter()
            .flat_map(|edges| edges.iter())
    }

    pub fn outgoing(&self, id: NodeId, relation: &str) -> impl Iterator<Item = &Edge> {
        self.outgoing_edges(id)
            .filter(move |edge| edge.relation == relation)
    }

    pub fn incoming(&self, id: NodeId, relation: &str) -> impl Iterator<Item = &Edge> {
        self.incoming_edges(id)
            .filter(move |edge| edge.relation == relation)
    }

    pub fn relation_targets(
        &self,
        element_id: &str,
        relation: &str,
    ) -> Result<Vec<&Element>, GraphError> {
        let node_id = self
            .node_id(element_id)
            .ok_or_else(|| GraphError::UnknownElement(element_id.to_string()))?;

        Ok(self
            .outgoing(node_id, relation)
            .filter_map(|edge| self.element(edge.target))
            .collect())
    }
}

impl Element {
    fn from_raw(id: NodeId, raw: KirElement) -> Self {
        let mut properties = raw.properties;
        properties.insert("element_id".to_string(), Value::String(raw.id.clone()));

        Self {
            id,
            element_id: raw.id,
            kind: raw.kind,
            layer: raw.layer,
            properties,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_element_id_as_property_without_creating_self_edge() {
        let graph = Graph::from_document(KirDocument {
            metadata: BTreeMap::new(),
            elements: vec![KirElement {
                id: "type.Demo.Vehicle".to_string(),
                kind: "SysML::Systems::PartDefinition".to_string(),
                layer: 2,
                properties: BTreeMap::new(),
            }],
        })
        .unwrap();

        let element = graph.element_by_element_id("type.Demo.Vehicle").unwrap();
        assert_eq!(element.element_id, "type.Demo.Vehicle");
        assert_eq!(
            element.properties.get("element_id"),
            Some(&Value::String("type.Demo.Vehicle".to_string()))
        );
        assert!(graph.edges().is_empty());
    }

    #[test]
    fn canonical_element_id_overwrites_mismatched_property() {
        let graph = Graph::from_document(KirDocument {
            metadata: BTreeMap::new(),
            elements: vec![KirElement {
                id: "type.Demo.Vehicle".to_string(),
                kind: "SysML::Systems::PartDefinition".to_string(),
                layer: 2,
                properties: BTreeMap::from([(
                    "element_id".to_string(),
                    Value::String("stale".to_string()),
                )]),
            }],
        })
        .unwrap();

        let element = graph.element_by_element_id("type.Demo.Vehicle").unwrap();
        assert_eq!(
            element.properties.get("element_id"),
            Some(&Value::String("type.Demo.Vehicle".to_string()))
        );
    }

    #[test]
    fn keeps_metatype_and_specialization_as_distinct_relations() {
        let graph = Graph::from_document(KirDocument {
            metadata: BTreeMap::new(),
            elements: vec![
                KirElement {
                    id: "type.Camera".to_string(),
                    kind: "SysML::Systems::PartDefinition".to_string(),
                    layer: 2,
                    properties: BTreeMap::from([
                        (
                            "metatype".to_string(),
                            Value::String("SysML::Systems::PartDefinition".to_string()),
                        ),
                        (
                            "specializes".to_string(),
                            Value::Array(vec![Value::String("type.ImagingDevice".to_string())]),
                        ),
                    ]),
                },
                KirElement {
                    id: "type.ImagingDevice".to_string(),
                    kind: "SysML::Systems::PartDefinition".to_string(),
                    layer: 2,
                    properties: BTreeMap::new(),
                },
                KirElement {
                    id: "SysML::Systems::PartDefinition".to_string(),
                    kind: "Metaclass".to_string(),
                    layer: 1,
                    properties: BTreeMap::new(),
                },
            ],
        })
        .unwrap();

        let camera_id = graph.node_id("type.Camera").unwrap();
        let metatype_targets = graph
            .outgoing(camera_id, "metatype")
            .filter_map(|edge| graph.element_id(edge.target))
            .collect::<Vec<_>>();
        let specialization_targets = graph
            .outgoing(camera_id, "specializes")
            .filter_map(|edge| graph.element_id(edge.target))
            .collect::<Vec<_>>();

        assert_eq!(metatype_targets, vec!["SysML::Systems::PartDefinition"]);
        assert_eq!(specialization_targets, vec!["type.ImagingDevice"]);
    }

    #[test]
    fn non_reference_strings_do_not_create_edges() {
        let graph = Graph::from_document(KirDocument {
            metadata: BTreeMap::new(),
            elements: vec![
                KirElement {
                    id: "type.Demo.Vehicle".to_string(),
                    kind: "SysML::Systems::PartDefinition".to_string(),
                    layer: 2,
                    properties: BTreeMap::from([(
                        "documentation".to_string(),
                        Value::String("type.Demo.Engine".to_string()),
                    )]),
                },
                KirElement {
                    id: "type.Demo.Engine".to_string(),
                    kind: "SysML::Systems::PartDefinition".to_string(),
                    layer: 2,
                    properties: BTreeMap::new(),
                },
            ],
        })
        .unwrap();

        assert!(graph.edges().is_empty());
    }

    #[test]
    fn registered_reference_list_scalar_creates_edge() {
        let graph = Graph::from_document(KirDocument {
            metadata: BTreeMap::new(),
            elements: vec![
                KirElement {
                    id: "type.Demo.Vehicle".to_string(),
                    kind: "SysML::Systems::PartDefinition".to_string(),
                    layer: 2,
                    properties: BTreeMap::from([(
                        "specializes".to_string(),
                        Value::String("type.Demo.Machine".to_string()),
                    )]),
                },
                KirElement {
                    id: "type.Demo.Machine".to_string(),
                    kind: "SysML::Systems::PartDefinition".to_string(),
                    layer: 2,
                    properties: BTreeMap::new(),
                },
            ],
        })
        .unwrap();

        assert_eq!(graph.edge_count(), 1);
        assert_eq!(graph.edges()[0].relation, "specializes");
    }
}
