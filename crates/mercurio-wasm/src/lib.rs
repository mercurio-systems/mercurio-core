use std::collections::BTreeMap;

use mercurio_core::frontend::ast::{Declaration, PartUsageDecl, SourceSpan};
use mercurio_core::frontend::sysml::parse_sysml_recovering;
use mercurio_core::{
    AssessmentSpec, AssessmentStatus, ExecutionContext, Fact, Graph, KirDocument,
    MetamodelAttributeRegistry, RulePack, Runtime, RuntimeAssessmentRequest, SourceLanguage,
    compile_kerml_text, compile_sysml_text_with_context_report, format_text, lint_text,
    load_default_rulepacks, parse_kerml, requirements_table_view, run_graph_assessment,
    run_runtime_assessment, sysml_module_assessment_facts,
};
use mercurio_views::{DiagramError, DiagramRenderRequestDto, list_diagram_kinds, render_diagram};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

const DEFAULT_STDLIB: &str =
    include_str!("../../../resources/stdlib-sources/sysml-2.0-pilot-0.57.0/stdlib.kir.json");

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

#[wasm_bindgen(js_name = version)]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[wasm_bindgen(js_name = compileSysml)]
pub fn compile_sysml(input: &str, options: JsValue) -> JsValue {
    json_response(|| {
        let options = CompileOptions::from_js(options)?;
        let stdlib = load_stdlib(options.stdlib)?;
        let report =
            compile_sysml_text_with_context_report(input, &options.source_name, &[], &stdlib);
        let value = json!({
            "status": semantic_status(report.status),
            "document": report.document,
        });
        Ok(Response {
            ok: report.document.is_some() && report.diagnostics.is_empty(),
            value: Some(value),
            diagnostics: serde_json::to_value(report.diagnostics)?,
            errors: Vec::new(),
            metadata: metadata([
                ("sourceName", json!(options.source_name)),
                ("language", json!("sysml")),
            ]),
        })
    })
}

#[wasm_bindgen(js_name = compileKerml)]
pub fn compile_kerml(input: &str, options: JsValue) -> JsValue {
    json_response(|| {
        let options = CompileOptions::from_js(options)?;
        let stdlib = load_stdlib(options.stdlib)?;
        match compile_kerml_text(input, &options.source_name, &stdlib) {
            Ok(document) => Ok(success(
                json!({ "status": "ok", "document": document }),
                [
                    ("sourceName", json!(options.source_name)),
                    ("language", json!("kerml")),
                ],
            )),
            Err(error) => Ok(error_response(
                "compile",
                error.to_string(),
                Some(serde_json::to_value(vec![error])?),
            )),
        }
    })
}

#[wasm_bindgen(js_name = lint)]
pub fn lint(input: &str, language: &str, options: JsValue) -> JsValue {
    json_response(|| {
        let options = CompileOptions::from_js(options)?;
        let language = parse_language(language)?;
        let stdlib = load_stdlib(options.stdlib)?;
        let report = lint_text(input, &options.source_name, language, &[], &stdlib);
        Ok(Response {
            ok: !report.has_errors(),
            value: Some(serde_json::to_value(report)?),
            diagnostics: json!([]),
            errors: Vec::new(),
            metadata: metadata([("sourceName", json!(options.source_name))]),
        })
    })
}

#[wasm_bindgen(js_name = formatText)]
pub fn format_source(input: &str, language: &str) -> JsValue {
    json_response(|| {
        let language = parse_language(language)?;
        let formatted = format_text(input, language)?;
        Ok(success(
            json!({ "text": formatted }),
            [("language", json!(language.as_str()))],
        ))
    })
}

#[wasm_bindgen(js_name = listDiagramKinds)]
pub fn wasm_list_diagram_kinds() -> JsValue {
    json_response(|| Ok(success(serde_json::to_value(list_diagram_kinds())?, [])))
}

#[wasm_bindgen(js_name = renderDiagram)]
pub fn wasm_render_diagram(document: JsValue, request: JsValue) -> JsValue {
    json_response(|| {
        let document: KirDocument = from_js(document)?;
        let request: DiagramRenderRequestDto = from_js(request)?;
        let graph = Graph::from_document(document)?;
        let registry = MetamodelAttributeRegistry::build(&graph);
        let view = render_diagram(&graph, &registry, request.spec)?;
        Ok(success(serde_json::to_value(view)?, []))
    })
}

#[wasm_bindgen(js_name = requirementsTable)]
pub fn wasm_requirements_table(document: JsValue) -> JsValue {
    json_response(|| {
        let document: KirDocument = from_js(document)?;
        let runtime = Runtime::from_document(document)?;
        Ok(success(
            serde_json::to_value(requirements_table_view(runtime.graph()))?,
            [],
        ))
    })
}

#[wasm_bindgen(js_name = queryRuntime)]
pub fn wasm_query_runtime(document: JsValue, query: JsValue) -> JsValue {
    json_response(|| {
        let document: KirDocument = from_js(document)?;
        let query: RuntimeQuery = from_js(query)?;
        let runtime = Runtime::from_document(document)?;
        Ok(success(run_runtime_query(&runtime, query)?, []))
    })
}

#[wasm_bindgen(js_name = runAssessment)]
pub fn wasm_run_assessment(document: JsValue, spec: JsValue) -> JsValue {
    json_response(|| {
        let document: KirDocument = from_js(document)?;
        let spec: AssessmentSpec = from_js(spec)?;
        let graph = Graph::from_document(document)?;
        let rulepacks = load_default_rulepacks()?;
        let report = run_graph_assessment(&graph, &rulepacks, &spec)?;
        Ok(success(serde_json::to_value(report)?, []))
    })
}

#[wasm_bindgen(js_name = runSourceAssessment)]
pub fn wasm_run_source_assessment(input: &str, request: JsValue) -> JsValue {
    json_response(|| {
        let request: SourceAssessmentRequest = from_js(request)?;
        let language = parse_language(&request.language)?;
        if language != SourceLanguage::Sysml {
            return Err(WasmError::new(
                "language",
                "source assessments currently support SysML sources",
            ));
        }

        let command = request.command.clone().unwrap_or_else(|| {
            format!(
                "mercurio assess {} --spec {}",
                request.filename, request.spec.id
            )
        });
        let parse_report = match parse_sysml_recovering(input) {
            Ok(report) => report,
            Err(diagnostic) => {
                return Ok(success(
                    json!({
                        "assessmentId": request.spec.id,
                        "status": "failed",
                        "command": command,
                        "report": null,
                        "transcript": [
                            "checking source assessment...",
                            "parsing source...",
                            format!("parse error: {}", diagnostic.message),
                            "result: failed",
                        ],
                        "facts": {
                            "factCount": 0,
                            "predicates": [],
                            "items": [],
                        },
                        "diagnostics": [snippet_diagnostic(&diagnostic)],
                    }),
                    [("runtime", json!("wasm"))],
                ));
            }
        };

        let diagnostics = parse_report
            .diagnostics
            .iter()
            .map(snippet_diagnostic)
            .collect::<Vec<_>>();
        let mut facts = sysml_module_assessment_facts(&parse_report.module);
        facts.extend(request.facts);
        let result = run_runtime_assessment(RuntimeAssessmentRequest {
            spec: request.spec,
            rulepacks: request.rulepacks,
            facts,
        })?;
        let passed = diagnostics.is_empty() && result.report.status == AssessmentStatus::Pass;
        let mut transcript = vec![
            "checking source assessment...".to_string(),
            "parsing source...".to_string(),
            "building assessment fact base...".to_string(),
            format!("running assessment `{}`...", result.report.id),
        ];
        if !diagnostics.is_empty() {
            transcript.push(format!("diagnostics: {}", diagnostics.len()));
        }
        for assertion in &result.report.assertions {
            transcript.push(format!(
                "assert {}: {}",
                assertion.id,
                match assertion.status {
                    AssessmentStatus::Pass => "pass",
                    AssessmentStatus::Failed => "failed",
                }
            ));
        }
        transcript.push(format!(
            "result: {}",
            if passed { "pass" } else { "failed" }
        ));

        Ok(success(
            json!({
                "assessmentId": result.report.id,
                "status": if passed { "pass" } else { "failed" },
                "command": command,
                "report": result.report,
                "transcript": transcript,
                "facts": assessment_fact_summary(&result.facts),
                "diagnostics": diagnostics,
            }),
            [("runtime", json!("wasm"))],
        ))
    })
}

#[wasm_bindgen(js_name = runSourceEvaluation)]
pub fn wasm_run_source_evaluation(input: &str, request: JsValue) -> JsValue {
    json_response(|| {
        let request: SourceEvaluationRequest = from_js(request)?;
        let language = parse_language(&request.language)?;
        if language != SourceLanguage::Sysml {
            return Err(WasmError::new(
                "language",
                "source evaluation currently supports SysML sources",
            ));
        }

        let stdlib = load_stdlib(None)?;
        let report = compile_sysml_text_with_context_report(input, &request.filename, &[], &stdlib);
        let diagnostics = report
            .diagnostics
            .iter()
            .map(snippet_diagnostic)
            .collect::<Vec<_>>();
        let Some(document) = report.document else {
            return Ok(success(
                json!({
                    "evaluationId": request.evaluation_id,
                    "status": "failed",
                    "diagnostics": diagnostics,
                    "scenarios": [],
                    "error": "source did not produce an evaluatable semantic document",
                }),
                [("runtime", json!("wasm"))],
            ));
        };
        if !diagnostics.is_empty() {
            return Ok(success(
                json!({
                    "evaluationId": request.evaluation_id,
                    "status": "failed",
                    "diagnostics": diagnostics,
                    "scenarios": [],
                    "error": "Resolve semantic diagnostics before evaluating this expression.",
                }),
                [("runtime", json!("wasm"))],
            ));
        }

        let merged_document = KirDocument::merge([stdlib, document])?;
        let runtime = Runtime::from_document(merged_document.clone())?;
        let mut scenario_results = Vec::new();
        for scenario in request.scenarios {
            let feature_id =
                find_feature_id(&merged_document, &scenario.feature_name).ok_or_else(|| {
                    WasmError::new(
                        "evaluation",
                        format!("feature `{}` not found", scenario.feature_name),
                    )
                })?;
            let owner_id = scenario
                .owner_name
                .as_deref()
                .and_then(|owner_name| find_owner_id_by_name(&merged_document, owner_name))
                .or_else(|| find_owner_id_for_feature(&merged_document, &feature_id))
                .ok_or_else(|| {
                    WasmError::new(
                        "evaluation",
                        format!("owner for feature `{}` not found", scenario.feature_name),
                    )
                })?;
            let mut context = ExecutionContext::default();
            for parameter in &scenario.parameters {
                context.values.insert(
                    (owner_id.clone(), parameter.name.clone()),
                    parameter.value.clone(),
                );
            }
            let result = runtime.evaluate(&feature_id, &owner_id, &context);
            scenario_results.push(match result {
                Ok(result) => json!({
                    "id": scenario.id,
                    "label": scenario.label,
                    "featureId": feature_id,
                    "ownerId": owner_id,
                    "ok": true,
                    "value": result.value,
                    "valueType": value_type(&result.value),
                    "explanation": result.explanation,
                    "error": null,
                    "parameters": scenario.parameters,
                }),
                Err(err) => json!({
                    "id": scenario.id,
                    "label": scenario.label,
                    "featureId": feature_id,
                    "ownerId": owner_id,
                    "ok": false,
                    "value": null,
                    "valueType": null,
                    "explanation": [],
                    "error": err.to_string(),
                    "parameters": scenario.parameters,
                }),
            });
        }

        let passed = scenario_results
            .iter()
            .all(|scenario| scenario.get("ok").and_then(Value::as_bool).unwrap_or(false));
        Ok(success(
            json!({
                "evaluationId": request.evaluation_id,
                "status": if passed { "pass" } else { "failed" },
                "diagnostics": diagnostics,
                "scenarios": scenario_results,
                "error": null,
            }),
            [("runtime", json!("wasm"))],
        ))
    })
}

#[wasm_bindgen(js_name = parseSysmlSnippet)]
pub fn wasm_parse_sysml_snippet(input: &str, request: JsValue) -> JsValue {
    json_response(|| {
        let request: SnippetParseRequest = from_js(request)?;
        let parse_report = match parse_sysml_recovering(input) {
            Ok(report) => report,
            Err(diagnostic) => {
                return Ok(success(
                    json!({
                        "diagnostics": [snippet_diagnostic(&diagnostic)],
                        "symbols": [],
                        "outline": [],
                    }),
                    [
                        ("runtime", json!("wasm")),
                        ("sourceName", json!(request.path)),
                    ],
                ));
            }
        };
        if !parse_report.diagnostics.is_empty() {
            return Ok(success(
                json!({
                    "diagnostics": parse_report.diagnostics.iter().map(snippet_diagnostic).collect::<Vec<_>>(),
                    "symbols": [],
                    "outline": [],
                }),
                [
                    ("runtime", json!("wasm")),
                    ("sourceName", json!(request.path)),
                ],
            ));
        }

        let mut symbols = Vec::new();
        let mut outline = Vec::new();
        if parse_report.module.members.is_empty() {
            if let Some(package) = &parse_report.module.package {
                let id = package.name.as_colon_string();
                outline.push(package_outline_node(
                    &id,
                    &id,
                    &package.span,
                    &package.members,
                    &mut symbols,
                ));
            }
        } else {
            for declaration in &parse_report.module.members {
                outline.push(declaration_outline_node(declaration, None, &mut symbols));
            }
        }
        let stdlib = load_stdlib(None)?;
        let semantic_report =
            compile_sysml_text_with_context_report(input, &request.path, &[], &stdlib);
        let diagnostics = semantic_report
            .diagnostics
            .iter()
            .map(snippet_diagnostic)
            .collect::<Vec<_>>();

        Ok(success(
            json!({
                "diagnostics": diagnostics,
                "symbols": symbols,
                "outline": outline,
            }),
            [
                ("runtime", json!("wasm")),
                ("sourceName", json!(request.path)),
            ],
        ))
    })
}

#[wasm_bindgen(js_name = MercurioSession)]
pub struct MercurioSession {
    stdlib: KirDocument,
    sources: Vec<SessionSource>,
}

#[wasm_bindgen(js_class = MercurioSession)]
impl MercurioSession {
    #[wasm_bindgen(constructor)]
    pub fn new(options: JsValue) -> Result<MercurioSession, JsValue> {
        let options = CompileOptions::from_js(options).map_err(js_error)?;
        let stdlib = load_stdlib(options.stdlib).map_err(js_error)?;
        Ok(Self {
            stdlib,
            sources: Vec::new(),
        })
    }

    #[wasm_bindgen(js_name = addSource)]
    pub fn add_source(&mut self, language: &str, source_name: &str, input: &str) -> JsValue {
        json_response(|| {
            let language = parse_language(language)?;
            let context = self
                .sources
                .iter()
                .map(|source| source.module.clone())
                .collect::<Vec<_>>();
            let module = match language {
                SourceLanguage::Sysml => parse_sysml_recovering(input)?.module,
                SourceLanguage::Kerml => parse_kerml(input)?,
            };
            let document = match language {
                SourceLanguage::Sysml => compile_sysml_text_with_context_report(
                    input,
                    source_name,
                    &context,
                    &self.stdlib,
                )
                .document
                .ok_or_else(|| WasmError::new("compile", "SysML compilation failed"))?,
                SourceLanguage::Kerml => compile_kerml_text(input, source_name, &self.stdlib)?,
            };
            self.sources.push(SessionSource {
                source_name: source_name.to_string(),
                language,
                module,
                document,
            });
            Ok(success(
                json!({ "sourceName": source_name, "sourceCount": self.sources.len() }),
                [("language", json!(language.as_str()))],
            ))
        })
    }

    #[wasm_bindgen(js_name = clear)]
    pub fn clear(&mut self) {
        self.sources.clear();
    }

    #[wasm_bindgen(js_name = document)]
    pub fn document(&self) -> JsValue {
        json_response(|| {
            let document = self.merged_document()?;
            Ok(success(
                serde_json::to_value(document)?,
                [("sourceCount", json!(self.sources.len()))],
            ))
        })
    }

    #[wasm_bindgen(js_name = renderDiagram)]
    pub fn render_diagram(&self, request: JsValue) -> JsValue {
        json_response(|| {
            let request: DiagramRenderRequestDto = from_js(request)?;
            let graph = self.graph()?;
            let registry = MetamodelAttributeRegistry::build(&graph);
            let view = render_diagram(&graph, &registry, request.spec)?;
            Ok(success(serde_json::to_value(view)?, []))
        })
    }

    #[wasm_bindgen(js_name = requirementsTable)]
    pub fn requirements_table(&self) -> JsValue {
        json_response(|| {
            let runtime = Runtime::from_document(self.merged_document()?)?;
            Ok(success(
                serde_json::to_value(requirements_table_view(runtime.graph()))?,
                [],
            ))
        })
    }

    #[wasm_bindgen(js_name = queryRuntime)]
    pub fn query_runtime(&self, query: JsValue) -> JsValue {
        json_response(|| {
            let query: RuntimeQuery = from_js(query)?;
            let runtime = Runtime::from_document(self.merged_document()?)?;
            Ok(success(run_runtime_query(&runtime, query)?, []))
        })
    }

    #[wasm_bindgen(js_name = runAssessment)]
    pub fn run_assessment(&self, spec: JsValue) -> JsValue {
        json_response(|| {
            let spec: AssessmentSpec = from_js(spec)?;
            let graph = self.graph()?;
            let rulepacks = load_default_rulepacks()?;
            let report = run_graph_assessment(&graph, &rulepacks, &spec)?;
            Ok(success(serde_json::to_value(report)?, []))
        })
    }
}

impl MercurioSession {
    fn merged_document(&self) -> Result<KirDocument, WasmError> {
        let mut elements = self.stdlib.elements.clone();
        for source in &self.sources {
            elements.extend(source.document.elements.clone());
        }
        let document = KirDocument {
            metadata: BTreeMap::from([
                ("source_count".to_string(), json!(self.sources.len())),
                (
                    "sources".to_string(),
                    json!(
                        self.sources
                            .iter()
                            .map(|source| json!({
                                "sourceName": source.source_name,
                                "language": source.language.as_str(),
                                "elementCount": source.document.elements.len(),
                            }))
                            .collect::<Vec<_>>()
                    ),
                ),
            ]),
            elements,
        };
        document.validate()?;
        Ok(document)
    }

    fn graph(&self) -> Result<Graph, WasmError> {
        Ok(Graph::from_document(self.merged_document()?)?)
    }
}

struct SessionSource {
    source_name: String,
    language: SourceLanguage,
    module: mercurio_core::frontend::ast::SysmlModule,
    document: KirDocument,
}

#[derive(Default)]
struct CompileOptions {
    source_name: String,
    stdlib: Option<KirDocument>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeQuery {
    kind: RuntimeQueryKind,
    #[serde(default)]
    type_id: Option<String>,
    #[serde(default)]
    feature_id: Option<String>,
    #[serde(default)]
    owner_id: Option<String>,
    #[serde(default)]
    context: RuntimeContextDto,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum RuntimeQueryKind {
    Subtypes,
    Features,
    Evaluate,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeContextDto {
    #[serde(default)]
    version: u64,
    #[serde(default)]
    values: Vec<RuntimeValueDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeValueDto {
    owner_id: String,
    feature_id: String,
    value: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceAssessmentRequest {
    spec: AssessmentSpec,
    #[serde(default)]
    rulepacks: Vec<RulePack>,
    #[serde(default)]
    facts: Vec<Fact>,
    #[serde(default = "default_source_name")]
    filename: String,
    #[serde(default = "default_source_language")]
    language: String,
    #[serde(default)]
    command: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceEvaluationRequest {
    evaluation_id: String,
    scenarios: Vec<SourceEvaluationScenario>,
    #[serde(default = "default_source_name")]
    filename: String,
    #[serde(default = "default_source_language")]
    language: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceEvaluationScenario {
    id: String,
    label: String,
    feature_name: String,
    #[serde(default)]
    owner_name: Option<String>,
    #[serde(default)]
    parameters: Vec<SourceEvaluationParameter>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceEvaluationParameter {
    name: String,
    #[serde(default)]
    label: Option<String>,
    value: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnippetParseRequest {
    #[serde(default = "default_source_name")]
    path: String,
}

impl CompileOptions {
    fn from_js(value: JsValue) -> Result<Self, WasmError> {
        if value.is_null() || value.is_undefined() {
            return Ok(Self {
                source_name: "memory.sysml".to_string(),
                stdlib: None,
            });
        }
        let raw: Value = from_js(value)?;
        let source_name = raw
            .get("sourceName")
            .or_else(|| raw.get("source_name"))
            .and_then(Value::as_str)
            .unwrap_or("memory.sysml")
            .to_string();
        let stdlib = raw
            .get("stdlib")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?;
        Ok(Self {
            source_name,
            stdlib,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    ok: bool,
    value: Option<Value>,
    diagnostics: Value,
    errors: Vec<WasmError>,
    metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WasmError {
    code: String,
    message: String,
}

impl WasmError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for WasmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for WasmError {}

impl From<serde_json::Error> for WasmError {
    fn from(value: serde_json::Error) -> Self {
        Self::new("json", value.to_string())
    }
}

macro_rules! impl_error {
    ($source:ty, $code:literal) => {
        impl From<$source> for WasmError {
            fn from(value: $source) -> Self {
                Self::new($code, value.to_string())
            }
        }
    };
}

impl_error!(mercurio_core::DatalogError, "datalog");
impl_error!(mercurio_core::GraphError, "graph");
impl_error!(mercurio_core::KirError, "kir");
impl_error!(mercurio_core::RuntimeError, "runtime");
impl_error!(mercurio_core::AssessmentError, "assessment");
impl_error!(DiagramError, "diagram");
impl_error!(mercurio_core::FormatError, "format");
impl_error!(
    mercurio_core::frontend::diagnostics::Diagnostic,
    "diagnostic"
);

fn load_stdlib(stdlib: Option<KirDocument>) -> Result<KirDocument, WasmError> {
    match stdlib {
        Some(document) => {
            document.validate()?;
            Ok(document)
        }
        None => KirDocument::from_str(DEFAULT_STDLIB).map_err(Into::into),
    }
}

fn parse_language(language: &str) -> Result<SourceLanguage, WasmError> {
    match language.to_ascii_lowercase().as_str() {
        "sysml" | "sysml2" => Ok(SourceLanguage::Sysml),
        "kerml" => Ok(SourceLanguage::Kerml),
        _ => Err(WasmError::new(
            "language",
            format!("unsupported source language: {language}"),
        )),
    }
}

fn semantic_status(status: mercurio_core::SemanticCompileStatus) -> &'static str {
    match status {
        mercurio_core::SemanticCompileStatus::Ok => "ok",
        mercurio_core::SemanticCompileStatus::Partial => "partial",
        mercurio_core::SemanticCompileStatus::Failed => "failed",
    }
}

fn assessment_fact_summary(facts: &[Fact]) -> Value {
    let mut predicates = facts
        .iter()
        .map(|fact| fact.predicate.clone())
        .collect::<Vec<_>>();
    predicates.sort();
    predicates.dedup();
    json!({
        "factCount": facts.len(),
        "predicates": predicates,
        "items": facts,
    })
}

fn find_feature_id(document: &KirDocument, feature_name: &str) -> Option<String> {
    document
        .elements
        .iter()
        .find(|element| {
            element_name(&element.properties) == Some(feature_name)
                && element.properties.contains_key("expression_ir")
        })
        .or_else(|| {
            document
                .elements
                .iter()
                .find(|element| element_name(&element.properties) == Some(feature_name))
        })
        .map(|element| element.id.clone())
}

fn find_owner_id_by_name(document: &KirDocument, owner_name: &str) -> Option<String> {
    document
        .elements
        .iter()
        .find(|element| element_name(&element.properties) == Some(owner_name))
        .map(|element| element.id.clone())
}

fn find_owner_id_for_feature(document: &KirDocument, feature_id: &str) -> Option<String> {
    document
        .elements
        .iter()
        .find(|element| {
            property_array_contains(&element.properties, "features", feature_id)
                || property_array_contains(&element.properties, "members", feature_id)
                || property_array_contains(&element.properties, "member_ids", feature_id)
        })
        .map(|element| element.id.clone())
}

fn property_array_contains(
    properties: &BTreeMap<String, Value>,
    key: &str,
    expected: &str,
) -> bool {
    properties
        .get(key)
        .and_then(Value::as_array)
        .map(|values| values.iter().any(|value| value.as_str() == Some(expected)))
        .unwrap_or(false)
}

fn element_name(properties: &BTreeMap<String, Value>) -> Option<&str> {
    properties
        .get("declared_name")
        .or_else(|| properties.get("name"))
        .and_then(Value::as_str)
}

fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn snippet_diagnostic(diagnostic: &mercurio_core::frontend::diagnostics::Diagnostic) -> Value {
    let (line, column) = diagnostic
        .span
        .as_ref()
        .map(|span| (span.start_line, span.start_col))
        .unwrap_or((1, 1));
    json!({
        "severity": "error",
        "message": diagnostic.message,
        "startLineNumber": line,
        "startColumn": column,
        "endLineNumber": diagnostic.span.as_ref().map(|span| span.end_line).unwrap_or(line),
        "endColumn": diagnostic.span.as_ref().map(|span| span.end_col).unwrap_or(column),
        "start_line_number": line,
        "start_column": column,
        "end_line_number": diagnostic.span.as_ref().map(|span| span.end_line).unwrap_or(line),
        "end_column": diagnostic.span.as_ref().map(|span| span.end_col).unwrap_or(column),
        "line": line,
        "column": column,
    })
}

fn push_ast_symbol(symbols: &mut Vec<Value>, id: &str, kind: &str, label: &str, span: &SourceSpan) {
    symbols.push(json!({
        "id": id,
        "kind": kind,
        "label": label,
        "startLineNumber": span.start_line,
        "start_line_number": span.start_line,
    }));
}

fn declaration_outline_node(
    declaration: &Declaration,
    owner: Option<&str>,
    symbols: &mut Vec<Value>,
) -> Value {
    match declaration {
        Declaration::Package(package) => {
            let name = package.name.as_colon_string();
            let id = scoped_ast_id(owner, &name);
            package_outline_node(&id, &name, &package.span, &package.members, symbols)
        }
        Declaration::PartDefinition(definition) => {
            let id = scoped_ast_id(owner, &definition.name);
            push_ast_symbol(
                symbols,
                &id,
                "PartDefinition",
                &definition.name,
                &definition.span,
            );
            let mut children = definition
                .members
                .iter()
                .map(|member| declaration_outline_node(member, Some(&id), symbols))
                .collect::<Vec<_>>();
            children.extend(
                definition
                    .part_members
                    .iter()
                    .map(|member| part_usage_outline_node(member, Some(&id), symbols)),
            );
            json!({
                "id": id,
                "elementId": id,
                "element_id": id,
                "label": definition.name,
                "kind": "PartDefinition",
                "properties": ast_properties(&definition.name, &definition.span),
                "children": children,
            })
        }
        Declaration::PartUsage(usage) => part_usage_outline_node(usage, owner, symbols),
        Declaration::GenericDefinition(definition) => {
            let id = scoped_ast_id(owner, &definition.name);
            let kind = format!("{}Definition", pascal_keyword(&definition.keyword));
            push_ast_symbol(symbols, &id, &kind, &definition.name, &definition.span);
            let children = definition
                .members
                .iter()
                .map(|member| declaration_outline_node(member, Some(&id), symbols))
                .collect::<Vec<_>>();
            json!({
                "id": id,
                "elementId": id,
                "element_id": id,
                "label": definition.name,
                "kind": kind,
                "properties": ast_properties(&definition.name, &definition.span),
                "children": children,
            })
        }
        Declaration::GenericUsage(usage) => {
            let id = scoped_ast_id(owner, &usage.name);
            let kind = format!("{}Usage", pascal_keyword(&usage.keyword));
            push_ast_symbol(symbols, &id, &kind, &usage.name, &usage.span);
            let children = usage
                .body_members
                .iter()
                .map(|member| declaration_outline_node(member, Some(&id), symbols))
                .collect::<Vec<_>>();
            json!({
                "id": id,
                "elementId": id,
                "element_id": id,
                "label": usage.name,
                "kind": kind,
                "properties": ast_properties(&usage.name, &usage.span),
                "children": children,
            })
        }
        Declaration::Import(import) => {
            let name = import.path.as_colon_string();
            let id = scoped_ast_id(owner, &name);
            push_ast_symbol(symbols, &id, "Import", &name, &import.span);
            json!({
                "id": id,
                "elementId": id,
                "element_id": id,
                "label": name,
                "kind": "Import",
                "properties": ast_properties(&name, &import.span),
                "children": [],
            })
        }
        Declaration::Alias(alias) => {
            let id = scoped_ast_id(owner, &alias.name);
            push_ast_symbol(symbols, &id, "Alias", &alias.name, &alias.span);
            json!({
                "id": id,
                "elementId": id,
                "element_id": id,
                "label": alias.name,
                "kind": "Alias",
                "properties": ast_properties(&alias.name, &alias.span),
                "children": [],
            })
        }
    }
}

fn package_outline_node(
    id: &str,
    name: &str,
    span: &SourceSpan,
    members: &[Declaration],
    symbols: &mut Vec<Value>,
) -> Value {
    push_ast_symbol(symbols, id, "Package", name, span);
    let children = members
        .iter()
        .map(|member| declaration_outline_node(member, Some(id), symbols))
        .collect::<Vec<_>>();
    json!({
        "id": id,
        "elementId": id,
        "element_id": id,
        "label": name,
        "kind": "Package",
        "properties": ast_properties(name, span),
        "children": children,
    })
}

fn part_usage_outline_node(
    usage: &PartUsageDecl,
    owner: Option<&str>,
    symbols: &mut Vec<Value>,
) -> Value {
    let id = scoped_ast_id(owner, &usage.name);
    push_ast_symbol(symbols, &id, "PartUsage", &usage.name, &usage.span);
    let children = usage
        .body_members
        .iter()
        .map(|member| declaration_outline_node(member, Some(&id), symbols))
        .collect::<Vec<_>>();
    json!({
        "id": id,
        "elementId": id,
        "element_id": id,
        "label": usage.name,
        "kind": "PartUsage",
        "properties": ast_properties(&usage.name, &usage.span),
        "children": children,
    })
}

fn scoped_ast_id(owner: Option<&str>, name: &str) -> String {
    owner
        .map(|owner| format!("{owner}::{name}"))
        .unwrap_or_else(|| name.to_string())
}

fn ast_properties(name: &str, span: &SourceSpan) -> Value {
    json!({
        "name": name,
        "metadata": {
            "name": name,
            "source_span": {
                "start_line": span.start_line,
                "start_col": span.start_col,
                "end_line": span.end_line,
                "end_col": span.end_col,
            },
        },
        "source_span": {
            "start_line": span.start_line,
            "start_col": span.start_col,
            "end_line": span.end_line,
            "end_col": span.end_col,
        },
    })
}

fn pascal_keyword(keyword: &str) -> String {
    keyword
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<String>()
}

fn default_source_name() -> String {
    "memory.sysml".to_string()
}

fn default_source_language() -> String {
    "sysml".to_string()
}

fn run_runtime_query(runtime: &Runtime, query: RuntimeQuery) -> Result<Value, WasmError> {
    match query.kind {
        RuntimeQueryKind::Subtypes => {
            let type_id = required(query.type_id, "typeId")?;
            let result = runtime.get_subtypes(&type_id)?;
            Ok(json!({
                "value": result.value,
                "explanation": result.explanation,
            }))
        }
        RuntimeQueryKind::Features => {
            let type_id = required(query.type_id, "typeId")?;
            let result = runtime.get_features(&type_id)?;
            Ok(json!({
                "value": result.value,
                "explanation": result.explanation,
            }))
        }
        RuntimeQueryKind::Evaluate => {
            let feature_id = required(query.feature_id, "featureId")?;
            let owner_id = required(query.owner_id, "ownerId")?;
            let context = execution_context(query.context);
            let result = runtime.evaluate(&feature_id, &owner_id, &context)?;
            Ok(json!({
                "value": result.value,
                "explanation": result.explanation,
            }))
        }
    }
}

fn execution_context(context: RuntimeContextDto) -> ExecutionContext {
    let values = context
        .values
        .into_iter()
        .map(|entry| ((entry.owner_id, entry.feature_id), entry.value))
        .collect();
    ExecutionContext {
        values,
        version: context.version,
    }
}

fn required(value: Option<String>, field: &str) -> Result<String, WasmError> {
    value.ok_or_else(|| WasmError::new("query", format!("missing runtime query field: {field}")))
}

fn from_js<T>(value: JsValue) -> Result<T, WasmError>
where
    T: serde::de::DeserializeOwned,
{
    serde_wasm_bindgen::from_value(value).map_err(|err| WasmError::new("js", err.to_string()))
}

fn to_js<T>(value: &T) -> JsValue
where
    T: Serialize,
{
    serde_wasm_bindgen::to_value(value).unwrap_or_else(|err| {
        JsValue::from_str(&format!("failed to serialize wasm response: {err}"))
    })
}

fn json_response(action: impl FnOnce() -> Result<Response, WasmError>) -> JsValue {
    match action() {
        Ok(response) => to_js(&response),
        Err(error) => to_js(&Response {
            ok: false,
            value: None,
            diagnostics: json!([]),
            errors: vec![error],
            metadata: BTreeMap::new(),
        }),
    }
}

fn success<const N: usize>(value: Value, metadata_items: [(&str, Value); N]) -> Response {
    Response {
        ok: true,
        value: Some(value),
        diagnostics: json!([]),
        errors: Vec::new(),
        metadata: metadata(metadata_items),
    }
}

fn error_response(code: &str, message: String, diagnostics: Option<Value>) -> Response {
    Response {
        ok: false,
        value: None,
        diagnostics: diagnostics.unwrap_or_else(|| json!([])),
        errors: vec![WasmError::new(code, message)],
        metadata: BTreeMap::new(),
    }
}

fn metadata<const N: usize>(items: [(&str, Value); N]) -> BTreeMap<String, Value> {
    items
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn js_error(error: WasmError) -> JsValue {
    JsValue::from_str(&error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_stdlib_is_valid_kir() {
        let document = load_stdlib(None).unwrap();
        assert!(!document.elements.is_empty());
    }

    #[test]
    fn default_stdlib_resolves_port_and_interface_definitions() {
        let stdlib = load_stdlib(None).unwrap();
        let report = compile_sysml_text_with_context_report(
            "package Demo {
                item def Command;

                port def CommandPort {
                    item command: Command;
                }

                interface def CommandInterface {
                    end controller: CommandPort;
                    end rotor: CommandPort;
                }
            }",
            "ports.sysml",
            &[],
            &stdlib,
        );

        assert!(
            report.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            report.diagnostics
        );
        assert!(report.document.is_some());
    }

    #[test]
    fn session_merges_user_sources_with_stdlib() {
        let stdlib = load_stdlib(None).unwrap();
        let module = parse_sysml_recovering("package Demo { }").unwrap().module;
        let document =
            compile_sysml_text_with_context_report("package Demo { }", "demo.sysml", &[], &stdlib)
                .document
                .unwrap();
        let mut session = MercurioSession {
            stdlib,
            sources: Vec::new(),
        };

        session.sources.push(SessionSource {
            source_name: "demo.sysml".to_string(),
            language: SourceLanguage::Sysml,
            module,
            document,
        });
        assert!(session.merged_document().unwrap().elements.len() > session.stdlib.elements.len());
    }
}
