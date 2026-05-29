use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{Shell, generate};
use mercurio_core::frontend::ast::{Declaration, SysmlModule};
use mercurio_core::frontend::diagnostics::Diagnostic;
use mercurio_core::frontend::kerml::{compile_kerml_text, parse_kerml};
use mercurio_core::frontend::sysml::{compile_sysml_text_with_context_report, parse_sysml};
use mercurio_core::plugin_registry as registry;
use mercurio_core::{
    KirDocument, KparPackageBuild, KparPackageSource, LibraryProviderConfig, LintReport,
    LintSeverity, LocalPackageManifest, LocalPackageRepository, LocalPackageSource,
    PROJECT_DESCRIPTOR_FILE_NAME, ProjectDescriptor, QueryEngine, QueryResultSet, Runtime,
    SemanticCompileStatus, SourceLanguage, default_stdlib_path, lint_text, parse_query,
    resolve_project_context, write_kpar_package,
};
use serde::{Deserialize, Serialize};
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
    Query(QueryCommand),
    Lint(LintCommand),
    Package(PackageCommand),
    Plugin(PluginCommand),
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
    #[arg(long)]
    kpar: Option<PathBuf>,
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
    kpar: Option<PathBuf>,
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
struct QueryCommand {
    #[command(flatten)]
    input: SingleInput,
    #[arg(long)]
    kir: Option<PathBuf>,
    #[arg(long)]
    kpar: Option<PathBuf>,
    #[arg(long)]
    query: Option<String>,
    #[arg(long, value_name = "PATH")]
    query_file: Option<PathBuf>,
    #[arg(long, value_enum)]
    language: Option<LanguageArg>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
    #[arg(long)]
    stdlib: Option<PathBuf>,
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

#[derive(Debug, Args)]
struct PluginCommand {
    #[command(subcommand)]
    command: PluginSubcommand,
}

#[derive(Debug, Subcommand)]
enum PluginSubcommand {
    Install(PluginInstallCommand),
    List(PluginListCommand),
    Inspect(PluginInspectCommand),
}

#[derive(Debug, Args)]
struct PluginInstallCommand {
    manifest: PathBuf,
    #[arg(long = "from")]
    from: Option<String>,
    #[arg(long)]
    root: Option<PathBuf>,
    #[arg(long)]
    force: bool,
    #[arg(long)]
    quiet: bool,
}

#[derive(Debug, Args)]
struct PluginListCommand {
    #[arg(long)]
    root: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

#[derive(Debug, Args)]
struct PluginInspectCommand {
    id: String,
    #[arg(long)]
    version: Option<String>,
    #[arg(long)]
    root: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
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
    List(PackageListCommand),
    Inspect(PackageInspectCommand),
    Verify(PackageVerifyCommand),
    Compile(PackageCompileCommand),
    Publish(PackagePublishCommand),
    Install(PackageInstallCommand),
    Pull(PackagePullCommand),
}

#[derive(Debug, Args)]
struct PackageBuildCommand {
    #[arg(long = "file")]
    files: Vec<PathBuf>,
    #[arg(long)]
    kir: Option<PathBuf>,
    #[arg(long)]
    out: Option<PathBuf>,
    #[arg(long)]
    stdlib: Option<PathBuf>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    version: Option<String>,
    #[arg(long)]
    include_kir: bool,
    #[arg(long)]
    quiet: bool,
}

#[derive(Debug, Args)]
struct PackageListCommand {
    #[arg(long)]
    repo: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct PackageInspectCommand {
    name: String,
    #[arg(long)]
    version: String,
    #[arg(long)]
    repo: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct PackageVerifyCommand {
    name: String,
    #[arg(long)]
    version: String,
    #[arg(long)]
    repo: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

#[derive(Debug, Args)]
struct PackageCompileCommand {
    name: String,
    #[arg(long)]
    version: String,
    #[arg(long)]
    repo: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

#[derive(Debug, Args)]
struct PackagePublishCommand {
    name: String,
    #[arg(long)]
    version: String,
    #[arg(long)]
    to: String,
    #[arg(long)]
    repo: Option<PathBuf>,
    #[arg(long)]
    force: bool,
    #[arg(long)]
    quiet: bool,
}

#[derive(Debug, Args)]
struct PackageInstallCommand {
    coordinate: String,
    #[arg(long = "from")]
    from: String,
    #[arg(long)]
    repo: Option<PathBuf>,
    #[arg(long)]
    force: bool,
    #[arg(long)]
    quiet: bool,
}

#[derive(Debug, Args)]
struct PackagePullCommand {
    name: String,
    #[arg(long)]
    version: String,
    #[arg(long = "from")]
    from: String,
    #[arg(long)]
    repo: Option<PathBuf>,
    #[arg(long)]
    force: bool,
    #[arg(long)]
    quiet: bool,
}

#[derive(Debug, Clone, Args)]
struct SingleInput {
    #[arg(long)]
    file: Option<PathBuf>,
    #[arg(long)]
    text: Option<String>,
    #[arg(long)]
    url: Option<String>,
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

#[derive(Debug, Clone)]
struct ModelInput {
    source: String,
    document: KirDocument,
}

struct EvaluateInput {
    source: String,
    language: Option<SourceLanguage>,
    project_descriptor: ProjectDescriptorOutput,
    compile_status: &'static str,
    diagnostics: Vec<Diagnostic>,
    document: KirDocument,
}

struct QueryModelInput {
    source: String,
    project_descriptor: ProjectDescriptorOutput,
    document: KirDocument,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PluginManifestEnvelope {
    id: String,
    version: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    services: Vec<PluginServiceEnvelope>,
    #[serde(rename = "cliActions", alias = "cli_actions", default)]
    cli_actions: Vec<PluginCliActionEnvelope>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PluginServiceEnvelope {
    id: String,
    #[serde(default)]
    runtime: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PluginCliActionEnvelope {
    command: String,
    service: String,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
struct InstalledPluginSummary {
    id: String,
    version: String,
    name: String,
    services: Vec<String>,
    cli_actions: Vec<String>,
    manifest_path: String,
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

fn registry_error_to_cli(error: registry::PluginRegistryError) -> CliError {
    match error {
        registry::PluginRegistryError::Io(message) => CliError::execution(message),
        registry::PluginRegistryError::Invalid(message) => CliError::usage(message),
    }
}

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
        Command::Query(command) => run_query(command),
        Command::Lint(command) => run_lint(command),
        Command::Package(command) => run_package(command),
        Command::Plugin(command) => run_plugin(command),
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
    let input_count = single_input_count(&command.input) + usize::from(command.kpar.is_some());
    if input_count != 1 {
        return Err(CliError::usage(
            "provide exactly one of --file, --text, --url, or --kpar",
        ));
    }

    let library_context =
        load_library_context(command.stdlib.as_deref(), compile_context_path(&command)?)?;
    let mut response = if let Some(path) = &command.kpar {
        compile_kpar_model_input(path, &library_context.document)?
    } else if let Some(url) = &command.input.url
        && is_kpar_url(url)
    {
        compile_kpar_url_model_input(url, &library_context.document)?
    } else {
        let source = read_single_input(&command.input, command.language)?;
        compile_source(&source, &library_context.document)
    };
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

fn run_query(command: QueryCommand) -> Result<RunResult, CliError> {
    let model = read_query_model_input(&command)?;
    let query_text = read_query_text(&command)?;
    let query =
        parse_query(&query_text).map_err(|err| CliError::usage(format!("invalid query: {err}")))?;
    let result = QueryEngine::new(&model.document)
        .execute(&query)
        .map_err(|err| CliError::execution(format!("query failed: {err}")))?;

    let response = QueryResponse {
        source: model.source,
        project_descriptor: model.project_descriptor,
        result,
    };
    let stdout = match command.format {
        OutputFormat::Text => format_query_text(&response),
        OutputFormat::Json => to_pretty_json(&response)?,
    };

    Ok(RunResult {
        exit_code: 0,
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
        PackageSubcommand::List(command) => run_package_list(command),
        PackageSubcommand::Inspect(command) => run_package_inspect(command),
        PackageSubcommand::Verify(command) => run_package_verify(command),
        PackageSubcommand::Compile(command) => run_package_compile(command),
        PackageSubcommand::Publish(command) => run_package_publish(command),
        PackageSubcommand::Install(command) => run_package_install(command),
        PackageSubcommand::Pull(command) => run_package_pull(command),
    }
}

fn run_plugin(command: PluginCommand) -> Result<RunResult, CliError> {
    match command.command {
        PluginSubcommand::Install(command) => run_plugin_install(command),
        PluginSubcommand::List(command) => run_plugin_list(command),
        PluginSubcommand::Inspect(command) => run_plugin_inspect(command),
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
        libraries: Vec::new(),
        plugins: Vec::new(),
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
    let sources = if command.files.is_empty() {
        Vec::new()
    } else {
        read_package_sources(&command.files)?
    };
    if sources.is_empty() && command.kir.is_none() {
        return Err(CliError::usage(
            "provide at least one --file or a --kir document",
        ));
    }
    let precompiled_kir = command
        .kir
        .as_deref()
        .map(KirDocument::from_path)
        .transpose()
        .map_err(|err| CliError::execution(format!("failed to read KIR document: {err}")))?;
    let package_name = match (&command.name, &command.out) {
        (Some(name), _) => name.clone(),
        (None, Some(out)) => derive_package_name(out),
        (None, None) => {
            return Err(CliError::usage(
                "provide --name when staging a package without --out",
            ));
        }
    };
    if command.out.is_none() && command.version.is_none() {
        return Err(CliError::usage(
            "provide --version when staging a package without --out",
        ));
    }
    let package = KparPackageBuild {
        name: package_name,
        version: command.version.clone(),
        sources,
        precompiled_kir,
    };
    let library_context =
        load_library_context(command.stdlib.as_deref(), package_context_path(&command)?)?;
    let output_path = command.out.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(LocalPackageRepository::package_file_name(
            &package.name,
            package.version.as_deref().unwrap_or("0.0.0"),
        ))
    });
    let temp_path = temp_kpar_path(&output_path)?;

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
    if command.include_kir && package.precompiled_kir.is_none() {
        let package = KparPackageBuild {
            precompiled_kir: Some(artifact.document.clone()),
            ..package.clone()
        };
        write_kpar_package(&temp_path, &package)
            .map_err(|err| CliError::execution(format!("failed to write package: {err}")))?;
        let validation = LibraryProviderConfig::KparFile {
            path: temp_path.display().to_string(),
        }
        .resolve_with_context("package", None, Some(&library_context.document));
        if let Err(err) = validation {
            let _ = std::fs::remove_file(&temp_path);
            let stdout = format!("package validation failed: {err}\n");
            return Ok(RunResult {
                exit_code: 1,
                stdout,
            });
        }
    }

    let wrote_path = if let Some(out) = &command.out {
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                CliError::execution(format!(
                    "failed to create output directory {}: {err}",
                    parent.display()
                ))
            })?;
        }
        std::fs::copy(&temp_path, out).map_err(|err| {
            CliError::execution(format!(
                "failed to write output package {}: {err}",
                out.display()
            ))
        })?;
        out.clone()
    } else {
        let version = package.version.as_deref().unwrap();
        let repo = LocalPackageRepository::default_user();
        let source = command.files.first().map(|path| LocalPackageSource {
            kind: if path.is_dir() { "directory" } else { "file" }.to_string(),
            path: path.display().to_string(),
        });
        repo.stage_kpar(&temp_path, &package.name, version, source)
            .map_err(|err| CliError::execution(format!("failed to stage package: {err}")))?;
        repo.package_path(&package.name, version)
    };
    let _ = std::fs::remove_file(&temp_path);

    let stdout = if command.quiet {
        String::new()
    } else {
        format!(
            "wrote: {}\nproject_descriptor: {}\nsources: {}\nelements: {}\n",
            wrote_path.display(),
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

fn run_package_list(command: PackageListCommand) -> Result<RunResult, CliError> {
    let repo = package_repo(command.repo);
    let mut rows = Vec::new();
    if repo.root().is_dir() {
        collect_package_manifest_rows(repo.root(), repo.root(), &mut rows)?;
    }
    rows.sort();
    let stdout = if rows.is_empty() {
        format!("repo: {}\npackages: 0\n", repo.root().display())
    } else {
        format!(
            "repo: {}\npackages: {}\n{}\n",
            repo.root().display(),
            rows.len(),
            rows.join("\n")
        )
    };
    Ok(RunResult {
        exit_code: 0,
        stdout,
    })
}

fn run_package_inspect(command: PackageInspectCommand) -> Result<RunResult, CliError> {
    let repo = package_repo(command.repo);
    let manifest = repo
        .read_manifest(&command.name, &command.version)
        .map_err(|err| CliError::execution(format!("failed to inspect package: {err}")))?;
    let stdout = serde_json::to_string_pretty(&manifest)
        .map(|json| format!("{json}\n"))
        .map_err(|err| CliError::execution(format!("failed to render manifest: {err}")))?;
    Ok(RunResult {
        exit_code: 0,
        stdout,
    })
}

fn run_package_verify(command: PackageVerifyCommand) -> Result<RunResult, CliError> {
    let repo = package_repo(command.repo);
    let verification = repo
        .verify_package(&command.name, &command.version)
        .map_err(|err| CliError::execution(format!("failed to verify package: {err}")))?;
    let stdout = match command.format {
        OutputFormat::Json => to_pretty_json(&verification)?,
        OutputFormat::Text => format!(
            "status: ok\npackage: {}:{}\nrepo: {}\nfile: {}\ndigest: {}\nsources: {}\nprecompiled_kir: {}\nelements: {}\n",
            verification.name,
            verification.version,
            repo.root().display(),
            verification.file,
            verification.digest,
            verification.source_count,
            verification.has_precompiled_kir,
            verification
                .precompiled_kir_element_count
                .map(|count| count.to_string())
                .unwrap_or_else(|| "n/a".to_string())
        ),
    };
    Ok(RunResult {
        exit_code: 0,
        stdout,
    })
}

fn run_package_compile(command: PackageCompileCommand) -> Result<RunResult, CliError> {
    let repo = package_repo(command.repo);
    let Some(path) = repo
        .find_package(&command.name, &command.version)
        .map_err(|err| CliError::execution(format!("failed to resolve package: {err}")))?
    else {
        return Err(CliError::execution(format!(
            "package {} version {} was not found in {}",
            command.name,
            command.version,
            repo.root().display()
        )));
    };
    run_compile(CompileCommand {
        input: SingleInput {
            file: None,
            text: None,
            url: None,
        },
        kpar: Some(path),
        language: None,
        format: command.format,
        stdlib: None,
    })
}

fn run_package_publish(command: PackagePublishCommand) -> Result<RunResult, CliError> {
    let source_repo = package_repo(command.repo);
    let (manifest, target_label) = if is_http_package_repository(&command.to) {
        let manifest = publish_to_http_package_repository(
            &source_repo,
            &command.to,
            &command.name,
            &command.version,
            command.force,
        )?;
        (manifest, command.to.clone())
    } else {
        let target_repo = package_publish_target_repo(&command.to)?;
        let manifest = source_repo
            .publish_to_repository(&target_repo, &command.name, &command.version, command.force)
            .map_err(|err| CliError::execution(format!("failed to publish package: {err}")))?;
        (manifest, target_repo.root().display().to_string())
    };
    let stdout = if command.quiet {
        String::new()
    } else {
        format!(
            "published: {}:{}\nfrom: {}\nto: {}\ndigest: {}\n",
            manifest.name,
            manifest.version,
            source_repo.root().display(),
            target_label,
            manifest.digest
        )
    };
    Ok(RunResult {
        exit_code: 0,
        stdout,
    })
}

fn run_package_pull(command: PackagePullCommand) -> Result<RunResult, CliError> {
    let target_repo = package_repo(command.repo);
    let source_repo = package_repository_target(&command.from, "pull source")?;
    let manifest = target_repo
        .pull_from_repository(&source_repo, &command.name, &command.version, command.force)
        .map_err(|err| CliError::execution(format!("failed to pull package: {err}")))?;
    let stdout = if command.quiet {
        String::new()
    } else {
        format!(
            "pulled: {}:{}\nfrom: {}\nto: {}\ndigest: {}\n",
            manifest.name,
            manifest.version,
            source_repo.root().display(),
            target_repo.root().display(),
            manifest.digest
        )
    };
    Ok(RunResult {
        exit_code: 0,
        stdout,
    })
}

fn run_plugin_install(command: PluginInstallCommand) -> Result<RunResult, CliError> {
    let install_source = resolve_plugin_install_input(&command)?;
    let source =
        registry::read_plugin_install_source(&install_source).map_err(registry_error_to_cli)?;
    let manifest: PluginManifestEnvelope = serde_json::from_value(source.manifest.clone())
        .map_err(|err| {
            CliError::usage(format!(
                "invalid plugin manifest {}: {err}",
                install_source.display()
            ))
        })?;
    validate_plugin_manifest(&manifest)?;
    let root = registry::plugin_registry_root(command.root);
    let target_path = registry::install_plugin_manifest(
        &root,
        &manifest.id,
        &manifest.version,
        &source.manifest,
        source.package_path.as_deref(),
        command.force,
    )
    .map_err(registry_error_to_cli)?;
    let stdout = if command.quiet {
        String::new()
    } else {
        format!(
            "installed plugin: {}:{}\nname: {}\nto: {}\n",
            manifest.id,
            manifest.version,
            manifest.name,
            target_path.display()
        )
    };
    Ok(RunResult {
        exit_code: 0,
        stdout,
    })
}

fn resolve_plugin_install_input(command: &PluginInstallCommand) -> Result<PathBuf, CliError> {
    let Some(source_repository) = &command.from else {
        return Ok(command.manifest.clone());
    };
    if source_repository.starts_with("oci://")
        || source_repository.starts_with("http://")
        || source_repository.starts_with("https://")
    {
        return Err(CliError::usage(
            "remote plugin package repositories are not implemented yet; use a local repository path or file:// path",
        ));
    }
    let coordinate = command.manifest.to_string_lossy();
    let (id, version) = parse_plugin_coordinate(&coordinate)?;
    let repo_path = source_repository
        .strip_prefix("file://")
        .or_else(|| source_repository.strip_prefix("file:"))
        .unwrap_or(source_repository);
    let root = PathBuf::from(repo_path);
    let id_segment = safe_package_path_segment(&id);
    let version_segment = safe_package_path_segment(&version);
    let candidates = [
        root.join(&id_segment)
            .join(&version_segment)
            .join("plugin.mpack"),
        root.join(&id_segment)
            .join(&version_segment)
            .join(format!("{}-{}.mpack", id_segment, version_segment)),
    ];
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            CliError::execution(format!(
                "plugin package {id}:{version} was not found in {}",
                root.display()
            ))
        })
}

fn parse_plugin_coordinate(coordinate: &str) -> Result<(String, String), CliError> {
    let Some(remainder) = coordinate.strip_prefix("mpack:") else {
        return Err(CliError::usage(
            "plugin coordinate must use mpack:<id>:<version>",
        ));
    };
    let Some((id, version)) = remainder.rsplit_once(':') else {
        return Err(CliError::usage(
            "plugin coordinate must use mpack:<id>:<version>",
        ));
    };
    if id.trim().is_empty() || version.trim().is_empty() {
        return Err(CliError::usage(
            "plugin coordinate must include non-empty id and version",
        ));
    }
    Ok((id.to_string(), version.to_string()))
}

fn run_plugin_list(command: PluginListCommand) -> Result<RunResult, CliError> {
    let root = registry::plugin_registry_root(command.root);
    let plugins = installed_plugin_summaries(&root)?;
    let stdout = match command.format {
        OutputFormat::Json => to_pretty_json(&plugins)?,
        OutputFormat::Text => format_installed_plugins_text(&plugins),
    };
    Ok(RunResult {
        exit_code: 0,
        stdout,
    })
}

fn run_plugin_inspect(command: PluginInspectCommand) -> Result<RunResult, CliError> {
    let root = registry::plugin_registry_root(command.root);
    let manifests = installed_plugin_manifests(&root)?;
    let matches = manifests
        .into_iter()
        .filter(|(_, manifest)| {
            manifest.id == command.id
                && command
                    .version
                    .as_deref()
                    .is_none_or(|version| manifest.version == version)
        })
        .collect::<Vec<_>>();
    let Some((path, manifest)) = select_plugin_manifest_match(matches, &command.id)? else {
        return Err(CliError::execution(format!(
            "plugin {} was not found in {}",
            command.id,
            root.display()
        )));
    };
    let stdout = match command.format {
        OutputFormat::Json => to_pretty_json(&manifest)?,
        OutputFormat::Text => format_plugin_manifest_text(&manifest, &path),
    };
    Ok(RunResult {
        exit_code: 0,
        stdout,
    })
}

fn run_package_install(command: PackageInstallCommand) -> Result<RunResult, CliError> {
    let target_repo = package_repo(command.repo);
    let (name, version) = parse_package_coordinate(&command.coordinate)?;
    let manifest = if is_http_package_repository(&command.from) {
        install_from_http_package_repository(
            &target_repo,
            &command.from,
            &name,
            &version,
            command.force,
        )?
    } else {
        let source_repo = package_repository_target(&command.from, "install source")?;
        target_repo
            .pull_from_repository(&source_repo, &name, &version, command.force)
            .map_err(|err| CliError::execution(format!("failed to install package: {err}")))?
    };
    let stdout = if command.quiet {
        String::new()
    } else {
        format!(
            "installed: {}:{}\nfrom: {}\nto: {}\ndigest: {}\n",
            manifest.name,
            manifest.version,
            command.from,
            target_repo.root().display(),
            manifest.digest
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
    match (&input.file, &input.text, &input.url) {
        (Some(path), None, None) => read_file_source(path, language),
        (None, Some(text), None) => read_text_source(text, language),
        (None, None, Some(url)) => read_url_source(url, language),
        _ => Err(CliError::usage(
            "provide exactly one of --file, --text, or --url",
        )),
    }
}

fn read_evaluate_input(command: &EvaluateCommand) -> Result<EvaluateInput, CliError> {
    let input_count = single_input_count(&command.input)
        + usize::from(command.kir.is_some())
        + usize::from(command.kpar.is_some());
    if input_count != 1 {
        return Err(CliError::usage(
            "provide exactly one of --file, --text, --url, --kir, or --kpar",
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

    if let Some(path) = &command.kpar {
        let library_context = load_library_context(command.stdlib.as_deref(), path.clone())?;
        let model = read_kpar_model_input(path, &library_context.document)?;
        return Ok(EvaluateInput {
            source: model.source,
            language: None,
            project_descriptor: library_context.project_descriptor_output(),
            compile_status: "ok",
            diagnostics: Vec::new(),
            document: model.document,
        });
    }

    if let Some(url) = &command.input.url
        && is_kpar_url(url)
    {
        let library_context =
            load_library_context(command.stdlib.as_deref(), current_directory_context_path()?)?;
        let model = read_kpar_url_model_input(url, &library_context.document)?;
        return Ok(EvaluateInput {
            source: model.source,
            language: None,
            project_descriptor: library_context.project_descriptor_output(),
            compile_status: "ok",
            diagnostics: Vec::new(),
            document: model.document,
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
        language: response.language,
        project_descriptor: library_context.project_descriptor_output(),
        compile_status: response.status,
        diagnostics: response.diagnostics,
        document: response.document.unwrap_or_else(|| KirDocument {
            metadata: Default::default(),
            elements: Vec::new(),
        }),
    })
}

fn read_query_model_input(command: &QueryCommand) -> Result<QueryModelInput, CliError> {
    let input_count = single_input_count(&command.input)
        + usize::from(command.kir.is_some())
        + usize::from(command.kpar.is_some());
    if input_count != 1 {
        return Err(CliError::usage(
            "provide exactly one of --file, --text, --url, --kir, or --kpar",
        ));
    }

    if let Some(path) = &command.kir {
        let document = KirDocument::from_path(path).map_err(|err| {
            CliError::execution(format!(
                "failed to load KIR document {}: {err}",
                path.display()
            ))
        })?;
        return Ok(QueryModelInput {
            source: path.display().to_string(),
            project_descriptor: ProjectDescriptorOutput {
                used: false,
                path: None,
                status: "not_applicable",
            },
            document,
        });
    }

    if let Some(path) = &command.kpar {
        let library_context = load_library_context(command.stdlib.as_deref(), path.clone())?;
        let model = read_kpar_model_input(path, &library_context.document)?;
        return Ok(QueryModelInput {
            source: model.source,
            project_descriptor: library_context.project_descriptor_output(),
            document: model.document,
        });
    }

    if let Some(url) = &command.input.url
        && is_kpar_url(url)
    {
        let library_context =
            load_library_context(command.stdlib.as_deref(), current_directory_context_path()?)?;
        let model = read_kpar_url_model_input(url, &library_context.document)?;
        return Ok(QueryModelInput {
            source: model.source,
            project_descriptor: library_context.project_descriptor_output(),
            document: model.document,
        });
    }

    let source = read_single_input(&command.input, command.language)?;
    let library_context = load_library_context(
        command.stdlib.as_deref(),
        single_input_context_path(&command.input)?,
    )?;
    let response = compile_source(&source, &library_context.document);
    if response.status == "failed" || !response.diagnostics.is_empty() {
        return Err(CliError::execution(format!(
            "compile diagnostics prevented query: {} diagnostic(s)",
            response.diagnostics.len()
        )));
    }
    let document = response
        .document
        .ok_or_else(|| CliError::execution("compile succeeded without producing a KIR document"))?;

    Ok(QueryModelInput {
        source: response.source,
        project_descriptor: library_context.project_descriptor_output(),
        document,
    })
}

fn read_query_text(command: &QueryCommand) -> Result<String, CliError> {
    match (&command.query, &command.query_file) {
        (Some(_), Some(_)) => Err(CliError::usage(
            "provide exactly one of --query or --query-file",
        )),
        (None, None) => Err(CliError::usage(
            "provide exactly one of --query or --query-file",
        )),
        (Some(query), None) => Ok(query.clone()),
        (None, Some(path)) => std::fs::read_to_string(path).map_err(|err| {
            CliError::execution(format!(
                "failed to read query file {}: {err}",
                path.display()
            ))
        }),
    }
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
    if path_has_mercurio_component(path) {
        return Ok(());
    }

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

fn path_has_mercurio_component(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str().to_str() == Some(".mercurio"))
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

fn read_url_source(url: &str, language: Option<LanguageArg>) -> Result<SourceInput, CliError> {
    let resolved_language = resolve_url_language(url, language)?;
    let content = download_url_text(url)?;
    Ok(SourceInput {
        source_name: url.to_string(),
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

fn resolve_url_language(
    url: &str,
    language: Option<LanguageArg>,
) -> Result<SourceLanguage, CliError> {
    match language {
        None | Some(LanguageArg::Auto) => {
            SourceLanguage::from_path(Path::new(url)).ok_or_else(|| {
                CliError::usage(format!(
                    "cannot infer language from {url}; use --language sysml|kerml"
                ))
            })
        }
        Some(LanguageArg::Sysml) => Ok(SourceLanguage::Sysml),
        Some(LanguageArg::Kerml) => Ok(SourceLanguage::Kerml),
    }
}

fn single_input_count(input: &SingleInput) -> usize {
    usize::from(input.file.is_some())
        + usize::from(input.text.is_some())
        + usize::from(input.url.is_some())
}

fn download_url_text(url: &str) -> Result<String, CliError> {
    reqwest::blocking::get(url)
        .and_then(|response| response.error_for_status())
        .map_err(|err| CliError::execution(format!("failed to fetch {url}: {err}")))?
        .text()
        .map_err(|err| CliError::execution(format!("failed to read response from {url}: {err}")))
}

fn download_url_bytes(url: &str) -> Result<Vec<u8>, CliError> {
    reqwest::blocking::get(url)
        .and_then(|response| response.error_for_status())
        .map_err(|err| CliError::execution(format!("failed to fetch {url}: {err}")))?
        .bytes()
        .map(|bytes| bytes.to_vec())
        .map_err(|err| CliError::execution(format!("failed to read response from {url}: {err}")))
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
                language: Some(source.language),
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
                    language: Some(source.language),
                    status: "ok",
                    project_descriptor: ProjectDescriptorOutput::not_set(),
                    diagnostics: Vec::new(),
                    document: Some(document),
                },
                Err(diagnostic) => CompileResponse {
                    source: source.source_name.clone(),
                    language: Some(source.language),
                    status: "failed",
                    project_descriptor: ProjectDescriptorOutput::not_set(),
                    diagnostics: vec![diagnostic],
                    document: None,
                },
            }
        }
    }
}

fn compile_kpar_model_input(
    path: &Path,
    stdlib: &KirDocument,
) -> Result<CompileResponse, CliError> {
    let model = read_kpar_model_input(path, stdlib)?;
    Ok(CompileResponse {
        source: model.source,
        language: None,
        status: "ok",
        project_descriptor: ProjectDescriptorOutput::not_set(),
        diagnostics: Vec::new(),
        document: Some(model.document),
    })
}

fn compile_kpar_url_model_input(
    url: &str,
    stdlib: &KirDocument,
) -> Result<CompileResponse, CliError> {
    let model = read_kpar_url_model_input(url, stdlib)?;
    Ok(CompileResponse {
        source: model.source,
        language: None,
        status: "ok",
        project_descriptor: ProjectDescriptorOutput::not_set(),
        diagnostics: Vec::new(),
        document: Some(model.document),
    })
}

fn read_kpar_model_input(path: &Path, stdlib: &KirDocument) -> Result<ModelInput, CliError> {
    let artifact = LibraryProviderConfig::KparFile {
        path: path.display().to_string(),
    }
    .resolve_with_context("input", None, Some(stdlib))
    .map_err(|err| CliError::execution(format!("failed to load KPAR {}: {err}", path.display())))?;

    Ok(ModelInput {
        source: path.display().to_string(),
        document: artifact.document,
    })
}

fn read_kpar_url_model_input(url: &str, stdlib: &KirDocument) -> Result<ModelInput, CliError> {
    let bytes = download_url_bytes(url)?;
    let temp_path = temp_url_kpar_path(url);
    std::fs::write(&temp_path, bytes).map_err(|err| {
        CliError::execution(format!(
            "failed to write temporary KPAR {}: {err}",
            temp_path.display()
        ))
    })?;
    let result = read_kpar_model_input(&temp_path, stdlib).map(|mut model| {
        model.source = url.to_string();
        model
    });
    let _ = std::fs::remove_file(&temp_path);
    result
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

fn compile_context_path(command: &CompileCommand) -> Result<PathBuf, CliError> {
    if let Some(path) = &command.kpar {
        return Ok(path.clone());
    }
    single_input_context_path(&command.input)
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
    if let Some(path) = &command.kir {
        return Ok(path.clone());
    }
    command
        .files
        .first()
        .cloned()
        .map(Ok)
        .unwrap_or_else(current_directory_context_path)
}

fn package_repo(path: Option<PathBuf>) -> LocalPackageRepository {
    path.map(LocalPackageRepository::new)
        .unwrap_or_else(LocalPackageRepository::default_user)
}

fn package_publish_target_repo(target: &str) -> Result<LocalPackageRepository, CliError> {
    package_repository_target(target, "publish target")
}

fn package_repository_target(
    target: &str,
    target_label: &str,
) -> Result<LocalPackageRepository, CliError> {
    if target.starts_with("oci://") {
        return Err(CliError::usage(format!(
            "OCI package transfer is not implemented yet; use a package repository path or file:// path for {target_label}"
        )));
    }
    let path = target
        .strip_prefix("file://")
        .or_else(|| target.strip_prefix("file:"))
        .unwrap_or(target);
    if path.trim().is_empty() {
        return Err(CliError::usage(format!("{target_label} must not be empty")));
    }
    Ok(LocalPackageRepository::new(PathBuf::from(path)))
}

fn read_plugin_manifest(path: &Path) -> Result<PluginManifestEnvelope, CliError> {
    let manifest = registry::read_plugin_manifest(path).map_err(registry_error_to_cli)?;
    serde_json::from_value(manifest).map_err(|err| {
        CliError::usage(format!("invalid plugin manifest {}: {err}", path.display()))
    })
}

fn validate_plugin_manifest(manifest: &PluginManifestEnvelope) -> Result<(), CliError> {
    if manifest.id.trim().is_empty() {
        return Err(CliError::usage("plugin manifest id must not be empty"));
    }
    if manifest.version.trim().is_empty() {
        return Err(CliError::usage("plugin manifest version must not be empty"));
    }
    if manifest.name.trim().is_empty() {
        return Err(CliError::usage("plugin manifest name must not be empty"));
    }
    let service_ids = manifest
        .services
        .iter()
        .map(|service| service.id.as_str())
        .collect::<Vec<_>>();
    for service in &manifest.services {
        if service.id.trim().is_empty() {
            return Err(CliError::usage("plugin service id must not be empty"));
        }
    }
    for action in &manifest.cli_actions {
        if action.command.trim().is_empty() {
            return Err(CliError::usage(
                "plugin CLI action command must not be empty",
            ));
        }
        if !service_ids
            .iter()
            .any(|service_id| *service_id == action.service)
        {
            return Err(CliError::usage(format!(
                "plugin CLI action `{}` references undeclared service `{}`",
                action.command, action.service
            )));
        }
    }
    Ok(())
}

fn installed_plugin_manifests(
    root: &Path,
) -> Result<Vec<(PathBuf, PluginManifestEnvelope)>, CliError> {
    let mut manifests = registry::installed_plugin_manifest_paths(root)
        .map_err(registry_error_to_cli)?
        .into_iter()
        .map(|path| {
            let manifest = read_plugin_manifest(&path)?;
            validate_plugin_manifest(&manifest)?;
            Ok((path, manifest))
        })
        .collect::<Result<Vec<_>, CliError>>()?;
    manifests.sort_by(|left, right| {
        left.1
            .id
            .cmp(&right.1.id)
            .then_with(|| left.1.version.cmp(&right.1.version))
    });
    Ok(manifests)
}

fn installed_plugin_summaries(root: &Path) -> Result<Vec<InstalledPluginSummary>, CliError> {
    installed_plugin_manifests(root).map(|manifests| {
        manifests
            .into_iter()
            .map(|(path, manifest)| InstalledPluginSummary {
                id: manifest.id,
                version: manifest.version,
                name: manifest.name,
                services: manifest
                    .services
                    .into_iter()
                    .map(|service| service.id)
                    .collect(),
                cli_actions: manifest
                    .cli_actions
                    .into_iter()
                    .map(|action| action.command)
                    .collect(),
                manifest_path: path.display().to_string(),
            })
            .collect()
    })
}

fn select_plugin_manifest_match(
    mut matches: Vec<(PathBuf, PluginManifestEnvelope)>,
    id: &str,
) -> Result<Option<(PathBuf, PluginManifestEnvelope)>, CliError> {
    if matches.is_empty() {
        return Ok(None);
    }
    if matches.len() == 1 {
        return Ok(matches.pop());
    }
    let versions = matches
        .iter()
        .map(|(_, manifest)| manifest.version.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Err(CliError::usage(format!(
        "plugin {id} has multiple installed versions ({versions}); pass --version"
    )))
}

fn format_installed_plugins_text(plugins: &[InstalledPluginSummary]) -> String {
    let mut output = String::new();
    for plugin in plugins {
        output.push_str(&format!(
            "{}:{}\t{}\tservices={}\tactions={}\n",
            plugin.id,
            plugin.version,
            plugin.name,
            plugin.services.join(","),
            plugin.cli_actions.join(",")
        ));
    }
    output
}

fn format_plugin_manifest_text(manifest: &PluginManifestEnvelope, path: &Path) -> String {
    let mut output = String::new();
    output.push_str(&format!("id: {}\n", manifest.id));
    output.push_str(&format!("version: {}\n", manifest.version));
    output.push_str(&format!("name: {}\n", manifest.name));
    if let Some(description) = &manifest.description {
        output.push_str(&format!("description: {description}\n"));
    }
    output.push_str(&format!("manifest: {}\n", path.display()));
    output.push_str("services:\n");
    for service in &manifest.services {
        output.push_str(&format!(
            "- {} runtime={}\n",
            service.id,
            service.runtime.as_deref().unwrap_or("-")
        ));
    }
    output.push_str("cli_actions:\n");
    for action in &manifest.cli_actions {
        output.push_str(&format!("- {} -> {}\n", action.command, action.service));
    }
    output
}

fn parse_package_coordinate(coordinate: &str) -> Result<(String, String), CliError> {
    let coordinate = coordinate
        .strip_prefix("kpar:")
        .unwrap_or(coordinate)
        .trim();
    let Some((name, version)) = coordinate.rsplit_once(':') else {
        return Err(CliError::usage(
            "package coordinate must be NAME:VERSION or kpar:NAME:VERSION",
        ));
    };
    let name = name.trim();
    let version = version.trim();
    if name.is_empty() || version.is_empty() {
        return Err(CliError::usage(
            "package coordinate must include a non-empty name and version",
        ));
    }
    Ok((name.to_string(), version.to_string()))
}

fn is_http_package_repository(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

fn install_from_http_package_repository(
    target_repo: &LocalPackageRepository,
    base_url: &str,
    name: &str,
    version: &str,
    force: bool,
) -> Result<LocalPackageManifest, CliError> {
    let target_manifest_path = target_repo.manifest_path(name, version);
    let target_package_path = target_repo.package_path(name, version);
    if !force && (target_manifest_path.exists() || target_package_path.exists()) {
        return Err(CliError::execution(format!(
            "package {name} version {version} already exists in {}; use --force to overwrite",
            target_repo.root().display()
        )));
    }

    let manifest_url = http_package_manifest_url(base_url, name, version);
    let manifest: LocalPackageManifest = http_get_json(&manifest_url)?;
    if manifest.name != name || manifest.version != version {
        return Err(CliError::execution(format!(
            "remote package manifest identity mismatch: expected {name}:{version}, got {}:{}",
            manifest.name, manifest.version
        )));
    }
    if manifest.kind != "kpar" {
        return Err(CliError::execution(format!(
            "unsupported remote package kind '{}'",
            manifest.kind
        )));
    }
    if !is_safe_remote_package_file(&manifest.file) {
        return Err(CliError::execution(format!(
            "remote package manifest has unsafe file name '{}'",
            manifest.file
        )));
    }

    let package_url = http_package_file_url(base_url, name, version, &manifest.file);
    let bytes = http_get_bytes(&package_url)?;
    let digest = stable_file_digest(&bytes);
    if digest != manifest.digest {
        return Err(CliError::execution(format!(
            "remote package digest mismatch for {name}:{version}: expected {}, got {}",
            manifest.digest, digest
        )));
    }

    let temp_path = temporary_package_download_path(name, version);
    std::fs::write(&temp_path, &bytes).map_err(|err| {
        CliError::execution(format!(
            "failed to write temporary package {}: {err}",
            temp_path.display()
        ))
    })?;
    let staged = target_repo
        .stage_kpar(
            &temp_path,
            name,
            version,
            Some(LocalPackageSource {
                kind: "http_package_repository".to_string(),
                path: package_url,
            }),
        )
        .map_err(|err| CliError::execution(format!("failed to stage package: {err}")))?;
    let _ = std::fs::remove_file(temp_path);
    Ok(staged)
}

fn publish_to_http_package_repository(
    source_repo: &LocalPackageRepository,
    base_url: &str,
    name: &str,
    version: &str,
    force: bool,
) -> Result<LocalPackageManifest, CliError> {
    let Some(package_path) = source_repo
        .find_package(name, version)
        .map_err(|err| CliError::execution(format!("failed to resolve package: {err}")))?
    else {
        return Err(CliError::execution(format!(
            "package {name} version {version} was not found in {}",
            source_repo.root().display()
        )));
    };
    let manifest = source_repo
        .read_manifest(name, version)
        .map_err(|err| CliError::execution(format!("failed to read package manifest: {err}")))?;
    if manifest.name != name || manifest.version != version {
        return Err(CliError::execution(format!(
            "local package manifest identity mismatch: expected {name}:{version}, got {}:{}",
            manifest.name, manifest.version
        )));
    }
    if manifest.kind != "kpar" {
        return Err(CliError::execution(format!(
            "unsupported package kind '{}'",
            manifest.kind
        )));
    }
    if !is_safe_remote_package_file(&manifest.file) {
        return Err(CliError::execution(format!(
            "local package manifest has unsafe file name '{}'",
            manifest.file
        )));
    }

    let manifest_url = http_package_manifest_url(base_url, name, version);
    if !force && http_package_manifest_exists(&manifest_url)? {
        return Err(CliError::execution(format!(
            "package {name} version {version} already exists at {base_url}; use --force to overwrite"
        )));
    }

    let package_url = http_package_file_url(base_url, name, version, &manifest.file);
    let package_bytes = std::fs::read(&package_path).map_err(|err| {
        CliError::execution(format!(
            "failed to read package {}: {err}",
            package_path.display()
        ))
    })?;
    let digest = stable_file_digest(&package_bytes);
    if digest != manifest.digest {
        return Err(CliError::execution(format!(
            "local package digest mismatch for {name}:{version}: expected {}, got {}",
            manifest.digest, digest
        )));
    }

    http_put_bytes(&package_url, package_bytes, "application/vnd.mercurio.kpar")?;
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|err| CliError::execution(format!("failed to encode package manifest: {err}")))?;
    http_put_bytes(&manifest_url, manifest_bytes, "application/json")?;
    Ok(manifest)
}

fn http_package_manifest_exists(url: &str) -> Result<bool, CliError> {
    let response = reqwest::blocking::Client::new()
        .get(url)
        .send()
        .map_err(|err| CliError::execution(format!("failed to check {url}: {err}")))?;
    if response.status().is_success() {
        return Ok(true);
    }
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    Err(CliError::execution(format!(
        "failed to check {url}: HTTP {}",
        response.status()
    )))
}

fn http_put_bytes(url: &str, bytes: Vec<u8>, content_type: &str) -> Result<(), CliError> {
    reqwest::blocking::Client::new()
        .put(url)
        .header(reqwest::header::CONTENT_TYPE, content_type)
        .body(bytes)
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|err| CliError::execution(format!("failed to upload {url}: {err}")))?;
    Ok(())
}

fn http_get_json<T>(url: &str) -> Result<T, CliError>
where
    T: serde::de::DeserializeOwned,
{
    let text = reqwest::blocking::get(url)
        .and_then(|response| response.error_for_status())
        .map_err(|err| CliError::execution(format!("failed to fetch {url}: {err}")))?
        .text()
        .map_err(|err| CliError::execution(format!("failed to read {url}: {err}")))?;
    serde_json::from_str(&text)
        .map_err(|err| CliError::execution(format!("failed to parse JSON from {url}: {err}")))
}

fn http_get_bytes(url: &str) -> Result<Vec<u8>, CliError> {
    let bytes = reqwest::blocking::get(url)
        .and_then(|response| response.error_for_status())
        .map_err(|err| CliError::execution(format!("failed to fetch {url}: {err}")))?
        .bytes()
        .map_err(|err| CliError::execution(format!("failed to read {url}: {err}")))?;
    Ok(bytes.to_vec())
}

fn http_package_manifest_url(base_url: &str, name: &str, version: &str) -> String {
    http_package_file_url(base_url, name, version, "manifest.json")
}

fn http_package_file_url(base_url: &str, name: &str, version: &str, file: &str) -> String {
    let mut url = base_url.trim_end_matches('/').to_string();
    for segment in name.split('/') {
        url.push('/');
        url.push_str(&safe_package_path_segment(segment));
    }
    url.push('/');
    url.push_str(&safe_package_path_segment(version));
    url.push('/');
    url.push_str(file);
    url
}

fn is_safe_remote_package_file(file: &str) -> bool {
    !file.is_empty()
        && !file.contains('/')
        && !file.contains('\\')
        && file != "."
        && file != ".."
        && file.ends_with(".kpar")
}

fn temporary_package_download_path(name: &str, version: &str) -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "mercurio-package-install-{}-{}-{now}.kpar",
        safe_package_path_segment(name),
        safe_package_path_segment(version)
    ))
}

fn safe_package_path_segment(value: &str) -> String {
    let mut segment = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if segment.is_empty() || segment == "." || segment == ".." {
        segment = "package".to_string();
    }
    segment
}

fn stable_file_digest(bytes: &[u8]) -> String {
    format_stable_digest([("file".as_bytes(), bytes)])
}

fn format_stable_digest<'a, I>(chunks: I) -> String
where
    I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
{
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for (label, bytes) in chunks {
        for byte in label
            .iter()
            .chain(&(bytes.len() as u64).to_le_bytes())
            .chain(bytes)
        {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }

    format!("fnv1a64:{hash:016x}")
}

fn collect_package_manifest_rows(
    repo_root: &Path,
    current: &Path,
    rows: &mut Vec<String>,
) -> Result<(), CliError> {
    let mut entries = std::fs::read_dir(current)
        .map_err(|err| {
            CliError::execution(format!(
                "failed to read directory {}: {err}",
                current.display()
            ))
        })?
        .collect::<Result<Vec<_>, std::io::Error>>()
        .map_err(|err| CliError::execution(format!("failed to read directory entry: {err}")))?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_package_manifest_rows(repo_root, &path, rows)?;
        } else if path.file_name().and_then(|value| value.to_str()) == Some("manifest.json") {
            let input = std::fs::read_to_string(&path).map_err(|err| {
                CliError::execution(format!("failed to read {}: {err}", path.display()))
            })?;
            let manifest: serde_json::Value = serde_json::from_str(&input).map_err(|err| {
                CliError::execution(format!("failed to parse {}: {err}", path.display()))
            })?;
            let name = manifest["name"].as_str().unwrap_or("unknown");
            let version = manifest["version"].as_str().unwrap_or("unknown");
            let relative = path
                .parent()
                .unwrap_or(&path)
                .strip_prefix(repo_root)
                .unwrap_or(path.parent().unwrap_or(&path))
                .display();
            rows.push(format!("{name}:{version}\t{relative}"));
        }
    }

    Ok(())
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

fn temp_url_kpar_path(url: &str) -> PathBuf {
    let file_name = Path::new(url)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("model.kpar");
    std::env::temp_dir().join(format!(".mercurio-url-{}-{file_name}", std::process::id()))
}

fn is_kpar_url(url: &str) -> bool {
    url.split(['?', '#'])
        .next()
        .is_some_and(|path| path.to_ascii_lowercase().ends_with(".kpar"))
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
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<SourceLanguage>,
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

#[derive(Serialize)]
struct QueryResponse {
    source: String,
    project_descriptor: ProjectDescriptorOutput,
    result: QueryResultSet,
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
    if let Some(language) = response.language {
        output.push_str(&format!("language: {language}\n"));
    }
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

fn format_query_text(response: &QueryResponse) -> String {
    let mut output = String::new();
    output.push_str(&format!("source: {}\n", response.source));
    output.push_str(&format!(
        "project_descriptor: {}\n",
        response.project_descriptor.to_text()
    ));
    output.push_str(&format!("rows: {}\n", response.result.rows.len()));

    if response.result.columns.is_empty() {
        return output;
    }

    output.push_str(&response.result.columns.join("\t"));
    output.push('\n');
    for row in &response.result.rows {
        let values = response
            .result
            .columns
            .iter()
            .map(|column| format_query_cell(row.get(column).unwrap_or(&Value::Null)))
            .collect::<Vec<_>>();
        output.push_str(&values.join("\t"));
        output.push('\n');
    }

    output
}

fn format_query_cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Array(_) | Value::Object(_) => value.to_string(),
    }
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
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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

    fn write_test_plugin_package(path: &Path, manifest: &str) {
        use std::io::Write as _;

        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("extension.json", options).unwrap();
        writer.write_all(manifest.as_bytes()).unwrap();
        writer.finish().unwrap();
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
    fn package_build_directory_excludes_mercurio_contents() {
        let root = temp_dir("mercurio-cli-package-excludes-mercurio");
        std::fs::create_dir_all(root.join(".mercurio").join("cache")).unwrap();
        let out_path = root.join("model.kpar");
        std::fs::write(
            root.join("model.sysml"),
            "package Demo { part def Vehicle; }",
        )
        .unwrap();
        std::fs::write(
            root.join(".mercurio").join("cache").join("generated.sysml"),
            "package Hidden { part def CacheOnly; }",
        )
        .unwrap();

        let result = run_args(&[
            "package",
            "build",
            "--file",
            root.to_str().unwrap(),
            "--out",
            out_path.to_str().unwrap(),
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
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
        assert!(
            !artifact
                .document
                .elements
                .iter()
                .any(|element| element.id == "type.Hidden.CacheOnly")
        );
    }

    #[test]
    fn package_publish_copies_staged_package_to_target_repo() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = temp_dir("mercurio-cli-package-publish");
        let source_repo = root.join("stage");
        let target_repo = root.join("published");
        let source_dir = root.join("src");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(
            source_dir.join("model.sysml"),
            "package Demo { part def Vehicle; }",
        )
        .unwrap();

        unsafe {
            std::env::set_var("MERCURIO_PACKAGE_REPO", &source_repo);
        }
        let build = run_args(&[
            "package",
            "build",
            "--file",
            source_dir.to_str().unwrap(),
            "--name",
            "domain-lib",
            "--version",
            "1.2.3",
            "--quiet",
        ])
        .unwrap();
        assert_eq!(build.exit_code, 0);

        let publish = run_args(&[
            "package",
            "publish",
            "domain-lib",
            "--version",
            "1.2.3",
            "--to",
            target_repo.to_str().unwrap(),
        ])
        .unwrap();
        assert_eq!(publish.exit_code, 0);
        assert!(publish.stdout.contains("published: domain-lib:1.2.3"));

        let list = run_args(&["package", "list", "--repo", target_repo.to_str().unwrap()]).unwrap();
        assert_eq!(list.exit_code, 0);
        assert!(list.stdout.contains("domain-lib:1.2.3"));

        let duplicate = run_args(&[
            "package",
            "publish",
            "domain-lib",
            "--version",
            "1.2.3",
            "--to",
            target_repo.to_str().unwrap(),
        ])
        .unwrap_err();
        assert!(duplicate.message.contains("already exists"));

        let forced = run_args(&[
            "package",
            "publish",
            "domain-lib",
            "--version",
            "1.2.3",
            "--to",
            target_repo.to_str().unwrap(),
            "--force",
            "--quiet",
        ])
        .unwrap();
        assert_eq!(forced.exit_code, 0);
        assert!(forced.stdout.is_empty());

        unsafe {
            std::env::remove_var("MERCURIO_PACKAGE_REPO");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_pull_copies_package_from_source_repo_to_local_repo() {
        let root = temp_dir("mercurio-cli-package-pull");
        let source_repo = root.join("source");
        let target_repo = root.join("target");
        let source_dir = root.join("src");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(
            source_dir.join("model.sysml"),
            "package Demo { part def Vehicle; }",
        )
        .unwrap();

        let build = run_args(&[
            "package",
            "build",
            "--file",
            source_dir.to_str().unwrap(),
            "--name",
            "domain-lib",
            "--version",
            "1.2.3",
            "--out",
            root.join("domain-lib.kpar").to_str().unwrap(),
            "--quiet",
        ])
        .unwrap();
        assert_eq!(build.exit_code, 0);

        let source_package = root.join("domain-lib.kpar");
        let source_repo_model = LocalPackageRepository::new(&source_repo);
        source_repo_model
            .stage_kpar(&source_package, "domain-lib", "1.2.3", None)
            .unwrap();

        let pull = run_args(&[
            "package",
            "pull",
            "domain-lib",
            "--version",
            "1.2.3",
            "--from",
            source_repo.to_str().unwrap(),
            "--repo",
            target_repo.to_str().unwrap(),
        ])
        .unwrap();
        assert_eq!(pull.exit_code, 0);
        assert!(pull.stdout.contains("pulled: domain-lib:1.2.3"));

        let list = run_args(&["package", "list", "--repo", target_repo.to_str().unwrap()]).unwrap();
        assert_eq!(list.exit_code, 0);
        assert!(list.stdout.contains("domain-lib:1.2.3"));

        let duplicate = run_args(&[
            "package",
            "pull",
            "domain-lib",
            "--version",
            "1.2.3",
            "--from",
            source_repo.to_str().unwrap(),
            "--repo",
            target_repo.to_str().unwrap(),
        ])
        .unwrap_err();
        assert!(duplicate.message.contains("already exists"));

        let forced = run_args(&[
            "package",
            "pull",
            "domain-lib",
            "--version",
            "1.2.3",
            "--from",
            source_repo.to_str().unwrap(),
            "--repo",
            target_repo.to_str().unwrap(),
            "--force",
            "--quiet",
        ])
        .unwrap();
        assert_eq!(forced.exit_code, 0);
        assert!(forced.stdout.is_empty());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_install_accepts_coordinate_and_local_package_repo() {
        let root = temp_dir("mercurio-cli-package-install-local");
        let source_repo = root.join("source");
        let target_repo = root.join("target");
        let source_dir = root.join("src");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(
            source_dir.join("model.sysml"),
            "package Demo { part def Vehicle; }",
        )
        .unwrap();

        let build = run_args(&[
            "package",
            "build",
            "--file",
            source_dir.to_str().unwrap(),
            "--name",
            "domain-lib",
            "--version",
            "1.2.3",
            "--out",
            root.join("domain-lib.kpar").to_str().unwrap(),
            "--quiet",
        ])
        .unwrap();
        assert_eq!(build.exit_code, 0);

        let source_package = root.join("domain-lib.kpar");
        let source_repo_model = LocalPackageRepository::new(&source_repo);
        source_repo_model
            .stage_kpar(&source_package, "domain-lib", "1.2.3", None)
            .unwrap();

        let install = run_args(&[
            "package",
            "install",
            "kpar:domain-lib:1.2.3",
            "--from",
            source_repo.to_str().unwrap(),
            "--repo",
            target_repo.to_str().unwrap(),
        ])
        .unwrap();
        assert_eq!(install.exit_code, 0);
        assert!(install.stdout.contains("installed: domain-lib:1.2.3"));

        let list = run_args(&["package", "list", "--repo", target_repo.to_str().unwrap()]).unwrap();
        assert_eq!(list.exit_code, 0);
        assert!(list.stdout.contains("domain-lib:1.2.3"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn plugin_install_list_and_inspect_manage_manifest_metadata() {
        let root = temp_dir("mercurio-cli-plugin-registry");
        let manifest_path = root.join("requirements.extension.json");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            &manifest_path,
            r#"{
  "id": "org.mercurio.requirements",
  "version": "0.1.0",
  "name": "Requirements Reasoning",
  "description": "Requirement coverage plugin.",
  "services": [
    {
      "id": "requirements.coverage",
      "runtime": "in_process",
      "function": "coverage"
    }
  ],
  "cliActions": [
    {
      "command": "requirements coverage",
      "service": "requirements.coverage"
    }
  ]
}"#,
        )
        .unwrap();

        let registry = root.join("plugins");
        let install = run_args(&[
            "plugin",
            "install",
            manifest_path.to_str().unwrap(),
            "--root",
            registry.to_str().unwrap(),
        ])
        .unwrap();
        assert_eq!(install.exit_code, 0);
        assert!(
            install
                .stdout
                .contains("installed plugin: org.mercurio.requirements:0.1.0")
        );

        let list = run_args(&["plugin", "list", "--root", registry.to_str().unwrap()]).unwrap();
        assert_eq!(list.exit_code, 0);
        assert!(list.stdout.contains("org.mercurio.requirements:0.1.0"));
        assert!(list.stdout.contains("requirements.coverage"));

        let inspect = run_args(&[
            "plugin",
            "inspect",
            "org.mercurio.requirements",
            "--version",
            "0.1.0",
            "--root",
            registry.to_str().unwrap(),
            "--format",
            "json",
        ])
        .unwrap();
        assert_eq!(inspect.exit_code, 0);
        let json: serde_json::Value = serde_json::from_str(&inspect.stdout).unwrap();
        assert_eq!(json["id"], "org.mercurio.requirements");
        assert_eq!(json["cliActions"][0]["command"], "requirements coverage");

        let duplicate = run_args(&[
            "plugin",
            "install",
            manifest_path.to_str().unwrap(),
            "--root",
            registry.to_str().unwrap(),
        ])
        .unwrap_err();
        assert!(duplicate.message.contains("already exists"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn plugin_install_accepts_mpack_archive() {
        let root = temp_dir("mercurio-cli-plugin-mpack-registry");
        let package_path = root.join("requirements.mpack");
        std::fs::create_dir_all(&root).unwrap();
        write_test_plugin_package(
            &package_path,
            r#"{
  "id": "org.mercurio.requirements",
  "version": "0.1.0",
  "name": "Requirements Reasoning",
  "services": [
    {
      "id": "requirements.coverage",
      "runtime": "in_process",
      "function": "coverage"
    }
  ],
  "cli_actions": [
    {
      "command": "requirements coverage",
      "service": "requirements.coverage"
    }
  ]
}"#,
        );

        let registry = root.join("plugins");
        let install = run_args(&[
            "plugin",
            "install",
            package_path.to_str().unwrap(),
            "--root",
            registry.to_str().unwrap(),
        ])
        .unwrap();
        assert_eq!(install.exit_code, 0);
        assert!(
            install
                .stdout
                .contains("installed plugin: org.mercurio.requirements:0.1.0")
        );

        let installed_dir = registry
            .join("installed")
            .join("org.mercurio.requirements")
            .join("0.1.0");
        assert!(installed_dir.join("extension.json").is_file());
        assert!(installed_dir.join("plugin.mpack").is_file());

        let inspect = run_args(&[
            "plugin",
            "inspect",
            "org.mercurio.requirements",
            "--root",
            registry.to_str().unwrap(),
            "--format",
            "json",
        ])
        .unwrap();
        assert_eq!(inspect.exit_code, 0);
        let json: serde_json::Value = serde_json::from_str(&inspect.stdout).unwrap();
        assert_eq!(json["id"], "org.mercurio.requirements");
        assert_eq!(json["cliActions"][0]["service"], "requirements.coverage");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn plugin_install_resolves_coordinate_from_local_repository() {
        let root = temp_dir("mercurio-cli-plugin-coordinate-install");
        let repo_package = root
            .join("repo")
            .join("org.mercurio.requirements")
            .join("0.1.0")
            .join("plugin.mpack");
        std::fs::create_dir_all(repo_package.parent().unwrap()).unwrap();
        write_test_plugin_package(
            &repo_package,
            r#"{
  "id": "org.mercurio.requirements",
  "version": "0.1.0",
  "name": "Requirements Reasoning",
  "services": [
    {
      "id": "requirements.coverage",
      "runtime": "in_process"
    }
  ]
}"#,
        );

        let registry = root.join("plugins");
        let install = run_args(&[
            "plugin",
            "install",
            "mpack:org.mercurio.requirements:0.1.0",
            "--from",
            root.join("repo").to_str().unwrap(),
            "--root",
            registry.to_str().unwrap(),
        ])
        .unwrap();

        assert_eq!(install.exit_code, 0);
        assert!(
            install
                .stdout
                .contains("installed plugin: org.mercurio.requirements:0.1.0")
        );
        assert!(
            registry
                .join("installed")
                .join("org.mercurio.requirements")
                .join("0.1.0")
                .join("plugin.mpack")
                .is_file()
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn http_package_urls_follow_repository_layout() {
        assert_eq!(
            http_package_manifest_url(
                "https://packages.example.com/mercurio/",
                "org.example/domain-lib",
                "1.2.3"
            ),
            "https://packages.example.com/mercurio/org.example/domain-lib/1.2.3/manifest.json"
        );
        assert_eq!(
            http_package_file_url(
                "https://packages.example.com/mercurio",
                "org.example/domain lib",
                "1.2.3",
                "domain_lib-1.2.3.kpar",
            ),
            "https://packages.example.com/mercurio/org.example/domain_lib/1.2.3/domain_lib-1.2.3.kpar"
        );
    }

    #[test]
    fn compile_kpar_file_returns_document() {
        let root = temp_dir("mercurio-cli-compile-kpar");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("model.sysml");
        let out_path = root.join("model.kpar");
        std::fs::write(&source_path, "package Demo { part def Vehicle; }").unwrap();
        run_args(&[
            "package",
            "build",
            "--file",
            source_path.to_str().unwrap(),
            "--out",
            out_path.to_str().unwrap(),
            "--quiet",
        ])
        .unwrap();

        let result = run_args(&[
            "compile",
            "--kpar",
            out_path.to_str().unwrap(),
            "--format",
            "json",
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
        let json: serde_json::Value = serde_json::from_str(&result.stdout).unwrap();
        assert_eq!(json["status"], "ok");
        assert!(json["language"].is_null());
        assert!(
            json["document"]["elements"]
                .as_array()
                .unwrap()
                .iter()
                .any(|element| element["id"] == "type.Demo.Vehicle")
        );
    }

    #[test]
    fn package_build_include_kir_writes_compilable_package() {
        let root = temp_dir("mercurio-cli-package-include-kir");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("model.sysml");
        let out_path = root.join("model.kpar");
        std::fs::write(&source_path, "package Demo { part def Vehicle; }").unwrap();

        let build = run_args(&[
            "package",
            "build",
            "--file",
            source_path.to_str().unwrap(),
            "--out",
            out_path.to_str().unwrap(),
            "--include-kir",
            "--quiet",
        ])
        .unwrap();

        assert_eq!(build.exit_code, 0);
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

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_build_kir_writes_kir_only_package() {
        let root = temp_dir("mercurio-cli-package-kir-only");
        std::fs::create_dir_all(&root).unwrap();
        let kir_path = root.join("document.kir.json");
        let out_path = root.join("stdlib.kpar");
        let document = KirDocument {
            metadata: Default::default(),
            elements: vec![mercurio_core::KirElement {
                id: "type.Stdlib.Thing".to_string(),
                kind: "PartDefinition".to_string(),
                layer: 2,
                properties: Default::default(),
            }],
        };
        document.write_pretty_to_path(&kir_path).unwrap();

        let build = run_args(&[
            "package",
            "build",
            "--kir",
            kir_path.to_str().unwrap(),
            "--out",
            out_path.to_str().unwrap(),
            "--name",
            "org.omg/sysml-stdlib",
            "--version",
            "2.0.0",
            "--quiet",
        ])
        .unwrap();

        assert_eq!(build.exit_code, 0);
        let artifact = LibraryProviderConfig::KparFile {
            path: out_path.display().to_string(),
        }
        .resolve("stdlib")
        .unwrap();
        assert_eq!(artifact.document.elements[0].id, "type.Stdlib.Thing");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn package_verify_checks_bundled_stdlib_package() {
        let repo = mercurio_core::bundled_package_repo_path();
        let result = run_args(&[
            "package",
            "verify",
            "org.omg/sysml-stdlib",
            "--version",
            "2.0.0",
            "--repo",
            repo.to_str().unwrap(),
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("status: ok"));
        assert!(
            result
                .stdout
                .contains("package: org.omg/sysml-stdlib:2.0.0")
        );
        assert!(result.stdout.contains("precompiled_kir: true"));
    }

    #[test]
    fn evaluate_kpar_file_accepts_qualified_names() {
        let root = temp_dir("mercurio-cli-evaluate-kpar");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("model.sysml");
        let out_path = root.join("model.kpar");
        std::fs::write(
            &source_path,
            "package Demo { part def Vehicle { attribute mass = 40+(2); } }",
        )
        .unwrap();
        run_args(&[
            "package",
            "build",
            "--file",
            source_path.to_str().unwrap(),
            "--out",
            out_path.to_str().unwrap(),
            "--quiet",
        ])
        .unwrap();

        let result = run_args(&[
            "evaluate",
            "--kpar",
            out_path.to_str().unwrap(),
            "--feature",
            "mass",
            "--owner",
            "Demo.Vehicle",
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("source:"));
        assert!(result.stdout.contains("compile_status: ok"));
        assert!(result.stdout.contains("owner_id: type.Demo.Vehicle"));
        assert!(result.stdout.contains("value: 42.0"));
    }

    #[test]
    fn query_text_filters_and_selects_elements() {
        let result = run_args(&[
            "query",
            "--text",
            "package Demo { part def Vehicle; attribute def Mass; }",
            "--query",
            r#"from elements where kind = "SysML::Systems::PartDefinition" select id, qualified_name"#,
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("rows: 1"));
        assert!(result.stdout.contains("type.Demo.Vehicle"));
        assert!(result.stdout.contains("Demo.Vehicle"));
    }

    #[test]
    fn query_kpar_file_returns_json_rows() {
        let root = temp_dir("mercurio-cli-query-kpar");
        std::fs::create_dir_all(&root).unwrap();
        let source_path = root.join("model.sysml");
        let out_path = root.join("model.kpar");
        std::fs::write(&source_path, "package Demo { part def Vehicle; }").unwrap();
        run_args(&[
            "package",
            "build",
            "--file",
            source_path.to_str().unwrap(),
            "--out",
            out_path.to_str().unwrap(),
            "--quiet",
        ])
        .unwrap();

        let result = run_args(&[
            "query",
            "--kpar",
            out_path.to_str().unwrap(),
            "--query",
            r#"from elements where qualified_name = "Demo.Vehicle" select id, kind"#,
            "--format",
            "json",
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
        let json: serde_json::Value = serde_json::from_str(&result.stdout).unwrap();
        assert_eq!(json["result"]["rows"].as_array().unwrap().len(), 1);
        assert_eq!(json["result"]["rows"][0]["id"], "type.Demo.Vehicle");
    }

    #[test]
    fn query_match_binds_feature_relationships() {
        let result = run_args(&[
            "query",
            "--text",
            "package Demo { part def Vehicle { attribute mass = 42; } }",
            "--query",
            r#"match ?type kind "SysML::Systems::PartDefinition" match ?type features ?feature select ?type, ?feature"#,
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("rows: 1"));
        assert!(result.stdout.contains("type.Demo.Vehicle"));
        assert!(result.stdout.contains("feature.Demo.Vehicle.mass"));
    }

    #[test]
    fn query_filters_requirements_by_metatype_contains() {
        let result = run_args(&[
            "query",
            "--text",
            "package Demo { requirement def VehicleNeed; requirement vehicleNeed : VehicleNeed; }",
            "--query",
            r#"from elements where metatype contains "Requirement" select id, qualified_name, metatype"#,
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("rows: 2"));
        assert!(result.stdout.contains("type.Demo.VehicleNeed"));
        assert!(result.stdout.contains("requirement.Demo.vehicleNeed"));
    }

    #[test]
    fn query_match_selects_bound_element_fields() {
        let result = run_args(&[
            "query",
            "--text",
            "package Demo { part def Vehicle { attribute mass = 42; } }",
            "--query",
            r#"match ?type kind "SysML::Systems::PartDefinition" match ?type features ?feature select ?type.qualified_name, ?feature.qualified_name"#,
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("rows: 1"));
        assert!(result.stdout.contains("Demo.Vehicle"));
        assert!(result.stdout.contains("Demo.Vehicle.mass"));
    }

    #[test]
    fn query_supports_multiple_filters_not_equals_and_order_by() {
        let result = run_args(&[
            "query",
            "--text",
            "package Demo { requirement def VehicleNeed; requirement def SkipNeed; requirement vehicleNeed : VehicleNeed; }",
            "--query",
            r#"from elements where metatype contains "Requirement" where qualified_name != "Demo.SkipNeed" select id, qualified_name, metatype order by qualified_name desc"#,
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("rows: 2"));
        assert!(result.stdout.contains("Demo.VehicleNeed"));
        assert!(result.stdout.contains("Demo.vehicleNeed"));
        assert!(!result.stdout.contains("Demo.SkipNeed\t"));
    }

    #[test]
    fn query_match_supports_where_filters() {
        let result = run_args(&[
            "query",
            "--text",
            "package Demo { part def Vehicle { attribute mass = 42; } }",
            "--query",
            r#"match ?type features ?feature where ?feature.metatype = "SysML::Systems::AttributeUsage" select ?type.qualified_name, ?feature.qualified_name"#,
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("rows: 1"));
        assert!(result.stdout.contains("Demo.Vehicle"));
        assert!(result.stdout.contains("Demo.Vehicle.mass"));
    }

    #[test]
    fn query_file_reads_query_from_disk() {
        let root = temp_dir("mercurio-cli-query-file");
        std::fs::create_dir_all(&root).unwrap();
        let query_path = root.join("requirements.mq");
        std::fs::write(
            &query_path,
            r#"from elements where metatype contains "Requirement" select id, qualified_name order by qualified_name"#,
        )
        .unwrap();

        let result = run_args(&[
            "query",
            "--text",
            "package Demo { requirement def VehicleNeed; }",
            "--query-file",
            query_path.to_str().unwrap(),
        ])
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("rows: 1"));
        assert!(result.stdout.contains("type.Demo.VehicleNeed"));
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
  "libraries": [
    {
      "id": "missing",
      "role": "baseline",
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
  "libraries": [
    {
      "id": "missing",
      "role": "baseline",
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
