use std::collections::BTreeMap;

use mercurio_core::runtime::Runtime;
use mercurio_core::views::{RequirementSourceDto, RequirementTableRowDto, requirements_table_view};
use mercurio_reasoner_api::{
    CapabilityDescriptor, CapabilityKind, EvidenceGraph, EvidenceNode, EvidenceNodeKind,
    FindingSeverity, REASONING_API_VERSION, ReasoningArtifact, ReasoningFinding, ReasoningReport,
    ReasoningStatus, SemanticContextRef, SemanticElementRef, SourceSpanRef,
};
use serde_json::{Value, json};

pub const REQUIREMENT_COVERAGE_CAPABILITY_ID: &str = "mercurio.requirement.coverage";

pub fn builtin_reasoning_capabilities() -> Vec<CapabilityDescriptor> {
    vec![requirement_coverage_capability_descriptor()]
}

pub fn requirement_coverage_capability_descriptor() -> CapabilityDescriptor {
    CapabilityDescriptor {
        id: REQUIREMENT_COVERAGE_CAPABILITY_ID.to_string(),
        kind: CapabilityKind::RequirementCoverage,
        name: "Requirement Coverage".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        api_version: REASONING_API_VERSION.to_string(),
        deterministic: true,
        input_artifact_kinds: vec![
            "runtime_artifact".to_string(),
            "derived_indexes".to_string(),
        ],
        output_artifact_kinds: vec![
            "finding".to_string(),
            "evidence_graph".to_string(),
            "requirement_coverage_summary".to_string(),
        ],
    }
}

pub fn analyze_requirement_coverage(
    runtime: &Runtime,
    context: SemanticContextRef,
    request_id: impl Into<String>,
) -> ReasoningReport {
    let view = requirements_table_view(runtime.graph());
    let mut findings = Vec::new();
    let mut evidence_nodes = Vec::new();

    for requirement in &view.rows {
        evidence_nodes.push(requirement_evidence_node(requirement));

        if requirement.satisfied_by.is_empty() {
            findings.push(missing_trace_finding(
                requirement,
                "satisfy",
                "Requirement has no satisfaction evidence",
                "No satisfy relationship reaches this requirement.",
                FindingSeverity::Warning,
            ));
        }

        if requirement.verified_by.is_empty() {
            findings.push(missing_trace_finding(
                requirement,
                "verify",
                "Requirement has no verification evidence",
                "No verify relationship reaches this requirement.",
                FindingSeverity::Error,
            ));
        }
    }

    for warning in &view.warnings {
        findings.push(ReasoningFinding {
            id: "requirement.coverage.no_requirements".to_string(),
            title: "No requirements found".to_string(),
            severity: FindingSeverity::Warning,
            message: warning.clone(),
            elements: Vec::new(),
            source_spans: Vec::new(),
            evidence_ids: Vec::new(),
            properties: BTreeMap::new(),
        });
    }

    let status = if findings.iter().any(|finding| {
        matches!(
            finding.severity,
            FindingSeverity::Error | FindingSeverity::Critical
        )
    }) {
        ReasoningStatus::Failed
    } else if findings.is_empty() {
        ReasoningStatus::Passed
    } else {
        ReasoningStatus::Inconclusive
    };

    let summary_payload = json!({
        "requirementCount": view.rows.len(),
        "satisfiedCount": view.rows.iter().filter(|row| !row.satisfied_by.is_empty()).count(),
        "verifiedCount": view.rows.iter().filter(|row| !row.verified_by.is_empty()).count(),
        "findingCount": findings.len(),
    });

    ReasoningReport {
        request_id: request_id.into(),
        capability: requirement_coverage_capability_descriptor(),
        context,
        status,
        findings,
        artifacts: vec![ReasoningArtifact {
            id: "artifact.requirement_coverage.summary".to_string(),
            kind: "requirement_coverage_summary".to_string(),
            schema: "mercurio.requirement_coverage.summary.v1".to_string(),
            digest: summary_digest(&summary_payload),
            element_refs: view.rows.iter().map(requirement_element_ref).collect(),
            payload: summary_payload,
        }],
        evidence: EvidenceGraph {
            nodes: evidence_nodes,
            edges: Vec::new(),
        },
    }
}

fn missing_trace_finding(
    requirement: &RequirementTableRowDto,
    trace_kind: &str,
    title: &str,
    message: &str,
    severity: FindingSeverity,
) -> ReasoningFinding {
    ReasoningFinding {
        id: format!(
            "finding.requirement.{trace_kind}.missing.{}",
            requirement.id
        ),
        title: title.to_string(),
        severity,
        message: message.to_string(),
        elements: vec![requirement_element_ref(requirement)],
        source_spans: source_spans(requirement),
        evidence_ids: vec![requirement_evidence_id(requirement)],
        properties: BTreeMap::from([
            (
                "requirementId".to_string(),
                Value::String(requirement.id.clone()),
            ),
            (
                "traceKind".to_string(),
                Value::String(trace_kind.to_string()),
            ),
        ]),
    }
}

fn requirement_evidence_node(requirement: &RequirementTableRowDto) -> EvidenceNode {
    EvidenceNode {
        id: requirement_evidence_id(requirement),
        kind: EvidenceNodeKind::KirElement,
        label: requirement
            .name
            .clone()
            .unwrap_or_else(|| requirement.id.clone()),
        element_refs: vec![requirement_element_ref(requirement)],
        source_spans: source_spans(requirement),
        properties: BTreeMap::from([
            (
                "satisfiedBy".to_string(),
                Value::Array(
                    requirement
                        .satisfied_by
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            ),
            (
                "verifiedBy".to_string(),
                Value::Array(
                    requirement
                        .verified_by
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            ),
        ]),
    }
}

fn requirement_element_ref(requirement: &RequirementTableRowDto) -> SemanticElementRef {
    SemanticElementRef {
        element_id: requirement.id.clone(),
        qualified_name: None,
        label: requirement.name.clone(),
    }
}

fn requirement_evidence_id(requirement: &RequirementTableRowDto) -> String {
    format!("evidence.requirement.{}", requirement.id)
}

fn source_spans(requirement: &RequirementTableRowDto) -> Vec<SourceSpanRef> {
    requirement
        .source
        .as_ref()
        .and_then(source_span_ref)
        .into_iter()
        .collect()
}

fn source_span_ref(source: &RequirementSourceDto) -> Option<SourceSpanRef> {
    Some(SourceSpanRef {
        file: source.file.clone()?,
        start_line: u32::try_from(source.start_line?).ok()?,
        start_col: 1,
        end_line: u32::try_from(source.end_line?).ok()?,
        end_col: 1,
    })
}

fn summary_digest(value: &Value) -> String {
    let encoded = serde_json::to_string(value).unwrap_or_default();
    let mut hash = 0xcbf29ce484222325u64;
    for byte in encoded.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64_{hash:016x}")
}

#[cfg(test)]
mod tests {
    use mercurio_core::{KirDocument, Runtime, repo_path};
    use mercurio_reasoner_api::{SemanticArtifactRef, SemanticContextKind, SemanticContextRef};

    use super::*;

    #[test]
    fn requirement_coverage_reports_missing_verification() {
        let document =
            KirDocument::from_path(&repo_path("examples/requirements_table_model.json")).unwrap();
        let runtime = Runtime::from_document(document).unwrap();
        let report = analyze_requirement_coverage(&runtime, test_context(), "req-coverage-test");

        assert_eq!(report.status, ReasoningStatus::Failed);
        assert!(report.findings.iter().any(|finding| {
            finding
                .id
                .contains("verify.missing.req.VehicleSafety.DriverAlert")
        }));
        assert_eq!(report.evidence.nodes.len(), 3);
        assert_eq!(
            report.artifacts[0].payload["verifiedCount"],
            serde_json::Value::from(2)
        );
    }

    fn test_context() -> SemanticContextRef {
        SemanticContextRef {
            context_id: "ctx.test".to_string(),
            kind: SemanticContextKind::Accepted,
            artifact: SemanticArtifactRef {
                artifact_key: "artifact.test".to_string(),
                kir_schema_version: "0.1".to_string(),
                source_authority: Some("test_fixture".to_string()),
                source_revision: None,
            },
        }
    }
}
