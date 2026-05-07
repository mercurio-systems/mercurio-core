use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::datalog::{Atom, DatalogError, Evaluation, Fact, RulePack, Term, evaluate, extract_graph_facts};
use crate::frontend::ast::{Declaration, SysmlModule};
use crate::graph::Graph;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssessmentSpec {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assertions: Vec<AssessmentAssertion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssessmentAssertion {
    pub id: String,
    pub description: String,
    pub query: AssessmentQuery,
    pub expect: AssessmentExpectation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssessmentQuery {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub find: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub where_atoms: Vec<Atom>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum AssessmentExpectation {
    Exists,
    CountEq { value: usize },
    CountAtLeast { value: usize },
    ContainsBinding { variable: String, value: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssessmentReport {
    pub id: String,
    pub title: String,
    pub status: AssessmentStatus,
    pub assertions: Vec<AssessmentAssertionReport>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AssessmentStatus {
    Pass,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssessmentAssertionReport {
    pub id: String,
    pub description: String,
    pub status: AssessmentStatus,
    pub binding_count: usize,
    pub bindings: Vec<BTreeMap<String, String>>,
    pub message: String,
}

#[derive(Debug)]
pub enum AssessmentError {
    Datalog(DatalogError),
}

impl fmt::Display for AssessmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Datalog(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for AssessmentError {}

impl From<DatalogError> for AssessmentError {
    fn from(value: DatalogError) -> Self {
        Self::Datalog(value)
    }
}

pub fn run_graph_assessment(
    graph: &Graph,
    rulepacks: &[RulePack],
    spec: &AssessmentSpec,
) -> Result<AssessmentReport, AssessmentError> {
    let mut facts = extract_graph_facts(graph);
    for pack in rulepacks {
        facts.extend(pack.facts.iter().cloned());
    }
    let rules = rulepacks
        .iter()
        .flat_map(|pack| pack.rules.iter().cloned())
        .collect::<Vec<_>>();
    let evaluation = evaluate(facts, &rules)?;
    run_evaluation_assessment(&evaluation, spec)
}

pub fn run_evaluation_assessment(
    evaluation: &Evaluation,
    spec: &AssessmentSpec,
) -> Result<AssessmentReport, AssessmentError> {
    let assertions = spec
        .assertions
        .iter()
        .map(|assertion| {
            let bindings = query_evaluation(evaluation, &assertion.query)?;
            Ok(report_assertion(assertion, bindings))
        })
        .collect::<Result<Vec<_>, AssessmentError>>()?;
    let status = if assertions
        .iter()
        .all(|assertion| assertion.status == AssessmentStatus::Pass)
    {
        AssessmentStatus::Pass
    } else {
        AssessmentStatus::Failed
    };

    Ok(AssessmentReport {
        id: spec.id.clone(),
        title: spec.title.clone(),
        status,
        assertions,
    })
}

pub fn query_evaluation(
    evaluation: &Evaluation,
    query: &AssessmentQuery,
) -> Result<Vec<BTreeMap<String, String>>, AssessmentError> {
    let mut bindings = vec![BTreeMap::<String, String>::new()];

    for atom in &query.where_atoms {
        let candidates = evaluation
            .facts()
            .iter()
            .filter(|fact| fact.predicate == atom.predicate)
            .collect::<Vec<_>>();
        let mut next = Vec::new();

        for binding in bindings {
            for fact in &candidates {
                if fact.terms.len() != atom.terms.len() {
                    return Err(DatalogError::ArityMismatch {
                        predicate: atom.predicate.clone(),
                        expected: atom.terms.len(),
                        actual: fact.terms.len(),
                    }
                    .into());
                }
                if let Some(next_binding) = unify_query_atom(atom, fact, &binding) {
                    next.push(project_binding(next_binding, &query.find));
                }
            }
        }

        bindings = dedupe_bindings(next);
        if bindings.is_empty() {
            break;
        }
    }

    Ok(dedupe_bindings(bindings))
}

pub fn sysml_module_assessment_facts(module: &SysmlModule) -> Vec<Fact> {
    let mut facts = Vec::new();
    collect_declaration_assessment_facts(&module.members, None, &mut facts);
    facts
}

fn collect_declaration_assessment_facts(
    declarations: &[Declaration],
    owner: Option<&str>,
    facts: &mut Vec<Fact>,
) {
    for declaration in declarations {
        match declaration {
            Declaration::Package(package) => {
                let name = package.name.as_colon_string();
                let id = owner
                    .map(|owner| format!("{owner}::{name}"))
                    .unwrap_or_else(|| name.clone());
                facts.push(Fact::new("package", [id.clone()]));
                facts.push(Fact::new("name", [id.clone(), name]));
                if owner.is_none() {
                    facts.push(Fact::new("top_level_package", [id.clone()]));
                }
                if let Some(owner) = owner {
                    facts.push(Fact::new("owns", [owner.to_string(), id.clone()]));
                }
                collect_declaration_assessment_facts(&package.members, Some(&id), facts);
            }
            Declaration::PartDefinition(definition) => {
                let id = scoped_id(owner, &definition.name);
                facts.push(Fact::new("part_definition", [id.clone()]));
                facts.push(Fact::new("name", [id, definition.name.clone()]));
            }
            Declaration::PartUsage(usage) => {
                let id = scoped_id(owner, &usage.name);
                facts.push(Fact::new("part_usage", [id.clone()]));
                facts.push(Fact::new("name", [id, usage.name.clone()]));
            }
            _ => {}
        }
    }
}

fn scoped_id(owner: Option<&str>, name: &str) -> String {
    owner
        .map(|owner| format!("{owner}::{name}"))
        .unwrap_or_else(|| name.to_string())
}

fn report_assertion(
    assertion: &AssessmentAssertion,
    bindings: Vec<BTreeMap<String, String>>,
) -> AssessmentAssertionReport {
    let binding_count = bindings.len();
    let passed = match &assertion.expect {
        AssessmentExpectation::Exists => binding_count > 0,
        AssessmentExpectation::CountEq { value } => binding_count == *value,
        AssessmentExpectation::CountAtLeast { value } => binding_count >= *value,
        AssessmentExpectation::ContainsBinding { variable, value } => bindings
            .iter()
            .any(|binding| binding.get(variable).is_some_and(|actual| actual == value)),
    };

    AssessmentAssertionReport {
        id: assertion.id.clone(),
        description: assertion.description.clone(),
        status: if passed {
            AssessmentStatus::Pass
        } else {
            AssessmentStatus::Failed
        },
        binding_count,
        bindings,
        message: assertion_message(&assertion.expect, binding_count, passed),
    }
}

fn assertion_message(expectation: &AssessmentExpectation, binding_count: usize, passed: bool) -> String {
    let prefix = if passed { "pass" } else { "failed" };
    match expectation {
        AssessmentExpectation::Exists => {
            format!("{prefix}: expected at least one binding; found {binding_count}")
        }
        AssessmentExpectation::CountEq { value } => {
            format!("{prefix}: expected {value} binding(s); found {binding_count}")
        }
        AssessmentExpectation::CountAtLeast { value } => {
            format!("{prefix}: expected at least {value} binding(s); found {binding_count}")
        }
        AssessmentExpectation::ContainsBinding { variable, value } => {
            format!("{prefix}: expected binding {variable}={value}; found {binding_count} binding(s)")
        }
    }
}

fn unify_query_atom(
    atom: &Atom,
    fact: &Fact,
    binding: &BTreeMap<String, String>,
) -> Option<BTreeMap<String, String>> {
    let mut next = binding.clone();
    for (term, value) in atom.terms.iter().zip(&fact.terms) {
        match term {
            Term::Const(expected) if expected != value => return None,
            Term::Const(_) => {}
            Term::Var(name) => {
                if let Some(existing) = next.get(name) {
                    if existing != value {
                        return None;
                    }
                } else {
                    next.insert(name.clone(), value.clone());
                }
            }
        }
    }
    Some(next)
}

fn project_binding(
    binding: BTreeMap<String, String>,
    find: &[String],
) -> BTreeMap<String, String> {
    if find.is_empty() {
        return binding;
    }
    find.iter()
        .filter_map(|variable| {
            binding
                .get(variable)
                .map(|value| (variable.clone(), value.clone()))
        })
        .collect()
}

fn dedupe_bindings(bindings: Vec<BTreeMap<String, String>>) -> Vec<BTreeMap<String, String>> {
    let mut deduped = Vec::new();
    for binding in bindings {
        if !deduped.iter().any(|existing| existing == &binding) {
            deduped.push(binding);
        }
    }
    deduped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::sysml::parse_sysml_recovering;

    #[test]
    fn assesses_package_count_and_name_from_sysml_module_facts() {
        let parse_report = parse_sysml_recovering(
            "package UavLibrary { part def BatteryPack; } package UavSystem { }",
        )
        .unwrap();
        let facts = sysml_module_assessment_facts(&parse_report.module);
        let evaluation = evaluate(facts, &[]).unwrap();
        let spec = AssessmentSpec {
            id: "training.packages.1_1".to_string(),
            title: "Package structure".to_string(),
            assertions: vec![
                AssessmentAssertion {
                    id: "two-packages".to_string(),
                    description: "Model declares two top-level packages".to_string(),
                    query: AssessmentQuery {
                        find: vec!["P".to_string()],
                        where_atoms: vec![Atom {
                            predicate: "top_level_package".to_string(),
                            terms: vec![Term::Var("P".to_string())],
                        }],
                    },
                    expect: AssessmentExpectation::CountEq { value: 2 },
                },
                AssessmentAssertion {
                    id: "has-uav-library".to_string(),
                    description: "One package is named UavLibrary".to_string(),
                    query: AssessmentQuery {
                        find: vec!["P".to_string()],
                        where_atoms: vec![
                            Atom {
                                predicate: "package".to_string(),
                                terms: vec![Term::Var("P".to_string())],
                            },
                            Atom {
                                predicate: "name".to_string(),
                                terms: vec![
                                    Term::Var("P".to_string()),
                                    Term::Const("UavLibrary".to_string()),
                                ],
                            },
                        ],
                    },
                    expect: AssessmentExpectation::Exists,
                },
            ],
        };

        let report = run_evaluation_assessment(&evaluation, &spec).unwrap();

        assert_eq!(report.status, AssessmentStatus::Pass);
        assert_eq!(report.assertions[0].binding_count, 2);
        assert_eq!(report.assertions[1].binding_count, 1);
    }
}
