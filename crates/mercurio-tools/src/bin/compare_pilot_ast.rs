use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use mercurio_core::frontend::sysml::parse_sysml;
use mercurio_core::{
    SyntaxComparisonReport, SyntaxSnapshot, SyntaxSnapshotNode, SyntaxSourceSpan,
    build_rust_syntax_snapshot, compare_syntax_snapshots, repo_path,
};
use mercurio_tools::default_pilot_root;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args()?;
    let corpus_seed = PilotCorpusSeed::load()?;
    match (&args.relative_path, &args.corpus_name) {
        (Some(relative_path), None) => {
            let output = run_compare_case(&args.pilot_root, relative_path, &corpus_seed)?;
            if let Some(parent) = args.output_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&args.output_path, serde_json::to_string_pretty(&output)?)?;

            println!("pilot syntax comparison");
            println!("  file: {}", relative_path);
            println!("  output: {}", args.output_path.display());
            println!("  rust nodes: {}", output.report.rust_count);
            println!("  pilot nodes: {}", output.report.pilot_count);
            println!("  exact matches: {}", output.report.exact_match_count);
            println!("  mismatches: {}", output.report.mismatches.len());
            println!("  rust only: {}", output.report.rust_only.len());
            println!("  pilot only: {}", output.report.pilot_only.len());
            println!("  rust total ms: {}", output.timings.rust.total_ms);
            println!("  pilot total ms: {}", output.timings.pilot.total_ms);
        }
        (None, Some(corpus_name)) => {
            let relative_paths = corpus_seed
                .corpus(corpus_name)
                .ok_or_else(|| format!("unknown corpus `{corpus_name}`"))?;
            let mut cases = Vec::new();

            for relative_path in relative_paths {
                match run_compare_case(&args.pilot_root, relative_path, &corpus_seed) {
                    Ok(output) => {
                        println!(
                            "compared {}: mismatches={} rust_only={} pilot_only={}",
                            relative_path,
                            output.report.mismatches.len(),
                            output.report.rust_only.len(),
                            output.report.pilot_only.len()
                        );
                        cases.push(CorpusCaseSummary::from_output(output));
                    }
                    Err(err) => {
                        println!("failed {}: {}", relative_path, err);
                        cases.push(CorpusCaseSummary::failure(
                            relative_path.to_string(),
                            corpus_seed.support_paths_for(relative_path).len(),
                            err.to_string(),
                        ));
                    }
                }
            }

            let corpus_output = CorpusCompareOutput {
                generated_at_utc: now_utc_rfc3339()?,
                pilot_root: args.pilot_root.display().to_string(),
                corpus_name: corpus_name.clone(),
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

            println!("pilot syntax corpus comparison");
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
        }
        _ => unreachable!(),
    }
    Ok(())
}

#[derive(Debug)]
struct Args {
    pilot_root: PathBuf,
    relative_path: Option<String>,
    corpus_name: Option<String>,
    output_path: PathBuf,
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

    fn corpus(&self, name: &str) -> Option<&[String]> {
        self.corpora.get(name).map(Vec::as_slice)
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
    rust_snapshot: SyntaxSnapshot,
    pilot_snapshot: SyntaxSnapshot,
    report: SyntaxComparisonReport,
}

#[derive(Debug, Serialize)]
struct CompareTimings {
    rust: EngineTimings,
    pilot: EngineTimings,
    compare_ms: u64,
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
    exact: bool,
    mismatches: usize,
    rust_only: usize,
    pilot_only: usize,
    timings: Option<CompareTimings>,
}

#[derive(Debug, Serialize)]
struct CorpusAggregate {
    exact_match_cases: usize,
    failed_cases: usize,
    total_mismatches: usize,
    total_rust_only: usize,
    total_pilot_only: usize,
}

impl CorpusCaseSummary {
    fn from_output(output: CompareOutput) -> Self {
        Self {
            relative_path: output.relative_path,
            support_file_count: output.support_paths.len(),
            status: "ok".to_string(),
            error: None,
            exact: output.report.mismatches.is_empty()
                && output.report.rust_only.is_empty()
                && output.report.pilot_only.is_empty(),
            mismatches: output.report.mismatches.len(),
            rust_only: output.report.rust_only.len(),
            pilot_only: output.report.pilot_only.len(),
            timings: Some(output.timings),
        }
    }

    fn failure(relative_path: String, support_file_count: usize, error: String) -> Self {
        Self {
            relative_path,
            support_file_count,
            status: "error".to_string(),
            error: Some(error),
            exact: false,
            mismatches: 0,
            rust_only: 0,
            pilot_only: 0,
            timings: None,
        }
    }
}

impl CorpusAggregate {
    fn from_cases(cases: &[CorpusCaseSummary]) -> Self {
        Self {
            exact_match_cases: cases.iter().filter(|case| case.exact).count(),
            failed_cases: cases.iter().filter(|case| case.status == "error").count(),
            total_mismatches: cases.iter().map(|case| case.mismatches).sum(),
            total_rust_only: cases.iter().map(|case| case.rust_only).sum(),
            total_pilot_only: cases.iter().map(|case| case.pilot_only).sum(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct PilotSyntaxSnapshotDocument {
    root_kind: String,
    nodes: Vec<PilotSyntaxNode>,
}

#[derive(Debug, Deserialize)]
struct PilotSyntaxNode {
    path: String,
    family: String,
    kind: String,
    keyword: String,
    declared_name: Option<String>,
    source_file: Option<String>,
    span: PilotSyntaxSpan,
    #[serde(default)]
    properties: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct PilotSyntaxSpan {
    start_line: usize,
    start_col: usize,
    end_line: usize,
    end_col: usize,
}

fn parse_args() -> Result<Args, Box<dyn std::error::Error>> {
    let mut pilot_root = default_pilot_root();
    let mut relative_path = None;
    let mut corpus_name = None;
    let mut output_path = repo_path("target/pilot_ast_compare.json");
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

    if relative_path.is_some() == corpus_name.is_some() {
        return Err("provide exactly one of --relative-path or --corpus".into());
    }

    Ok(Args {
        pilot_root,
        relative_path,
        corpus_name,
        output_path,
    })
}

fn print_usage() {
    println!(
        "Usage: compare_pilot_ast --pilot-root <pilot-root> (--relative-path <path> | --corpus <name>) [--out <path>]"
    );
}

fn run_compare_case(
    pilot_root: &Path,
    relative_path: &str,
    corpus_seed: &PilotCorpusSeed,
) -> Result<CompareOutput, Box<dyn std::error::Error>> {
    let support_paths = corpus_seed.support_paths_for(relative_path).to_vec();

    let rust_start = Instant::now();
    let read_start = Instant::now();
    let source_text = std::fs::read_to_string(pilot_root.join(relative_path))?;
    let read_ms = elapsed_ms(read_start);
    let parse_start = Instant::now();
    let module = parse_sysml(&source_text)?;
    let parse_ms = elapsed_ms(parse_start);
    let snapshot_start = Instant::now();
    let rust_snapshot = normalize_rust_syntax_snapshot(build_rust_syntax_snapshot(&module));
    let snapshot_ms = elapsed_ms(snapshot_start);
    let rust_timings = EngineTimings {
        total_ms: elapsed_ms(rust_start),
        phases: vec![
            PhaseTiming {
                name: "read_source".to_string(),
                duration_ms: read_ms,
            },
            PhaseTiming {
                name: "parse_sysml".to_string(),
                duration_ms: parse_ms,
            },
            PhaseTiming {
                name: "build_snapshot".to_string(),
                duration_ms: snapshot_ms,
            },
        ],
    };

    let pilot_start = Instant::now();
    let export_start = Instant::now();
    let pilot_export_path = export_syntax_from_pilot(pilot_root, relative_path, &support_paths)?;
    let export_ms = elapsed_ms(export_start);
    let load_start = Instant::now();
    let pilot_snapshot_document: PilotSyntaxSnapshotDocument =
        serde_json::from_str(&std::fs::read_to_string(&pilot_export_path)?)?;
    let load_ms = elapsed_ms(load_start);
    let normalize_start = Instant::now();
    let pilot_snapshot = normalize_pilot_syntax_snapshot(pilot_snapshot_document);
    let normalize_ms = elapsed_ms(normalize_start);
    let pilot_timings = EngineTimings {
        total_ms: elapsed_ms(pilot_start),
        phases: vec![
            PhaseTiming {
                name: "java_syntax_export".to_string(),
                duration_ms: export_ms,
            },
            PhaseTiming {
                name: "load_export_json".to_string(),
                duration_ms: load_ms,
            },
            PhaseTiming {
                name: "normalize_snapshot".to_string(),
                duration_ms: normalize_ms,
            },
        ],
    };

    let compare_start = Instant::now();
    let report = compare_syntax_snapshots(rust_snapshot.clone(), pilot_snapshot.clone());
    let compare_ms = elapsed_ms(compare_start);

    Ok(CompareOutput {
        generated_at_utc: now_utc_rfc3339()?,
        pilot_root: pilot_root.display().to_string(),
        relative_path: relative_path.to_string(),
        support_paths,
        pilot_export_path: pilot_export_path.display().to_string(),
        timings: CompareTimings {
            rust: rust_timings,
            pilot: pilot_timings,
            compare_ms,
        },
        rust_snapshot,
        pilot_snapshot,
        report,
    })
}

fn normalize_pilot_syntax_snapshot(document: PilotSyntaxSnapshotDocument) -> SyntaxSnapshot {
    SyntaxSnapshot {
        root_kind: document.root_kind,
        nodes: document
            .nodes
            .into_iter()
            .filter_map(|node| {
                let _ = node.source_file;
                normalize_syntax_node(
                    SyntaxSnapshotNode {
                        path: node.path,
                        family: node.family,
                        kind: node.kind,
                        keyword: node.keyword,
                        declared_name: node.declared_name,
                        span: SyntaxSourceSpan {
                            start_line: node.span.start_line,
                            start_col: node.span.start_col,
                            end_line: node.span.end_line,
                            end_col: node.span.end_col,
                        },
                        properties: node.properties,
                    },
                    SyntaxSide::Pilot,
                )
            })
            .collect(),
    }
}

fn normalize_rust_syntax_snapshot(snapshot: SyntaxSnapshot) -> SyntaxSnapshot {
    SyntaxSnapshot {
        root_kind: snapshot.root_kind,
        nodes: snapshot
            .nodes
            .into_iter()
            .filter_map(|node| normalize_syntax_node(node, SyntaxSide::Rust))
            .collect(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyntaxSide {
    Rust,
    Pilot,
}

fn normalize_syntax_node(
    mut node: SyntaxSnapshotNode,
    side: SyntaxSide,
) -> Option<SyntaxSnapshotNode> {
    if should_drop_syntax_node(&node, side) {
        return None;
    }

    node.keyword = normalize_syntax_keyword(&node.keyword);
    node.properties.remove("text");
    node.properties.remove("path");
    node.properties.remove("implicit_name");

    for type_key in ["type", "additional_types"] {
        if let Some(values) = node.properties.get_mut(type_key) {
            *values = values
                .iter()
                .map(|value| normalize_syntax_type_value(value))
                .filter(|value| !value.is_empty() && value != "null")
                .collect();
        }
        if node.properties.get(type_key).is_some_and(Vec::is_empty) {
            node.properties.remove(type_key);
        }
    }

    if node.declared_name.is_none() {
        if let Some(redefined_name) = node
            .properties
            .get("redefines")
            .and_then(|values| values.first())
            .cloned()
        {
            node.declared_name = Some(redefined_name);
        }
    }

    if let Some(name) = node.declared_name.as_deref() {
        if is_synthetic_declared_name(name, &node.keyword) {
            node.declared_name = None;
        }
    }

    Some(node)
}

fn should_drop_syntax_node(node: &SyntaxSnapshotNode, _side: SyntaxSide) -> bool {
    node.family == "alias"
        || node.kind == "ConjugatedPortDefinition"
        || node.keyword == "conjugatedport"
}

fn normalize_syntax_keyword(keyword: &str) -> String {
    match keyword {
        "enum" => "enumeration".to_string(),
        "successionflow" => "succession".to_string(),
        "acceptaction" => "accept".to_string(),
        "performaction" => "perform".to_string(),
        "exhibitstate" => "exhibit".to_string(),
        other => other.to_string(),
    }
}

fn normalize_syntax_type_value(value: &str) -> String {
    let normalized = normalize_pilot_property_value(value);
    if normalized.contains("::") && !normalized.contains('*') {
        return normalized
            .rsplit("::")
            .next()
            .unwrap_or(&normalized)
            .trim()
            .to_string();
    }
    normalized
}

fn is_synthetic_declared_name(name: &str, keyword: &str) -> bool {
    matches!(
        (keyword, name),
        ("succession", "SuccessionFlowUsage")
            | ("accept", "AcceptActionUsage")
            | ("perform", "PerformActionUsage")
            | ("exhibit", "ExhibitStateUsage")
    )
}

fn normalize_pilot_property_value(value: &str) -> String {
    if let Some(index) = value.find("declaredName:") {
        let suffix = &value[index + "declaredName:".len()..];
        let normalized = suffix
            .split([',', ')', ']'])
            .next()
            .unwrap_or(suffix)
            .trim();
        if !normalized.is_empty() {
            return normalized.to_string();
        }
    }
    value.trim().to_string()
}

fn export_syntax_from_pilot(
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
        "target/pilot_ast_export.{}.json",
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

    run_java_syntax_exporter(
        &interactive_jar,
        &classes_dir,
        &library_root,
        &export_path,
        &input_paths,
    )?;
    Ok(export_path)
}

fn run_java_syntax_exporter(
    interactive_jar: &Path,
    classes_dir: &Path,
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
        let script_path = repo_path("target/run_pilot_ast_exporter.ps1");
        let mut script = format!(
            "$cp = '{}'\njava -cp $cp dev.mercurio.pilot.PilotModelExporter --syntax '{}' '{}'",
            classpath.replace('\'', "''"),
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
            .arg("dev.mercurio.pilot.PilotModelExporter")
            .arg("--syntax")
            .arg(library_root)
            .arg(export_path);
        for input_path in input_paths {
            command.arg(input_path);
        }
        command.status()?
    };

    if !status.success() {
        return Err("failed to run Java pilot syntax exporter".into());
    }

    Ok(())
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

fn relative_path_slug(relative_path: &str) -> String {
    relative_path
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
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
