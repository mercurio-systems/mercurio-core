use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use mercurio_core::frontend::kerml::{compile_kerml_text, parse_kerml};
use mercurio_core::{
    Graph, KirDocument, MetamodelAttributeRegistry, SnapshotMode, SyntaxSnapshot,
    SyntaxSnapshotNode, SyntaxSourceSpan, build_rust_syntax_snapshot, build_semantic_snapshot,
    build_semantic_snapshot_with_registry, compare_snapshots, compare_syntax_snapshots,
    default_stdlib_path, load_pilot_export, normalize_pilot_export_for_compare, repo_path,
};
use mercurio_tools::{default_kerml_examples_root, default_pilot_root};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args()?;
    let files = if let Some(relative_path) = &args.relative_path {
        vec![relative_path.clone()]
    } else {
        discover_kerml_files(&args.examples_root)?
    };

    let mut cases = Vec::new();
    for relative_path in files {
        let case = compare_case(&args, &relative_path);
        println!(
            "{}: parse={} compile={} syntax={} semantic={}",
            relative_path,
            case.mercurio_parse.status,
            case.mercurio_semantic.status,
            case.syntax_compare
                .as_ref()
                .map(|compare| compare.status.as_str())
                .unwrap_or("skipped"),
            case.semantic_compare
                .as_ref()
                .map(|compare| compare.status.as_str())
                .unwrap_or("skipped")
        );
        cases.push(case);
    }

    let output = CorpusOutput {
        generated_at_utc: now_utc_rfc3339()?,
        examples_root: args.examples_root.display().to_string(),
        pilot_root: args.pilot_root.display().to_string(),
        case_count: cases.len(),
        aggregate: CorpusAggregate::from_cases(&cases),
        cases,
    };

    if let Some(parent) = args.output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.output_path, serde_json::to_string_pretty(&output)?)?;

    println!("KerML examples comparison");
    println!("  cases: {}", output.case_count);
    println!("  output: {}", args.output_path.display());
    println!("  parsed: {}", output.aggregate.mercurio_parse_pass);
    println!(
        "  semantic snapshots: {}",
        output.aggregate.mercurio_semantic_pass
    );
    println!("  syntax compared: {}", output.aggregate.syntax_compared);
    println!(
        "  semantic compared: {}",
        output.aggregate.semantic_compared
    );
    Ok(())
}

#[derive(Debug)]
struct Args {
    pilot_root: PathBuf,
    examples_root: PathBuf,
    relative_path: Option<String>,
    output_path: PathBuf,
}

#[derive(Debug, Serialize)]
struct CorpusOutput {
    generated_at_utc: String,
    examples_root: String,
    pilot_root: String,
    case_count: usize,
    aggregate: CorpusAggregate,
    cases: Vec<CaseOutput>,
}

#[derive(Debug, Serialize)]
struct CorpusAggregate {
    mercurio_parse_pass: usize,
    mercurio_semantic_pass: usize,
    pilot_syntax_pass: usize,
    pilot_semantic_pass: usize,
    syntax_compared: usize,
    syntax_exact: usize,
    syntax_mismatches: usize,
    syntax_mercurio_only: usize,
    syntax_pilot_only: usize,
    semantic_compared: usize,
    semantic_exact: usize,
    semantic_mismatches: usize,
    semantic_mercurio_only: usize,
    semantic_pilot_only: usize,
}

#[derive(Debug, Serialize)]
struct CaseOutput {
    relative_path: String,
    mercurio_parse: StageStatus,
    mercurio_semantic: StageStatus,
    pilot_syntax: StageStatus,
    pilot_semantic: StageStatus,
    syntax_compare: Option<CompareStatus>,
    semantic_compare: Option<CompareStatus>,
}

#[derive(Debug, Serialize)]
struct StageStatus {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    count: Option<usize>,
    elapsed_ms: u64,
}

#[derive(Debug, Serialize)]
struct CompareStatus {
    status: String,
    exact: bool,
    mismatches: usize,
    mercurio_only: usize,
    pilot_only: usize,
    elapsed_ms: u64,
}

#[derive(Debug, Deserialize)]
struct PilotSyntaxSnapshotDocument {
    root_kind: String,
    nodes: Vec<PilotSyntaxNodeDocument>,
}

#[derive(Debug, Deserialize)]
struct PilotSyntaxNodeDocument {
    path: String,
    family: String,
    kind: String,
    keyword: String,
    declared_name: Option<String>,
    source_file: Option<String>,
    span: PilotSyntaxSpanDocument,
    #[serde(default)]
    properties: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct PilotSyntaxSpanDocument {
    start_line: usize,
    start_col: usize,
    end_line: usize,
    end_col: usize,
}

fn parse_args() -> Result<Args, Box<dyn std::error::Error>> {
    let mut pilot_root = default_pilot_root();
    let mut examples_root =
        default_kerml_examples_root(repo_path("test_files/examples/kerml/examples"));
    let mut relative_path = None;
    let mut output_path = repo_path("target/kerml_examples_compare.json");
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--pilot-root" => {
                index += 1;
                pilot_root =
                    PathBuf::from(args.get(index).ok_or("missing value for --pilot-root")?);
            }
            "--examples-root" => {
                index += 1;
                examples_root =
                    PathBuf::from(args.get(index).ok_or("missing value for --examples-root")?);
            }
            "--relative-path" => {
                index += 1;
                relative_path = Some(
                    args.get(index)
                        .ok_or("missing value for --relative-path")?
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

    Ok(Args {
        pilot_root,
        examples_root,
        relative_path,
        output_path,
    })
}

fn print_usage() {
    println!(
        "Usage: cargo run -p mercurio-tools --bin compare_kerml_examples -- [--examples-root PATH] [--relative-path PATH] [--pilot-root PATH] [--out PATH]"
    );
}

fn compare_case(args: &Args, relative_path: &str) -> CaseOutput {
    let source_path = args.examples_root.join(relative_path);
    let source_name = normalize_relative_path(Path::new(relative_path));
    let source_text = match std::fs::read_to_string(&source_path) {
        Ok(text) => text,
        Err(err) => {
            let failure = StageStatus::failure(err.to_string(), 0);
            return CaseOutput {
                relative_path: relative_path.to_string(),
                mercurio_parse: failure,
                mercurio_semantic: StageStatus::skipped(),
                pilot_syntax: StageStatus::skipped(),
                pilot_semantic: StageStatus::skipped(),
                syntax_compare: None,
                semantic_compare: None,
            };
        }
    };

    let parse_start = Instant::now();
    let mercurio_syntax = match parse_kerml(&source_text) {
        Ok(module) => {
            let snapshot = normalize_rust_syntax_snapshot(build_rust_syntax_snapshot(&module));
            let status = StageStatus::ok(snapshot.nodes.len(), elapsed_ms(parse_start));
            (status, Some(snapshot))
        }
        Err(err) => (
            StageStatus::failure(err.to_string(), elapsed_ms(parse_start)),
            None,
        ),
    };

    let semantic_start = Instant::now();
    let stdlib = KirDocument::from_path(&default_stdlib_path());
    let mercurio_semantic = match stdlib {
        Ok(stdlib) => match compile_kerml_text(&source_text, &source_name, &stdlib) {
            Ok(document) => {
                let registry = MetamodelAttributeRegistry::build(
                    &Graph::from_document(stdlib.clone()).expect("stdlib graph"),
                );
                match KirDocument::merge([stdlib, document]).and_then(|merged| {
                    build_semantic_snapshot(merged, &source_name, SnapshotMode::Mercurio)
                        .map_err(|err| mercurio_core::KirError::Frontend(err.to_string()))
                }) {
                    Ok(snapshot) => {
                        let status =
                            StageStatus::ok(snapshot.elements.len(), elapsed_ms(semantic_start));
                        (status, Some(snapshot), Some(registry))
                    }
                    Err(err) => (
                        StageStatus::failure(err.to_string(), elapsed_ms(semantic_start)),
                        None,
                        Some(registry),
                    ),
                }
            }
            Err(err) => (
                StageStatus::failure(err.to_string(), elapsed_ms(semantic_start)),
                None,
                None,
            ),
        },
        Err(err) => (
            StageStatus::failure(err.to_string(), elapsed_ms(semantic_start)),
            None,
            None,
        ),
    };

    let pilot_syntax_start = Instant::now();
    let pilot_syntax = match export_syntax_from_pilot(args, relative_path, &source_path) {
        Ok(path) => match load_pilot_syntax_snapshot(&path) {
            Ok(document) => {
                let snapshot = normalize_pilot_syntax_snapshot(document);
                let status = StageStatus::ok(snapshot.nodes.len(), elapsed_ms(pilot_syntax_start));
                (status, Some(snapshot))
            }
            Err(err) => (
                StageStatus::failure(err.to_string(), elapsed_ms(pilot_syntax_start)),
                None,
            ),
        },
        Err(err) => (
            StageStatus::failure(err.to_string(), elapsed_ms(pilot_syntax_start)),
            None,
        ),
    };

    let pilot_semantic_start = Instant::now();
    let pilot_semantic = match (
        export_model_from_pilot(args, relative_path, &source_path),
        mercurio_semantic.2.as_ref(),
    ) {
        (Ok(path), Some(registry)) => match load_pilot_export(&path)
            .and_then(normalize_pilot_export_for_compare)
            .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })
            .and_then(|document| {
                build_semantic_snapshot_with_registry(
                    document,
                    &source_name,
                    SnapshotMode::Pilot,
                    registry,
                )
                .map_err(|err| err.into())
            }) {
            Ok(snapshot) => {
                let status =
                    StageStatus::ok(snapshot.elements.len(), elapsed_ms(pilot_semantic_start));
                (status, Some(snapshot))
            }
            Err(err) => (
                StageStatus::failure(err.to_string(), elapsed_ms(pilot_semantic_start)),
                None,
            ),
        },
        (Ok(_), None) => (StageStatus::skipped(), None),
        (Err(err), _) => (
            StageStatus::failure(err.to_string(), elapsed_ms(pilot_semantic_start)),
            None,
        ),
    };

    let syntax_compare = match (&mercurio_syntax.1, &pilot_syntax.1) {
        (Some(mercurio), Some(pilot)) => {
            let start = Instant::now();
            let report = compare_syntax_snapshots(mercurio.clone(), pilot.clone());
            Some(CompareStatus {
                status: "ok".to_string(),
                exact: report.mismatches.is_empty()
                    && report.rust_only.is_empty()
                    && report.pilot_only.is_empty(),
                mismatches: report.mismatches.len(),
                mercurio_only: report.rust_only.len(),
                pilot_only: report.pilot_only.len(),
                elapsed_ms: elapsed_ms(start),
            })
        }
        _ => None,
    };

    let semantic_compare = match (&mercurio_semantic.1, &pilot_semantic.1) {
        (Some(mercurio), Some(pilot)) => {
            let start = Instant::now();
            match compare_snapshots(mercurio.clone(), pilot.clone()) {
                Ok(report) => Some(CompareStatus {
                    status: "ok".to_string(),
                    exact: report.mismatches.is_empty()
                        && report.mercurio_only.is_empty()
                        && report.pilot_only.is_empty(),
                    mismatches: report.mismatches.len(),
                    mercurio_only: report.mercurio_only.len(),
                    pilot_only: report.pilot_only.len(),
                    elapsed_ms: elapsed_ms(start),
                }),
                Err(err) => Some(CompareStatus {
                    status: format!("error: {err}"),
                    exact: false,
                    mismatches: 0,
                    mercurio_only: 0,
                    pilot_only: 0,
                    elapsed_ms: elapsed_ms(start),
                }),
            }
        }
        _ => None,
    };

    CaseOutput {
        relative_path: relative_path.to_string(),
        mercurio_parse: mercurio_syntax.0,
        mercurio_semantic: mercurio_semantic.0,
        pilot_syntax: pilot_syntax.0,
        pilot_semantic: pilot_semantic.0,
        syntax_compare,
        semantic_compare,
    }
}

impl StageStatus {
    fn ok(count: usize, elapsed_ms: u64) -> Self {
        Self {
            status: "ok".to_string(),
            error: None,
            count: Some(count),
            elapsed_ms,
        }
    }

    fn failure(error: String, elapsed_ms: u64) -> Self {
        Self {
            status: "error".to_string(),
            error: Some(error),
            count: None,
            elapsed_ms,
        }
    }

    fn skipped() -> Self {
        Self {
            status: "skipped".to_string(),
            error: None,
            count: None,
            elapsed_ms: 0,
        }
    }
}

impl CorpusAggregate {
    fn from_cases(cases: &[CaseOutput]) -> Self {
        let syntax = cases
            .iter()
            .filter_map(|case| case.syntax_compare.as_ref())
            .collect::<Vec<_>>();
        let semantic = cases
            .iter()
            .filter_map(|case| case.semantic_compare.as_ref())
            .collect::<Vec<_>>();
        Self {
            mercurio_parse_pass: cases
                .iter()
                .filter(|case| case.mercurio_parse.status == "ok")
                .count(),
            mercurio_semantic_pass: cases
                .iter()
                .filter(|case| case.mercurio_semantic.status == "ok")
                .count(),
            pilot_syntax_pass: cases
                .iter()
                .filter(|case| case.pilot_syntax.status == "ok")
                .count(),
            pilot_semantic_pass: cases
                .iter()
                .filter(|case| case.pilot_semantic.status == "ok")
                .count(),
            syntax_compared: syntax.len(),
            syntax_exact: syntax.iter().filter(|compare| compare.exact).count(),
            syntax_mismatches: syntax.iter().map(|compare| compare.mismatches).sum(),
            syntax_mercurio_only: syntax.iter().map(|compare| compare.mercurio_only).sum(),
            syntax_pilot_only: syntax.iter().map(|compare| compare.pilot_only).sum(),
            semantic_compared: semantic.len(),
            semantic_exact: semantic.iter().filter(|compare| compare.exact).count(),
            semantic_mismatches: semantic.iter().map(|compare| compare.mismatches).sum(),
            semantic_mercurio_only: semantic.iter().map(|compare| compare.mercurio_only).sum(),
            semantic_pilot_only: semantic.iter().map(|compare| compare.pilot_only).sum(),
        }
    }
}

fn discover_kerml_files(root: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut files = Vec::new();
    collect_kerml_files(root, root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_kerml_files(
    root: &Path,
    current: &Path,
    files: &mut Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in std::fs::read_dir(current)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_kerml_files(root, &path, files)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("kerml") {
            files.push(normalize_relative_path(path.strip_prefix(root)?));
        }
    }
    Ok(())
}

fn normalize_pilot_syntax_snapshot(document: PilotSyntaxSnapshotDocument) -> SyntaxSnapshot {
    SyntaxSnapshot {
        root_kind: document.root_kind,
        nodes: document
            .nodes
            .into_iter()
            .filter_map(|node| {
                let _ = node.source_file;
                normalize_syntax_node(SyntaxSnapshotNode {
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
                })
            })
            .collect(),
    }
}

fn load_pilot_syntax_snapshot(
    path: &Path,
) -> Result<PilotSyntaxSnapshotDocument, Box<dyn std::error::Error>> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

fn normalize_rust_syntax_snapshot(snapshot: SyntaxSnapshot) -> SyntaxSnapshot {
    SyntaxSnapshot {
        root_kind: snapshot.root_kind,
        nodes: snapshot
            .nodes
            .into_iter()
            .filter_map(normalize_syntax_node)
            .collect(),
    }
}

fn normalize_syntax_node(mut node: SyntaxSnapshotNode) -> Option<SyntaxSnapshotNode> {
    if node.family == "alias" {
        return None;
    }
    node.properties.remove("text");
    node.properties.remove("path");
    node.properties.remove("implicit_name");
    Some(node)
}

fn export_syntax_from_pilot(
    args: &Args,
    relative_path: &str,
    source_path: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let export_path = repo_path(&format!(
        "target/kerml_syntax_export.{}.json",
        relative_path_slug(relative_path)
    ));
    run_java_exporter(args, "syntax", &export_path, source_path)?;
    Ok(export_path)
}

fn export_model_from_pilot(
    args: &Args,
    relative_path: &str,
    source_path: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let export_path = repo_path(&format!(
        "target/kerml_model_export.{}.json",
        relative_path_slug(relative_path)
    ));
    run_java_exporter(args, "model", &export_path, source_path)?;
    Ok(export_path)
}

fn run_java_exporter(
    args: &Args,
    mode: &str,
    export_path: &Path,
    source_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = export_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let pilot_root = args.pilot_root.canonicalize()?;
    let library_root = pilot_root.join("sysml.library");
    let interactive_jar = find_interactive_jar(&pilot_root)?;
    let classes_dir = repo_path("target/pilot-exporter-classes");
    let java_source =
        repo_path("tools/pilot-exporter/src/main/java/dev/mercurio/pilot/PilotModelExporter.java");
    compile_java_exporter(
        &interactive_jar,
        &java_source,
        &classes_dir,
        "dev/mercurio/pilot/PilotModelExporter.class",
    )?;

    let classes_dir = absolute_path(&classes_dir)?;
    let interactive_jar = absolute_path(&interactive_jar)?;
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
        let script_path = repo_path(&format!("target/run_kerml_{mode}_exporter.ps1"));
        let mode_arg = if mode == "syntax" { "--syntax " } else { "" };
        let script = format!(
            "$cp = '{}'\njava -cp $cp dev.mercurio.pilot.PilotModelExporter {}'{}' '{}' '{}'\n",
            classpath.replace('\'', "''"),
            mode_arg,
            java_path_string(&library_root).replace('\'', "''"),
            java_path_string(export_path).replace('\'', "''"),
            java_path_string(source_path).replace('\'', "''"),
        );
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
            .arg("dev.mercurio.pilot.PilotModelExporter");
        if mode == "syntax" {
            command.arg("--syntax");
        }
        command.arg(library_root).arg(export_path).arg(source_path);
        command.status()?
    };

    if !status.success() {
        return Err(format!("failed to run Java pilot {mode} exporter").into());
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

fn normalize_relative_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
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
