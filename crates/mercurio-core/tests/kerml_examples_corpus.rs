use std::path::{Path, PathBuf};

use mercurio_core::frontend::ast::{Declaration, SysmlModule};
use mercurio_core::frontend::kerml::{compile_kerml_module_with_context, parse_kerml};
use mercurio_core::{
    KirDocument, SnapshotMode, build_rust_syntax_snapshot, build_semantic_snapshot, repo_path,
};

#[test]
#[ignore = "coverage harness for the checked-in KerML examples corpus"]
fn parse_kerml_examples_corpus() {
    let files = kerml_example_files();
    assert!(!files.is_empty(), "expected KerML example files");

    let mut failures = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path).expect("failed to read KerML example");
        match parse_kerml(&text) {
            Ok(module) => {
                let snapshot = build_rust_syntax_snapshot(&module);
                if snapshot.nodes.is_empty() {
                    failures.push(format!(
                        "{}: parsed but produced empty AST snapshot",
                        relative(path).display()
                    ));
                }
            }
            Err(err) => failures.push(format!("{}: {err}", relative(path).display())),
        }
    }

    assert!(
        failures.is_empty(),
        "parsed {}/{} KerML examples; failures:\n{}",
        files.len() - failures.len(),
        files.len(),
        failures.join("\n")
    );
}

#[test]
#[ignore = "coverage harness for the checked-in KerML examples corpus"]
fn compile_and_snapshot_kerml_examples_corpus() {
    let files = kerml_example_files();
    assert!(!files.is_empty(), "expected KerML example files");
    let stdlib = KirDocument::from_path(&repo_path("resources/stdlib-sources/sysml-2.0-pilot-0.57.0/stdlib.full.kir.json")).unwrap();
    let parsed_modules = files
        .iter()
        .map(|path| {
            let text = std::fs::read_to_string(path).expect("failed to read KerML example");
            parse_kerml(&text).map(|module| (path.clone(), module))
        })
        .collect::<Result<Vec<_>, _>>()
        .expect("expected KerML corpus to parse before semantic compile");
    let mut failures = Vec::new();
    for (path, module) in &parsed_modules {
        let source_name = relative(path).display().to_string();
        let import_prefixes = imported_prefixes(module);
        let context_modules = parsed_modules
            .iter()
            .filter(|(context_path, context_module)| {
                context_path == path
                    || (context_path.parent() == path.parent()
                        && exported_names(context_module)
                            .iter()
                            .any(|name| import_prefixes.iter().any(|prefix| prefix == name)))
            })
            .map(|(_, module)| module.clone())
            .collect::<Vec<_>>();
        match compile_kerml_module_with_context(module, &source_name, &context_modules, &stdlib) {
            Ok(document) => {
                match KirDocument::merge([stdlib.clone(), document]).and_then(|merged| {
                    build_semantic_snapshot(merged, &source_name, SnapshotMode::Mercurio)
                        .map_err(|err| mercurio_core::KirError::Frontend(err.to_string()))
                }) {
                    Ok(snapshot) => {
                        if snapshot.elements.is_empty() {
                            failures.push(format!(
                                "{}: compiled but produced empty semantic snapshot",
                                source_name
                            ));
                        }
                    }
                    Err(err) => failures.push(format!("{}: {err}", source_name)),
                }
            }
            Err(err) => failures.push(format!("{}: {err}", source_name)),
        }
    }

    assert!(
        failures.is_empty(),
        "compiled {}/{} KerML examples; failures:\n{}",
        files.len() - failures.len(),
        files.len(),
        failures.join("\n")
    );
}

fn kerml_example_files() -> Vec<PathBuf> {
    let root = repo_path("test_files/examples/kerml/examples");
    let mut files = Vec::new();
    collect_kerml_files(&root, &mut files);
    files.sort();
    files
}

fn collect_kerml_files(path: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(path).expect("failed to read KerML examples directory") {
        let path = entry.expect("failed to read KerML example entry").path();
        if path.is_dir() {
            collect_kerml_files(&path, files);
        } else if path.extension().and_then(|value| value.to_str()) == Some("kerml") {
            files.push(path);
        }
    }
}

fn relative(path: &Path) -> PathBuf {
    path.strip_prefix(repo_path(""))
        .unwrap_or(path)
        .to_path_buf()
}

fn imported_prefixes(module: &SysmlModule) -> Vec<String> {
    let mut prefixes = Vec::new();
    collect_imported_prefixes(&module.members, &mut prefixes);
    prefixes.sort();
    prefixes.dedup();
    prefixes
}

fn collect_imported_prefixes(declarations: &[Declaration], prefixes: &mut Vec<String>) {
    for declaration in declarations {
        match declaration {
            Declaration::Import(import) => {
                if let Some(first) = import.path.segments.first() {
                    prefixes.push(first.clone());
                }
            }
            Declaration::Package(package) => collect_imported_prefixes(&package.members, prefixes),
            Declaration::GenericDefinition(definition) => {
                collect_imported_prefixes(&definition.members, prefixes)
            }
            Declaration::GenericUsage(usage) => {
                collect_imported_prefixes(&usage.body_members, prefixes)
            }
            Declaration::PartDefinition(definition) => {
                collect_imported_prefixes(&definition.members, prefixes)
            }
            Declaration::PartUsage(usage) => {
                collect_imported_prefixes(&usage.body_members, prefixes)
            }
            Declaration::Alias(_) => {}
        }
    }
}

fn exported_names(module: &SysmlModule) -> Vec<String> {
    let mut names = Vec::new();
    collect_exported_names(&module.members, &mut names);
    names.sort();
    names.dedup();
    names
}

fn collect_exported_names(declarations: &[Declaration], names: &mut Vec<String>) {
    for declaration in declarations {
        match declaration {
            Declaration::Package(package) => {
                if let Some(name) = package.name.segments.last() {
                    names.push(name.clone());
                }
            }
            Declaration::GenericDefinition(definition) => names.push(definition.name.clone()),
            Declaration::PartDefinition(definition) => names.push(definition.name.clone()),
            _ => {}
        }
    }
}
