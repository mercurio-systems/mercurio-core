use std::collections::{BTreeSet, VecDeque};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::graph::{Element, Graph, NodeId};
use crate::metamodel::MetamodelAttributeRegistry;

const DEFAULT_MAX_DEPTH: usize = 8;
const DEFAULT_MAX_NODES: usize = 350;
const DEFAULT_MAX_EDGES: usize = 900;
const MAX_RELATION_FANOUT_PER_NODE: usize = 250;
const TIMING_WARNING_THRESHOLD_MS: u128 = 250;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagramKindDto {
    Structure,
    PackageTree,
    CompositionGraph,
    ReferenceGraph,
    DependencyGraph,
    MetatypeInstanceMap,
    ImpactView,
    PropertyInheritance,
    ValidationView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagramDirectionDto {
    Parents,
    Children,
    Both,
}

impl Default for DiagramDirectionDto {
    fn default() -> Self {
        Self::Children
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiagramQueryOptionsDto {
    #[serde(default = "default_diagram_relations")]
    pub relations: Vec<String>,
    #[serde(default)]
    pub direction: DiagramDirectionDto,
    #[serde(default = "default_diagram_depth")]
    pub depth: usize,
    #[serde(default = "default_true")]
    pub include_libraries: bool,
    #[serde(default = "default_true")]
    pub include_user_model: bool,
    #[serde(default = "default_max_nodes")]
    pub max_nodes: usize,
    #[serde(default = "default_max_edges")]
    pub max_edges: usize,
}

impl Default for DiagramQueryOptionsDto {
    fn default() -> Self {
        Self {
            relations: default_diagram_relations(),
            direction: DiagramDirectionDto::default(),
            depth: default_diagram_depth(),
            include_libraries: true,
            include_user_model: true,
            max_nodes: default_max_nodes(),
            max_edges: default_max_edges(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiagramLayoutOptionsDto {
    #[serde(default = "default_layout_engine")]
    pub engine: String,
    #[serde(default = "default_layout_direction")]
    pub direction: String,
}

impl Default for DiagramLayoutOptionsDto {
    fn default() -> Self {
        Self {
            engine: default_layout_engine(),
            direction: default_layout_direction(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiagramStyleOptionsDto {
    #[serde(default = "default_true")]
    pub show_attributes: bool,
    #[serde(default = "default_true")]
    pub show_edge_labels: bool,
    #[serde(default)]
    pub group_by_layer: bool,
}

impl Default for DiagramStyleOptionsDto {
    fn default() -> Self {
        Self {
            show_attributes: true,
            show_edge_labels: true,
            group_by_layer: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiagramSpecDto {
    pub version: u8,
    pub kind: DiagramKindDto,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default)]
    pub query: DiagramQueryOptionsDto,
    #[serde(default)]
    pub layout: DiagramLayoutOptionsDto,
    #[serde(default)]
    pub style: DiagramStyleOptionsDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiagramRenderRequestDto {
    pub spec: DiagramSpecDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiagramViewDto {
    pub spec: DiagramSpecDto,
    pub nodes: Vec<DiagramNodeDto>,
    pub edges: Vec<DiagramEdgeDto>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiagramNodeDto {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub layer: u8,
    pub badges: Vec<String>,
    pub attributes: Vec<DiagramAttributeDto>,
    pub properties: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiagramAttributeDto {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiagramEdgeDto {
    pub id: String,
    pub source: String,
    pub target: String,
    pub relation: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagramError {
    UnsupportedKind(DiagramKindDto),
    UnsupportedVersion(u8),
    MissingRoot,
    RootNotFound(String),
}

impl std::fmt::Display for DiagramError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedKind(kind) => write!(f, "diagram kind is not implemented: {kind:?}"),
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported diagram spec version: {version}")
            }
            Self::MissingRoot => write!(f, "diagram root is required"),
            Self::RootNotFound(root) => write!(f, "diagram root not found: {root}"),
        }
    }
}

impl std::error::Error for DiagramError {}

pub fn list_diagram_kinds() -> Vec<DiagramKindDto> {
    vec![
        DiagramKindDto::Structure,
        DiagramKindDto::PackageTree,
        DiagramKindDto::CompositionGraph,
        DiagramKindDto::ReferenceGraph,
        DiagramKindDto::DependencyGraph,
        DiagramKindDto::MetatypeInstanceMap,
        DiagramKindDto::ImpactView,
        DiagramKindDto::PropertyInheritance,
        DiagramKindDto::ValidationView,
    ]
}

pub fn render_diagram(
    graph: &Graph,
    metamodel_registry: &MetamodelAttributeRegistry,
    spec: DiagramSpecDto,
) -> Result<DiagramViewDto, DiagramError> {
    if spec.version != 1 {
        return Err(DiagramError::UnsupportedVersion(spec.version));
    }

    match spec.kind {
        DiagramKindDto::Structure => render_structure_diagram(graph, metamodel_registry, spec),
        _ => Err(DiagramError::UnsupportedKind(spec.kind)),
    }
}

fn render_structure_diagram(
    graph: &Graph,
    metamodel_registry: &MetamodelAttributeRegistry,
    spec: DiagramSpecDto,
) -> Result<DiagramViewDto, DiagramError> {
    let total_start = Instant::now();
    let mut timings = Vec::new();
    let mut warnings = Vec::new();

    let root_start = Instant::now();
    let root = spec.root.as_deref().ok_or(DiagramError::MissingRoot)?;
    let root =
        resolve_root(graph, root).ok_or_else(|| DiagramError::RootNotFound(root.to_string()))?;
    timings.push(("root", root_start.elapsed()));

    let relation_start = Instant::now();
    let relations = if spec.query.relations.is_empty() {
        default_diagram_relations()
    } else {
        spec.query.relations.clone()
    };
    timings.push(("relations", relation_start.elapsed()));

    let traversal_start = Instant::now();
    let traversal = collect_structure_ids(graph, root.id, &spec.query, &relations);
    timings.push(("traversal", traversal_start.elapsed()));
    warnings.extend(traversal.warnings);

    let node_start = Instant::now();
    let mut nodes = traversal
        .visible_ids
        .iter()
        .filter_map(|node_id| graph.element(*node_id))
        .filter(|element| include_element(element, &spec.query))
        .take(effective_max_nodes(&spec.query))
        .map(|element| diagram_node(graph, metamodel_registry, element))
        .collect::<Vec<_>>();
    nodes.sort_by(|left, right| left.id.cmp(&right.id));
    timings.push(("nodes", node_start.elapsed()));

    if nodes.is_empty() {
        warnings.push("No diagram nodes matched the requested filters.".to_string());
    }
    if traversal.visible_ids.len() > nodes.len() {
        warnings.push(format!(
            "Diagram node limit reached; showing {} of {} traversed nodes.",
            nodes.len(),
            traversal.visible_ids.len()
        ));
    }

    let edge_start = Instant::now();
    let retained_ids = nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut edges = Vec::new();
    let max_edges = effective_max_edges(&spec.query);
    'node_edges: for node_id in &traversal.visible_ids {
        for edge in graph.outgoing_edges(*node_id) {
            if !relations.iter().any(|relation| relation == &edge.relation) {
                continue;
            }
            let Some(source) = graph.element_id(edge.source) else {
                continue;
            };
            let Some(target) = graph.element_id(edge.target) else {
                continue;
            };
            if retained_ids.contains(source) && retained_ids.contains(target) {
                edges.push(DiagramEdgeDto {
                    id: format!("{}:{}:{}", edge.relation, source, target),
                    source: source.to_string(),
                    target: target.to_string(),
                    relation: edge.relation.clone(),
                    label: edge.relation.clone(),
                });
                if edges.len() >= max_edges {
                    warnings.push(format!(
                        "Diagram edge limit reached; showing first {max_edges} matching edges."
                    ));
                    break 'node_edges;
                }
            }
        }
    }
    edges.sort_by(|left, right| left.id.cmp(&right.id));
    edges.dedup_by(|left, right| left.id == right.id);
    timings.push(("edges", edge_start.elapsed()));

    timings.push(("total", total_start.elapsed()));
    let slow_phases = timings
        .iter()
        .filter(|(_, elapsed)| elapsed.as_millis() >= TIMING_WARNING_THRESHOLD_MS)
        .map(|(phase, elapsed)| format!("{phase}={}ms", elapsed.as_millis()))
        .collect::<Vec<_>>();
    if !slow_phases.is_empty() {
        warnings.push(format!(
            "Diagram render timing: {}.",
            slow_phases.join(", ")
        ));
    }

    Ok(DiagramViewDto {
        spec,
        nodes,
        edges,
        warnings,
    })
}

struct StructureTraversal {
    visible_ids: BTreeSet<NodeId>,
    warnings: Vec<String>,
}

fn collect_structure_ids(
    graph: &Graph,
    root_id: NodeId,
    query: &DiagramQueryOptionsDto,
    relations: &[String],
) -> StructureTraversal {
    let mut visited = BTreeSet::new();
    let mut queue = VecDeque::from([(root_id, 0usize)]);
    let mut warnings = Vec::new();
    let max_depth = query.depth.min(DEFAULT_MAX_DEPTH);
    let max_nodes = effective_max_nodes(query);
    if query.depth > max_depth {
        warnings.push(format!(
            "Diagram depth limit reached; requested depth {} capped at {max_depth}.",
            query.depth
        ));
    }

    while let Some((node_id, depth)) = queue.pop_front() {
        if !visited.insert(node_id) {
            continue;
        }
        if visited.len() >= max_nodes {
            warnings.push(format!(
                "Diagram traversal node limit reached at {max_nodes} nodes."
            ));
            break;
        }
        if depth >= max_depth {
            continue;
        }

        if matches!(
            query.direction,
            DiagramDirectionDto::Parents | DiagramDirectionDto::Both
        ) {
            for relation in relations {
                for edge in graph
                    .outgoing(node_id, relation)
                    .take(MAX_RELATION_FANOUT_PER_NODE)
                {
                    queue.push_back((edge.target, depth + 1));
                }
                if graph.outgoing(node_id, relation).count() > MAX_RELATION_FANOUT_PER_NODE {
                    warnings.push(format!(
                        "Diagram relation fan-out limit reached for `{relation}`."
                    ));
                }
            }
        }

        if matches!(
            query.direction,
            DiagramDirectionDto::Children | DiagramDirectionDto::Both
        ) {
            for relation in relations {
                for edge in graph
                    .incoming(node_id, relation)
                    .take(MAX_RELATION_FANOUT_PER_NODE)
                {
                    queue.push_back((edge.source, depth + 1));
                }
                if graph.incoming(node_id, relation).count() > MAX_RELATION_FANOUT_PER_NODE {
                    warnings.push(format!(
                        "Diagram relation fan-out limit reached for incoming `{relation}`."
                    ));
                }
            }
        }
    }

    StructureTraversal {
        visible_ids: visited,
        warnings,
    }
}

fn resolve_root<'a>(graph: &'a Graph, root: &str) -> Option<&'a Element> {
    if let Some(element) = graph.element_by_element_id(root) {
        return Some(element);
    }

    let normalized_root = root.trim().to_ascii_lowercase();
    graph.elements().iter().find(|element| {
        label_for_id(&element.element_id).to_ascii_lowercase() == normalized_root
            || element
                .element_id
                .rsplit("::")
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case(root))
            || element
                .element_id
                .rsplit('.')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case(root))
    })
}

fn include_element(element: &Element, query: &DiagramQueryOptionsDto) -> bool {
    if element.layer < 2 {
        return query.include_libraries;
    }

    query.include_user_model
}

fn diagram_node(
    graph: &Graph,
    metamodel_registry: &MetamodelAttributeRegistry,
    element: &Element,
) -> DiagramNodeDto {
    let attributes =
        crate::metamodel::query_element_attributes(graph, metamodel_registry, element.id, None)
            .map(|query| query.rows)
            .unwrap_or_default()
            .into_iter()
            .map(|attribute| DiagramAttributeDto {
                name: attribute.name,
                type_label: attribute
                    .effective_value
                    .as_ref()
                    .map(|value| value_type_label(value).to_string()),
            })
            .collect();

    DiagramNodeDto {
        id: element.element_id.clone(),
        label: label_for_id(&element.element_id),
        kind: element.kind.clone(),
        layer: element.layer,
        badges: vec![format!("L{}", element.layer)],
        attributes,
        properties: element
            .properties
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    }
}

fn label_for_id(id: &str) -> String {
    id.rsplit("::")
        .next()
        .and_then(|segment| segment.rsplit('.').next())
        .filter(|segment| !segment.is_empty())
        .unwrap_or(id)
        .to_string()
}

fn default_diagram_relations() -> Vec<String> {
    vec!["specializes".to_string()]
}

fn default_diagram_depth() -> usize {
    3
}

fn default_max_nodes() -> usize {
    DEFAULT_MAX_NODES
}

fn default_max_edges() -> usize {
    DEFAULT_MAX_EDGES
}

fn effective_max_nodes(query: &DiagramQueryOptionsDto) -> usize {
    query.max_nodes.clamp(1, DEFAULT_MAX_NODES)
}

fn effective_max_edges(query: &DiagramQueryOptionsDto) -> usize {
    query.max_edges.clamp(1, DEFAULT_MAX_EDGES)
}

fn default_layout_engine() -> String {
    "dagre".to_string()
}

fn default_layout_direction() -> String {
    "LR".to_string()
}

fn default_true() -> bool {
    true
}

fn value_type_label(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
