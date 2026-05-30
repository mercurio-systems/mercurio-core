use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use mercurio_core::frontend::diagnostics::Diagnostic;
use mercurio_core::source_set::{
    SourceCompileContext, SourceDocument, compile_source_document_with_context,
};
use mercurio_core::{KirDocument, default_stdlib_path, repo_path};
use mercurio_tools::default_pilot_root;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args()?;
    let corpus_seed = PilotCorpusSeed::load()?;
    let pilot_runner = PilotRunner::new(&args.pilot_root)?;

    match &args.relative_path {
        Some(relative_path) => {
            let output = run_compare_case(&pilot_runner, relative_path, &corpus_seed)?;
            if let Some(parent) = args.output_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&args.output_path, serde_json::to_string_pretty(&output)?)?;

            println!("pilot compile-error comparison");
            println!("  file: {}", relative_path);
            println!("  output: {}", args.output_path.display());
            println!("  mercurio status: {}", output.mercurio.status);
            println!("  pilot status: {}", output.pilot.status);
            println!("  status match: {}", output.comparison.status_match);
            println!(
                "  primary problem match: {}",
                output.comparison.primary_problem_match
            );
            println!("  mercurio total ms: {}", output.mercurio.timings.total_ms);
            println!("  pilot total ms: {}", output.pilot.timings.total_ms);
        }
        None => {
            let (corpus_name, relative_paths) = args.corpus_paths(&corpus_seed)?;
            let pilot_cases = export_diagnostics_corpus_from_pilot(
                &pilot_runner,
                &corpus_name,
                &relative_paths,
                &corpus_seed,
            )?;
            let mut cases = Vec::new();

            for relative_path in &relative_paths {
                match run_compare_case_from_batch(
                    &pilot_runner,
                    relative_path,
                    &corpus_seed,
                    &pilot_cases,
                ) {
                    Ok(output) => {
                        println!(
                            "checked {}: rust={} pilot={} status_match={} primary_match={}",
                            relative_path,
                            output.mercurio.status,
                            output.pilot.status,
                            output.comparison.status_match,
                            output.comparison.primary_problem_match
                        );
                        cases.push(CorpusCaseSummary::from_output(output));
                    }
                    Err(err) => {
                        println!("failed {}: {}", relative_path, err);
                        cases.push(CorpusCaseSummary::failure(
                            relative_path.to_string(),
                            corpus_seed
                                .support_paths_for_case(&args.pilot_root, relative_path)
                                .len(),
                            err.to_string(),
                        ));
                    }
                }
            }

            let corpus_output = CorpusCompareOutput {
                generated_at_utc: now_utc_rfc3339()?,
                pilot_root: args.pilot_root.display().to_string(),
                corpus_name,
                case_count: cases.len(),
                aggregate: CorpusAggregate::from_cases(&cases),
                cases,
            };

            if let Some(parent) = args.output_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(
                &args.output_path,
                serde_json::to_string_pretty(&corpus_output)?,
            )?;

            println!("pilot compile-error corpus comparison");
            println!("  corpus: {}", corpus_output.corpus_name);
            println!("  cases: {}", corpus_output.case_count);
            println!("  output: {}", args.output_path.display());
            println!(
                "  status matches: {}",
                corpus_output.aggregate.status_match_cases
            );
            println!(
                "  both fail same primary problem: {}",
                corpus_output.aggregate.primary_problem_match_cases
            );
            println!(
                "  rust-only failures: {}",
                corpus_output.aggregate.rust_only_fail_cases
            );
            println!(
                "  pilot-only failures: {}",
                corpus_output.aggregate.pilot_only_fail_cases
            );
        }
    }

    Ok(())
}

#[derive(Debug)]
struct Args {
    pilot_root: PathBuf,
    relative_path: Option<String>,
    corpus_name: Option<String>,
    paths_file: Option<PathBuf>,
    output_path: PathBuf,
}

impl Args {
    fn corpus_paths(
        &self,
        corpus_seed: &PilotCorpusSeed,
    ) -> Result<(String, Vec<String>), Box<dyn std::error::Error>> {
        if let Some(paths_file) = &self.paths_file {
            let paths = std::fs::read_to_string(paths_file)?
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            let name = paths_file
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("paths")
                .to_string();
            return Ok((name, paths));
        }

        let corpus_name = self
            .corpus_name
            .as_deref()
            .ok_or("provide --relative-path, --corpus, or --paths-file")?;
        Ok((
            corpus_name.to_string(),
            corpus_seed.corpus_paths(corpus_name, &self.pilot_root)?,
        ))
    }
}

#[derive(Debug, Deserialize)]
struct PilotCorpusSeed {
    #[serde(default)]
    corpora: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    support_dependencies: BTreeMap<String, Vec<String>>,
}

impl PilotCorpusSeed {
    fn load() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(serde_json::from_str(&std::fs::read_to_string(repo_path(
            "crates/mercurio-tools/corpus/pilot_corpus.seed.json",
        ))?)?)
    }

    fn support_paths_for(&self, relative_path: &str) -> &[String] {
        self.support_dependencies
            .get(relative_path)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn support_paths_for_case(&self, pilot_root: &Path, relative_path: &str) -> Vec<String> {
        let mut support_paths = Vec::new();
        for path in self.support_paths_for(relative_path) {
            push_unique(&mut support_paths, path.clone());
        }
        for path in same_folder_sysml_paths(pilot_root, relative_path) {
            if path != relative_path {
                push_unique(&mut support_paths, path);
            }
        }
        support_paths
    }

    fn corpus_paths(
        &self,
        name: &str,
        pilot_root: &Path,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        if name == "all" {
            return discover_all_pilot_examples(pilot_root);
        }
        self.corpora
            .get(name)
            .cloned()
            .ok_or_else(|| format!("unknown corpus `{name}`").into())
    }
}

fn discover_all_pilot_examples(
    pilot_root: &Path,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut stack = vec![
        pilot_root.join("sysml/src/examples"),
        pilot_root.join("sysml/src/training"),
        pilot_root.join("sysml/src/validation"),
    ];
    let mut files = Vec::new();

    while let Some(path) = stack.pop() {
        if path.is_dir() {
            for entry in std::fs::read_dir(&path)? {
                stack.push(entry?.path());
            }
            continue;
        }

        if path
            .extension()
            .is_some_and(|extension| extension == "sysml")
        {
            files.push(
                path.strip_prefix(pilot_root)?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }

    files.sort();
    Ok(files)
}

fn same_folder_sysml_paths(pilot_root: &Path, relative_path: &str) -> Vec<String> {
    let Some(parent) = Path::new(relative_path).parent() else {
        return Vec::new();
    };
    let folder = pilot_root.join(parent);
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };

    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "sysml")
        })
        .filter_map(|path| {
            path.strip_prefix(pilot_root)
                .ok()
                .map(|path| path.to_string_lossy().replace('\\', "/"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn push_unique(paths: &mut Vec<String>, path: String) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

#[derive(Debug)]
struct PilotRunner {
    pilot_root: PathBuf,
    library_root: PathBuf,
    interactive_jar: PathBuf,
    classes_dir: PathBuf,
}

impl PilotRunner {
    fn new(pilot_root: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let pilot_root = pilot_root.canonicalize()?;
        let interactive_jar = find_interactive_jar(&pilot_root)?;
        let classes_dir = repo_path("target/pilot-exporter-classes");
        let java_source = repo_path(
            "tools/pilot-exporter/src/main/java/dev/mercurio/pilot/PilotModelExporter.java",
        );
        compile_java_exporter(
            &interactive_jar,
            &java_source,
            &classes_dir,
            "dev/mercurio/pilot/PilotModelExporter.class",
        )?;
        Ok(Self {
            library_root: pilot_root.join("sysml.library"),
            pilot_root,
            interactive_jar,
            classes_dir,
        })
    }
}

#[derive(Debug, Serialize)]
struct CompareOutput {
    generated_at_utc: String,
    pilot_root: String,
    relative_path: String,
    support_paths: Vec<String>,
    comparison: CompileErrorComparison,
    mercurio: EngineCompileResult,
    pilot: EngineCompileResult,
}

#[derive(Debug, Serialize)]
struct EngineCompileResult {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_stage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_input: Option<String>,
    diagnostics: Vec<NormalizedCompileDiagnostic>,
    timings: EngineTimings,
}

#[derive(Debug, Serialize, Clone)]
struct NormalizedCompileDiagnostic {
    stage: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    column: Option<u32>,
    message: String,
    problem_kind: String,
}

#[derive(Debug, Serialize)]
struct CompileErrorComparison {
    status_match: bool,
    both_pass: bool,
    both_fail: bool,
    failure_stage_match: bool,
    primary_problem_match: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    mercurio_primary_problem: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pilot_primary_problem: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
struct EngineTimings {
    total_ms: u64,
    phases: Vec<PhaseTiming>,
}

#[derive(Debug, Serialize, Clone)]
struct PhaseTiming {
    name: String,
    duration_ms: u64,
}

#[derive(Debug, Serialize)]
struct CorpusCompareOutput {
    generated_at_utc: String,
    pilot_root: String,
    corpus_name: String,
    case_count: usize,
    aggregate: CorpusAggregate,
    cases: Vec<CorpusCaseSummary>,
}

#[derive(Debug, Serialize)]
struct CorpusCaseSummary {
    relative_path: String,
    support_file_count: usize,
    status: String,
    error: Option<String>,
    comparison: Option<CompileErrorComparison>,
    mercurio_status: Option<String>,
    pilot_status: Option<String>,
    mercurio_timings: Option<EngineTimings>,
    pilot_timings: Option<EngineTimings>,
}

#[derive(Debug, Serialize)]
struct CorpusAggregate {
    status_match_cases: usize,
    both_pass_cases: usize,
    both_fail_cases: usize,
    primary_problem_match_cases: usize,
    rust_only_fail_cases: usize,
    pilot_only_fail_cases: usize,
    failed_cases: usize,
}

impl CorpusCaseSummary {
    fn from_output(output: CompareOutput) -> Self {
        Self {
            relative_path: output.relative_path,
            support_file_count: output.support_paths.len(),
            status: "ok".to_string(),
            error: None,
            comparison: Some(output.comparison),
            mercurio_status: Some(output.mercurio.status),
            pilot_status: Some(output.pilot.status),
            mercurio_timings: Some(output.mercurio.timings),
            pilot_timings: Some(output.pilot.timings),
        }
    }

    fn failure(relative_path: String, support_file_count: usize, error: String) -> Self {
        Self {
            relative_path,
            support_file_count,
            status: "error".to_string(),
            error: Some(error),
            comparison: None,
            mercurio_status: None,
            pilot_status: None,
            mercurio_timings: None,
            pilot_timings: None,
        }
    }
}

impl CorpusAggregate {
    fn from_cases(cases: &[CorpusCaseSummary]) -> Self {
        Self {
            status_match_cases: cases
                .iter()
                .filter(|case| case.comparison.as_ref().is_some_and(|cmp| cmp.status_match))
                .count(),
            both_pass_cases: cases
                .iter()
                .filter(|case| case.comparison.as_ref().is_some_and(|cmp| cmp.both_pass))
                .count(),
            both_fail_cases: cases
                .iter()
                .filter(|case| case.comparison.as_ref().is_some_and(|cmp| cmp.both_fail))
                .count(),
            primary_problem_match_cases: cases
                .iter()
                .filter(|case| {
                    case.comparison
                        .as_ref()
                        .is_some_and(|cmp| cmp.primary_problem_match)
                })
                .count(),
            rust_only_fail_cases: cases
                .iter()
                .filter(|case| {
                    case.comparison.as_ref().is_some_and(|cmp| {
                        !cmp.status_match && case.mercurio_status.as_deref() == Some("error")
                    })
                })
                .count(),
            pilot_only_fail_cases: cases
                .iter()
                .filter(|case| {
                    case.comparison.as_ref().is_some_and(|cmp| {
                        !cmp.status_match && case.pilot_status.as_deref() == Some("error")
                    })
                })
                .count(),
            failed_cases: cases.iter().filter(|case| case.status == "error").count(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct PilotDiagnosticRunDocument {
    status: String,
    failure_stage: Option<String>,
    exception_message: Option<String>,
    #[serde(default)]
    diagnostics: Vec<PilotCompileDiagnostic>,
    timings: PilotDiagnosticTimings,
}

#[derive(Debug, Clone, Deserialize)]
struct PilotDiagnosticTimings {
    total_ms: u64,
    #[serde(default)]
    phases: Vec<PilotPhaseTiming>,
}

#[derive(Debug, Clone, Deserialize)]
struct PilotPhaseTiming {
    name: String,
    duration_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct PilotCompileDiagnostic {
    stage: String,
    file: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PilotDiagnosticsBatchDocument {
    cases: Vec<PilotDiagnosticsBatchCase>,
}

#[derive(Debug, Deserialize)]
struct PilotDiagnosticsBatchCase {
    relative_path: String,
    result: PilotDiagnosticRunDocument,
}

#[derive(Debug, Serialize)]
struct PilotCorpusSpec<'a> {
    cases: Vec<PilotCorpusSpecCase<'a>>,
}

#[derive(Debug, Serialize)]
struct PilotCorpusSpecCase<'a> {
    relative_path: &'a str,
    input_files: Vec<String>,
}

fn parse_args() -> Result<Args, Box<dyn std::error::Error>> {
    let mut pilot_root = default_pilot_root();
    let mut relative_path = None;
    let mut corpus_name = None;
    let mut paths_file = None;
    let mut output_path = repo_path("target/pilot_compile_error_compare.json");
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--pilot-root" => {
                index += 1;
                pilot_root =
                    PathBuf::from(args.get(index).ok_or("missing value for --pilot-root")?);
            }
            "--relative-path" => {
                index += 1;
                relative_path = Some(
                    args.get(index)
                        .ok_or("missing value for --relative-path")?
                        .to_string(),
                );
            }
            "--corpus" => {
                index += 1;
                corpus_name = Some(
                    args.get(index)
                        .ok_or("missing value for --corpus")?
                        .to_string(),
                );
            }
            "--paths-file" => {
                index += 1;
                paths_file = Some(PathBuf::from(
                    args.get(index).ok_or("missing value for --paths-file")?,
                ));
            }
            "--out" => {
                index += 1;
                output_path = PathBuf::from(args.get(index).ok_or("missing value for --out")?);
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            unknown => return Err(format!("unknown argument: {unknown}").into()),
        }
        index += 1;
    }

    let selector_count = usize::from(relative_path.is_some())
        + usize::from(corpus_name.is_some())
        + usize::from(paths_file.is_some());
    if selector_count != 1 {
        return Err("provide exactly one of --relative-path, --corpus, or --paths-file".into());
    }

    Ok(Args {
        pilot_root,
        relative_path,
        corpus_name,
        paths_file,
        output_path,
    })
}

fn print_usage() {
    println!(
        "Usage: compare_pilot_compile_errors --pilot-root <pilot-root> (--relative-path <path> | --corpus <name|all> | --paths-file <path>) [--out <path>]"
    );
}

fn run_compare_case(
    pilot_runner: &PilotRunner,
    relative_path: &str,
    corpus_seed: &PilotCorpusSeed,
) -> Result<CompareOutput, Box<dyn std::error::Error>> {
    let support_paths = corpus_seed.support_paths_for_case(&pilot_runner.pilot_root, relative_path);
    let mercurio = build_mercurio_case(&pilot_runner.pilot_root, relative_path, &support_paths)?;
    let pilot = build_pilot_case(pilot_runner, relative_path, &support_paths)?;
    let comparison = compare_results(&mercurio, &pilot);

    Ok(CompareOutput {
        generated_at_utc: now_utc_rfc3339()?,
        pilot_root: pilot_runner.pilot_root.display().to_string(),
        relative_path: relative_path.to_string(),
        support_paths,
        comparison,
        mercurio,
        pilot,
    })
}

fn run_compare_case_from_batch(
    pilot_runner: &PilotRunner,
    relative_path: &str,
    corpus_seed: &PilotCorpusSeed,
    pilot_cases: &BTreeMap<String, PilotDiagnosticRunDocument>,
) -> Result<CompareOutput, Box<dyn std::error::Error>> {
    let support_paths = corpus_seed.support_paths_for_case(&pilot_runner.pilot_root, relative_path);
    let mercurio = build_mercurio_case(&pilot_runner.pilot_root, relative_path, &support_paths)?;
    let pilot_document = pilot_cases
        .get(relative_path)
        .ok_or_else(|| format!("pilot batch diagnostics missing case `{relative_path}`"))?;
    let pilot = pilot_result_from_document(relative_path, pilot_document);
    let comparison = compare_results(&mercurio, &pilot);

    Ok(CompareOutput {
        generated_at_utc: now_utc_rfc3339()?,
        pilot_root: pilot_runner.pilot_root.display().to_string(),
        relative_path: relative_path.to_string(),
        support_paths,
        comparison,
        mercurio,
        pilot,
    })
}

fn build_mercurio_case(
    pilot_root: &Path,
    relative_path: &str,
    support_paths: &[String],
) -> Result<EngineCompileResult, Box<dyn std::error::Error>> {
    let start = Instant::now();
    let mut phases = Vec::new();

    let load_stdlib_start = Instant::now();
    let augmented = KirDocument::from_path(&default_stdlib_path())?;
    phases.push(PhaseTiming {
        name: "load_stdlib_json".to_string(),
        duration_ms: elapsed_ms(load_stdlib_start),
    });

    let read_parse_start = Instant::now();
    let source_documents = read_source_documents(pilot_root, support_paths, relative_path)?;
    phases.push(PhaseTiming {
        name: "read_parse_source_set".to_string(),
        duration_ms: elapsed_ms(read_parse_start),
    });

    let context_start = Instant::now();
    let compile_context =
        match SourceCompileContext::from_source_documents(&source_documents, &augmented) {
            Ok(context) => {
                phases.push(PhaseTiming {
                    name: "build_resolver_context".to_string(),
                    duration_ms: elapsed_ms(context_start),
                });
                context
            }
            Err(diagnostic) => {
                phases.push(PhaseTiming {
                    name: "build_resolver_context".to_string(),
                    duration_ms: elapsed_ms(context_start),
                });
                return Ok(error_result(
                    "resolve_context",
                    None,
                    diagnostic_to_normalized("resolve_context", None, &diagnostic),
                    start,
                    phases,
                ));
            }
        };

    let target_document = source_documents
        .iter()
        .find(|file| file.path == relative_path)
        .ok_or_else(|| format!("source set missing target `{relative_path}`"))?;

    let compile_start = Instant::now();
    if let Err(diagnostic) =
        compile_source_document_with_context(target_document, &compile_context, &augmented)
    {
        phases.push(PhaseTiming {
            name: "compile_target_with_source_set".to_string(),
            duration_ms: elapsed_ms(compile_start),
        });
        return Ok(error_result(
            "resolve_transpile",
            Some(target_document.path.clone()),
            diagnostic_to_normalized(
                "resolve_transpile",
                Some(&target_document.path),
                &diagnostic,
            ),
            start,
            phases,
        ));
    }
    phases.push(PhaseTiming {
        name: "compile_target_with_source_set".to_string(),
        duration_ms: elapsed_ms(compile_start),
    });

    Ok(EngineCompileResult {
        status: "ok".to_string(),
        failure_stage: None,
        failure_input: None,
        diagnostics: Vec::new(),
        timings: EngineTimings {
            total_ms: elapsed_ms(start),
            phases,
        },
    })
}

fn build_pilot_case(
    pilot_runner: &PilotRunner,
    relative_path: &str,
    support_paths: &[String],
) -> Result<EngineCompileResult, Box<dyn std::error::Error>> {
    let export_path = run_java_diagnostics_exporter(pilot_runner, relative_path, support_paths)?;
    let document: PilotDiagnosticRunDocument =
        serde_json::from_str(&std::fs::read_to_string(&export_path)?)?;
    Ok(pilot_result_from_document(relative_path, &document))
}

fn read_source_documents(
    pilot_root: &Path,
    support_paths: &[String],
    relative_path: &str,
) -> Result<Vec<SourceDocument>, Box<dyn std::error::Error>> {
    let mut paths = support_paths.to_vec();
    paths.push(relative_path.to_string());
    paths
        .into_iter()
        .map(|path| {
            let content = std::fs::read_to_string(pilot_root.join(&path))?;
            Ok(SourceDocument::new(path.clone(), content))
        })
        .collect()
}

fn pilot_result_from_document(
    relative_path: &str,
    document: &PilotDiagnosticRunDocument,
) -> EngineCompileResult {
    let diagnostics = if document.diagnostics.is_empty() && document.exception_message.is_some() {
        vec![NormalizedCompileDiagnostic {
            stage: document
                .failure_stage
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            input_path: Some(relative_path.to_string()),
            line: None,
            column: None,
            message: document.exception_message.clone().unwrap_or_default(),
            problem_kind: classify_problem_kind(
                &document.exception_message.clone().unwrap_or_default(),
            ),
        }]
    } else {
        document
            .diagnostics
            .iter()
            .cloned()
            .map(|diagnostic| {
                let message = diagnostic
                    .message
                    .unwrap_or_else(|| "pilot compile error".to_string());
                NormalizedCompileDiagnostic {
                    stage: diagnostic.stage,
                    input_path: diagnostic.file,
                    line: diagnostic.line,
                    column: diagnostic.column,
                    problem_kind: classify_problem_kind(&message),
                    message,
                }
            })
            .collect()
    };

    EngineCompileResult {
        status: document.status.clone(),
        failure_stage: document.failure_stage.clone(),
        failure_input: diagnostics
            .first()
            .and_then(|diagnostic| diagnostic.input_path.clone()),
        diagnostics,
        timings: EngineTimings {
            total_ms: document.timings.total_ms,
            phases: document
                .timings
                .phases
                .clone()
                .into_iter()
                .map(|phase| PhaseTiming {
                    name: phase.name,
                    duration_ms: phase.duration_ms,
                })
                .collect(),
        },
    }
}

fn compare_results(
    mercurio: &EngineCompileResult,
    pilot: &EngineCompileResult,
) -> CompileErrorComparison {
    let mercurio_primary = mercurio.diagnostics.first();
    let pilot_primary = pilot.diagnostics.first();
    let both_pass = mercurio.status == "ok" && pilot.status == "ok";
    let both_fail = mercurio.status == "error" && pilot.status == "error";
    let failure_stage_match = mercurio.failure_stage == pilot.failure_stage;
    let primary_problem_match = mercurio_primary.map(|diag| diag.problem_kind.as_str())
        == pilot_primary.map(|diag| diag.problem_kind.as_str())
        && both_fail;

    CompileErrorComparison {
        status_match: mercurio.status == pilot.status,
        both_pass,
        both_fail,
        failure_stage_match,
        primary_problem_match,
        mercurio_primary_problem: mercurio_primary.map(|diag| diag.problem_kind.clone()),
        pilot_primary_problem: pilot_primary.map(|diag| diag.problem_kind.clone()),
    }
}

fn error_result(
    failure_stage: &str,
    failure_input: Option<String>,
    diagnostic: NormalizedCompileDiagnostic,
    start: Instant,
    phases: Vec<PhaseTiming>,
) -> EngineCompileResult {
    EngineCompileResult {
        status: "error".to_string(),
        failure_stage: Some(failure_stage.to_string()),
        failure_input,
        diagnostics: vec![diagnostic],
        timings: EngineTimings {
            total_ms: elapsed_ms(start),
            phases,
        },
    }
}

fn diagnostic_to_normalized(
    stage: &str,
    input_path: Option<&str>,
    diagnostic: &Diagnostic,
) -> NormalizedCompileDiagnostic {
    let (line, column) = diagnostic
        .span
        .as_ref()
        .map(|span| {
            (
                u32::try_from(span.start_line).ok(),
                u32::try_from(span.start_col).ok(),
            )
        })
        .unwrap_or((None, None));
    NormalizedCompileDiagnostic {
        stage: stage.to_string(),
        input_path: input_path.map(ToOwned::to_owned),
        line,
        column,
        problem_kind: classify_problem_kind(&diagnostic.message),
        message: diagnostic.message.clone(),
    }
}

fn classify_problem_kind(message: &str) -> String {
    let normalized = normalize_message(message);
    if normalized.contains("unresolved") || normalized.contains("could not resolve") {
        "unresolved_reference".to_string()
    } else if normalized.contains("expected `") || normalized.contains("expected a declaration") {
        "parse_expected_token".to_string()
    } else if normalized.contains("duplicate") {
        "duplicate_definition".to_string()
    } else if normalized.contains("import") {
        "import_error".to_string()
    } else if normalized.contains("transform") {
        "transform_error".to_string()
    } else {
        "other".to_string()
    }
}

fn normalize_message(message: &str) -> String {
    message
        .to_ascii_lowercase()
        .replace('\\', "/")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn run_java_diagnostics_exporter(
    pilot_runner: &PilotRunner,
    relative_path: &str,
    support_paths: &[String],
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let export_path = repo_path(&format!(
        "target/pilot_compile_diagnostics.{}.json",
        relative_path_slug(relative_path)
    ));
    if let Some(parent) = export_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let classes_dir = absolute_path(&pilot_runner.classes_dir)?;
    let interactive_jar = absolute_path(&pilot_runner.interactive_jar)?;
    let lib_dir = interactive_jar
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("lib")
        .to_path_buf();
    let separator = if cfg!(windows) { ";" } else { ":" };
    let classpath = format!(
        "{}{}{}{}{}",
        java_path_string(&classes_dir),
        separator,
        java_path_string(&interactive_jar),
        separator,
        java_path_string(&lib_dir.join("*"))
    );

    let mut input_paths = support_paths
        .iter()
        .map(|path| pilot_runner.pilot_root.join(path))
        .collect::<Vec<_>>();
    input_paths.push(pilot_runner.pilot_root.join(relative_path));

    let status = if cfg!(windows) {
        let script_path = repo_path("target/run_pilot_compile_diagnostics.ps1");
        let mut script = format!(
            "$cp = '{}'\njava -cp $cp dev.mercurio.pilot.PilotModelExporter --diagnostics '{}' '{}'",
            classpath.replace('\'', "''"),
            java_path_string(&pilot_runner.library_root).replace('\'', "''"),
            java_path_string(&export_path).replace('\'', "''"),
        );
        for input_path in &input_paths {
            script.push_str(&format!(
                " '{}'",
                java_path_string(input_path).replace('\'', "''")
            ));
        }
        script.push('\n');
        std::fs::write(&script_path, script)?;
        Command::new("powershell")
            .arg("-File")
            .arg(script_path)
            .status()?
    } else {
        let mut command = Command::new("java");
        command
            .arg("-cp")
            .arg(classpath)
            .arg("dev.mercurio.pilot.PilotModelExporter")
            .arg("--diagnostics")
            .arg(&pilot_runner.library_root)
            .arg(&export_path);
        for input_path in &input_paths {
            command.arg(input_path);
        }
        command.status()?
    };

    if !status.success() {
        return Err("failed to run Java pilot diagnostics exporter".into());
    }

    Ok(export_path)
}

fn export_diagnostics_corpus_from_pilot(
    pilot_runner: &PilotRunner,
    corpus_name: &str,
    relative_paths: &[String],
    corpus_seed: &PilotCorpusSeed,
) -> Result<BTreeMap<String, PilotDiagnosticRunDocument>, Box<dyn std::error::Error>> {
    let mut all_cases = BTreeMap::new();
    for (folder, folder_paths) in group_paths_by_folder(relative_paths) {
        let group_slug = format!(
            "{}.mf2.{}",
            corpus_name.replace(['\\', '/', ' '], "_"),
            relative_path_slug(&folder)
        );
        all_cases.extend(export_diagnostics_path_group_from_pilot(
            pilot_runner,
            &group_slug,
            &folder_paths,
            corpus_seed,
        )?);
    }
    Ok(all_cases)
}

fn export_diagnostics_path_group_from_pilot(
    pilot_runner: &PilotRunner,
    group_slug: &str,
    relative_paths: &[String],
    corpus_seed: &PilotCorpusSeed,
) -> Result<BTreeMap<String, PilotDiagnosticRunDocument>, Box<dyn std::error::Error>> {
    let export_path = repo_path(&format!(
        "target/pilot_compile_diagnostics.batch.{group_slug}.json"
    ));
    let spec_path = repo_path(&format!(
        "target/pilot_compile_diagnostics.batch.{group_slug}.spec.json"
    ));

    let spec = PilotCorpusSpec {
        cases: relative_paths
            .iter()
            .map(|relative_path| PilotCorpusSpecCase {
                relative_path,
                input_files: corpus_seed
                    .support_paths_for_case(&pilot_runner.pilot_root, relative_path)
                    .iter()
                    .map(|path| pilot_runner.pilot_root.join(path).display().to_string())
                    .chain(std::iter::once(
                        pilot_runner
                            .pilot_root
                            .join(relative_path)
                            .display()
                            .to_string(),
                    ))
                    .collect(),
            })
            .collect(),
    };
    let spec_json = serde_json::to_string_pretty(&spec)?;

    if let Some(cases) =
        load_existing_diagnostics_batch(&export_path, &spec_path, &spec_json, relative_paths)?
    {
        return Ok(cases);
    }

    if let Some(parent) = spec_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&spec_path, spec_json)?;

    run_java_diagnostics_exporter_batch(pilot_runner, &spec_path, &export_path)?;

    let document: PilotDiagnosticsBatchDocument =
        serde_json::from_str(&std::fs::read_to_string(&export_path)?)?;
    Ok(document
        .cases
        .into_iter()
        .map(|case| (case.relative_path, case.result))
        .collect())
}

fn group_paths_by_folder(relative_paths: &[String]) -> BTreeMap<String, Vec<String>> {
    let mut groups = BTreeMap::new();
    for relative_path in relative_paths {
        let folder = Path::new(relative_path)
            .parent()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        groups
            .entry(folder)
            .or_insert_with(Vec::new)
            .push(relative_path.clone());
    }
    groups
}

fn load_existing_diagnostics_batch(
    export_path: &Path,
    spec_path: &Path,
    expected_spec_json: &str,
    relative_paths: &[String],
) -> Result<Option<BTreeMap<String, PilotDiagnosticRunDocument>>, Box<dyn std::error::Error>> {
    if !export_path.exists() || !spec_path.exists() {
        return Ok(None);
    }
    if std::fs::read_to_string(spec_path)? != expected_spec_json {
        return Ok(None);
    }

    let document: PilotDiagnosticsBatchDocument =
        serde_json::from_str(&std::fs::read_to_string(export_path)?)?;
    if document.cases.len() != relative_paths.len() {
        return Ok(None);
    }

    let cases = document
        .cases
        .into_iter()
        .map(|case| (case.relative_path, case.result))
        .collect::<BTreeMap<_, _>>();
    if relative_paths.iter().all(|path| cases.contains_key(path)) {
        Ok(Some(cases))
    } else {
        Ok(None)
    }
}

fn run_java_diagnostics_exporter_batch(
    pilot_runner: &PilotRunner,
    spec_path: &Path,
    export_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = export_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let classes_dir = absolute_path(&pilot_runner.classes_dir)?;
    let interactive_jar = absolute_path(&pilot_runner.interactive_jar)?;
    let spec_path = absolute_path(spec_path)?;
    let export_path = absolute_path(export_path)?;
    let lib_dir = interactive_jar
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("lib")
        .to_path_buf();
    let separator = if cfg!(windows) { ";" } else { ":" };
    let classpath = format!(
        "{}{}{}{}{}",
        java_path_string(&classes_dir),
        separator,
        java_path_string(&interactive_jar),
        separator,
        java_path_string(&lib_dir.join("*"))
    );

    let status = if cfg!(windows) {
        let script_path = repo_path("target/run_pilot_compile_diagnostics_batch.ps1");
        let script = format!(
            "$cp = '{}'\njava -cp $cp dev.mercurio.pilot.PilotModelExporter --diagnostics-batch '{}' '{}' '{}'\n",
            classpath.replace('\'', "''"),
            java_path_string(&pilot_runner.library_root).replace('\'', "''"),
            java_path_string(&spec_path).replace('\'', "''"),
            java_path_string(&export_path).replace('\'', "''"),
        );
        std::fs::write(&script_path, script)?;
        Command::new("powershell")
            .arg("-File")
            .arg(script_path)
            .status()?
    } else {
        Command::new("java")
            .arg("-cp")
            .arg(classpath)
            .arg("dev.mercurio.pilot.PilotModelExporter")
            .arg("--diagnostics-batch")
            .arg(&pilot_runner.library_root)
            .arg(&spec_path)
            .arg(&export_path)
            .status()?
    };

    if !status.success() {
        return Err("failed to run Java pilot diagnostics batch exporter".into());
    }

    Ok(())
}

fn relative_path_slug(relative_path: &str) -> String {
    relative_path
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

fn find_interactive_jar(pilot_root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let target_dir = pilot_root.join("org.omg.sysml.interactive/target");
    let mut jars = std::fs::read_dir(&target_dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| {
                    name.starts_with("org.omg.sysml.interactive-") && name.ends_with("-all.jar")
                })
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    jars.sort();

    jars.into_iter().last().ok_or_else(|| {
        format!(
            "could not find org.omg.sysml.interactive-*-all.jar under {}",
            target_dir.display()
        )
        .into()
    })
}

fn compile_java_exporter(
    interactive_jar: &Path,
    java_source: &Path,
    classes_dir: &Path,
    class_file_relative: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let class_file = classes_dir.join(class_file_relative);
    let should_compile = match (
        std::fs::metadata(java_source),
        std::fs::metadata(&class_file),
    ) {
        (Ok(source), Ok(class)) => source.modified()? > class.modified()?,
        _ => true,
    };

    if !should_compile {
        return Ok(());
    }

    std::fs::create_dir_all(classes_dir)?;
    let status = Command::new("javac")
        .arg("-cp")
        .arg(interactive_jar)
        .arg("-d")
        .arg(classes_dir)
        .arg(java_source)
        .status()?;

    if !status.success() {
        return Err("failed to compile Java pilot exporter".into());
    }

    Ok(())
}

fn absolute_path(path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(env::current_dir()?.join(path))
}

fn java_path_string(path: &Path) -> String {
    path.display().to_string().replace("\\\\?\\", "")
}

fn now_utc_rfc3339() -> Result<String, Box<dyn std::error::Error>> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}
