use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use mercurio_core::{
    Graph, PilotExportDocument, RulePack, default_stdlib_path, load_pilot_export,
    normalize_pilot_export, repo_path, repo_root,
};
use mercurio_tools::sha256_file;
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args()?;
    let input_path = if let Some(pilot_root) = args.pilot_root.as_deref() {
        export_from_pilot(pilot_root, &args.input_path)?
    } else {
        args.input_path.clone()
    };

    let export = load_pilot_export(&input_path)?;
    let mut kir = normalize_pilot_export(export.clone())?;
    kir.metadata = build_kir_metadata(&args, &input_path, &export)?;
    let rulepack = RulePack::metamodel_adapter_from_graph(&Graph::from_document(kir.clone())?);
    kir.write_pretty_to_path(&args.output_path)?;
    write_rulepack(&rulepack, &args.rulepack_output_path)?;

    println!("Imported pilot stdlib export:");
    println!("  input: {}", input_path.display());
    println!("  output: {}", args.output_path.display());
    println!("  rulepack: {}", args.rulepack_output_path.display());
    println!("  elements: {}", kir.elements.len());
    println!("  adapter facts: {}", rulepack.facts.len());
    Ok(())
}

struct Args {
    input_path: PathBuf,
    output_path: PathBuf,
    rulepack_output_path: PathBuf,
    pilot_root: Option<PathBuf>,
}

fn parse_args() -> Result<Args, Box<dyn std::error::Error>> {
    let mut input_path = None;
    let mut output_path = default_stdlib_path();
    let mut rulepack_output_path = default_rulepack_path(&output_path);
    let mut pilot_root = None;
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--from-export" => {
                index += 1;
                let value = args.get(index).ok_or("missing value for --from-export")?;
                input_path = Some(PathBuf::from(value));
            }
            "--out" => {
                index += 1;
                let value = args.get(index).ok_or("missing value for --out")?;
                output_path = PathBuf::from(value);
                rulepack_output_path = default_rulepack_path(&output_path);
            }
            "--rulepack-out" => {
                index += 1;
                let value = args.get(index).ok_or("missing value for --rulepack-out")?;
                rulepack_output_path = PathBuf::from(value);
            }
            "--pilot-root" => {
                index += 1;
                let value = args.get(index).ok_or("missing value for --pilot-root")?;
                pilot_root = Some(PathBuf::from(value));
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            unknown => {
                return Err(format!("unknown argument: {unknown}").into());
            }
        }
        index += 1;
    }

    let input_path = match (input_path, pilot_root.as_ref()) {
        (Some(input_path), _) => input_path,
        (None, Some(_)) => repo_path("target/stdlib-release/pilot-stdlib-export.json"),
        (None, None) => {
            return Err("expected --from-export PATH or --pilot-root PATH".into());
        }
    };

    Ok(Args {
        input_path,
        output_path,
        rulepack_output_path,
        pilot_root,
    })
}

fn print_usage() {
    println!(
        "Usage: cargo run -p mercurio-tools --bin import_pilot_stdlib -- [--pilot-root PATH] [--from-export PATH] [--out PATH] [--rulepack-out PATH]"
    );
}

fn default_rulepack_path(output_path: &Path) -> PathBuf {
    let file_name = output_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("stdlib.kir.json");
    let rulepack_name = if let Some(prefix) = file_name.strip_suffix(".kir.json") {
        format!("{prefix}.rulepack.json")
    } else {
        format!("{file_name}.rulepack.json")
    };
    output_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(rulepack_name)
}

fn write_rulepack(
    rulepack: &RulePack,
    output_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    rulepack.write_pretty_to_path(output_path)?;
    Ok(())
}

fn build_kir_metadata(
    args: &Args,
    input_path: &Path,
    export: &PilotExportDocument,
) -> Result<BTreeMap<String, Value>, Box<dyn std::error::Error>> {
    let mut metadata = BTreeMap::new();
    metadata.insert(
        "import_source".to_string(),
        Value::String("pilot".to_string()),
    );
    metadata.insert(
        "imported_at_utc".to_string(),
        Value::String(now_utc_rfc3339()?),
    );
    metadata.insert(
        "importer_version".to_string(),
        Value::String(env!("CARGO_PKG_VERSION").to_string()),
    );
    metadata.insert(
        "input_export_path".to_string(),
        Value::String(metadata_path_string(input_path)),
    );
    metadata.insert(
        "input_export_sha256".to_string(),
        Value::String(sha256_file(input_path)?),
    );

    if let Some(pilot_root) = &args.pilot_root {
        metadata.insert(
            "pilot_root".to_string(),
            Value::String(metadata_path_string(pilot_root)),
        );
        metadata.insert(
            "library_root".to_string(),
            Value::String(metadata_path_string(&pilot_root.join("sysml.library"))),
        );
        if let Some(commit) = git_stdout(pilot_root, ["rev-parse", "HEAD"]) {
            metadata.insert("pilot_commit".to_string(), Value::String(commit));
        }
        if let Some(describe) =
            git_stdout(pilot_root, ["describe", "--tags", "--always", "--dirty"])
        {
            metadata.insert("pilot_git_describe".to_string(), Value::String(describe));
        }
        if let Some(dirty) = git_dirty(pilot_root) {
            metadata.insert("pilot_dirty".to_string(), Value::Bool(dirty));
        }
    }

    if let Some(stdlib_version) = infer_stdlib_version(args, export) {
        metadata.insert("stdlib_version".to_string(), Value::String(stdlib_version));
    }

    if let Some(export_metadata) = &export.metadata {
        metadata.insert("source_export".to_string(), export_metadata.clone());
    }

    metadata.insert(
        "element_count".to_string(),
        json!(kir_element_count(export)),
    );
    metadata.insert(
        "relationship_count".to_string(),
        json!(export.relationships.len()),
    );

    Ok(metadata)
}

fn infer_stdlib_version(args: &Args, export: &PilotExportDocument) -> Option<String> {
    if let Some(version) = export
        .metadata
        .as_ref()
        .and_then(|value| value.get("pilot_version"))
        .and_then(Value::as_str)
    {
        return Some(version.to_string());
    }

    args.pilot_root
        .as_deref()
        .and_then(|pilot_root| find_interactive_jar(pilot_root).ok())
        .and_then(|jar| {
            jar.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .and_then(|file_name| {
            file_name
                .strip_prefix("org.omg.sysml.interactive-")
                .and_then(|name| name.strip_suffix("-all.jar"))
                .map(str::to_string)
        })
}

fn git_stdout<const N: usize>(repo: &Path, args: [&str; N]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn git_dirty(repo: &Path) -> Option<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["status", "--porcelain"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    Some(!output.stdout.is_empty())
}

fn kir_element_count(export: &PilotExportDocument) -> usize {
    export.elements.len()
}

fn now_utc_rfc3339() -> Result<String, Box<dyn std::error::Error>> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}

fn metadata_path_string(path: &Path) -> String {
    let repo_root = repo_root();
    let absolute_path = path
        .canonicalize()
        .unwrap_or_else(|_| absolute_path_lossy(path));
    let absolute_repo_root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| absolute_path_lossy(&repo_root));

    if let Ok(relative) = absolute_path.strip_prefix(&absolute_repo_root) {
        return path_to_slash_string(relative);
    }

    if let Some(parent) = absolute_repo_root.parent() {
        if let Ok(relative) = absolute_path.strip_prefix(parent) {
            return format!("../{}", path_to_slash_string(relative));
        }
    }

    path_to_slash_string(&absolute_path)
}

fn absolute_path_lossy(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn path_to_slash_string(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn export_from_pilot(
    pilot_root: &Path,
    export_path: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let pilot_root = pilot_root.canonicalize()?;
    let library_root = pilot_root.join("sysml.library");
    let interactive_jar = find_interactive_jar(&pilot_root)?;
    let classes_dir = repo_path("target/pilot-exporter-classes");
    let java_source =
        repo_path("tools/pilot-exporter/src/main/java/dev/mercurio/pilot/PilotStdlibExporter.java");

    compile_java_exporter(&interactive_jar, &java_source, &classes_dir)?;
    run_java_exporter(&interactive_jar, &classes_dir, &library_root, export_path)?;
    Ok(export_path.to_path_buf())
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
) -> Result<(), Box<dyn std::error::Error>> {
    let class_file = classes_dir.join("dev/mercurio/pilot/PilotStdlibExporter.class");
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
    library_root: &Path,
    export_path: &Path,
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
        let script_path = repo_path("target/run_pilot_exporter.ps1");
        let script = format!(
            "$cp = '{}'\njava -cp $cp dev.mercurio.pilot.PilotStdlibExporter '{}' '{}'\n",
            classpath.replace('\'', "''"),
            java_path_string(library_root).replace('\'', "''"),
            java_path_string(export_path).replace('\'', "''"),
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
            .arg("dev.mercurio.pilot.PilotStdlibExporter")
            .arg(library_root)
            .arg(export_path)
            .status()?
    };

    if !status.success() {
        return Err("failed to run Java pilot exporter".into());
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
