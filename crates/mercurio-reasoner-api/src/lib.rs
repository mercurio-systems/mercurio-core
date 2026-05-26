use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const REASONING_API_VERSION: &str = "0.1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticArtifactRef {
    pub artifact_key: String,
    pub kir_schema_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_authority: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticContextKind {
    Accepted,
    DraftOverlay { overlay_digest: String },
    Proposal { proposal_id: String },
    Compare { base_artifact_key: String },
    HistoricalSnapshot { snapshot_id: String },
    ScenarioRun { scenario_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticContextRef {
    pub context_id: String,
    pub kind: SemanticContextKind,
    pub artifact: SemanticArtifactRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticElementRef {
    pub element_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualified_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpanRef {
    pub file: String,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    StaticAnalysis,
    RequirementCoverage,
    ConstraintSolver,
    ContractAnalysis,
    Simulation,
    Verification,
    TradeStudy,
    CustomReasoning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDescriptor {
    pub id: String,
    pub kind: CapabilityKind,
    pub name: String,
    pub version: String,
    pub api_version: String,
    pub deterministic: bool,
    #[serde(default)]
    pub input_artifact_kinds: Vec<String>,
    #[serde(default)]
    pub output_artifact_kinds: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningRequest {
    pub request_id: String,
    pub capability_id: String,
    pub context: SemanticContextRef,
    #[serde(default)]
    pub focus: Vec<SemanticElementRef>,
    #[serde(default)]
    pub parameters: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReasoningReport {
    pub request_id: String,
    pub capability: CapabilityDescriptor,
    pub context: SemanticContextRef,
    pub status: ReasoningStatus,
    #[serde(default)]
    pub findings: Vec<ReasoningFinding>,
    #[serde(default)]
    pub artifacts: Vec<ReasoningArtifact>,
    #[serde(default)]
    pub evidence: EvidenceGraph,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningStatus {
    Passed,
    Failed,
    Inconclusive,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReasoningFinding {
    pub id: String,
    pub title: String,
    pub severity: FindingSeverity,
    pub message: String,
    #[serde(default)]
    pub elements: Vec<SemanticElementRef>,
    #[serde(default)]
    pub source_spans: Vec<SourceSpanRef>,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
    #[serde(default)]
    pub properties: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReasoningArtifact {
    pub id: String,
    pub kind: String,
    pub schema: String,
    pub digest: String,
    #[serde(default)]
    pub element_refs: Vec<SemanticElementRef>,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EvidenceGraph {
    #[serde(default)]
    pub nodes: Vec<EvidenceNode>,
    #[serde(default)]
    pub edges: Vec<EvidenceEdge>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceNode {
    pub id: String,
    pub kind: EvidenceNodeKind,
    pub label: String,
    #[serde(default)]
    pub element_refs: Vec<SemanticElementRef>,
    #[serde(default)]
    pub source_spans: Vec<SourceSpanRef>,
    #[serde(default)]
    pub properties: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceNodeKind {
    KirElement,
    SourceSpan,
    Fact,
    Rule,
    AnalysisRun,
    Plugin,
    Artifact,
    HumanDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceEdge {
    pub source_id: String,
    pub target_id: String,
    pub relation: EvidenceRelation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceRelation {
    Supports,
    DerivedFrom,
    ProducedBy,
    Consumed,
    Affects,
    Explains,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

pub trait ReasoningCapability {
    fn descriptor(&self) -> CapabilityDescriptor;

    fn analyze(&self, request: ReasoningRequest) -> Result<ReasoningReport, ReasoningError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_report_round_trips_as_json() {
        let context = SemanticContextRef {
            context_id: "ctx.accepted".to_string(),
            kind: SemanticContextKind::Accepted,
            artifact: SemanticArtifactRef {
                artifact_key: "sha256-test".to_string(),
                kir_schema_version: "0.1".to_string(),
                source_authority: Some("local_git".to_string()),
                source_revision: Some("abc123".to_string()),
            },
        };
        let report = ReasoningReport {
            request_id: "req-1".to_string(),
            capability: CapabilityDescriptor {
                id: "requirement.coverage".to_string(),
                kind: CapabilityKind::RequirementCoverage,
                name: "Requirement Coverage".to_string(),
                version: "0.1.0".to_string(),
                api_version: REASONING_API_VERSION.to_string(),
                deterministic: true,
                input_artifact_kinds: vec!["kir".to_string()],
                output_artifact_kinds: vec!["finding".to_string()],
            },
            context: context.clone(),
            status: ReasoningStatus::Failed,
            findings: vec![ReasoningFinding {
                id: "finding.req.unverified".to_string(),
                title: "Requirement is unverified".to_string(),
                severity: FindingSeverity::Error,
                message: "No verification evidence reaches this requirement.".to_string(),
                elements: vec![SemanticElementRef {
                    element_id: "req.demo.startup".to_string(),
                    qualified_name: Some("Demo::startup".to_string()),
                    label: Some("startup".to_string()),
                }],
                source_spans: Vec::new(),
                evidence_ids: vec!["fact.req.demo.startup".to_string()],
                properties: BTreeMap::new(),
            }],
            artifacts: Vec::new(),
            evidence: EvidenceGraph {
                nodes: vec![EvidenceNode {
                    id: "fact.req.demo.startup".to_string(),
                    kind: EvidenceNodeKind::Fact,
                    label: "requirement(req.demo.startup)".to_string(),
                    element_refs: Vec::new(),
                    source_spans: Vec::new(),
                    properties: BTreeMap::new(),
                }],
                edges: Vec::new(),
            },
        };

        let encoded = serde_json::to_string(&report).expect("report serializes");
        let decoded: ReasoningReport = serde_json::from_str(&encoded).expect("report deserializes");

        assert_eq!(decoded.context, context);
        assert_eq!(decoded.findings[0].evidence_ids, ["fact.req.demo.startup"]);
    }
}
