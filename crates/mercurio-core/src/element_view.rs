use serde_json::Value;

use crate::graph::{Element, Graph, NodeId};
use crate::metamodel::{
    ElementAttributeQuery, ElementSummary, MetamodelAttributeRegistry,
    collect_specialization_ancestors, effective_properties_with_derived, element_metatype,
    query_element_attributes,
};
use crate::derived_properties;

#[derive(Debug, Clone, Copy)]
pub struct ElementView<'a> {
    graph: &'a Graph,
    registry: &'a MetamodelAttributeRegistry,
    node_id: NodeId,
}

impl<'a> ElementView<'a> {
    pub fn new(
        graph: &'a Graph,
        registry: &'a MetamodelAttributeRegistry,
        node_id: NodeId,
    ) -> Option<Self> {
        graph.element(node_id)?;
        Some(Self {
            graph,
            registry,
            node_id,
        })
    }

    pub fn by_id(
        graph: &'a Graph,
        registry: &'a MetamodelAttributeRegistry,
        element_id: &str,
    ) -> Option<Self> {
        Self::new(graph, registry, graph.node_id(element_id)?)
    }

    pub fn node_id(&self) -> NodeId {
        self.node_id
    }

    pub fn element(&self) -> &'a Element {
        self.graph
            .element(self.node_id)
            .expect("ElementView node id is validated at construction")
    }

    pub fn id(&self) -> &'a str {
        &self.element().element_id
    }

    pub fn kind(&self) -> &'a str {
        &self.element().kind
    }

    pub fn layer(&self) -> u8 {
        self.element().layer
    }

    pub fn get(&self, name: &str) -> Option<&'a Value> {
        self.element().properties.get(name)
    }

    pub fn get_str(&self, name: &str) -> Option<&'a str> {
        self.get(name).and_then(Value::as_str)
    }

    pub fn effective(&self, name: &str) -> Option<Value> {
        let element = self.element();
        let ancestors = collect_specialization_ancestors(self.graph, self.node_id);
        let derived = derived_properties(self.graph, element);
        let effective = effective_properties_with_derived(&ancestors, &element.properties, &derived);
        effective.get(name).cloned()
    }

    pub fn attributes(&self) -> Option<ElementAttributeQuery> {
        query_element_attributes(self.graph, self.registry, self.node_id, None)
    }

    pub fn metatype(&self) -> Option<ElementSummary> {
        element_metatype(self.graph, self.node_id).map(|element| ElementSummary {
            id: element.element_id.clone(),
            label: element
                .element_id
                .split("::")
                .last()
                .unwrap_or(&element.element_id)
                .split('.')
                .last()
                .unwrap_or(&element.element_id)
                .to_string(),
            kind: element.kind.clone(),
            layer: element.layer,
        })
    }

    pub fn references(&self, relation: &str) -> Vec<ElementView<'a>> {
        self.graph
            .outgoing(self.node_id, relation)
            .filter_map(|edge| ElementView::new(self.graph, self.registry, edge.target))
            .collect()
    }

    pub fn outgoing(&self, relation: &str) -> Vec<ElementView<'a>> {
        self.references(relation)
    }

    pub fn incoming(&self, relation: &str) -> Vec<ElementView<'a>> {
        self.graph
            .incoming(self.node_id, relation)
            .filter_map(|edge| ElementView::new(self.graph, self.registry, edge.source))
            .collect()
    }

    pub fn specializes(&self, target_element_id: &str) -> bool {
        if self.id() == target_element_id {
            return true;
        }
        collect_specialization_ancestors(self.graph, self.node_id)
            .iter()
            .any(|ancestor| ancestor.element_id == target_element_id)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::Value;

    use crate::{Graph, KirDocument, KirElement, MetamodelAttributeRegistry};

    use super::*;

    #[test]
    fn element_view_reads_properties_and_references() {
        let document = KirDocument {
            metadata: BTreeMap::new(),
            elements: vec![
                KirElement {
                    id: "pkg.Demo".to_string(),
                    kind: "SysML::Package".to_string(),
                    layer: 2,
                    properties: BTreeMap::from([(
                        "features".to_string(),
                        Value::Array(vec![Value::String("type.Demo.Vehicle".to_string())]),
                    )]),
                },
                KirElement {
                    id: "type.Demo.Vehicle".to_string(),
                    kind: "SysML::Systems::PartDefinition".to_string(),
                    layer: 2,
                    properties: BTreeMap::from([(
                        "qualified_name".to_string(),
                        Value::String("Demo.Vehicle".to_string()),
                    )]),
                },
            ],
        };
        let graph = Graph::from_document(document).unwrap();
        let registry = MetamodelAttributeRegistry::build(&graph);
        let view = ElementView::by_id(&graph, &registry, "pkg.Demo").unwrap();

        assert_eq!(view.id(), "pkg.Demo");
        assert_eq!(view.references("features")[0].id(), "type.Demo.Vehicle");
    }
}
