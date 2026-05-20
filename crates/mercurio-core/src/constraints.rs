use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::graph::{Element, Graph};
use crate::runtime::ExecutionContext;

const EPSILON: f64 = 1.0e-9;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConstraintGraphRequestDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    #[serde(default = "default_radius")]
    pub radius: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario: Option<String>,
    #[serde(default)]
    pub context_values: BTreeMap<String, BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConstraintSolveRequestDto {
    #[serde(default)]
    pub context_values: BTreeMap<String, BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ConstraintGraphViewDto {
    pub variables: Vec<ConstraintVariableDto>,
    pub constraints: Vec<ConstraintRecordDto>,
    pub requirements: Vec<RequirementCheckDto>,
    pub edges: Vec<ConstraintGraphEdgeDto>,
    pub diagnostics: Vec<ConstraintDiagnosticDto>,
    pub explanations: Vec<ConstraintExplanationDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scenario: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ConstraintSolveResultDto {
    pub variables: Vec<ConstraintVariableDto>,
    pub constraints: Vec<ConstraintRecordDto>,
    pub requirements: Vec<RequirementCheckDto>,
    pub diagnostics: Vec<ConstraintDiagnosticDto>,
    pub explanations: Vec<ConstraintExplanationDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ConstraintVariableDto {
    pub id: String,
    pub label: String,
    pub status: ConstraintVariableStatusDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintVariableStatusDto {
    Given,
    Inferred,
    Unknown,
    Conflicting,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ConstraintRecordDto {
    pub id: String,
    pub label: String,
    pub expression: String,
    pub status: ConstraintStatusDto,
    pub variables: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintStatusDto {
    Satisfied,
    Solved,
    Blocked,
    Violated,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RequirementCheckDto {
    pub id: String,
    pub label: String,
    pub expression: String,
    pub status: RequirementStatusDto,
    pub variables: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RequirementStatusDto {
    Satisfied,
    Violated,
    Unverifiable,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConstraintGraphEdgeDto {
    pub id: String,
    pub source: String,
    pub target: String,
    pub relation: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConstraintDiagnosticDto {
    pub kind: String,
    pub message: String,
    pub element_id: String,
    pub variables: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ConstraintExplanationDto {
    pub variable_id: String,
    pub value: Value,
    pub constraint_id: String,
    pub steps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConstraintError {
    Parse { expression: String, message: String },
}

impl fmt::Display for ConstraintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse {
                expression,
                message,
            } => write!(
                f,
                "failed to parse constraint expression `{expression}`: {message}"
            ),
        }
    }
}

impl std::error::Error for ConstraintError {}

#[derive(Debug, Clone, PartialEq)]
enum Expr {
    Number(f64),
    Var(String),
    UnaryMinus(Box<Expr>),
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Compare {
        op: CompareOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompareOp {
    Eq,
    Le,
    Ge,
    Lt,
    Gt,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
    EqEq,
    Le,
    Ge,
    Lt,
    Gt,
}

#[derive(Debug, Clone)]
struct ConstraintSource {
    id: String,
    label: String,
    expression: String,
    kind: ConstraintSourceKind,
    variables: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConstraintSourceKind {
    Equation,
    Requirement,
}

#[derive(Debug, Clone)]
struct ParsedConstraintSource {
    source: ConstraintSource,
    expr: Option<Expr>,
    parse_error: Option<String>,
}

#[derive(Debug, Clone)]
struct SolveState {
    values: HashMap<String, f64>,
    value_sources: HashMap<String, String>,
    statuses: HashMap<String, ConstraintVariableStatusDto>,
    records: HashMap<String, ConstraintRecordDto>,
    requirements: HashMap<String, RequirementCheckDto>,
    diagnostics: Vec<ConstraintDiagnosticDto>,
    explanations: Vec<ConstraintExplanationDto>,
}

pub fn solve_constraints(
    graph: &Graph,
    context: &ExecutionContext,
) -> Result<ConstraintSolveResultDto, ConstraintError> {
    let mut sources = constraint_sources(graph);
    sources.sort_by(|left, right| left.id.cmp(&right.id));
    let parsed = parse_sources(&sources);
    let mut state = initial_state(context, &parsed);

    for _ in 0..(parsed.len().saturating_mul(8).max(1)) {
        let mut changed = false;
        for parsed_source in &parsed {
            if parsed_source.source.kind != ConstraintSourceKind::Equation {
                continue;
            }
            let Some(expr) = &parsed_source.expr else {
                continue;
            };
            if evaluate_equation(&parsed_source.source, expr, &mut state) {
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    for parsed_source in &parsed {
        match (&parsed_source.source.kind, &parsed_source.expr) {
            (ConstraintSourceKind::Equation, Some(expr)) => {
                finalize_equation(&parsed_source.source, expr, &mut state)
            }
            (ConstraintSourceKind::Requirement, Some(expr)) => {
                finalize_requirement(&parsed_source.source, expr, &mut state)
            }
            (ConstraintSourceKind::Equation, None) => {
                finalize_unsupported_equation(parsed_source, &mut state)
            }
            (ConstraintSourceKind::Requirement, None) => {
                finalize_unsupported_requirement(parsed_source, &mut state)
            }
        }
    }

    Ok(result_from_state(state))
}

pub fn render_constraint_graph(
    graph: &Graph,
    request: ConstraintGraphRequestDto,
) -> Result<ConstraintGraphViewDto, ConstraintError> {
    let context = execution_context_from_nested_values(&request.context_values);
    let result = solve_constraints(graph, &context)?;
    let focus = request.focus.clone();
    let visible = visible_variables(&result, focus.as_deref(), request.radius);
    let variables = result
        .variables
        .into_iter()
        .filter(|variable| visible.is_empty() || visible.contains(&variable.id))
        .collect::<Vec<_>>();
    let constraints = result
        .constraints
        .into_iter()
        .filter(|constraint| {
            visible.is_empty()
                || constraint
                    .variables
                    .iter()
                    .any(|variable| visible.contains(variable))
        })
        .collect::<Vec<_>>();
    let requirements = result
        .requirements
        .into_iter()
        .filter(|requirement| {
            visible.is_empty()
                || requirement
                    .variables
                    .iter()
                    .any(|variable| visible.contains(variable))
        })
        .collect::<Vec<_>>();
    let edges = graph_edges(&constraints, &requirements);

    Ok(ConstraintGraphViewDto {
        variables,
        constraints,
        requirements,
        edges,
        diagnostics: result.diagnostics,
        explanations: result.explanations,
        focus,
        scenario: request.scenario,
    })
}

pub fn execution_context_from_nested_values(
    values: &BTreeMap<String, BTreeMap<String, Value>>,
) -> ExecutionContext {
    let mut context = ExecutionContext::default();
    for (owner_id, features) in values {
        for (feature, value) in features {
            context
                .values
                .insert((owner_id.clone(), feature.clone()), value.clone());
        }
    }
    context
}

fn constraint_sources(graph: &Graph) -> Vec<ConstraintSource> {
    graph
        .elements()
        .iter()
        .filter_map(source_from_element)
        .collect()
}

fn source_from_element(element: &Element) -> Option<ConstraintSource> {
    let label = element_label(element);
    if let Some(expression) =
        string_property(element, &["equation", "constraint", "check", "expression"])
    {
        let kind =
            if element.kind.contains("Requirement") || element.properties.contains_key("check") {
                ConstraintSourceKind::Requirement
            } else if element.kind.contains("Constraint")
                || element.kind.contains("Calculation")
                || element.properties.contains_key("equation")
                || element.properties.contains_key("constraint")
            {
                ConstraintSourceKind::Equation
            } else {
                return None;
            };

        return Some(ConstraintSource {
            id: element.element_id.clone(),
            label,
            expression: expression.to_string(),
            kind,
            variables: Vec::new(),
        });
    }

    let expression_ir = element.properties.get("expression_ir")?;
    let target = variable_id_from_element(element);
    let rhs = render_expression_ir(expression_ir);
    let mut variables = BTreeSet::from([target.clone()]);
    collect_expression_ir_variables(expression_ir, &mut variables);
    Some(ConstraintSource {
        id: element.element_id.clone(),
        label,
        expression: format!("{target} == {rhs}"),
        kind: ConstraintSourceKind::Equation,
        variables: variables.into_iter().collect(),
    })
}

fn element_label(element: &Element) -> String {
    element
        .properties
        .get("label")
        .and_then(Value::as_str)
        .or_else(|| element.properties.get("name").and_then(Value::as_str))
        .unwrap_or_else(|| last_segment(&element.element_id))
        .to_string()
}

fn variable_id_from_element(element: &Element) -> String {
    element
        .properties
        .get("qualified_name")
        .and_then(Value::as_str)
        .or_else(|| element.properties.get("name").and_then(Value::as_str))
        .unwrap_or(&element.element_id)
        .to_string()
}

fn string_property<'a>(element: &'a Element, names: &[&str]) -> Option<&'a str> {
    names
        .iter()
        .find_map(|name| element.properties.get(*name).and_then(Value::as_str))
}

fn parse_sources(sources: &[ConstraintSource]) -> Vec<ParsedConstraintSource> {
    sources
        .iter()
        .map(|source| {
            let parsed = parse_expression(&source.expression);
            match parsed {
                Ok(expr) => ParsedConstraintSource {
                    source: source.clone(),
                    expr: Some(expr),
                    parse_error: None,
                },
                Err(message) => ParsedConstraintSource {
                    source: source.clone(),
                    expr: None,
                    parse_error: Some(message),
                },
            }
        })
        .collect()
}

fn initial_state(context: &ExecutionContext, parsed: &[ParsedConstraintSource]) -> SolveState {
    let mut values = HashMap::new();
    let mut sources = HashMap::new();
    let mut statuses = HashMap::new();

    for ((owner, feature), value) in &context.values {
        if let Some(number) = value.as_f64() {
            let qualified = format!("{owner}.{feature}");
            values.insert(qualified.clone(), number);
            sources.insert(qualified.clone(), "context".to_string());
            statuses.insert(qualified, ConstraintVariableStatusDto::Given);
            values.insert(feature.clone(), number);
            sources.insert(feature.clone(), format!("context:{owner}.{feature}"));
            statuses.insert(feature.clone(), ConstraintVariableStatusDto::Given);
        }
    }

    for parsed_source in parsed {
        for variable in parsed_source.variables() {
            statuses
                .entry(variable)
                .or_insert(ConstraintVariableStatusDto::Unknown);
        }
    }

    SolveState {
        values,
        value_sources: sources,
        statuses,
        records: HashMap::new(),
        requirements: HashMap::new(),
        diagnostics: Vec::new(),
        explanations: Vec::new(),
    }
}

impl ParsedConstraintSource {
    fn variables(&self) -> Vec<String> {
        self.expr
            .as_ref()
            .map(variables)
            .unwrap_or_else(|| self.source.variables.clone())
    }
}

fn evaluate_equation(source: &ConstraintSource, expr: &Expr, state: &mut SolveState) -> bool {
    let Expr::Compare {
        op: CompareOp::Eq,
        left,
        right,
    } = expr
    else {
        return false;
    };
    let vars = variables(expr);
    let unknowns = vars
        .iter()
        .filter(|var| !state.values.contains_key(*var))
        .cloned()
        .collect::<Vec<_>>();
    if unknowns.len() != 1 {
        return false;
    }
    let target = &unknowns[0];
    let solved = match (
        eval_number(left, &state.values),
        eval_number(right, &state.values),
    ) {
        (Some(value), None) => solve_for(right, target, value, &state.values),
        (None, Some(value)) => solve_for(left, target, value, &state.values),
        _ => None,
    };

    let Some(value) = solved else {
        return false;
    };
    let Some(json_value) = json_number(value) else {
        return false;
    };
    state.values.insert(target.clone(), value);
    state
        .value_sources
        .insert(target.clone(), source.id.clone());
    state
        .statuses
        .insert(target.clone(), ConstraintVariableStatusDto::Inferred);
    state.explanations.push(ConstraintExplanationDto {
        variable_id: target.clone(),
        value: json_value,
        constraint_id: source.id.clone(),
        steps: vec![
            format!("used {}", source.expression),
            format!("isolated {target}"),
            format!("{target} = {value}"),
        ],
    });
    true
}

fn finalize_equation(source: &ConstraintSource, expr: &Expr, state: &mut SolveState) {
    let vars = variables(expr);
    let status = match expr {
        Expr::Compare {
            op: CompareOp::Eq,
            left,
            right,
        } => match (
            eval_number(left, &state.values),
            eval_number(right, &state.values),
        ) {
            (Some(left), Some(right)) if nearly_equal(left, right) => {
                ConstraintStatusDto::Satisfied
            }
            (Some(_), Some(_)) => {
                state.diagnostics.push(ConstraintDiagnosticDto {
                    kind: "constraint.violation".to_string(),
                    message: format!("constraint {} is inconsistent", source.id),
                    element_id: source.id.clone(),
                    variables: vars.clone(),
                });
                ConstraintStatusDto::Violated
            }
            _ if vars.iter().any(|var| !state.values.contains_key(var)) => {
                state.diagnostics.push(ConstraintDiagnosticDto {
                    kind: "constraint.blocked".to_string(),
                    message: format!("constraint {} has unresolved variables", source.id),
                    element_id: source.id.clone(),
                    variables: vars
                        .iter()
                        .filter(|var| !state.values.contains_key(*var))
                        .cloned()
                        .collect(),
                });
                ConstraintStatusDto::Blocked
            }
            _ => ConstraintStatusDto::Unsupported,
        },
        _ => ConstraintStatusDto::Unsupported,
    };

    state.records.insert(
        source.id.clone(),
        ConstraintRecordDto {
            id: source.id.clone(),
            label: source.label.clone(),
            expression: source.expression.clone(),
            status,
            variables: vars,
        },
    );
}

fn finalize_unsupported_equation(parsed: &ParsedConstraintSource, state: &mut SolveState) {
    let vars = parsed.variables();
    if let Some(message) = &parsed.parse_error {
        state.diagnostics.push(ConstraintDiagnosticDto {
            kind: "constraint.unsupported".to_string(),
            message: format!(
                "constraint {} cannot be evaluated by the lightweight solver: {}",
                parsed.source.id, message
            ),
            element_id: parsed.source.id.clone(),
            variables: vars.clone(),
        });
    }
    state.records.insert(
        parsed.source.id.clone(),
        ConstraintRecordDto {
            id: parsed.source.id.clone(),
            label: parsed.source.label.clone(),
            expression: parsed.source.expression.clone(),
            status: ConstraintStatusDto::Unsupported,
            variables: vars,
        },
    );
}

fn finalize_requirement(source: &ConstraintSource, expr: &Expr, state: &mut SolveState) {
    let vars = variables(expr);
    let (status, margin) = match expr {
        Expr::Compare { op, left, right } => match (
            eval_number(left, &state.values),
            eval_number(right, &state.values),
        ) {
            (Some(left), Some(right)) => {
                let satisfied = compare_numbers(*op, left, right);
                let margin = match op {
                    CompareOp::Ge | CompareOp::Gt => Some(left - right),
                    CompareOp::Le | CompareOp::Lt => Some(right - left),
                    CompareOp::Eq => Some((left - right).abs()),
                };
                (
                    if satisfied {
                        RequirementStatusDto::Satisfied
                    } else {
                        RequirementStatusDto::Violated
                    },
                    margin,
                )
            }
            _ => {
                state.diagnostics.push(ConstraintDiagnosticDto {
                    kind: "requirement.unverifiable".to_string(),
                    message: format!("requirement {} cannot be checked", source.id),
                    element_id: source.id.clone(),
                    variables: vars
                        .iter()
                        .filter(|var| !state.values.contains_key(*var))
                        .cloned()
                        .collect(),
                });
                (RequirementStatusDto::Unverifiable, None)
            }
        },
        _ => (RequirementStatusDto::Unsupported, None),
    };

    state.requirements.insert(
        source.id.clone(),
        RequirementCheckDto {
            id: source.id.clone(),
            label: source.label.clone(),
            expression: source.expression.clone(),
            status,
            variables: vars,
            margin,
        },
    );
}

fn finalize_unsupported_requirement(parsed: &ParsedConstraintSource, state: &mut SolveState) {
    let vars = parsed.variables();
    if let Some(message) = &parsed.parse_error {
        state.diagnostics.push(ConstraintDiagnosticDto {
            kind: "requirement.unsupported".to_string(),
            message: format!(
                "requirement {} cannot be checked by the lightweight solver: {}",
                parsed.source.id, message
            ),
            element_id: parsed.source.id.clone(),
            variables: vars.clone(),
        });
    }
    state.requirements.insert(
        parsed.source.id.clone(),
        RequirementCheckDto {
            id: parsed.source.id.clone(),
            label: parsed.source.label.clone(),
            expression: parsed.source.expression.clone(),
            status: RequirementStatusDto::Unsupported,
            variables: vars,
            margin: None,
        },
    );
}

fn result_from_state(state: SolveState) -> ConstraintSolveResultDto {
    let mut variables = state
        .statuses
        .into_iter()
        .map(|(id, status)| ConstraintVariableDto {
            label: last_segment(&id).to_string(),
            value: state.values.get(&id).and_then(|value| json_number(*value)),
            source: state.value_sources.get(&id).cloned(),
            id,
            status,
        })
        .collect::<Vec<_>>();
    variables.sort_by(|left, right| left.id.cmp(&right.id));

    let mut constraints = state.records.into_values().collect::<Vec<_>>();
    constraints.sort_by(|left, right| left.id.cmp(&right.id));

    let mut requirements = state.requirements.into_values().collect::<Vec<_>>();
    requirements.sort_by(|left, right| left.id.cmp(&right.id));

    ConstraintSolveResultDto {
        variables,
        constraints,
        requirements,
        diagnostics: state.diagnostics,
        explanations: state.explanations,
    }
}

fn visible_variables(
    result: &ConstraintSolveResultDto,
    focus: Option<&str>,
    radius: usize,
) -> BTreeSet<String> {
    let Some(focus) = focus else {
        return BTreeSet::new();
    };
    let mut visible = BTreeSet::from([focus.to_string()]);
    for _ in 0..radius {
        let before = visible.len();
        for constraint in &result.constraints {
            if constraint.variables.iter().any(|var| visible.contains(var)) {
                visible.extend(constraint.variables.iter().cloned());
            }
        }
        for requirement in &result.requirements {
            if requirement
                .variables
                .iter()
                .any(|var| visible.contains(var))
            {
                visible.extend(requirement.variables.iter().cloned());
            }
        }
        if visible.len() == before {
            break;
        }
    }
    visible
}

fn graph_edges(
    constraints: &[ConstraintRecordDto],
    requirements: &[RequirementCheckDto],
) -> Vec<ConstraintGraphEdgeDto> {
    let mut edges = Vec::new();
    for constraint in constraints {
        for variable in &constraint.variables {
            edges.push(ConstraintGraphEdgeDto {
                id: format!("{}--{}", variable, constraint.id),
                source: variable.clone(),
                target: constraint.id.clone(),
                relation: "participates".to_string(),
                status: format!("{:?}", constraint.status).to_ascii_lowercase(),
            });
        }
    }
    for requirement in requirements {
        for variable in &requirement.variables {
            edges.push(ConstraintGraphEdgeDto {
                id: format!("{}--{}", variable, requirement.id),
                source: variable.clone(),
                target: requirement.id.clone(),
                relation: "checks".to_string(),
                status: format!("{:?}", requirement.status).to_ascii_lowercase(),
            });
        }
    }
    edges
}

fn render_expression_ir(value: &Value) -> String {
    let Some(object) = value.as_object() else {
        return value.to_string();
    };
    match object
        .get("kind")
        .or_else(|| object.get("type"))
        .and_then(Value::as_str)
    {
        Some("literal") => object
            .get("value")
            .map(render_literal_value)
            .unwrap_or_else(|| value.to_string()),
        Some("path") => render_expression_ir_path(value),
        Some("unary") => {
            let op = object.get("op").and_then(Value::as_str).unwrap_or("-");
            let operand = object
                .get("operand")
                .or_else(|| object.get("expr"))
                .map(render_expression_ir)
                .unwrap_or_else(|| "?".to_string());
            format!("{op}{operand}")
        }
        Some("binary") => {
            let op = render_expression_ir_binary_op(object.get("op").and_then(Value::as_str));
            let left = object
                .get("left")
                .map(render_expression_ir)
                .unwrap_or_else(|| "?".to_string());
            let right = object
                .get("right")
                .map(render_expression_ir)
                .unwrap_or_else(|| "?".to_string());
            format!("({left} {op} {right})")
        }
        Some("call") => {
            let function = object
                .get("function")
                .and_then(Value::as_str)
                .or_else(|| object.get("name").and_then(Value::as_str))
                .unwrap_or("call");
            let args = object
                .get("args")
                .or_else(|| object.get("arguments"))
                .and_then(Value::as_array)
                .map(|args| {
                    args.iter()
                        .map(render_expression_ir)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            format!("{function}({args})")
        }
        _ => value.to_string(),
    }
}

fn render_literal_value(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn render_expression_ir_binary_op(op: Option<&str>) -> &'static str {
    match op {
        Some("add" | "+") => "+",
        Some("subtract" | "sub" | "-") => "-",
        Some("multiply" | "mul" | "*") => "*",
        Some("divide" | "div" | "/") => "/",
        Some("equal" | "eq" | "==" | "=") => "==",
        Some("less_equal" | "le" | "<=") => "<=",
        Some("greater_equal" | "ge" | ">=") => ">=",
        Some("less" | "lt" | "<") => "<",
        Some("greater" | "gt" | ">") => ">",
        _ => "?",
    }
}

fn render_expression_ir_path(value: &Value) -> String {
    let Some(object) = value.as_object() else {
        return value.to_string();
    };
    if let Some(path) = object.get("path").and_then(Value::as_str) {
        return path.to_string();
    }
    if let Some(name) = object.get("name").and_then(Value::as_str) {
        return name.to_string();
    }
    object
        .get("segments")
        .and_then(Value::as_array)
        .map(|segments| {
            segments
                .iter()
                .filter_map(|segment| {
                    segment
                        .get("name")
                        .and_then(Value::as_str)
                        .or_else(|| segment.as_str())
                })
                .collect::<Vec<_>>()
                .join(".")
        })
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| value.to_string())
}

fn collect_expression_ir_variables(value: &Value, output: &mut BTreeSet<String>) {
    let Some(object) = value.as_object() else {
        return;
    };
    match object
        .get("kind")
        .or_else(|| object.get("type"))
        .and_then(Value::as_str)
    {
        Some("path") => {
            output.insert(render_expression_ir_path(value));
        }
        Some("unary") => {
            if let Some(operand) = object.get("operand").or_else(|| object.get("expr")) {
                collect_expression_ir_variables(operand, output);
            }
        }
        Some("binary") => {
            if let Some(left) = object.get("left") {
                collect_expression_ir_variables(left, output);
            }
            if let Some(right) = object.get("right") {
                collect_expression_ir_variables(right, output);
            }
        }
        Some("call") => {
            if let Some(args) = object
                .get("args")
                .or_else(|| object.get("arguments"))
                .and_then(Value::as_array)
            {
                for arg in args {
                    collect_expression_ir_variables(arg, output);
                }
            }
        }
        _ => {}
    }
}

fn eval_number(expr: &Expr, values: &HashMap<String, f64>) -> Option<f64> {
    match expr {
        Expr::Number(value) => Some(*value),
        Expr::Var(id) => values.get(id).copied(),
        Expr::UnaryMinus(expr) => eval_number(expr, values).map(|value| -value),
        Expr::Binary { op, left, right } => {
            let left = eval_number(left, values)?;
            let right = eval_number(right, values)?;
            match op {
                BinaryOp::Add => Some(left + right),
                BinaryOp::Sub => Some(left - right),
                BinaryOp::Mul => Some(left * right),
                BinaryOp::Div if !nearly_equal(right, 0.0) => Some(left / right),
                BinaryOp::Div => None,
            }
        }
        Expr::Compare { .. } => None,
    }
}

fn solve_for(
    expr: &Expr,
    target: &str,
    desired: f64,
    values: &HashMap<String, f64>,
) -> Option<f64> {
    match expr {
        Expr::Var(id) if id == target => Some(desired),
        Expr::UnaryMinus(inner) if contains_var(inner, target) => {
            solve_for(inner, target, -desired, values)
        }
        Expr::Binary { op, left, right } => {
            let left_has = contains_var(left, target);
            let right_has = contains_var(right, target);
            match (op, left_has, right_has) {
                (BinaryOp::Add, true, false) => {
                    solve_for(left, target, desired - eval_number(right, values)?, values)
                }
                (BinaryOp::Add, false, true) => {
                    solve_for(right, target, desired - eval_number(left, values)?, values)
                }
                (BinaryOp::Sub, true, false) => {
                    solve_for(left, target, desired + eval_number(right, values)?, values)
                }
                (BinaryOp::Sub, false, true) => {
                    solve_for(right, target, eval_number(left, values)? - desired, values)
                }
                (BinaryOp::Mul, true, false) => {
                    let known = eval_number(right, values)?;
                    (!nearly_equal(known, 0.0)).then(|| desired / known)
                }
                (BinaryOp::Mul, false, true) => {
                    let known = eval_number(left, values)?;
                    (!nearly_equal(known, 0.0)).then(|| desired / known)
                }
                (BinaryOp::Div, true, false) => {
                    solve_for(left, target, desired * eval_number(right, values)?, values)
                }
                (BinaryOp::Div, false, true) if !nearly_equal(desired, 0.0) => {
                    solve_for(right, target, eval_number(left, values)? / desired, values)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn variables(expr: &Expr) -> Vec<String> {
    let mut output = BTreeSet::new();
    collect_variables(expr, &mut output);
    output.into_iter().collect()
}

fn collect_variables(expr: &Expr, output: &mut BTreeSet<String>) {
    match expr {
        Expr::Var(id) => {
            output.insert(id.clone());
        }
        Expr::UnaryMinus(expr) => collect_variables(expr, output),
        Expr::Binary { left, right, .. } | Expr::Compare { left, right, .. } => {
            collect_variables(left, output);
            collect_variables(right, output);
        }
        Expr::Number(_) => {}
    }
}

fn contains_var(expr: &Expr, target: &str) -> bool {
    match expr {
        Expr::Var(id) => id == target,
        Expr::UnaryMinus(expr) => contains_var(expr, target),
        Expr::Binary { left, right, .. } | Expr::Compare { left, right, .. } => {
            contains_var(left, target) || contains_var(right, target)
        }
        Expr::Number(_) => false,
    }
}

fn compare_numbers(op: CompareOp, left: f64, right: f64) -> bool {
    match op {
        CompareOp::Eq => nearly_equal(left, right),
        CompareOp::Le => left <= right || nearly_equal(left, right),
        CompareOp::Ge => left >= right || nearly_equal(left, right),
        CompareOp::Lt => left < right,
        CompareOp::Gt => left > right,
    }
}

fn parse_expression(input: &str) -> Result<Expr, String> {
    let tokens = tokenize(input)?;
    let mut parser = Parser { tokens, index: 0 };
    let expr = parser.parse_comparison()?;
    if parser.index != parser.tokens.len() {
        return Err("unexpected trailing input".to_string());
    }
    Ok(expr)
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let mut chars = input.char_indices().peekable();
    let mut tokens = Vec::new();
    while let Some((start, ch)) = chars.peek().copied() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }
        if ch.is_ascii_digit() || ch == '.' {
            let mut end = start;
            while let Some((idx, next)) = chars.peek().copied() {
                if next.is_ascii_digit() || next == '.' {
                    end = idx + next.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            let raw = &input[start..end];
            tokens.push(Token::Number(
                raw.parse::<f64>()
                    .map_err(|_| format!("invalid number `{raw}`"))?,
            ));
            continue;
        }
        if is_ident_start(ch) {
            let mut end = start;
            while let Some((idx, next)) = chars.peek().copied() {
                if is_ident_continue(next) {
                    end = idx + next.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            tokens.push(Token::Ident(input[start..end].to_string()));
            continue;
        }
        chars.next();
        match ch {
            '+' => tokens.push(Token::Plus),
            '-' => tokens.push(Token::Minus),
            '*' => tokens.push(Token::Star),
            '/' => tokens.push(Token::Slash),
            '(' => tokens.push(Token::LParen),
            ')' => tokens.push(Token::RParen),
            '=' if chars.peek().is_some_and(|(_, next)| *next == '=') => {
                chars.next();
                tokens.push(Token::EqEq);
            }
            '<' if chars.peek().is_some_and(|(_, next)| *next == '=') => {
                chars.next();
                tokens.push(Token::Le);
            }
            '>' if chars.peek().is_some_and(|(_, next)| *next == '=') => {
                chars.next();
                tokens.push(Token::Ge);
            }
            '<' => tokens.push(Token::Lt),
            '>' => tokens.push(Token::Gt),
            '=' => tokens.push(Token::EqEq),
            _ => return Err(format!("unexpected character `{ch}`")),
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    index: usize,
}

impl Parser {
    fn parse_comparison(&mut self) -> Result<Expr, String> {
        let left = self.parse_additive()?;
        let Some(op) = self.parse_compare_op() else {
            return Ok(left);
        };
        let right = self.parse_additive()?;
        Ok(Expr::Compare {
            op,
            left: Box::new(left),
            right: Box::new(right),
        })
    }

    fn parse_additive(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_multiplicative()?;
        while let Some(op) = self.parse_additive_op() {
            let right = self.parse_multiplicative()?;
            expr = Expr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_unary()?;
        while let Some(op) = self.parse_multiplicative_op() {
            let right = self.parse_unary()?;
            expr = Expr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        if matches!(self.peek(), Some(Token::Minus)) {
            self.index += 1;
            return Ok(Expr::UnaryMinus(Box::new(self.parse_unary()?)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Token::Number(value)) => Ok(Expr::Number(value)),
            Some(Token::Ident(id)) => Ok(Expr::Var(id)),
            Some(Token::LParen) => {
                let expr = self.parse_comparison()?;
                match self.next() {
                    Some(Token::RParen) => Ok(expr),
                    _ => Err("expected `)`".to_string()),
                }
            }
            _ => Err("expected number, variable, or parenthesized expression".to_string()),
        }
    }

    fn parse_compare_op(&mut self) -> Option<CompareOp> {
        let op = match self.peek()? {
            Token::EqEq => CompareOp::Eq,
            Token::Le => CompareOp::Le,
            Token::Ge => CompareOp::Ge,
            Token::Lt => CompareOp::Lt,
            Token::Gt => CompareOp::Gt,
            _ => return None,
        };
        self.index += 1;
        Some(op)
    }

    fn parse_additive_op(&mut self) -> Option<BinaryOp> {
        let op = match self.peek()? {
            Token::Plus => BinaryOp::Add,
            Token::Minus => BinaryOp::Sub,
            _ => return None,
        };
        self.index += 1;
        Some(op)
    }

    fn parse_multiplicative_op(&mut self) -> Option<BinaryOp> {
        let op = match self.peek()? {
            Token::Star => BinaryOp::Mul,
            Token::Slash => BinaryOp::Div,
            _ => return None,
        };
        self.index += 1;
        Some(op)
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.index)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.index).cloned();
        self.index += usize::from(token.is_some());
        token
    }
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':')
}

fn json_number(value: f64) -> Option<Value> {
    serde_json::Number::from_f64(value).map(Value::Number)
}

fn nearly_equal(left: f64, right: f64) -> bool {
    (left - right).abs() <= EPSILON
}

fn last_segment(value: &str) -> &str {
    value
        .rsplit([':', '.'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(value)
}

fn default_radius() -> usize {
    2
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::graph::Graph;
    use crate::ir::{KirDocument, KirElement};

    #[test]
    fn propagates_simple_acausal_equation() {
        let graph = test_graph();
        let mut context = ExecutionContext::default();
        context
            .values
            .insert(("scenario".into(), "totalMass".into()), json!(120.0));
        context
            .values
            .insert(("scenario".into(), "dryMass".into()), json!(90.0));

        let result = solve_constraints(&graph, &context).unwrap();

        let fuel = result
            .variables
            .iter()
            .find(|var| var.id == "fuelMass")
            .unwrap();
        assert_eq!(fuel.status, ConstraintVariableStatusDto::Inferred);
        assert_eq!(fuel.value, Some(json!(30.0)));
    }

    #[test]
    fn checks_requirement_from_solved_value() {
        let graph = test_graph();
        let mut context = ExecutionContext::default();
        context
            .values
            .insert(("scenario".into(), "totalMass".into()), json!(120.0));
        context
            .values
            .insert(("scenario".into(), "dryMass".into()), json!(90.0));
        context
            .values
            .insert(("scenario".into(), "maxMass".into()), json!(110.0));

        let result = solve_constraints(&graph, &context).unwrap();

        let requirement = result
            .requirements
            .iter()
            .find(|requirement| requirement.id == "req.maxMass")
            .unwrap();
        assert_eq!(requirement.status, RequirementStatusDto::Violated);
        assert_eq!(requirement.margin, Some(-10.0));
    }

    #[test]
    fn propagates_chained_reverse_solve_before_requirement_check() {
        let graph = test_graph();
        let mut context = ExecutionContext::default();
        context
            .values
            .insert(("scenario".into(), "grossWeight".into()), json!(1177.2));
        context
            .values
            .insert(("scenario".into(), "gravity".into()), json!(9.81));
        context
            .values
            .insert(("scenario".into(), "dryMass".into()), json!(90.0));
        context
            .values
            .insert(("scenario".into(), "maxMass".into()), json!(125.0));

        let result = solve_constraints(&graph, &context).unwrap();

        let total_mass = result
            .variables
            .iter()
            .find(|var| var.id == "totalMass")
            .unwrap();
        assert_eq!(total_mass.status, ConstraintVariableStatusDto::Inferred);
        assert_eq!(total_mass.value, Some(json!(120.0)));

        let fuel = result
            .variables
            .iter()
            .find(|var| var.id == "fuelMass")
            .unwrap();
        assert_eq!(fuel.status, ConstraintVariableStatusDto::Inferred);
        assert_eq!(fuel.value, Some(json!(30.0)));

        let requirement = result
            .requirements
            .iter()
            .find(|requirement| requirement.id == "req.maxMass")
            .unwrap();
        assert_eq!(requirement.status, RequirementStatusDto::Satisfied);
        assert_eq!(requirement.margin, Some(5.0));
    }

    #[test]
    fn renders_focused_constraint_graph_with_requirement_edges() {
        let graph = test_graph();
        let request = ConstraintGraphRequestDto {
            focus: Some("totalMass".into()),
            radius: 1,
            scenario: Some("mission-a".into()),
            context_values: BTreeMap::from([(
                "scenario".into(),
                BTreeMap::from([
                    ("dryMass".into(), json!(90.0)),
                    ("fuelMass".into(), json!(30.0)),
                    ("maxMass".into(), json!(125.0)),
                ]),
            )]),
        };

        let view = render_constraint_graph(&graph, request).unwrap();

        assert_eq!(view.focus.as_deref(), Some("totalMass"));
        assert_eq!(view.scenario.as_deref(), Some("mission-a"));
        assert!(view.variables.iter().any(|var| var.id == "totalMass"));
        assert!(
            view.constraints
                .iter()
                .any(|constraint| constraint.id == "constraint.totalMass")
        );
        assert!(
            view.requirements
                .iter()
                .any(|requirement| requirement.id == "req.maxMass")
        );
        assert!(
            view.edges
                .iter()
                .any(|edge| edge.source == "totalMass" && edge.target == "req.maxMass")
        );
    }

    #[test]
    fn renders_expression_ir_attributes_as_constraints() {
        let graph = Graph::from_document(KirDocument {
            metadata: BTreeMap::new(),
            elements: vec![KirElement {
                id: "MassRollup::compositeThing.totalMass".into(),
                kind: "sysml.AttributeUsage".into(),
                layer: 2,
                properties: BTreeMap::from([
                    ("name".into(), json!("totalMass")),
                    (
                        "qualified_name".into(),
                        json!("MassRollup::compositeThing.totalMass"),
                    ),
                    (
                        "expression_ir".into(),
                        json!({
                            "kind": "binary",
                            "op": "add",
                            "left": {
                                "kind": "path",
                                "segments": [{ "name": "mass" }]
                            },
                            "right": {
                                "kind": "call",
                                "function": "sum",
                                "args": [{
                                    "kind": "path",
                                    "segments": [
                                        { "name": "subcomponents" },
                                        { "name": "totalMass" }
                                    ]
                                }]
                            }
                        }),
                    ),
                ]),
            }],
        })
        .unwrap();

        let view = render_constraint_graph(
            &graph,
            ConstraintGraphRequestDto {
                focus: None,
                radius: 2,
                scenario: None,
                context_values: BTreeMap::new(),
            },
        )
        .unwrap();

        let constraint = view
            .constraints
            .iter()
            .find(|constraint| constraint.id == "MassRollup::compositeThing.totalMass")
            .unwrap();
        assert_eq!(constraint.status, ConstraintStatusDto::Unsupported);
        assert!(
            constraint
                .expression
                .contains("sum(subcomponents.totalMass)")
        );
        assert!(
            constraint
                .variables
                .contains(&"MassRollup::compositeThing.totalMass".to_string())
        );
        assert!(constraint.variables.contains(&"mass".to_string()));
        assert!(
            constraint
                .variables
                .contains(&"subcomponents.totalMass".to_string())
        );
        assert!(
            view.diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind == "constraint.unsupported")
        );
    }

    fn test_graph() -> Graph {
        Graph::from_document(KirDocument {
            metadata: BTreeMap::new(),
            elements: vec![
                KirElement {
                    id: "constraint.totalMass".into(),
                    kind: "sysml.ConstraintUsage".into(),
                    layer: 2,
                    properties: BTreeMap::from([(
                        "equation".into(),
                        Value::String("totalMass == dryMass + fuelMass".into()),
                    )]),
                },
                KirElement {
                    id: "constraint.grossWeight".into(),
                    kind: "sysml.ConstraintUsage".into(),
                    layer: 2,
                    properties: BTreeMap::from([(
                        "equation".into(),
                        Value::String("grossWeight == totalMass * gravity".into()),
                    )]),
                },
                KirElement {
                    id: "req.maxMass".into(),
                    kind: "sysml.RequirementUsage".into(),
                    layer: 2,
                    properties: BTreeMap::from([(
                        "check".into(),
                        Value::String("totalMass <= maxMass".into()),
                    )]),
                },
            ],
        })
        .unwrap()
    }
}
