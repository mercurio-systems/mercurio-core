use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use mercurio_core::source_set::{
    SourceCompileContext, SourceDocument, compile_source_document_with_context,
};
use mercurio_core::{
    Graph, KirDocument, MetamodelAttributeRegistry, PilotExportDocument, SnapshotMode,
    build_semantic_snapshot, build_semantic_snapshot_with_registry, compare_snapshots,
    default_stdlib_path, load_pilot_export, normalize_pilot_export_for_compare, repo_path,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const DEFAULT_PILOT_ROOT: &str = "../SysML-v2-Pilot-Implementation";
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args()?;
    let corpus_seed = PilotCorpusSeed::load()?;
    match &args.relative_path {
        Some(relative_path) => {
            let output = run_compare_case(&args.pilot_root, relative_path, &corpus_seed)?;

            if let Some(parent) = args.output_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&args.output_path, serde_json::to_string_pretty(&output)?)?;

            println!("pilot semantic comparison");
            println!("  file: {}", relative_path);
            println!("  support files: {}", output.support_paths.len());
            println!("  output: {}", args.output_path.display());
            println!("  mercurio elements: {}", output.report.mercurio_count);
            println!("  pilot elements: {}", output.report.pilot_count);
            println!("  exact matches: {}", output.report.exact_match_count);
            println!("  mismatches: {}", output.report.mismatches.len());
            println!("  mercurio only: {}", output.report.mercurio_only.len());
            println!("  pilot only: {}", output.report.pilot_only.len());
            println!("  mercurio total ms: {}", output.timings.mercurio.total_ms);
            println!("  pilot total ms: {}", output.timings.pilot.total_ms);
            println!("  compare ms: {}", output.timings.compare_ms);
        }
        None => {
            let (corpus_name, relative_paths) = args.corpus_paths(&corpus_seed)?;
            let (pilot_cases, shared_timings) = export_corpus_from_pilot(
                &args.pilot_root,
                &corpus_name,
                &relative_paths,
                &corpus_seed,
            )?;
            let mut cases = Vec::new();

            for relative_path in &relative_paths {
                match run_compare_case_from_batch(
                    &args.pilot_root,
                    relative_path,
                    &corpus_seed,
                    &pilot_cases,
                ) {
                    Ok(output) => {
                        println!(
                            "timed {}: rust={}ms pilot={}ms mismatches={}",
                            relative_path,
                            output.timings.mercurio.total_ms,
                            output.timings.pilot.total_ms,
                            output.report.mismatches.len()
                        );
                        cases.push(CorpusCaseSummary::from_compare_output(output));
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
                shared_timings,
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

            println!("pilot semantic corpus timing");
            println!("  corpus: {}", corpus_output.corpus_name);
            println!("  cases: {}", corpus_output.case_count);
            println!("  output: {}", args.output_path.display());
            println!(
                "  exact cases: {}",
                corpus_output.aggregate.exact_match_cases
            );
            println!("  failed cases: {}", corpus_output.aggregate.failed_cases);
            println!(
                "  total mismatches: {}",
                corpus_output.aggregate.total_mismatches
            );
            println!(
                "  rust total ms: {} (avg {} median {})",
                corpus_output.aggregate.mercurio.total_ms,
                corpus_output.aggregate.mercurio.avg_ms,
                corpus_output.aggregate.mercurio.median_ms
            );
            println!(
                "  pilot total ms: {} (avg {} median {})",
                corpus_output.aggregate.pilot.total_ms,
                corpus_output.aggregate.pilot.avg_ms,
                corpus_output.aggregate.pilot.median_ms
            );
            if let Some(shared_timings) = &corpus_output.shared_timings {
                println!("  pilot shared setup ms: {}", shared_timings.pilot.total_ms);
            }
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

#[derive(Debug, Serialize)]
struct CompareOutput {
    generated_at_utc: String,
    pilot_root: String,
    relative_path: String,
    support_paths: Vec<String>,
    pilot_export_path: String,
    timings: CompareTimings,
    mercurio_snapshot: mercurio_core::SemanticSnapshot,
    pilot_snapshot: mercurio_core::SemanticSnapshot,
    report: mercurio_core::SemanticComparisonReport,
}

#[derive(Debug, Serialize)]
struct CorpusCompareOutput {
    generated_at_utc: String,
    pilot_root: String,
    corpus_name: String,
    case_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    shared_timings: Option<CorpusSharedTimings>,
    aggregate: CorpusAggregate,
    cases: Vec<CorpusCaseSummary>,
}

#[derive(Debug, Serialize)]
struct CorpusCaseSummary {
    relative_path: String,
    support_file_count: usize,
    status: String,
    exact: bool,
    error: Option<String>,
    mismatches: usize,
    mercurio_only: usize,
    pilot_only: usize,
    mercurio_elements: Option<usize>,
    pilot_elements: Option<usize>,
    timings: Option<CompareTimings>,
}

#[derive(Debug, Serialize)]
struct CorpusAggregate {
    exact_match_cases: usize,
    failed_cases: usize,
    total_mismatches: usize,
    total_mercurio_only: usize,
    total_pilot_only: usize,
    mercurio: AggregateTiming,
    pilot: AggregateTiming,
    compare: AggregateTiming,
}

#[derive(Debug, Serialize)]
struct CorpusSharedTimings {
    pilot: EngineTimings,
}

#[derive(Debug, Serialize)]
struct AggregateTiming {
    total_ms: u64,
    avg_ms: u64,
    median_ms: u64,
    min_ms: u64,
    max_ms: u64,
}

#[derive(Debug, Serialize)]
struct CompareTimings {
    note: String,
    mercurio: EngineTimings,
    pilot: EngineTimings,
    compare_ms: u64,
}

#[derive(Debug, Serialize)]
struct EngineTimings {
    total_ms: u64,
    phases: Vec<PhaseTiming>,
}

#[derive(Debug, Serialize)]
struct PhaseTiming {
    name: String,
    duration_ms: u64,
}

#[derive(Debug)]
struct MercurioCaseResult {
    support_paths: Vec<String>,
    metamodel_registry: MetamodelAttributeRegistry,
    snapshot: mercurio_core::SemanticSnapshot,
    timings: EngineTimings,
}

#[derive(Debug, Deserialize)]
struct PilotBatchExportDocument {
    metadata: PilotBatchExportMetadata,
    cases: Vec<PilotBatchExportCase>,
}

#[derive(Debug, Deserialize)]
struct PilotBatchExportMetadata {
    setup_timings: PilotBatchSetupTimings,
}

#[derive(Debug, Deserialize)]
struct PilotBatchSetupTimings {
    total_ms: u64,
    phases: Vec<PilotBatchPhaseTiming>,
}

#[derive(Debug, Deserialize)]
struct PilotBatchPhaseTiming {
    name: String,
    duration_ms: u64,
}

#[derive(Debug, Deserialize)]
struct PilotBatchExportCase {
    relative_path: String,
    export_ms: u64,
    document: PilotExportDocument,
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
            "mappings/l2/pilot_corpus.seed.json",
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

fn parse_args() -> Result<Args, Box<dyn std::error::Error>> {
    let mut pilot_root = PathBuf::from(DEFAULT_PILOT_ROOT);
    let mut relative_path = None;
    let mut corpus_name = None;
    let mut paths_file = None;
    let mut output_path = repo_path("target/pilot_semantic_compare.json");
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
        "Usage: cargo run -p mercurio-tools --bin compare_pilot_semantics -- (--relative-path PATH | --corpus NAME|all | --paths-file PATH) [--pilot-root PATH] [--out PATH]"
    );
}

fn run_compare_case(
    pilot_root: &Path,
    relative_path: &str,
    corpus_seed: &PilotCorpusSeed,
) -> Result<CompareOutput, Box<dyn std::error::Error>> {
    let mercurio = build_mercurio_case(pilot_root, relative_path, corpus_seed)?;
    let support_paths = mercurio.support_paths.clone();

    let pilot_start = Instant::now();
    let pilot_export_start = Instant::now();
    let pilot_export_path = export_model_from_pilot(pilot_root, relative_path, &support_paths)?;
    let pilot_export_ms = elapsed_ms(pilot_export_start);

    let pilot_load_start = Instant::now();
    let pilot_export: PilotExportDocument = load_pilot_export(&pilot_export_path)?;
    let pilot_load_ms = elapsed_ms(pilot_load_start);

    let pilot_snapshot_start = Instant::now();
    let pilot_snapshot = build_semantic_snapshot_with_registry(
        normalize_pilot_export_for_compare(pilot_export)?,
        relative_path,
        SnapshotMode::Pilot,
        &mercurio.metamodel_registry,
    )?;
    let pilot_snapshot_ms = elapsed_ms(pilot_snapshot_start);
    let pilot_total_ms = elapsed_ms(pilot_start);

    build_compare_output(
        pilot_root,
        relative_path,
        support_paths,
        pilot_export_path.display().to_string(),
        mercurio,
        pilot_snapshot,
        EngineTimings {
            total_ms: pilot_total_ms,
            phases: vec![
                PhaseTiming {
                    name: "java_export_model".to_string(),
                    duration_ms: pilot_export_ms,
                },
                PhaseTiming {
                    name: "load_export_json".to_string(),
                    duration_ms: pilot_load_ms,
                },
                PhaseTiming {
                    name: "normalize_and_snapshot".to_string(),
                    duration_ms: pilot_snapshot_ms,
                },
            ],
        },
        "Rust timings currently include loading prebuilt stdlib KIR JSON plus L2 compile/snapshot. Pilot timings currently include Java exporter wall-clock time for loading source libraries plus L2 export/snapshot.".to_string(),
    )
}

fn run_compare_case_from_batch(
    pilot_root: &Path,
    relative_path: &str,
    corpus_seed: &PilotCorpusSeed,
    pilot_cases: &BTreeMap<String, PilotBatchExportCase>,
) -> Result<CompareOutput, Box<dyn std::error::Error>> {
    let mercurio = build_mercurio_case(pilot_root, relative_path, corpus_seed)?;
    let support_paths = mercurio.support_paths.clone();
    let pilot_case = pilot_cases
        .get(relative_path)
        .ok_or_else(|| format!("pilot batch export missing case `{relative_path}`"))?;

    let pilot_snapshot_start = Instant::now();
    let pilot_snapshot = build_semantic_snapshot_with_registry(
        normalize_pilot_export_for_compare(pilot_case.document.clone())?,
        relative_path,
        SnapshotMode::Pilot,
        &mercurio.metamodel_registry,
    )?;
    let pilot_snapshot_ms = elapsed_ms(pilot_snapshot_start);

    build_compare_output(
        pilot_root,
        relative_path,
        support_paths,
        repo_path("target/pilot_model_export.compare.batch.json")
            .display()
            .to_string(),
        mercurio,
        pilot_snapshot,
        EngineTimings {
            total_ms: pilot_case.export_ms + pilot_snapshot_ms,
            phases: vec![
                PhaseTiming {
                    name: "export_case_in_batch".to_string(),
                    duration_ms: pilot_case.export_ms,
                },
                PhaseTiming {
                    name: "normalize_and_snapshot".to_string(),
                    duration_ms: pilot_snapshot_ms,
                },
            ],
        },
        "Rust timings currently include loading prebuilt stdlib KIR JSON plus L2 compile/snapshot. Pilot corpus timings exclude shared batch setup and JSON load; see shared_timings for those costs.".to_string(),
    )
}

fn build_mercurio_case(
    pilot_root: &Path,
    relative_path: &str,
    corpus_seed: &PilotCorpusSeed,
) -> Result<MercurioCaseResult, Box<dyn std::error::Error>> {
    let support_paths = corpus_seed.support_paths_for_case(pilot_root, relative_path);
    let mercurio_start = Instant::now();
    let load_stdlib_start = Instant::now();
    let stdlib = KirDocument::from_path(&default_stdlib_path())?;
    let load_stdlib_ms = elapsed_ms(load_stdlib_start);

    let read_parse_start = Instant::now();
    let source_documents = read_source_documents(pilot_root, &support_paths, relative_path)?;
    let read_parse_ms = elapsed_ms(read_parse_start);

    let context_start = Instant::now();
    let compile_context = SourceCompileContext::from_source_documents(&source_documents, &stdlib)?;
    let context_ms = elapsed_ms(context_start);

    let target_document = source_documents
        .iter()
        .find(|file| file.path == relative_path)
        .ok_or_else(|| format!("source set missing target `{relative_path}`"))?;

    let compile_start = Instant::now();
    let mut source_kir = Vec::new();
    source_kir.push(compile_source_document_with_context(
        target_document,
        &compile_context,
        &stdlib,
    )?);
    for file in source_documents
        .iter()
        .filter(|file| file.path != relative_path)
    {
        if let Ok(document) = compile_source_document_with_context(file, &compile_context, &stdlib)
        {
            source_kir.push(document);
        }
    }
    let source_document = KirDocument::merge(source_kir)?;
    let compile_ms = elapsed_ms(compile_start);

    let merged_document = KirDocument::merge([stdlib, source_document])?;
    let metamodel_registry =
        MetamodelAttributeRegistry::build(&Graph::from_document(merged_document.clone())?);

    let snapshot_start = Instant::now();
    let snapshot = build_semantic_snapshot(merged_document, relative_path, SnapshotMode::Mercurio)?;
    let snapshot_ms = elapsed_ms(snapshot_start);

    Ok(MercurioCaseResult {
        support_paths,
        metamodel_registry,
        snapshot,
        timings: EngineTimings {
            total_ms: elapsed_ms(mercurio_start),
            phases: vec![
                PhaseTiming {
                    name: "load_stdlib_json".to_string(),
                    duration_ms: load_stdlib_ms,
                },
                PhaseTiming {
                    name: "read_parse_source_set".to_string(),
                    duration_ms: read_parse_ms,
                },
                PhaseTiming {
                    name: "build_resolver_context".to_string(),
                    duration_ms: context_ms,
                },
                PhaseTiming {
                    name: "compile_source_set".to_string(),
                    duration_ms: compile_ms,
                },
                PhaseTiming {
                    name: "merge_and_snapshot".to_string(),
                    duration_ms: snapshot_ms,
                },
            ],
        },
    })
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

fn build_compare_output(
    pilot_root: &Path,
    relative_path: &str,
    support_paths: Vec<String>,
    pilot_export_path: String,
    mercurio: MercurioCaseResult,
    pilot_snapshot: mercurio_core::SemanticSnapshot,
    pilot_timings: EngineTimings,
    note: String,
) -> Result<CompareOutput, Box<dyn std::error::Error>> {
    let compare_start = Instant::now();
    let report = compare_snapshots(mercurio.snapshot.clone(), pilot_snapshot.clone())?;
    let compare_ms = elapsed_ms(compare_start);

    Ok(CompareOutput {
        generated_at_utc: now_utc_rfc3339()?,
        pilot_root: pilot_root.display().to_string(),
        relative_path: relative_path.to_string(),
        support_paths,
        pilot_export_path,
        timings: CompareTimings {
            note,
            mercurio: mercurio.timings,
            pilot: pilot_timings,
            compare_ms,
        },
        mercurio_snapshot: mercurio.snapshot,
        pilot_snapshot,
        report,
    })
}

impl CorpusCaseSummary {
    fn from_compare_output(output: CompareOutput) -> Self {
        Self {
            support_file_count: output.support_paths.len(),
            status: "ok".to_string(),
            exact: output.report.mismatches.is_empty()
                && output.report.mercurio_only.is_empty()
                && output.report.pilot_only.is_empty(),
            error: None,
            mismatches: output.report.mismatches.len(),
            mercurio_only: output.report.mercurio_only.len(),
            pilot_only: output.report.pilot_only.len(),
            mercurio_elements: Some(output.report.mercurio_count),
            pilot_elements: Some(output.report.pilot_count),
            relative_path: output.relative_path,
            timings: Some(output.timings),
        }
    }

    fn failure(relative_path: String, support_file_count: usize, error: String) -> Self {
        Self {
            relative_path,
            support_file_count,
            status: "error".to_string(),
            exact: false,
            error: Some(error),
            mismatches: 0,
            mercurio_only: 0,
            pilot_only: 0,
            mercurio_elements: None,
            pilot_elements: None,
            timings: None,
        }
    }
}

impl CorpusAggregate {
    fn from_cases(cases: &[CorpusCaseSummary]) -> Self {
        let mercurio = aggregate_timing(cases.iter().filter_map(|case| {
            case.timings
                .as_ref()
                .map(|timings| timings.mercurio.total_ms)
        }));
        let pilot = aggregate_timing(
            cases
                .iter()
                .filter_map(|case| case.timings.as_ref().map(|timings| timings.pilot.total_ms)),
        );
        let compare = aggregate_timing(
            cases
                .iter()
                .filter_map(|case| case.timings.as_ref().map(|timings| timings.compare_ms)),
        );

        Self {
            exact_match_cases: cases.iter().filter(|case| case.exact).count(),
            failed_cases: cases.iter().filter(|case| case.status == "error").count(),
            total_mismatches: cases.iter().map(|case| case.mismatches).sum(),
            total_mercurio_only: cases.iter().map(|case| case.mercurio_only).sum(),
            total_pilot_only: cases.iter().map(|case| case.pilot_only).sum(),
            mercurio,
            pilot,
            compare,
        }
    }
}

fn aggregate_timing(values: impl IntoIterator<Item = u64>) -> AggregateTiming {
    let mut values = values.into_iter().collect::<Vec<_>>();
    if values.is_empty() {
        return AggregateTiming {
            total_ms: 0,
            avg_ms: 0,
            median_ms: 0,
            min_ms: 0,
            max_ms: 0,
        };
    }

    values.sort_unstable();
    let total_ms = values.iter().sum::<u64>();
    let avg_ms = total_ms / values.len() as u64;
    let median_ms = values[values.len() / 2];
    let min_ms = *values.first().unwrap_or(&0);
    let max_ms = *values.last().unwrap_or(&0);

    AggregateTiming {
        total_ms,
        avg_ms,
        median_ms,
        min_ms,
        max_ms,
    }
}

fn export_model_from_pilot(
    pilot_root: &Path,
    relative_path: &str,
    support_paths: &[String],
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let pilot_root = pilot_root.canonicalize()?;
    let library_root = pilot_root.join("sysml.library");
    let interactive_jar = find_interactive_jar(&pilot_root)?;
    let classes_dir = repo_path("target/pilot-exporter-classes");
    let java_source =
        repo_path("tools/pilot-exporter/src/main/java/dev/mercurio/pilot/PilotModelExporter.java");
    let export_path = repo_path(&format!(
        "target/pilot_model_export.compare.{}.json",
        relative_path_slug(relative_path)
    ));

    compile_java_exporter(
        &interactive_jar,
        &java_source,
        &classes_dir,
        "dev/mercurio/pilot/PilotModelExporter.class",
    )?;

    let mut input_paths = support_paths
        .iter()
        .map(|path| pilot_root.join(path))
        .collect::<Vec<_>>();
    input_paths.push(pilot_root.join(relative_path));

    run_java_exporter(
        &interactive_jar,
        &classes_dir,
        "dev.mercurio.pilot.PilotModelExporter",
        &library_root,
        &export_path,
        &input_paths,
    )?;
    Ok(export_path)
}

fn relative_path_slug(relative_path: &str) -> String {
    relative_path
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

fn export_corpus_from_pilot(
    pilot_root: &Path,
    corpus_name: &str,
    relative_paths: &[String],
    corpus_seed: &PilotCorpusSeed,
) -> Result<
    (
        BTreeMap<String, PilotBatchExportCase>,
        Option<CorpusSharedTimings>,
    ),
    Box<dyn std::error::Error>,
> {
    let mut all_cases = BTreeMap::new();
    let mut total_shared_ms = 0;
    let mut shared_phases = Vec::new();

    for (folder, folder_paths) in group_paths_by_folder(relative_paths) {
        let group_slug = format!(
            "{}.{}",
            corpus_name.replace(['\\', '/', ' '], "_"),
            relative_path_slug(&folder)
        );
        let (cases, timings) =
            export_corpus_group_from_pilot(pilot_root, &group_slug, &folder_paths, corpus_seed)?;
        if let Some(timings) = timings {
            total_shared_ms += timings.pilot.total_ms;
            shared_phases.push(PhaseTiming {
                name: format!("folder_batch:{folder}"),
                duration_ms: timings.pilot.total_ms,
            });
        }
        all_cases.extend(cases);
    }

    let shared_timings = CorpusSharedTimings {
        pilot: EngineTimings {
            total_ms: total_shared_ms,
            phases: shared_phases,
        },
    };

    Ok((all_cases, Some(shared_timings)))
}

fn export_corpus_group_from_pilot(
    pilot_root: &Path,
    group_slug: &str,
    relative_paths: &[String],
    corpus_seed: &PilotCorpusSeed,
) -> Result<
    (
        BTreeMap<String, PilotBatchExportCase>,
        Option<CorpusSharedTimings>,
    ),
    Box<dyn std::error::Error>,
> {
    let pilot_root = pilot_root.canonicalize()?;
    let library_root = pilot_root.join("sysml.library");
    let interactive_jar = find_interactive_jar(&pilot_root)?;
    let classes_dir = repo_path("target/pilot-exporter-classes");
    let java_source =
        repo_path("tools/pilot-exporter/src/main/java/dev/mercurio/pilot/PilotModelExporter.java");
    let export_path = repo_path(&format!(
        "target/pilot_model_export.compare.batch.{group_slug}.json"
    ));
    let spec_path = repo_path(&format!(
        "target/pilot_model_export.compare.batch.{group_slug}.spec.json"
    ));

    compile_java_exporter(
        &interactive_jar,
        &java_source,
        &classes_dir,
        "dev/mercurio/pilot/PilotModelExporter.class",
    )?;

    let spec = PilotCorpusSpec {
        cases: relative_paths
            .iter()
            .map(|relative_path| PilotCorpusSpecCase {
                relative_path,
                input_files: corpus_seed
                    .support_paths_for_case(&pilot_root, relative_path)
                    .iter()
                    .map(|path| pilot_root.join(path).display().to_string())
                    .chain(std::iter::once(
                        pilot_root.join(relative_path).display().to_string(),
                    ))
                    .collect(),
            })
            .collect(),
    };

    if let Some(parent) = spec_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&spec_path, serde_json::to_string_pretty(&spec)?)?;

    run_java_exporter_batch(
        &interactive_jar,
        &classes_dir,
        "dev.mercurio.pilot.PilotModelExporter",
        &library_root,
        &spec_path,
        &export_path,
    )?;

    let batch_load_start = Instant::now();
    let batch_export: PilotBatchExportDocument =
        serde_json::from_str(&std::fs::read_to_string(&export_path)?)?;
    let batch_load_ms = elapsed_ms(batch_load_start);

    let shared_timings = CorpusSharedTimings {
        pilot: EngineTimings {
            total_ms: batch_export.metadata.setup_timings.total_ms + batch_load_ms,
            phases: batch_export
                .metadata
                .setup_timings
                .phases
                .into_iter()
                .map(|phase| PhaseTiming {
                    name: phase.name,
                    duration_ms: phase.duration_ms,
                })
                .chain(std::iter::once(PhaseTiming {
                    name: "load_batch_export_json".to_string(),
                    duration_ms: batch_load_ms,
                }))
                .collect(),
        },
    };

    let cases = batch_export
        .cases
        .into_iter()
        .map(|case| (case.relative_path.clone(), case))
        .collect();

    Ok((cases, Some(shared_timings)))
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

fn run_java_exporter(
    interactive_jar: &Path,
    classes_dir: &Path,
    main_class: &str,
    library_root: &Path,
    export_path: &Path,
    input_paths: &[PathBuf],
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = export_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let classes_dir = absolute_path(classes_dir)?;
    let interactive_jar = absolute_path(interactive_jar)?;
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
        let script_path = repo_path("target/run_pilot_model_exporter.ps1");
        let mut script = format!(
            "$cp = '{}'\njava -cp $cp {} '{}' '{}'",
            classpath.replace('\'', "''"),
            main_class,
            java_path_string(library_root).replace('\'', "''"),
            java_path_string(export_path).replace('\'', "''"),
        );
        for input_path in input_paths {
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
            .arg(main_class)
            .arg(library_root)
            .arg(export_path);
        for input_path in input_paths {
            command.arg(input_path);
        }
        command.status()?
    };

    if !status.success() {
        return Err("failed to run Java pilot exporter".into());
    }

    Ok(())
}

fn run_java_exporter_batch(
    interactive_jar: &Path,
    classes_dir: &Path,
    main_class: &str,
    library_root: &Path,
    spec_path: &Path,
    export_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = export_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let classes_dir = absolute_path(classes_dir)?;
    let interactive_jar = absolute_path(interactive_jar)?;
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
        let script_path = repo_path("target/run_pilot_model_exporter_batch.ps1");
        let script = format!(
            "$cp = '{}'\njava -cp $cp {} --batch-spec '{}' '{}' '{}'\n",
            classpath.replace('\'', "''"),
            main_class,
            java_path_string(library_root).replace('\'', "''"),
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
            .arg(main_class)
            .arg("--batch-spec")
            .arg(library_root)
            .arg(spec_path)
            .arg(export_path)
            .status()?
    };

    if !status.success() {
        return Err("failed to run Java pilot batch exporter".into());
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
