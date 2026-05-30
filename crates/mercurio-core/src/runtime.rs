use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use crate::datalog::{
    DatalogError, DerivedIndexes, RulePack, load_default_rulepacks, materialize_core_indexes,
};
use crate::expression::{
    ExpressionEvaluationContext, ExpressionEvaluationError, ExpressionIr, ExpressionPathSegment,
};
use crate::graph::{Graph, GraphArtifact, GraphError};
use crate::ir::KirDocument;

#[derive(Debug, Clone)]
pub struct Runtime {
    graph: Graph,
    derived: DerivedIndexes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeArtifact {
    pub graph: GraphArtifact,
    pub derived: DerivedIndexes,
}

#[derive(Debug, Clone, Default)]
pub struct ExecutionContext {
    pub values: HashMap<(String, String), Value>,
    pub version: u64,
}

#[derive(Debug, Clone)]
pub struct QueryResult<T> {
    pub value: T,
    pub explanation: Vec<String>,
}

#[derive(Debug)]
pub enum RuntimeError {
    Graph(GraphError),
    Datalog(DatalogError),
    InvalidExpression(String),
    MissingElement(String),
    UnsupportedAggregation(String),
    NonNumericValue { owner: String, feature: String },
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Graph(err) => write!(f, "{err}"),
            Self::Datalog(err) => write!(f, "{err}"),
            Self::InvalidExpression(expr) => write!(f, "invalid expression: {expr}"),
            Self::MissingElement(id) => write!(f, "missing element: {id}"),
            Self::UnsupportedAggregation(expr) => {
                write!(f, "unsupported aggregation expression: {expr}")
            }
            Self::NonNumericValue { owner, feature } => {
                write!(
                    f,
                    "non-numeric value encountered while reading {feature} from {owner}"
                )
            }
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<GraphError> for RuntimeError {
    fn from(value: GraphError) -> Self {
        Self::Graph(value)
    }
}

impl From<DatalogError> for RuntimeError {
    fn from(value: DatalogError) -> Self {
        Self::Datalog(value)
    }
}

impl From<ExpressionEvaluationError> for RuntimeError {
    fn from(value: ExpressionEvaluationError) -> Self {
        match value {
            ExpressionEvaluationError::InvalidExpression(expression) => {
                Self::InvalidExpression(expression)
            }
            ExpressionEvaluationError::UnsupportedAggregation { expression } => {
                Self::UnsupportedAggregation(expression)
            }
            ExpressionEvaluationError::UnsupportedFunction { .. } => {
                Self::InvalidExpression(value.to_string())
            }
            ExpressionEvaluationError::NonNumericValue { owner, feature } => {
                Self::NonNumericValue { owner, feature }
            }
        }
    }
}

impl Runtime {
    pub fn from_graph(graph: Graph) -> Result<Self, RuntimeError> {
        let rulepacks = load_default_rulepacks()?;
        Self::from_graph_with_rulepacks(graph, &rulepacks)
    }

    pub fn from_graph_with_rulepacks(
        graph: Graph,
        rulepacks: &[RulePack],
    ) -> Result<Self, RuntimeError> {
        let derived = materialize_core_indexes(&graph, rulepacks)?;
        Ok(Self { graph, derived })
    }

    pub fn from_document(document: KirDocument) -> Result<Self, RuntimeError> {
        Self::from_graph(Graph::from_document(document)?)
    }

    pub fn from_artifact(artifact: RuntimeArtifact) -> Result<Self, RuntimeError> {
        Ok(Self {
            graph: Graph::from_artifact(artifact.graph)?,
            derived: artifact.derived,
        })
    }

    pub fn into_artifact(self) -> RuntimeArtifact {
        RuntimeArtifact {
            graph: self.graph.artifact(),
            derived: self.derived,
        }
    }

    pub fn artifact(&self) -> RuntimeArtifact {
        RuntimeArtifact {
            graph: self.graph.artifact(),
            derived: self.derived.clone(),
        }
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    pub fn derived(&self) -> &DerivedIndexes {
        &self.derived
    }

    pub fn get_subtypes(&self, type_id: &str) -> Result<QueryResult<Vec<String>>, RuntimeError> {
        if self.graph.node_id(type_id).is_none() {
            return Err(RuntimeError::MissingElement(type_id.to_string()));
        }

        let subtypes = self
            .derived
            .subtypes
            .iter()
            .filter_map(|(subtype, supertype)| (supertype == type_id).then(|| subtype.to_string()))
            .collect::<Vec<_>>();
        let explanation = subtypes
            .iter()
            .map(|subtype| {
                if let Some(explanation) = self
                    .derived
                    .explanation_for("subtype", &[subtype.as_str(), type_id])
                {
                    format!("{subtype} derived by {}", explanation.rule_id)
                } else {
                    format!("{subtype} is a subtype of {type_id}")
                }
            })
            .collect();

        Ok(QueryResult {
            value: subtypes,
            explanation,
        })
    }

    pub fn get_features(&self, type_id: &str) -> Result<QueryResult<Vec<String>>, RuntimeError> {
        if self.graph.node_id(type_id).is_none() {
            return Err(RuntimeError::MissingElement(type_id.to_string()));
        }

        let features = self
            .derived
            .inherited_features
            .iter()
            .filter_map(|(owner, feature)| (owner == type_id).then(|| feature.to_string()))
            .collect::<Vec<_>>();
        let explanation = features
            .iter()
            .map(|feature| {
                if let Some(explanation) = self
                    .derived
                    .explanation_for("inherited_feature", &[type_id, feature.as_str()])
                {
                    format!("{feature} derived by {}", explanation.rule_id)
                } else {
                    format!("{type_id} owns feature {feature}")
                }
            })
            .collect();

        Ok(QueryResult {
            value: features,
            explanation,
        })
    }

    pub fn evaluate(
        &self,
        feature_id: &str,
        owner_id: &str,
        context: &ExecutionContext,
    ) -> Result<QueryResult<Value>, RuntimeError> {
        let feature = self
            .graph
            .element_by_element_id(feature_id)
            .ok_or_else(|| RuntimeError::MissingElement(feature_id.to_string()))?;
        if let Some(expression_ir) = feature.properties.get("expression_ir") {
            let value = self.evaluate_expression_ir(expression_ir, owner_id, context)?;
            return Ok(QueryResult {
                value,
                explanation: vec![
                    format!("read structured expression from {feature_id}"),
                    format!(
                        "evaluated against owner {owner_id} at context version {}",
                        context.version
                    ),
                ],
            });
        }

        let expression = feature
            .properties
            .get("expression")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                RuntimeError::InvalidExpression(format!("{feature_id} has no expression"))
            })?;

        let value = self.evaluate_expression(expression, owner_id, context)?;
        Ok(QueryResult {
            value,
            explanation: vec![
                format!("read expression `{expression}` from {feature_id}"),
                format!(
                    "evaluated against owner {owner_id} at context version {}",
                    context.version
                ),
            ],
        })
    }

    pub fn explain<T>(&self, result: &QueryResult<T>) -> String {
        result.explanation.join(" -> ")
    }

    fn evaluate_expression(
        &self,
        expression: &str,
        owner_id: &str,
        context: &ExecutionContext,
    ) -> Result<Value, RuntimeError> {
        if let Some(path) = parse_function(expression, "count") {
            let values = self.resolve_path(owner_id, path, context)?;
            return Ok(Value::Number(Number::from(values.len() as u64)));
        }

        if let Some(path) = parse_function(expression, "sum") {
            let values = self.resolve_path(owner_id, path, context)?;
            let mut total = 0.0_f64;

            for value in values {
                match value {
                    Value::Number(number) => {
                        total += number.as_f64().ok_or_else(|| {
                            RuntimeError::UnsupportedAggregation(expression.to_string())
                        })?;
                    }
                    _ => {
                        return Err(RuntimeError::NonNumericValue {
                            owner: owner_id.to_string(),
                            feature: expression.to_string(),
                        });
                    }
                }
            }

            let number = Number::from_f64(total)
                .ok_or_else(|| RuntimeError::UnsupportedAggregation(expression.to_string()))?;
            return Ok(Value::Number(number));
        }

        Err(RuntimeError::InvalidExpression(expression.to_string()))
    }

    fn evaluate_expression_ir(
        &self,
        expression: &Value,
        owner_id: &str,
        context: &ExecutionContext,
    ) -> Result<Value, RuntimeError> {
        let expression_ir = ExpressionIr::from_value(expression)
            .map_err(|err| RuntimeError::InvalidExpression(format!("{err}: {expression}")))?;
        let mut evaluation_context = RuntimeExpressionEvaluationContext {
            runtime: self,
            owner_id,
            context,
        };
        expression_ir
            .evaluate(&mut evaluation_context)
            .map_err(RuntimeError::from)
    }

    fn resolve_path(
        &self,
        owner_id: &str,
        path: &str,
        context: &ExecutionContext,
    ) -> Result<Vec<Value>, RuntimeError> {
        let segments: Vec<&str> = path.split('.').collect();
        if segments.first() != Some(&"self") || segments.len() < 2 {
            return Err(RuntimeError::InvalidExpression(path.to_string()));
        }

        self.resolve_path_segments(owner_id, &segments[1..], context)
    }

    fn resolve_path_segments(
        &self,
        owner_id: &str,
        segments: &[&str],
        context: &ExecutionContext,
    ) -> Result<Vec<Value>, RuntimeError> {
        if segments.is_empty() {
            return Err(RuntimeError::InvalidExpression("self".to_string()));
        }

        let mut current_ids = vec![owner_id.to_string()];

        for segment in &segments[..segments.len() - 1] {
            let mut next_ids = Vec::new();

            for current in &current_ids {
                let related = self.graph.relation_targets(current, segment)?;
                next_ids.extend(
                    related
                        .into_iter()
                        .map(|element| element.element_id.clone()),
                );
                for target in self.named_feature_targets(current, segment)? {
                    push_unique(&mut next_ids, target);
                }
            }

            current_ids = next_ids;
        }

        let final_segment = segments
            .last()
            .ok_or_else(|| RuntimeError::InvalidExpression("self".to_string()))?;

        let mut values = Vec::new();
        for current in &current_ids {
            let key = (current.clone(), (*final_segment).to_string());
            if let Some(value) = context.values.get(&key) {
                values.push(value.clone());
                continue;
            }

            if let Some(element) = self.graph.element_by_element_id(current) {
                if let Some(value) = element.properties.get(*final_segment) {
                    values.push(value.clone());
                    continue;
                }
            }

            let related = self.graph.relation_targets(current, final_segment)?;
            let mut related_values = related
                .into_iter()
                .map(|element| Value::String(element.element_id.clone()))
                .collect::<Vec<_>>();
            if related_values.is_empty() {
                let mut feature_ids = Vec::new();
                for feature_id in self.named_feature_targets(current, final_segment)? {
                    push_unique(&mut feature_ids, feature_id);
                }
                for feature_id in feature_ids {
                    values.push(self.feature_value(&feature_id, current, context)?);
                }
            } else {
                values.append(&mut related_values);
            }
        }

        Ok(values)
    }

    fn named_feature_targets(
        &self,
        owner_id: &str,
        feature_name: &str,
    ) -> Result<Vec<String>, RuntimeError> {
        let mut matches = self.direct_named_feature_targets(owner_id, feature_name)?;
        if !matches.is_empty() {
            return Ok(matches);
        }

        for relation in ["type", "definition"] {
            for target in self.graph.relation_targets(owner_id, relation)? {
                for matched in
                    self.direct_named_feature_targets(&target.element_id, feature_name)?
                {
                    push_unique(&mut matches, matched);
                }
            }
            if !matches.is_empty() {
                return Ok(matches);
            }
        }

        for target in self.graph.relation_targets(owner_id, "specializes")? {
            for matched in self.direct_named_feature_targets(&target.element_id, feature_name)? {
                push_unique(&mut matches, matched);
            }
        }

        Ok(matches)
    }

    fn direct_named_feature_targets(
        &self,
        owner_id: &str,
        feature_name: &str,
    ) -> Result<Vec<String>, RuntimeError> {
        let mut matches = Vec::new();
        for relation in ["features", "members"] {
            for target in self.graph.relation_targets(owner_id, relation)? {
                if element_name_matches(&target.properties, feature_name) {
                    push_unique(&mut matches, target.element_id.clone());
                }
            }
        }
        Ok(matches)
    }

    fn feature_value(
        &self,
        feature_id: &str,
        owner_id: &str,
        context: &ExecutionContext,
    ) -> Result<Value, RuntimeError> {
        let feature = self
            .graph
            .element_by_element_id(feature_id)
            .ok_or_else(|| RuntimeError::MissingElement(feature_id.to_string()))?;

        if let Some(name) = feature_name(&feature.properties)
            && let Some(value) = context
                .values
                .get(&(owner_id.to_string(), name.to_string()))
        {
            return Ok(value.clone());
        }

        if let Some(expression_ir) = feature.properties.get("expression_ir") {
            return self.evaluate_expression_ir(expression_ir, owner_id, context);
        }

        Ok(Value::String(feature_id.to_string()))
    }
}

fn element_name_matches(
    properties: &std::collections::BTreeMap<String, Value>,
    expected: &str,
) -> bool {
    feature_name(properties) == Some(expected)
}

fn feature_name(properties: &std::collections::BTreeMap<String, Value>) -> Option<&str> {
    properties
        .get("declared_name")
        .or_else(|| properties.get("name"))
        .and_then(Value::as_str)
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

struct RuntimeExpressionEvaluationContext<'a> {
    runtime: &'a Runtime,
    owner_id: &'a str,
    context: &'a ExecutionContext,
}

impl ExpressionEvaluationContext for RuntimeExpressionEvaluationContext<'_> {
    fn owner_id(&self) -> &str {
        self.owner_id
    }

    fn resolve_path(
        &mut self,
        segments: &[ExpressionPathSegment],
    ) -> Result<Vec<Value>, ExpressionEvaluationError> {
        let owned = segments
            .iter()
            .map(ExpressionPathSegment::name)
            .map(str::to_string)
            .collect::<Vec<_>>();
        let borrowed = owned.iter().map(String::as_str).collect::<Vec<_>>();
        self.runtime
            .resolve_path_segments(self.owner_id, &borrowed, self.context)
            .map_err(|err| ExpressionEvaluationError::InvalidExpression(err.to_string()))
    }
}

fn parse_function<'a>(expression: &'a str, function: &str) -> Option<&'a str> {
    let prefix = format!("{function}(");
    expression
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix(')'))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{ExecutionContext, Runtime};
    use crate::ir::{KirDocument, KirElement, load_model_stack};

    fn sample_runtime() -> Runtime {
        let document =
            load_model_stack(&crate::paths::repo_path("test_files/examples/vehicle_model.json"))
                .unwrap();
        Runtime::from_document(document).unwrap()
    }

    fn aggregate_feature(id: &str, function: &str) -> KirElement {
        KirElement {
            id: id.to_string(),
            kind: "KerML::Core::Feature".to_string(),
            layer: 2,
            properties: [(
                "expression_ir".to_string(),
                json!({
                    "kind": "call",
                    "function": function,
                    "args": [{
                        "kind": "path",
                        "root": "self",
                        "segments": ["parts", "mass"]
                    }]
                }),
            )]
            .into_iter()
            .collect(),
        }
    }

    #[test]
    fn finds_transitive_subtypes() {
        let runtime = sample_runtime();

        let result = runtime.get_subtypes("KerML::Core::Type").unwrap();
        assert!(
            result
                .value
                .contains(&"SysML::Systems::PartDefinition".to_string())
        );
        assert!(result.value.contains(&"type.Vehicle".to_string()));
    }

    #[test]
    fn inherits_features_across_specialization() {
        let runtime = sample_runtime();

        let result = runtime.get_features("type.Car").unwrap();
        assert!(result.value.contains(&"feature.engine".to_string()));
        assert!(result.value.contains(&"df.partCount".to_string()));
    }

    #[test]
    fn evaluates_derived_feature_against_overlay_context() {
        let runtime = sample_runtime();
        let mut context = ExecutionContext {
            values: std::collections::HashMap::new(),
            version: 7,
        };

        context.values.insert(
            ("part.engine_left".to_string(), "mass".to_string()),
            json!(120.5),
        );
        context.values.insert(
            ("part.engine_right".to_string(), "mass".to_string()),
            json!(130.0),
        );

        let result = runtime
            .evaluate("df.totalMass", "assembly.VehicleInstance", &context)
            .unwrap();
        assert_eq!(result.value, Value::from(250.5));
    }

    #[test]
    fn imported_stdlib_documentation_remains_passive_metadata() {
        let runtime = sample_runtime();
        let anything = runtime
            .graph()
            .element_by_element_id("Base::Anything")
            .unwrap();

        assert_eq!(anything.properties["doc"]["source"], "pilot");
        assert!(anything.properties.get("specializes").is_none());
    }

    #[test]
    fn evaluates_structured_expression_ir_against_overlay_context() {
        let document = KirDocument {
            metadata: Default::default(),
            elements: vec![
                KirElement {
                    id: "part.engine_left".to_string(),
                    kind: "type.Engine".to_string(),
                    layer: 2,
                    properties: Default::default(),
                },
                KirElement {
                    id: "part.engine_right".to_string(),
                    kind: "type.Engine".to_string(),
                    layer: 2,
                    properties: Default::default(),
                },
                KirElement {
                    id: "assembly.VehicleInstance".to_string(),
                    kind: "type.Vehicle".to_string(),
                    layer: 2,
                    properties: [(
                        "parts".to_string(),
                        json!(["part.engine_left", "part.engine_right"]),
                    )]
                    .into_iter()
                    .collect(),
                },
                KirElement {
                    id: "df.totalMass".to_string(),
                    kind: "KerML::Core::Feature".to_string(),
                    layer: 2,
                    properties: [(
                        "expression_ir".to_string(),
                        json!({
                            "kind": "call",
                            "function": "sum",
                            "args": [{
                                "kind": "path",
                                "root": "self",
                                "segments": ["parts", "mass"]
                            }]
                        }),
                    )]
                    .into_iter()
                    .collect(),
                },
            ],
        };
        let runtime = Runtime::from_document(document).unwrap();
        let mut context = ExecutionContext {
            values: std::collections::HashMap::new(),
            version: 11,
        };

        context.values.insert(
            ("part.engine_left".to_string(), "mass".to_string()),
            json!(120.5),
        );
        context.values.insert(
            ("part.engine_right".to_string(), "mass".to_string()),
            json!(130.0),
        );

        let result = runtime
            .evaluate("df.totalMass", "assembly.VehicleInstance", &context)
            .unwrap();
        assert_eq!(result.value, Value::from(250.5));
    }

    #[test]
    fn evaluates_structured_numeric_aggregate_functions() {
        let document = KirDocument {
            metadata: Default::default(),
            elements: vec![
                KirElement {
                    id: "part.engine_left".to_string(),
                    kind: "type.Engine".to_string(),
                    layer: 2,
                    properties: Default::default(),
                },
                KirElement {
                    id: "part.engine_center".to_string(),
                    kind: "type.Engine".to_string(),
                    layer: 2,
                    properties: Default::default(),
                },
                KirElement {
                    id: "part.engine_right".to_string(),
                    kind: "type.Engine".to_string(),
                    layer: 2,
                    properties: Default::default(),
                },
                KirElement {
                    id: "assembly.VehicleInstance".to_string(),
                    kind: "type.Vehicle".to_string(),
                    layer: 2,
                    properties: [(
                        "parts".to_string(),
                        json!([
                            "part.engine_left",
                            "part.engine_center",
                            "part.engine_right"
                        ]),
                    )]
                    .into_iter()
                    .collect(),
                },
                aggregate_feature("df.minMass", "min"),
                aggregate_feature("df.maxMass", "max"),
                aggregate_feature("df.avgMass", "avg"),
            ],
        };
        let runtime = Runtime::from_document(document).unwrap();
        let mut context = ExecutionContext::default();
        context.values.insert(
            ("part.engine_left".to_string(), "mass".to_string()),
            json!(100.0),
        );
        context.values.insert(
            ("part.engine_center".to_string(), "mass".to_string()),
            json!(125.0),
        );
        context.values.insert(
            ("part.engine_right".to_string(), "mass".to_string()),
            json!(150.0),
        );

        assert_eq!(
            runtime
                .evaluate("df.minMass", "assembly.VehicleInstance", &context)
                .unwrap()
                .value,
            Value::from(100.0)
        );
        assert_eq!(
            runtime
                .evaluate("df.maxMass", "assembly.VehicleInstance", &context)
                .unwrap()
                .value,
            Value::from(150.0)
        );
        assert_eq!(
            runtime
                .evaluate("df.avgMass", "assembly.VehicleInstance", &context)
                .unwrap()
                .value,
            Value::from(125.0)
        );
    }

    #[test]
    fn evaluates_structured_tuple_expression_ir() {
        let document = KirDocument {
            metadata: Default::default(),
            elements: vec![
                KirElement {
                    id: "assembly.VehicleInstance".to_string(),
                    kind: "type.Vehicle".to_string(),
                    layer: 2,
                    properties: Default::default(),
                },
                KirElement {
                    id: "df.tupleValue".to_string(),
                    kind: "KerML::Core::Feature".to_string(),
                    layer: 2,
                    properties: [(
                        "expression_ir".to_string(),
                        json!({
                            "kind": "tuple",
                            "items": [
                                {"kind": "literal", "value": 1},
                                {"kind": "literal", "value": true},
                                {"kind": "literal", "value": "ready"}
                            ]
                        }),
                    )]
                    .into_iter()
                    .collect(),
                },
            ],
        };
        let runtime = Runtime::from_document(document).unwrap();

        let result = runtime
            .evaluate(
                "df.tupleValue",
                "assembly.VehicleInstance",
                &ExecutionContext::default(),
            )
            .unwrap();
        assert_eq!(result.value, json!([1, true, "ready"]));
    }

    #[test]
    fn reports_unsupported_expression_ir_kind() {
        let document = KirDocument {
            metadata: Default::default(),
            elements: vec![
                KirElement {
                    id: "assembly.VehicleInstance".to_string(),
                    kind: "type.Vehicle".to_string(),
                    layer: 2,
                    properties: Default::default(),
                },
                KirElement {
                    id: "df.unsupported".to_string(),
                    kind: "KerML::Core::Feature".to_string(),
                    layer: 2,
                    properties: [(
                        "expression_ir".to_string(),
                        json!({"kind": "select", "source": {"kind": "self"}}),
                    )]
                    .into_iter()
                    .collect(),
                },
            ],
        };
        let runtime = Runtime::from_document(document).unwrap();

        let error = runtime
            .evaluate(
                "df.unsupported",
                "assembly.VehicleInstance",
                &ExecutionContext::default(),
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported expression_ir kind `select`")
        );
    }

    #[test]
    fn reports_unsupported_expression_ir_function() {
        let document = KirDocument {
            metadata: Default::default(),
            elements: vec![
                KirElement {
                    id: "assembly.VehicleInstance".to_string(),
                    kind: "type.Vehicle".to_string(),
                    layer: 2,
                    properties: Default::default(),
                },
                KirElement {
                    id: "df.unsupported".to_string(),
                    kind: "KerML::Core::Feature".to_string(),
                    layer: 2,
                    properties: [(
                        "expression_ir".to_string(),
                        json!({
                            "kind": "call",
                            "function": "median",
                            "args": [{"kind": "literal", "value": 1}]
                        }),
                    )]
                    .into_iter()
                    .collect(),
                },
            ],
        };
        let runtime = Runtime::from_document(document).unwrap();

        let error = runtime
            .evaluate(
                "df.unsupported",
                "assembly.VehicleInstance",
                &ExecutionContext::default(),
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported expression_ir function `median`")
        );
    }

    #[test]
    fn rejects_nonnumeric_values_in_structured_sum() {
        let document = KirDocument {
            metadata: Default::default(),
            elements: vec![
                KirElement {
                    id: "part.engine_left".to_string(),
                    kind: "type.Engine".to_string(),
                    layer: 2,
                    properties: Default::default(),
                },
                KirElement {
                    id: "assembly.VehicleInstance".to_string(),
                    kind: "type.Vehicle".to_string(),
                    layer: 2,
                    properties: [("parts".to_string(), json!(["part.engine_left"]))]
                        .into_iter()
                        .collect(),
                },
                KirElement {
                    id: "df.totalMass".to_string(),
                    kind: "KerML::Core::Feature".to_string(),
                    layer: 2,
                    properties: [(
                        "expression_ir".to_string(),
                        json!({
                            "kind": "call",
                            "function": "sum",
                            "args": [{
                                "kind": "path",
                                "root": "self",
                                "segments": [{"name": "parts", "feature": "feature.parts"}, {"name": "mass", "feature": "feature.mass"}]
                            }]
                        }),
                    )]
                    .into_iter()
                    .collect(),
                },
            ],
        };
        let runtime = Runtime::from_document(document).unwrap();
        let mut context = ExecutionContext::default();
        context.values.insert(
            ("part.engine_left".to_string(), "mass".to_string()),
            json!("heavy"),
        );

        let error = runtime
            .evaluate("df.totalMass", "assembly.VehicleInstance", &context)
            .unwrap_err();
        assert!(matches!(error, super::RuntimeError::NonNumericValue { .. }));
    }

    #[test]
    fn evaluates_feature_path_defaults_from_type_members() {
        let document = KirDocument {
            metadata: Default::default(),
            elements: vec![
                KirElement {
                    id: "type.EvalDemo.Engine".to_string(),
                    kind: "SysML::Systems::PartDefinition".to_string(),
                    layer: 2,
                    properties: [(
                        "features".to_string(),
                        json!(["feature.EvalDemo.Engine.mass"]),
                    )]
                    .into_iter()
                    .collect(),
                },
                KirElement {
                    id: "feature.EvalDemo.Engine.mass".to_string(),
                    kind: "KerML::Core::Feature".to_string(),
                    layer: 2,
                    properties: [
                        ("declared_name".to_string(), json!("mass")),
                        (
                            "expression_ir".to_string(),
                            json!({"kind": "literal", "value": 4.0}),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                },
                KirElement {
                    id: "type.EvalDemo.Vehicle".to_string(),
                    kind: "SysML::Systems::PartDefinition".to_string(),
                    layer: 2,
                    properties: [(
                        "features".to_string(),
                        json!([
                            "feature.EvalDemo.Vehicle.leftEngine",
                            "feature.EvalDemo.Vehicle.rightEngine"
                        ]),
                    )]
                    .into_iter()
                    .collect(),
                },
                KirElement {
                    id: "feature.EvalDemo.Vehicle.leftEngine".to_string(),
                    kind: "SysML::Parts::PartUsage".to_string(),
                    layer: 2,
                    properties: [
                        ("declared_name".to_string(), json!("leftEngine")),
                        ("type".to_string(), json!("type.EvalDemo.Engine")),
                    ]
                    .into_iter()
                    .collect(),
                },
                KirElement {
                    id: "feature.EvalDemo.Vehicle.rightEngine".to_string(),
                    kind: "SysML::Parts::PartUsage".to_string(),
                    layer: 2,
                    properties: [
                        ("declared_name".to_string(), json!("rightEngine")),
                        ("type".to_string(), json!("type.EvalDemo.Engine")),
                    ]
                    .into_iter()
                    .collect(),
                },
                KirElement {
                    id: "feature.EvalDemo.Vehicle.totalMass".to_string(),
                    kind: "KerML::Core::Feature".to_string(),
                    layer: 2,
                    properties: [(
                        "expression_ir".to_string(),
                        json!({
                            "kind": "binary",
                            "op": "add",
                            "left": {
                                "kind": "call",
                                "function": "sum",
                                "args": [{
                                    "kind": "path",
                                    "root": "self",
                                    "segments": ["leftEngine", "mass"]
                                }]
                            },
                            "right": {
                                "kind": "call",
                                "function": "sum",
                                "args": [{
                                    "kind": "path",
                                    "root": "self",
                                    "segments": ["rightEngine", "mass"]
                                }]
                            }
                        }),
                    )]
                    .into_iter()
                    .collect(),
                },
            ],
        };
        let runtime = Runtime::from_document(document).unwrap();

        let result = runtime
            .evaluate(
                "feature.EvalDemo.Vehicle.totalMass",
                "type.EvalDemo.Vehicle",
                &ExecutionContext::default(),
            )
            .unwrap();

        assert_eq!(result.value, Value::from(8.0));
    }
}
