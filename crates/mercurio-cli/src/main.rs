use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{Shell, generate};
use mercurio_core::frontend::ast::{Declaration, SysmlModule};
use mercurio_core::frontend::diagnostics::Diagnostic;
use mercurio_core::frontend::kerml::{compile_kerml_text, parse_kerml};
use mercurio_core::frontend::sysml::{compile_sysml_text_with_context_report, parse_sysml};
use mercurio_core::{
    KirDocument, KparPackageBuild, KparPackageSource, LibraryProviderConfig, LintReport,
    LintSeverity, PROJECT_DESCRIPTOR_FILE_NAME, ProjectDescriptor, Runtime, SemanticCompileStatus,
    SourceLanguage, default_stdlib_path, lint_text, resolve_project_context, write_kpar_package,
};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Parser)]
#[command(name = "mercurio")]
#[command(about = "Parse, compile, and lint SysML v2 and KerML sources.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Parse(ParseCommand),
    Compile(CompileCommand),
    Evaluate(EvaluateCommand),
    Lint(LintCommand),
    Package(PackageCommand),
    Project(ProjectCommand),
    Completions(CompletionsCommand),
}

#[derive(Debug, Args)]
struct ParseCommand {
    #[command(flatten)]
    input: SingleInput,
    #[arg(long, value_enum)]
    language: Option<LanguageArg>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

#[derive(Debug, Args)]
struct CompileCommand {
    #[command(flatten)]
    input: SingleInput,
    #[arg(long, value_enum)]
    language: Option<LanguageArg>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    #[arg(long)]
    stdlib: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct LintCommand {
    #[arg(long = "file")]
    files: Vec<PathBuf>,
    #[arg(long)]
    text: Option<String>,
    #[arg(long, value_enum)]
    language: Option<LanguageArg>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    #[arg(long)]
    stdlib: Option<PathBuf>,
    #[arg(long, alias = "deny-warnings")]
    warnings_as_errors: bool,
    #[arg(long)]
    quiet: bool,
}

#[derive(Debug, Args)]
struct EvaluateCommand {
    #[command(flatten)]
    input: SingleInput,
    #[arg(long)]
    kir: Option<PathBuf>,
    #[arg(long)]
    feature: String,
    #[arg(long)]
    owner: String,
    #[arg(long, value_enum)]
    language: Option<LanguageArg>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    #[arg(long)]
    stdlib: Option<PathBuf>,
    #[arg(long = "value", value_name = "OWNER.FEATURE=JSON")]
    values: Vec<String>,
    #[arg(long, value_name = "JSON")]
    context_json: Option<String>,
    #[arg(long, value_name = "PATH")]
    context_file: Option<PathBuf>,
    #[arg(long)]
    explain: bool,
}

#[derive(Debug, Args)]
struct PackageCommand {
    #[command(subcommand)]
    command: PackageSubcommand,
}

#[derive(Debug, Args)]
struct CompletionsCommand {
    #[arg(value_enum)]
    shell: Shell,
}

#[derive(Debug, Args)]
struct ProjectCommand {
    #[command(subcommand)]
    command: ProjectSubcommand,
}

#[derive(Debug, Subcommand)]
enum ProjectSubcommand {
    New(ProjectNewCommand),
}

#[derive(Debug, Args)]
struct ProjectNewCommand {
    path: PathBuf,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    force: bool,
    #[arg(long)]
    quiet: bool,
}

#[derive(Debug, Subcommand)]
enum PackageSubcommand {
    Build(PackageBuildCommand),
}

#[derive(Debug, Args)]
struct PackageBuildCommand {
    #[arg(long = "file")]
    files: Vec<PathBuf>,
    #[arg(long)]
    out: PathBuf,
    #[arg(long)]
    stdlib: Option<PathBuf>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    version: Option<String>,
    #[arg(long)]
    quiet: bool,
}

#[derive(Debug, Args)]
struct SingleInput {
    #[arg(long)]
    file: Option<PathBuf>,
    #[arg(long)]
    text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum LanguageArg {
    Auto,
    Sysml,
    Kerml,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Debug, Clone)]
struct SourceInput {
    source_name: String,
    language: SourceLanguage,
    content: String,
}

struct EvaluateInput {
    source: String,
    language: Option<SourceLanguage>,
    project_descriptor: ProjectDescriptorOutput,
    compile_status: &'static str,
    diagnostics: Vec<Diagnostic>,
    document: KirDocument,
}

#[derive(Debug, Clone)]
struct ResolvedEvaluationTarget {
    owner_id: String,
    feature_id: String,
}

#[derive(Debug)]
struct CliError {
    message: String,
    code: i32,
}

impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: 2,
        }
    }

    fn execution(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: 2,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

fn main() {
    let cli = Cli::parse();
    match run(cli) {
        Ok(result) => {
            if !result.stdout.is_empty() {
                print!("{}", result.stdout);
            }
            std::process::exit(result.exit_code);
        }
        Err(error) => {
            eprintln!("mercurio: {error}");
            std::process::exit(error.code);
        }
    }
}

#[derive(Debug)]
struct RunResult {
    exit_code: i32,
    stdout: String,
}

fn run(cli: Cli) -> Result<RunResult, CliError> {
    match cli.command {
        Command::Parse(command) => run_parse(command),
        Command::Compile(command) => run_compile(command),
        Command::Evaluate(command) => run_evaluate(command),
        Command::Lint(command) => run_lint(command),
        Command::Package(command) => run_package(command),
        Command::Project(command) => run_project(command),
        Command::Completions(command) => run_completions(command),
    }
}

fn run_parse(command: ParseCommand) -> Result<RunResult, CliError> {
    let source = read_single_input(&command.input, command.language)?;
    let parsed = parse_source(&source);
    let failed = parsed.is_err();
    let response = match parsed {
        Ok(module) => ParseResponse {
            source: source.source_name,
            language: source.language,
            status: "ok",
            diagnostics: Vec::new(),
            ast: Some(module),
        },
        Err(diagnostic) => ParseResponse {
            source: source.source_name,
            language: source.language,
            status: "failed",
            diagnostics: vec![diagnostic],
            ast: None,
        },
    };

    let stdout = match command.format {
        OutputFormat::Text => format_parse_text(&response),
        OutputFormat::Json => to_pretty_json(&response)?,
    };

    Ok(RunResult {
        exit_code: if failed { 1 } else { 0 },
        stdout,
    })
}

fn run_compile(command: CompileCommand) -> Result<RunResult, CliError> {
    let source = read_single_input(&command.input, command.language)?;
    let library_context = load_library_context(
        command.stdlib.as_deref(),
        single_input_context_path(&command.input)?,
    )?;
    let mut response = compile_source(&source, &library_context.document);
    response.project_descriptor = library_context.project_descriptor_output();
    let failed = response.status == "failed" || !response.diagnostics.is_empty();
    let stdout = match command.format {
        OutputFormat::Text => format_compile_text(&response),
        OutputFormat::Json => to_pretty_json(&response)?,
    };

    Ok(RunResult {
        exit_code: if failed { 1 } else { 0 },
        stdout,
    })
}

fn run_evaluate(command: EvaluateCommand) -> Result<RunResult, CliError> {
    let mut context = read_execution_context(&command)?;
    let evaluation_input = read_evaluate_input(&command)?;
    let mut response = EvaluateResponse {
        source: evaluation_input.source,
        language: evaluation_input.language,
        project_descriptor: evaluation_input.project_descriptor,
        compile_status: evaluation_input.compile_status,
        diagnostics: evaluation_input.diagnostics,
        feature: command.feature.clone(),
        feature_id: None,
        owner: command.owner.clone(),
        owner_id: None,
        status: "failed",
        value: None,
        explanation: Vec::new(),
        error: None,
    };

    if !response.diagnostics.is_empty() || response.compile_status == "failed" {
        response.error = Some("compile diagnostics prevented evaluation".to_string());
        let stdout = match command.format {
            OutputFormat::Text => format_evaluate_text(&response, command.explain),
            OutputFormat::Json => to_pretty_json(&response)?,
        };
        return Ok(RunResult {
            exit_code: 1,
            stdout,
        });
    }

    let target = match resolve_evaluation_target(
        &evaluation_input.document,
        &command.owner,
        &command.feature,
    ) {
        Ok(target) => target,
        Err(err) => {
            response.error = Some(err.to_string());
            let stdout = match command.format {
                OutputFormat::Text => format_evaluate_text(&response, command.explain),
                OutputFormat::Json => to_pretty_json(&response)?,
            };
            return Ok(RunResult {
                exit_code: 1,
                stdout,
            });
        }
    };
    response.owner_id = Some(target.owner_id.clone());
    response.feature_id = Some(target.feature_id.clone());
    add_resolved_context_value_aliases(&mut context, &evaluation_input.document);

    let runtime = match Runtime::from_document(evaluation_input.document) {
        Ok(runtime) => runtime,
        Err(err) => {
            response.error = Some(err.to_string());
            let stdout = match command.format {
                OutputFormat::Text => format_evaluate_text(&response, command.explain),
                OutputFormat::Json => to_pretty_json(&response)?,
            };
            return Ok(RunResult {
                exit_code: 1,
                stdout,
            });
        }
    };

    match runtime.evaluate(&target.feature_id, &target.owner_id, &context) {
        Ok(result) => {
            response.status = "ok";
            response.value = Some(result.value);
            response.explanation = result.explanation;
        }
        Err(err) => {
            response.error = Some(err.to_string());
        }
    }

    let failed = response.status != "ok";
    let stdout = match command.format {
        OutputFormat::Text => format_evaluate_text(&response, command.explain),
        OutputFormat::Json => to_pretty_json(&response)?,
    };

    Ok(RunResult {
        exit_code: if failed { 1 } else { 0 },
        stdout,
    })
}

fn run_lint(command: LintCommand) -> Result<RunResult, CliError> {
    let sources = read_lint_inputs(&command)?;
    let library_context =
        load_library_context(command.stdlib.as_deref(), lint_context_path(&command)?)?;
    let context_modules = sources
        .iter()
        .filter_map(|source| parse_source(source).ok())
        .collect::<Vec<_>>();
    let reports = sources
        .iter()
        .map(|source| {
            lint_text(
                &source.content,
                &source.source_name,
                source.language,
                &context_modules,
                &library_context.document,
            )
        })
        .collect::<Vec<_>>();
    let response = LintResponse {
        project_descriptor: library_context.project_descriptor_output(),
        reports,
    };

    let failing = lint_should_fail(&response.reports, command.warnings_as_errors);
    let stdout = if command.quiet {
        String::new()
    } else {
        match command.format {
            OutputFormat::Text => format_lint_text(&response),
            OutputFormat::Json => to_pretty_json(&response)?,
        }
    };

    Ok(RunResult {
        exit_code: if failing { 1 } else { 0 },
        stdout,
    })
}

fn run_package(command: PackageCommand) -> Result<RunResult, CliError> {
    match command.command {
        PackageSubcommand::Build(command) => run_package_build(command),
    }
}

fn run_completions(command: CompletionsCommand) -> Result<RunResult, CliError> {
    let mut clap_command = Cli::command();
    let mut buffer = Vec::new();
    generate(command.shell, &mut clap_command, "mercurio", &mut buffer);
    let stdout = String::from_utf8(buffer).map_err(|err| {
        CliError::execution(format!(
            "failed to render completion script as UTF-8: {err}"
        ))
    })?;

    Ok(RunResult {
        exit_code: 0,
        stdout,
    })
}

fn run_project(command: ProjectCommand) -> Result<RunResult, CliError> {
    match command.command {
        ProjectSubcommand::New(command) => run_project_new(command),
    }
}

fn run_project_new(command: ProjectNewCommand) -> Result<RunResult, CliError> {
    if command.path.exists() && !command.path.is_dir() {
        return Err(CliError::usage(format!(
            "project path exists and is not a directory: {}",
            command.path.display()
        )));
    }

    if command.path.is_dir() && !command.force && !directory_is_empty(&command.path)? {
        return Err(CliError::usage(format!(
            "project directory is not empty: {}; use --force to write scaffold files",
            command.path.display()
        )));
    }

    let project_name = command
        .name
        .clone()
        .unwrap_or_else(|| derive_project_name(&command.path));
    let package_name = sanitize_sysml_identifier(&project_name);
    let src_dir = command.path.join("src");
    let descriptor_path = command.path.join(PROJECT_DESCRIPTOR_FILE_NAME);
    let sample_path = src_dir.join("main.sysml");

    if !command.force {
        for path in [&descriptor_path, &sample_path] {
            if path.exists() {
                return Err(CliError::usage(format!(
                    "project scaffold file already exists: {}; use --force to overwrite it",
                    path.display()
                )));
            }
        }
    }

    std::fs::create_dir_all(&src_dir).map_err(|err| {
        CliError::execution(format!(
            "failed to create project directory {}: {err}",
            src_dir.display()
        ))
    })?;

    let descriptor = ProjectDescriptor {
        version: 1,
        name: Some(project_name),
        baseline_libraries: Vec::new(),
        libraries: Vec::new(),
    };
    let descriptor_json = to_pretty_json(&descriptor)?;
    std::fs::write(&descriptor_path, descriptor_json).map_err(|err| {
        CliError::execution(format!(
            "failed to write project descriptor {}: {err}",
            descriptor_path.display()
        ))
    })?;

    let sample = format!("package {package_name} {{\n    part def System;\n}}\n");
    std::fs::write(&sample_path, sample).map_err(|err| {
        CliError::execution(format!(
            "failed to write sample file {}: {err}",
            sample_path.display()
        ))
    })?;

    let stdout = if command.quiet {
        String::new()
    } else {
        format!(
            "created: {}\ndescriptor: {}\nsample: {}\n",
            command.path.display(),
            descriptor_path.display(),
            sample_path.display()
        )
    };

    Ok(RunResult {
        exit_code: 0,
        stdout,
    })
}

fn run_package_build(command: PackageBuildCommand) -> Result<RunResult, CliError> {
    let sources = read_package_sources(&command.files)?;
    let package_name = command
        .name
        .clone()
        .unwrap_or_else(|| derive_package_name(&command.out));
    let package = KparPackageBuild {
        name: package_name,
        version: command.version.clone(),
        sources,
    };
    let library_context =
        load_library_context(command.stdlib.as_deref(), package_context_path(&command)?)?;
    let temp_path = temp_kpar_path(&command.out)?;

    write_kpar_package(&temp_path, &package)
        .map_err(|err| CliError::execution(format!("failed to write package: {err}")))?;

    let validation = LibraryProviderConfig::KparFile {
        path: temp_path.display().to_string(),
    }
    .resolve_with_context("package", None, Some(&library_context.document));

    let artifact = match validation {
        Ok(artifact) => artifact,
        Err(err) => {
            let _ = std::fs::remove_file(&temp_path);
            let stdout = format!("package validation failed: {err}\n");
            return Ok(RunResult {
                exit_code: 1,
                stdout,
            });
        }
    };

    if let Some(parent) = command.out.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            CliError::execution(format!(
                "failed to create output directory {}: {err}",
                parent.display()
            ))
        })?;
    }
    std::fs::copy(&temp_path, &command.out).map_err(|err| {
        CliError::execution(format!(
            "failed to write output package {}: {err}",
            command.out.display()
        ))
    })?;
    std::fs::remove_file(&temp_path).map_err(|err| {
        CliError::execution(format!(
            "failed to remove temporary package {}: {err}",
            temp_path.display()
        ))
    })?;

    let stdout = if command.quiet {
        String::new()
    } else {
        format!(
            "wrote: {}\nproject_descriptor: {}\nsources: {}\nelements: {}\n",
            command.out.display(),
            library_context.project_descriptor_text(),
            package.sources.len(),
            artifact.document.elements.len()
        )
    };

    Ok(RunResult {
        exit_code: 0,
        stdout,
    })
}

fn read_single_input(
    input: &SingleInput,
    language: Option<LanguageArg>,
) -> Result<SourceInput, CliError> {
    match (&input.file, &input.text) {
        (Some(_), Some(_)) => Err(CliError::usage("provide exactly one of --file or --text")),
        (None, None) => Err(CliError::usage("provide exactly one of --file or --text")),
        (Some(path), None) => read_file_source(path, language),
        (None, Some(text)) => read_text_source(text, language),
    }
}

fn read_evaluate_input(command: &EvaluateCommand) -> Result<EvaluateInput, CliError> {
    let input_count = usize::from(command.input.file.is_some())
        + usize::from(command.input.text.is_some())
        + usize::from(command.kir.is_some());
    if input_count != 1 {
        return Err(CliError::usage(
            "provide exactly one of --file, --text, or --kir",
        ));
    }

    if let Some(path) = &command.kir {
        let document = KirDocument::from_path(path).map_err(|err| {
            CliError::execution(format!(
                "failed to load KIR document {}: {err}",
                path.display()
            ))
        })?;
        return Ok(EvaluateInput {
            source: path.display().to_string(),
            language: None,
            project_descriptor: ProjectDescriptorOutput {
                used: false,
                path: None,
                status: "not_applicable",
            },
            compile_status: "ok",
            diagnostics: Vec::new(),
            document,
        });
    }

    let source = read_single_input(&command.input, command.language)?;
    let library_context = load_library_context(
        command.stdlib.as_deref(),
        single_input_context_path(&command.input)?,
    )?;
    let response = compile_source(&source, &library_context.document);
    Ok(EvaluateInput {
        source: response.source,
        language: Some(response.language),
        project_descriptor: library_context.project_descriptor_output(),
        compile_status: response.status,
        diagnostics: response.diagnostics,
        document: response.document.unwrap_or_else(|| KirDocument {
            metadata: Default::default(),
            elements: Vec::new(),
        }),
    })
}

fn read_package_sources(paths: &[PathBuf]) -> Result<Vec<KparPackageSource>, CliError> {
    if paths.is_empty() {
        return Err(CliError::usage("provide at least one --file"));
    }

    let mut sources = Vec::new();
    for path in paths {
        collect_package_sources(path, path, &mut sources)?;
    }
    sources.sort_by(|left, right| left.path.cmp(&right.path));

    let mut seen = std::collections::BTreeSet::new();
    for source in &sources {
        if !seen.insert(source.path.clone()) {
            return Err(CliError::usage(format!(
                "duplicate package source path: {}",
                source.path
            )));
        }
    }

    Ok(sources)
}

fn collect_package_sources(
    root: &Path,
    path: &Path,
    sources: &mut Vec<KparPackageSource>,
) -> Result<(), CliError> {
    if path.is_file() {
        if SourceLanguage::from_path(path).is_none() {
            return Err(CliError::usage(format!(
                "unsupported file extension: {}",
                path.display()
            )));
        }
        let content = std::fs::read_to_string(path).map_err(|err| {
            CliError::execution(format!("failed to read {}: {err}", path.display()))
        })?;
        sources.push(KparPackageSource {
            path: package_entry_path(root, path)?,
            content,
        });
        return Ok(());
    }

    if path.is_dir() {
        let mut entries = std::fs::read_dir(path)
            .map_err(|err| {
                CliError::execution(format!(
                    "failed to read directory {}: {err}",
                    path.display()
                ))
            })?
            .collect::<Result<Vec<_>, std::io::Error>>()
            .map_err(|err| CliError::execution(format!("failed to read directory entry: {err}")))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let entry_path = entry.path();
            if entry_path.is_dir() || SourceLanguage::from_path(&entry_path).is_some() {
                collect_package_sources(root, &entry_path, sources)?;
            }
        }
        return Ok(());
    }

    Err(CliError::usage(format!(
        "input does not exist: {}",
        path.display()
    )))
}

fn package_entry_path(root: &Path, path: &Path) -> Result<String, CliError> {
    let relative = if root.is_dir() {
        path.strip_prefix(root).unwrap_or(path)
    } else {
        path.file_name()
            .map(Path::new)
            .ok_or_else(|| CliError::usage(format!("invalid source path: {}", path.display())))?
    };
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn read_lint_inputs(command: &LintCommand) -> Result<Vec<SourceInput>, CliError> {
    if command.text.is_some() && !command.files.is_empty() {
        return Err(CliError::usage("provide --text or --file, not both"));
    }
    if command.text.is_none() && command.files.is_empty() {
        return Err(CliError::usage("provide at least one --file or --text"));
    }
    if let Some(text) = &command.text {
        return Ok(vec![read_text_source(text, command.language)?]);
    }

    let mut files = Vec::new();
    for path in &command.files {
        collect_lint_files(path, &mut files, command.language)?;
    }
    files.sort();
    files.dedup();

    files
        .iter()
        .map(|path| read_file_source(path, command.language))
        .collect()
}

fn read_text_source(text: &str, language: Option<LanguageArg>) -> Result<SourceInput, CliError> {
    let language = match language {
        None => SourceLanguage::Sysml,
        Some(LanguageArg::Auto) => {
            return Err(CliError::usage(
                "--language auto is not valid with --text; use sysml or kerml",
            ));
        }
        Some(LanguageArg::Sysml) => SourceLanguage::Sysml,
        Some(LanguageArg::Kerml) => SourceLanguage::Kerml,
    };

    Ok(SourceInput {
        source_name: inline_source_name(language).to_string(),
        language,
        content: text.to_string(),
    })
}

fn read_file_source(path: &Path, language: Option<LanguageArg>) -> Result<SourceInput, CliError> {
    let resolved_language = resolve_file_language(path, language)?;
    let content = std::fs::read_to_string(path)
        .map_err(|err| CliError::execution(format!("failed to read {}: {err}", path.display())))?;
    Ok(SourceInput {
        source_name: path.display().to_string(),
        language: resolved_language,
        content,
    })
}

fn resolve_file_language(
    path: &Path,
    language: Option<LanguageArg>,
) -> Result<SourceLanguage, CliError> {
    match language {
        None | Some(LanguageArg::Auto) => SourceLanguage::from_path(path).ok_or_else(|| {
            CliError::usage(format!(
                "cannot infer language from {}; use --language sysml|kerml",
                path.display()
            ))
        }),
        Some(LanguageArg::Sysml) => Ok(SourceLanguage::Sysml),
        Some(LanguageArg::Kerml) => Ok(SourceLanguage::Kerml),
    }
}

fn collect_lint_files(
    path: &Path,
    files: &mut Vec<PathBuf>,
    language: Option<LanguageArg>,
) -> Result<(), CliError> {
    if path.is_file() {
        if !matches!(language, None | Some(LanguageArg::Auto))
            || SourceLanguage::from_path(path).is_some()
        {
            files.push(path.to_path_buf());
            return Ok(());
        }
        return Err(CliError::usage(format!(
            "unsupported file extension: {}",
            path.display()
        )));
    }
    if path.is_dir() {
        for entry in std::fs::read_dir(path).map_err(|err| {
            CliError::execution(format!(
                "failed to read directory {}: {err}",
                path.display()
            ))
        })? {
            let entry = entry.map_err(|err| {
                CliError::execution(format!("failed to read directory entry: {err}"))
            })?;
            collect_lint_files(&entry.path(), files, language)?;
        }
        return Ok(());
    }

    Err(CliError::usage(format!(
        "input does not exist: {}",
        path.display()
    )))
}

fn resolve_evaluation_target(
    document: &KirDocument,
    owner: &str,
    feature: &str,
) -> Result<ResolvedEvaluationTarget, CliError> {
    let owner_id = resolve_owner_id(document, owner)?;
    let feature_id = resolve_feature_id(document, &owner_id, feature)?;
    Ok(ResolvedEvaluationTarget {
        owner_id,
        feature_id,
    })
}

fn resolve_owner_id(document: &KirDocument, owner: &str) -> Result<String, CliError> {
    if element_by_id(document, owner).is_some() {
        return Ok(owner.to_string());
    }
    if let Some(id) = resolve_qualified_name(document, owner, "owner")? {
        return Ok(id);
    }

    let type_id = format!("type.{owner}");
    if element_by_id(document, &type_id).is_some() {
        return Ok(type_id);
    }

    Err(CliError::usage(format!(
        "could not resolve owner `{owner}` as a qualified name or KIR id"
    )))
}

fn resolve_feature_id(
    document: &KirDocument,
    owner_id: &str,
    feature: &str,
) -> Result<String, CliError> {
    if element_by_id(document, feature).is_some() {
        return Ok(feature.to_string());
    }
    if let Some(id) = resolve_qualified_name(document, feature, "feature")? {
        return Ok(id);
    }

    if !feature.contains('.') {
        let owner_qualified_name = owner_qualified_name(document, owner_id);
        let relative_name = format!("{owner_qualified_name}.{feature}");
        if let Some(id) = resolve_qualified_name(document, &relative_name, "feature")? {
            return Ok(id);
        }

        let relative_id = format!("feature.{relative_name}");
        if element_by_id(document, &relative_id).is_some() {
            return Ok(relative_id);
        }
    }

    let feature_id = format!("feature.{feature}");
    if element_by_id(document, &feature_id).is_some() {
        return Ok(feature_id);
    }

    Err(CliError::usage(format!(
        "could not resolve feature `{feature}` as a qualified name, owner-relative name, or KIR id"
    )))
}

fn resolve_qualified_name(
    document: &KirDocument,
    qualified_name: &str,
    label: &str,
) -> Result<Option<String>, CliError> {
    let matches = document
        .elements
        .iter()
        .filter(|element| {
            element
                .properties
                .get("qualified_name")
                .and_then(Value::as_str)
                == Some(qualified_name)
        })
        .map(|element| element.id.clone())
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [] => Ok(None),
        [only] => Ok(Some(only.clone())),
        _ => Err(CliError::usage(format!(
            "ambiguous {label} `{qualified_name}`; candidates: {}",
            matches.join(", ")
        ))),
    }
}

fn element_by_id<'a>(document: &'a KirDocument, id: &str) -> Option<&'a mercurio_core::KirElement> {
    document.elements.iter().find(|element| element.id == id)
}

fn owner_qualified_name(document: &KirDocument, owner_id: &str) -> String {
    element_by_id(document, owner_id)
        .and_then(|element| {
            element
                .properties
                .get("qualified_name")
                .and_then(Value::as_str)
        })
        .map(str::to_string)
        .unwrap_or_else(|| strip_known_id_prefix(owner_id).to_string())
}

fn strip_known_id_prefix(id: &str) -> &str {
    for prefix in ["type.", "feature.", "pkg."] {
        if let Some(stripped) = id.strip_prefix(prefix) {
            return stripped;
        }
    }
    id
}

fn add_resolved_context_value_aliases(
    context: &mut mercurio_core::ExecutionContext,
    document: &KirDocument,
) {
    let aliases = context
        .values
        .iter()
        .filter_map(|((owner, feature), value)| {
            let owner_id = resolve_owner_id(document, owner).ok()?;
            if owner_id == *owner {
                return None;
            }
            Some(((owner_id, feature.clone()), value.clone()))
        })
        .collect::<Vec<_>>();

    for (key, value) in aliases {
        context.values.entry(key).or_insert(value);
    }
}

fn read_execution_context(
    command: &EvaluateCommand,
) -> Result<mercurio_core::ExecutionContext, CliError> {
    let context_input_count =
        usize::from(command.context_json.is_some()) + usize::from(command.context_file.is_some());
    if context_input_count > 1 {
        return Err(CliError::usage(
            "provide at most one of --context-json or --context-file",
        ));
    }

    let mut values = HashMap::new();
    if let Some(path) = &command.context_file {
        let content = std::fs::read_to_string(path).map_err(|err| {
            CliError::execution(format!(
                "failed to read context file {}: {err}",
                path.display()
            ))
        })?;
        extend_context_values(&mut values, &content)?;
    }
    if let Some(content) = &command.context_json {
        extend_context_values(&mut values, content)?;
    }
    for value in &command.values {
        let (key, raw_value) = value
            .split_once('=')
            .ok_or_else(|| CliError::usage("expected --value OWNER.FEATURE=JSON"))?;
        let (owner, feature) = key
            .rsplit_once('.')
            .ok_or_else(|| CliError::usage("expected --value OWNER.FEATURE=JSON"))?;
        let parsed = serde_json::from_str::<Value>(raw_value)
            .unwrap_or_else(|_| Value::String(raw_value.to_string()));
        values.insert((owner.to_string(), feature.to_string()), parsed);
    }

    Ok(mercurio_core::ExecutionContext { values, version: 0 })
}

fn extend_context_values(
    values: &mut HashMap<(String, String), Value>,
    content: &str,
) -> Result<(), CliError> {
    let parsed: BTreeMap<String, BTreeMap<String, Value>> = serde_json::from_str(content)
        .map_err(|err| CliError::usage(format!("invalid context JSON: {err}")))?;
    for (owner, features) in parsed {
        for (feature, value) in features {
            values.insert((owner.clone(), feature), value);
        }
    }
    Ok(())
}

fn parse_source(source: &SourceInput) -> Result<SysmlModule, Diagnostic> {
    match source.language {
        SourceLanguage::Sysml => parse_sysml(&source.content),
        SourceLanguage::Kerml => parse_kerml(&source.content),
    }
}

fn compile_source(source: &SourceInput, stdlib: &KirDocument) -> CompileResponse {
    match source.language {
        SourceLanguage::Sysml => {
            let report = compile_sysml_text_with_context_report(
                &source.content,
                &source.source_name,
                &[],
                stdlib,
            );
            CompileResponse {
                source: source.source_name.clone(),
                language: source.language,
                status: compile_status_str(report.status),
                project_descriptor: ProjectDescriptorOutput::not_set(),
                diagnostics: report.diagnostics,
                document: report.document,
            }
        }
        SourceLanguage::Kerml => {
            match compile_kerml_text(&source.content, &source.source_name, stdlib) {
                Ok(document) => CompileResponse {
                    source: source.source_name.clone(),
                    language: source.language,
                    status: "ok",
                    project_descriptor: ProjectDescriptorOutput::not_set(),
                    diagnostics: Vec::new(),
                    document: Some(document),
                },
                Err(diagnostic) => CompileResponse {
                    source: source.source_name.clone(),
                    language: source.language,
                    status: "failed",
                    project_descriptor: ProjectDescriptorOutput::not_set(),
                    diagnostics: vec![diagnostic],
                    document: None,
                },
            }
        }
    }
}

fn load_stdlib(path: Option<&Path>) -> Result<KirDocument, CliError> {
    let path = path
        .map(Path::to_path_buf)
        .unwrap_or_else(default_stdlib_path);
    KirDocument::from_path(&path).map_err(|err| {
        CliError::execution(format!("failed to load stdlib {}: {err}", path.display()))
    })
}

fn load_library_context(
    stdlib: Option<&Path>,
    open_path: PathBuf,
) -> Result<LibraryContext, CliError> {
    if let Some(stdlib) = stdlib {
        return load_stdlib(Some(stdlib)).map(|document| LibraryContext {
            document,
            project_descriptor: ProjectDescriptorUsage::OverriddenByStdlib,
        });
    }

    resolve_project_context(&open_path)
        .map(|context| LibraryContext {
            document: context.library_context_document,
            project_descriptor: context
                .descriptor_path
                .map(ProjectDescriptorUsage::Used)
                .unwrap_or(ProjectDescriptorUsage::NotFound),
        })
        .map_err(|err| {
            CliError::execution(format!(
                "failed to load project context for {}: {err}",
                open_path.display()
            ))
        })
}

#[derive(Debug)]
struct LibraryContext {
    document: KirDocument,
    project_descriptor: ProjectDescriptorUsage,
}

impl LibraryContext {
    fn project_descriptor_output(&self) -> ProjectDescriptorOutput {
        ProjectDescriptorOutput::from_usage(&self.project_descriptor)
    }

    fn project_descriptor_text(&self) -> String {
        self.project_descriptor_output().to_text()
    }
}

#[derive(Debug)]
enum ProjectDescriptorUsage {
    Used(PathBuf),
    NotFound,
    OverriddenByStdlib,
}

#[derive(Debug, Clone, Serialize)]
struct ProjectDescriptorOutput {
    used: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    status: &'static str,
}

impl ProjectDescriptorOutput {
    fn not_set() -> Self {
        Self {
            used: false,
            path: None,
            status: "not_set",
        }
    }

    fn from_usage(usage: &ProjectDescriptorUsage) -> Self {
        match usage {
            ProjectDescriptorUsage::Used(path) => Self {
                used: true,
                path: Some(path.display().to_string()),
                status: "used",
            },
            ProjectDescriptorUsage::NotFound => Self {
                used: false,
                path: None,
                status: "not_found",
            },
            ProjectDescriptorUsage::OverriddenByStdlib => Self {
                used: false,
                path: None,
                status: "overridden_by_stdlib",
            },
        }
    }

    fn to_text(&self) -> String {
        match &self.path {
            Some(path) => path.clone(),
            None => self.status.to_string(),
        }
    }
}

fn single_input_context_path(input: &SingleInput) -> Result<PathBuf, CliError> {
    if let Some(path) = &input.file {
        return Ok(path.clone());
    }
    current_directory_context_path()
}

fn lint_context_path(command: &LintCommand) -> Result<PathBuf, CliError> {
    command
        .files
        .first()
        .cloned()
        .map(Ok)
        .unwrap_or_else(current_directory_context_path)
}

fn package_context_path(command: &PackageBuildCommand) -> Result<PathBuf, CliError> {
    command
        .files
        .first()
        .cloned()
        .map(Ok)
        .unwrap_or_else(current_directory_context_path)
}

fn current_directory_context_path() -> Result<PathBuf, CliError> {
    std::env::current_dir()
        .map_err(|err| CliError::execution(format!("failed to read current directory: {err}")))
}

fn compile_status_str(status: SemanticCompileStatus) -> &'static str {
    match status {
        SemanticCompileStatus::Ok => "ok",
        SemanticCompileStatus::Partial => "partial",
        SemanticCompileStatus::Failed => "failed",
    }
}

fn inline_source_name(language: SourceLanguage) -> &'static str {
    match language {
        SourceLanguage::Sysml => "<inline.sysml>",
        SourceLanguage::Kerml => "<inline.kerml>",
    }
}

fn derive_package_name(out: &Path) -> String {
    out.file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("package")
        .to_string()
}

fn derive_project_name(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("mercurio-project")
        .to_string()
}

fn sanitize_sysml_identifier(value: &str) -> String {
    let mut identifier = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            identifier.push(character);
        } else if !identifier.ends_with('_') {
            identifier.push('_');
        }
    }

    let identifier = identifier.trim_matches('_');
    let mut identifier = if identifier.is_empty() {
        "Project".to_string()
    } else {
        identifier.to_string()
    };

    if identifier
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_digit())
    {
        identifier.insert(0, '_');
    }

    identifier
}

fn directory_is_empty(path: &Path) -> Result<bool, CliError> {
    let mut entries = std::fs::read_dir(path).map_err(|err| {
        CliError::execution(format!(
            "failed to read project directory {}: {err}",
            path.display()
        ))
    })?;
    Ok(entries.next().is_none())
}

fn temp_kpar_path(out: &Path) -> Result<PathBuf, CliError> {
    let file_name = out
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| CliError::usage(format!("invalid output path: {}", out.display())))?;
    let temp_name = format!(".{file_name}.{}.tmp", std::process::id());
    Ok(out
        .parent()
        .map(|parent| parent.join(&temp_name))
        .unwrap_or_else(|| PathBuf::from(temp_name)))
}

#[derive(Serialize)]
struct ParseResponse {
    source: String,
    language: SourceLanguage,
    status: &'static str,
    diagnostics: Vec<Diagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ast: Option<SysmlModule>,
}

#[derive(Serialize)]
struct CompileResponse {
    source: String,
    language: SourceLanguage,
    status: &'static str,
    project_descriptor: ProjectDescriptorOutput,
    diagnostics: Vec<Diagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    document: Option<KirDocument>,
}

#[derive(Serialize)]
struct LintResponse {
    project_descriptor: ProjectDescriptorOutput,
    reports: Vec<LintReport>,
}

#[derive(Serialize)]
struct EvaluateResponse {
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<SourceLanguage>,
    project_descriptor: ProjectDescriptorOutput,
    compile_status: &'static str,
    diagnostics: Vec<Diagnostic>,
    feature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    feature_id: Option<String>,
    owner: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_id: Option<String>,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    explanation: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn format_parse_text(response: &ParseResponse) -> String {
    let mut output = String::new();
    output.push_str(&format!("source: {}\n", response.source));
    output.push_str(&format!("language: {}\n", response.language));
    output.push_str(&format!("status: {}\n", response.status));

    if let Some(module) = &response.ast {
        output.push_str(&format!(
            "package: {}\n",
            module
                .package
                .as_ref()
                .map(|package| package.name.as_dot_string())
                .unwrap_or_else(|| "<none>".to_string())
        ));
        output.push_str(&format!("top-level members: {}\n", module.members.len()));
        for (kind, count) in declaration_counts(module) {
            output.push_str(&format!("{kind}: {count}\n"));
        }
    }

    for diagnostic in &response.diagnostics {
        output.push_str(&format!("error: {diagnostic}\n"));
    }

    output
}

fn format_compile_text(response: &CompileResponse) -> String {
    let mut output = String::new();
    output.push_str(&format!("source: {}\n", response.source));
    output.push_str(&format!("language: {}\n", response.language));
    output.push_str(&format!("status: {}\n", response.status));
    output.push_str(&format!(
        "project_descriptor: {}\n",
        response.project_descriptor.to_text()
    ));
    output.push_str(&format!("diagnostics: {}\n", response.diagnostics.len()));
    output.push_str(&format!(
        "elements: {}\n",
        response
            .document
            .as_ref()
            .map(|document| document.elements.len())
            .unwrap_or(0)
    ));
    for diagnostic in &response.diagnostics {
        output.push_str(&format!("diagnostic: {diagnostic}\n"));
    }
    output
}

fn format_evaluate_text(response: &EvaluateResponse, explain: bool) -> String {
    let mut output = String::new();
    output.push_str(&format!("source: {}\n", response.source));
    if let Some(language) = response.language {
        output.push_str(&format!("language: {language}\n"));
    }
    output.push_str(&format!(
        "project_descriptor: {}\n",
        response.project_descriptor.to_text()
    ));
    output.push_str(&format!("compile_status: {}\n", response.compile_status));
    output.push_str(&format!("diagnostics: {}\n", response.diagnostics.len()));
    output.push_str(&format!("feature: {}\n", response.feature));
    if let Some(feature_id) = &response.feature_id {
        output.push_str(&format!("feature_id: {feature_id}\n"));
    }
    output.push_str(&format!("owner: {}\n", response.owner));
    if let Some(owner_id) = &response.owner_id {
        output.push_str(&format!("owner_id: {owner_id}\n"));
    }
    output.push_str(&format!("status: {}\n", response.status));
    if let Some(value) = &response.value {
        output.push_str(&format!("value: {value}\n"));
    }
    if let Some(error) = &response.error {
        output.push_str(&format!("error: {error}\n"));
    }
    for diagnostic in &response.diagnostics {
        output.push_str(&format!("diagnostic: {diagnostic}\n"));
    }
    if explain {
        for item in &response.explanation {
            output.push_str(&format!("explain: {item}\n"));
        }
    }
    output
}

fn format_lint_text(response: &LintResponse) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "project_descriptor: {}\n",
        response.project_descriptor.to_text()
    ));
    for report in &response.reports {
        if report.diagnostics.is_empty() {
            output.push_str(&format!("{}: ok\n", report.source_name));
            continue;
        }
        for diagnostic in &report.diagnostics {
            output.push_str(&format!(
                "{}: {} [{}] {}\n",
                report.source_name, diagnostic.severity, diagnostic.code, diagnostic.message
            ));
            if let Some(span) = &diagnostic.span {
                output.push_str(&format!(
                    "  at {}:{}-{}:{}\n",
                    span.start_line, span.start_col, span.end_line, span.end_col
                ));
            }
        }
    }
    output
}

fn declaration_counts(module: &SysmlModule) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    for declaration in &module.members {
        count_declaration(declaration, &mut counts);
    }
    counts
}

fn count_declaration<'a>(declaration: &'a Declaration, counts: &mut BTreeMap<&'static str, usize>) {
    let key = match declaration {
        Declaration::Package(package) => {
            for member in &package.members {
                count_declaration(member, counts);
            }
            "packages"
        }
        Declaration::Import(_) => "imports",
        Declaration::PartDefinition(definition) => {
            for member in &definition.members {
                count_declaration(member, counts);
            }
            "part definitions"
        }
        Declaration::PartUsage(usage) => {
            for member in &usage.body_members {
                count_declaration(member, counts);
            }
            "part usages"
        }
        Declaration::GenericDefinition(definition) => {
            for member in &definition.members {
                count_declaration(member, counts);
            }
            "generic definitions"
        }
        Declaration::GenericUsage(usage) => {
            for member in &usage.body_members {
                count_declaration(member, counts);
            }
            "generic usages"
        }
        Declaration::Alias(_) => "aliases",
    };
    *counts.entry(key).or_insert(0) += 1;
}

fn lint_should_fail(reports: &[LintReport], warnings_as_errors: bool) -> bool {
    reports.iter().any(|report| {
        report.diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == LintSeverity::Error
                || (warnings_as_errors && diagnostic.severity == LintSeverity::Warning)
        })
    })
}

fn to_pretty_json(value: &impl Serialize) -> Result<String, CliError> {
    serde_json::to_string_pretty(value)
        .map(|mut value| {
            value.push('\n');
            value
        })
        .map_err(|err| CliError::execution(format!("failed to serialize JSON: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn run_args(args: &[&str]) -> Result<RunResult, CliError> {
        let cli = Cli::try_parse_from(std::iter::once("mercurio").chain(args.iter().copied()))
            .map_err(|err| CliError::usage(err.to_string()))?;
        run(cli)
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nanos}"))
    }

    #[test]
    fn parse_text_sysml_succeeds() {
        let result = run_args(&["parse", "--text", "package Demo { part def Vehicle; }"]).unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("status: ok"));
        assert!(result.stdout.contains("package: Demo"));
    }

    #[test]
    fn parse_file_kerml_succeeds() {
        let root = temp_dir("mercurio-cli-kerml");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("model.kerml");
        std::fs::write(&path, "package Demo { classifier Vehicle; }").unwrap();

        let result = run_args(&["parse", "--file", path.to_str().unwrap()]).unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("language: kerml"));
        assert!(result.stdout.contains("package: Demo"));
    }

    #[test]
    fn compile_text_json_returns_document() {
        let result = run_args(&[
            "compile",
            "--text",
            "package Demo { part def Vehicle; }",
            "--format",
            "json",
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
        let json: serde_json::Value = serde_json::from_str(&result.stdout).unwrap();
        assert_eq!(json["status"], "ok");
        assert!(json["document"]["elements"].as_array().unwrap().len() > 0);
    }

    #[test]
    fn evaluate_kir_returns_value() {
        let root = temp_dir("mercurio-cli-evaluate");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("eval.kir.json");
        std::fs::write(
            &path,
            serde_json::to_string(&sample_evaluation_document()).unwrap(),
        )
        .unwrap();

        let result = run_args(&[
            "evaluate",
            "--kir",
            path.to_str().unwrap(),
            "--feature",
            "feature.EvalDemo.Vehicle.totalMass",
            "--owner",
            "type.EvalDemo.Vehicle",
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("status: ok"));
        assert!(result.stdout.contains("value: 8.0"));
    }

    #[test]
    fn evaluate_kir_accepts_qualified_names() {
        let root = temp_dir("mercurio-cli-evaluate-qualified");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("eval.kir.json");
        std::fs::write(
            &path,
            serde_json::to_string(&sample_evaluation_document()).unwrap(),
        )
        .unwrap();

        let result = run_args(&[
            "evaluate",
            "--kir",
            path.to_str().unwrap(),
            "--feature",
            "totalMass",
            "--owner",
            "EvalDemo.Vehicle",
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("feature: totalMass"));
        assert!(
            result
                .stdout
                .contains("feature_id: feature.EvalDemo.Vehicle.totalMass")
        );
        assert!(result.stdout.contains("owner: EvalDemo.Vehicle"));
        assert!(result.stdout.contains("owner_id: type.EvalDemo.Vehicle"));
        assert!(result.stdout.contains("value: 8.0"));
    }

    #[test]
    fn evaluate_text_accepts_qualified_names() {
        let result = run_args(&[
            "evaluate",
            "--text",
            "package Demo { part def Vehicle { attribute mass = 40+(2); } }",
            "--feature",
            "mass",
            "--owner",
            "Demo.Vehicle",
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(
            result
                .stdout
                .contains("feature_id: feature.Demo.Vehicle.mass")
        );
        assert!(result.stdout.contains("owner_id: type.Demo.Vehicle"));
        assert!(result.stdout.contains("value: 42.0"));
    }

    #[test]
    fn evaluate_kir_json_includes_result() {
        let root = temp_dir("mercurio-cli-evaluate-json");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("eval.kir.json");
        std::fs::write(
            &path,
            serde_json::to_string(&sample_evaluation_document()).unwrap(),
        )
        .unwrap();

        let result = run_args(&[
            "evaluate",
            "--kir",
            path.to_str().unwrap(),
            "--feature",
            "feature.EvalDemo.Vehicle.totalMass",
            "--owner",
            "type.EvalDemo.Vehicle",
            "--format",
            "json",
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
        let json: serde_json::Value = serde_json::from_str(&result.stdout).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["value"], 8.0);
        assert_eq!(json["owner_id"], "type.EvalDemo.Vehicle");
        assert_eq!(json["feature_id"], "feature.EvalDemo.Vehicle.totalMass");
        assert_eq!(json["project_descriptor"]["status"], "not_applicable");
    }

    #[test]
    fn lint_file_directory_scans_model_files() {
        let root = temp_dir("mercurio-cli-lint");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.sysml"), "package A { part def Vehicle; }").unwrap();
        std::fs::write(root.join("b.kerml"), "package B { classifier Vehicle; }").unwrap();

        let result = run_args(&["lint", "--file", root.to_str().unwrap()]).unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("a.sysml"));
        assert!(result.stdout.contains("b.kerml"));
    }

    fn sample_evaluation_document() -> KirDocument {
        KirDocument {
            metadata: Default::default(),
            elements: vec![
                mercurio_core::KirElement {
                    id: "type.EvalDemo.Engine".to_string(),
                    kind: "SysML::Systems::PartDefinition".to_string(),
                    layer: 2,
                    properties: [
                        (
                            "qualified_name".to_string(),
                            serde_json::json!("EvalDemo.Engine"),
                        ),
                        (
                            "features".to_string(),
                            serde_json::json!(["feature.EvalDemo.Engine.mass"]),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                },
                mercurio_core::KirElement {
                    id: "feature.EvalDemo.Engine.mass".to_string(),
                    kind: "KerML::Core::Feature".to_string(),
                    layer: 2,
                    properties: [
                        (
                            "qualified_name".to_string(),
                            serde_json::json!("EvalDemo.Engine.mass"),
                        ),
                        ("declared_name".to_string(), serde_json::json!("mass")),
                        (
                            "expression_ir".to_string(),
                            serde_json::json!({"kind": "literal", "value": 4.0}),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                },
                mercurio_core::KirElement {
                    id: "type.EvalDemo.Vehicle".to_string(),
                    kind: "SysML::Systems::PartDefinition".to_string(),
                    layer: 2,
                    properties: [
                        (
                            "qualified_name".to_string(),
                            serde_json::json!("EvalDemo.Vehicle"),
                        ),
                        (
                            "features".to_string(),
                            serde_json::json!([
                                "feature.EvalDemo.Vehicle.leftEngine",
                                "feature.EvalDemo.Vehicle.rightEngine"
                            ]),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                },
                mercurio_core::KirElement {
                    id: "feature.EvalDemo.Vehicle.leftEngine".to_string(),
                    kind: "SysML::Parts::PartUsage".to_string(),
                    layer: 2,
                    properties: [
                        (
                            "qualified_name".to_string(),
                            serde_json::json!("EvalDemo.Vehicle.leftEngine"),
                        ),
                        ("declared_name".to_string(), serde_json::json!("leftEngine")),
                        (
                            "type".to_string(),
                            serde_json::json!("type.EvalDemo.Engine"),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                },
                mercurio_core::KirElement {
                    id: "feature.EvalDemo.Vehicle.rightEngine".to_string(),
                    kind: "SysML::Parts::PartUsage".to_string(),
                    layer: 2,
                    properties: [
                        (
                            "qualified_name".to_string(),
                            serde_json::json!("EvalDemo.Vehicle.rightEngine"),
                        ),
                        (
                            "declared_name".to_string(),
                            serde_json::json!("rightEngine"),
                        ),
                        (
                            "type".to_string(),
                            serde_json::json!("type.EvalDemo.Engine"),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                },
                mercurio_core::KirElement {
                    id: "feature.EvalDemo.Vehicle.totalMass".to_string(),
                    kind: "KerML::Core::Feature".to_string(),
                    layer: 2,
                    properties: [
                        (
                            "qualified_name".to_string(),
                            serde_json::json!("EvalDemo.Vehicle.totalMass"),
                        ),
                        ("declared_name".to_string(), serde_json::json!("totalMass")),
                        (
                            "expression_ir".to_string(),
                            serde_json::json!({
                                "kind": "binary",
                                "op": "add",
                                "left": {
                                    "kind": "call",
                                    "function": "sum",
                                    "args": [{
                                        "kind": "path",
                                        "root": "self",
                                        "segments": ["leftEngine", "mass"]
                                    }]
                                },
                                "right": {
                                    "kind": "call",
                                    "function": "sum",
                                    "args": [{
                                        "kind": "path",
                                        "root": "self",
                                        "segments": ["rightEngine", "mass"]
                                    }]
                                }
                            }),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                },
            ],
        }
    }

    #[test]
    fn rejects_both_file_and_text() {
        let root = temp_dir("mercurio-cli-both");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("model.sysml");
        std::fs::write(&path, "package Demo { }").unwrap();

        let err = run_args(&[
            "parse",
            "--file",
            path.to_str().unwrap(),
            "--text",
            "package Demo { }",
        ])
        .unwrap_err();

        assert_eq!(err.code, 2);
    }

    #[test]
    fn rejects_missing_input() {
        let err = run_args(&["compile"]).unwrap_err();

        assert_eq!(err.code, 2);
    }

    #[test]
    fn rejects_text_language_auto() {
        let err =
            run_args(&["lint", "--text", "package Demo { }", "--language", "auto"]).unwrap_err();

        assert_eq!(err.code, 2);
    }

    #[test]
    fn diagnostic_returns_exit_code_one() {
        let result = run_args(&["parse", "--text", "package Demo { part def ; }"]).unwrap();

        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn package_build_file_writes_kpar() {
        let root = temp_dir("mercurio-cli-package");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("model.sysml");
        let out_path = root.join("model.kpar");
        std::fs::write(&source_path, "package Demo { part def Vehicle; }").unwrap();

        let result = run_args(&[
            "package",
            "build",
            "--file",
            source_path.to_str().unwrap(),
            "--out",
            out_path.to_str().unwrap(),
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(out_path.exists());
        let artifact = LibraryProviderConfig::KparFile {
            path: out_path.display().to_string(),
        }
        .resolve("demo")
        .unwrap();
        assert!(
            artifact
                .document
                .elements
                .iter()
                .any(|element| element.id == "type.Demo.Vehicle")
        );
    }

    #[test]
    fn compile_file_uses_project_descriptor_context() {
        let root = temp_dir("mercurio-cli-project-context");
        let src_dir = root.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let source_path = src_dir.join("model.sysml");
        std::fs::write(&source_path, "package Demo { part def Vehicle; }").unwrap();
        std::fs::write(
            root.join(PROJECT_DESCRIPTOR_FILE_NAME),
            r#"{
  "version": 1,
  "baseline_libraries": [
    {
      "id": "missing",
      "provider": {
        "kind": "precompiled_kir_artifact",
        "path": "missing.kir.json"
      }
    }
  ]
}"#,
        )
        .unwrap();

        let err = run_args(&["compile", "--file", source_path.to_str().unwrap()]).unwrap_err();

        assert_eq!(err.code, 2);
        assert!(err.message.contains("failed to load project context"));
    }

    #[test]
    fn compile_stdlib_override_skips_project_descriptor_context() {
        let root = temp_dir("mercurio-cli-project-context-override");
        let src_dir = root.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let source_path = src_dir.join("model.sysml");
        std::fs::write(&source_path, "package Demo { part def Vehicle; }").unwrap();
        std::fs::write(
            root.join(PROJECT_DESCRIPTOR_FILE_NAME),
            r#"{
  "version": 1,
  "baseline_libraries": [
    {
      "id": "missing",
      "provider": {
        "kind": "precompiled_kir_artifact",
        "path": "missing.kir.json"
      }
    }
  ]
}"#,
        )
        .unwrap();
        let stdlib = default_stdlib_path();

        let result = run_args(&[
            "compile",
            "--file",
            source_path.to_str().unwrap(),
            "--stdlib",
            stdlib.to_str().unwrap(),
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("status: ok"));
    }

    #[test]
    fn completions_generates_shell_script() {
        let result = run_args(&["completions", "powershell"]).unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("Register-ArgumentCompleter"));
        assert!(result.stdout.contains("mercurio"));
    }

    #[test]
    fn project_new_creates_descriptor_and_sample_file() {
        let root = temp_dir("mercurio-cli-project-new");

        let result = run_args(&[
            "project",
            "new",
            root.to_str().unwrap(),
            "--name",
            "Demo Project",
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
        let descriptor_path = root.join(PROJECT_DESCRIPTOR_FILE_NAME);
        let sample_path = root.join("src").join("main.sysml");
        assert!(descriptor_path.exists());
        assert!(sample_path.exists());

        let descriptor = ProjectDescriptor::from_path(&descriptor_path).unwrap();
        assert_eq!(descriptor.version, 1);
        assert_eq!(descriptor.name.as_deref(), Some("Demo Project"));
        assert!(descriptor.baseline_libraries.is_empty());
        assert!(descriptor.libraries.is_empty());

        let sample = std::fs::read_to_string(sample_path).unwrap();
        assert!(sample.contains("package Demo_Project"));
        parse_sysml(&sample).unwrap();
    }

    #[test]
    fn project_new_rejects_non_empty_directory_without_force() {
        let root = temp_dir("mercurio-cli-project-non-empty");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("README.md"), "# Existing\n").unwrap();

        let err = run_args(&["project", "new", root.to_str().unwrap()]).unwrap_err();

        assert_eq!(err.code, 2);
        assert!(err.message.contains("not empty"));
    }

    #[test]
    fn project_new_force_writes_scaffold_into_non_empty_directory() {
        let root = temp_dir("mercurio-cli-project-force");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("README.md"), "# Existing\n").unwrap();

        let result = run_args(&["project", "new", root.to_str().unwrap(), "--force"]).unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(root.join("README.md").exists());
        assert!(root.join(PROJECT_DESCRIPTOR_FILE_NAME).exists());
        assert!(root.join("src").join("main.sysml").exists());
    }
}
