# Mercurio

Mercurio is the open source Rust library and CLI workspace for working with SysML v2, KerML, and Mercurio's KIR JSON model representation.

The goal of this repository is to make the modeling kernel useful on its own: parse source models, compile them into semantic KIR, lint them, package them, and expose reusable library APIs that private products and external tools can build on.

## Objectives

- Provide a reusable Rust library for SysML v2 and KerML model processing.
- Keep the core model semantics independent from any particular server, desktop app, or hosted product.
- Offer a small public CLI that demonstrates the library without requiring the private product repo.
- Use KIR as the stable semantic interchange format for graph queries, derived values, package loading, and downstream applications.
- Optimize for high-performance model loading, compilation, and runtime use, with attention to both wall-clock speed and memory footprint.
- Keep maintainer-only diagnostics, benchmarks, and Pilot comparison workflows separate from the public CLI.

## What Lives Here

- `mercurio-core` parses, compiles, lints, loads libraries, builds runtime graphs, and computes derived values.
- `mercurio-cli` provides the public `mercurio` command for parse, compile, lint, and package workflows.
- `mercurio-tools` contains maintainer tools for diagnostics, benchmarks, demos, and Pilot comparison/export workflows.
- `resources/` contains bundled runtime and standard library artifacts.
- `examples/` and `fixtures/` provide SysML, KerML, and KIR models for tests and demonstrations.

The hosted product, UI, and privileged console API live in the private `mercurio-product` repository. They depend on `mercurio-core` for domain behavior.

## Core Concepts

### Source Languages

Mercurio reads SysML v2 and KerML source files. Files ending in `.sysml` are treated as SysML, and files ending in `.kerml` are treated as KerML. Inline CLI text defaults to SysML unless `--language kerml` is provided.

### KIR

KIR is Mercurio's semantic model document format. It is JSON, validated by the core library, and used by graph queries, derived values, requirements views, package loading, and product hosts.

### Standard Library

Semantic compilation and linting use the bundled default standard library unless a command is given `--stdlib PATH`. The default path is provided by `mercurio_core::default_stdlib_path()`.

### KPAR Packages

A `.kpar` package is a source-backed zip package containing SysML/KerML sources plus package metadata. Mercurio can build these packages from source files and load them later as baseline libraries.

## Performance

Mercurio is designed as a high-performance modeling kernel. The Rust core keeps the default standard library precompiled as KIR, uses bounded semantic caches for warm project workflows, and tracks both load speed and memory use in benchmark runs.

Latest local benchmark, run on May 6, 2026 on Windows with Rust 1.90.0 and OpenJDK 21.0.6:

| Engine | Corpus | Files | Bytes | Full-load result | Warm result | Peak working set |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Mercurio release benchmark | `examples/src/examples` with bundled stdlib | 96 | 229,386 | 11.246s cold workspace load | 76ms unchanged warm scope | 418.3 MiB |
| Java Pilot direct diagnostics | same 96-file corpus with Pilot stdlib | 96 | 229,386 | 351.926s before resolve/transform failure | n/a | 1,492.1 MiB |

Commands used:

```powershell
cargo run -q --release -p mercurio-tools --bin benchmark_examples -- --all --root examples/src/examples
```

The Java comparison used `PilotModelExporter --diagnostics` against the same input file set and the Pilot `sysml.library`. It is not a perfect apples-to-apples semantic comparison because the Java run ended with a Pilot resolve/transform diagnostic failure, but it is useful as a full-load stress reference for wall time and process memory.

For a shared semantic comparison corpus, the largest curated Pilot corpus currently in the repo is `extended`. On the same machine:

| Corpus | Cases | Mercurio total | Java Pilot per-case total | Java Pilot shared setup |
| --- | ---: | ---: | ---: | ---: |
| `extended` | 10 | 3.115s | 24.927s | 60.846s |

```powershell
cargo run -q --release -p mercurio-tools --bin compare_pilot_semantics -- --corpus extended --pilot-root ..\SysML-v2-Pilot-Implementation --out target\pilot_semantic_compare_extended.json
```

## Requirements

- Rust toolchain with Cargo
- Java, only for the Pilot comparison/export tools under `tools/pilot-exporter`

Most commands below assume you are running them from the repository root.

## Quick Start

Build the workspace:

```powershell
cargo build
```

Run the test suite:

```powershell
cargo test
```

Show the public CLI:

```powershell
mercurio --help
```

Parse an inline SysML model:

```powershell
mercurio parse --text "package Demo { part def Vehicle; }"
```

## CLI Examples

The public CLI is one cohesive `mercurio` binary with `project`, `parse`, `compile`, `evaluate`, `lint`, and `package` subcommands. `parse`, `compile`, `evaluate`, and `lint` accept source input from `--file PATH` or `--text TEXT`; inline text defaults to SysML, and file input defaults from `.sysml` or `.kerml`.

### Create a Project

Create a new project directory with a project descriptor and sample SysML file:

```powershell
mercurio project new my-model --name "My Model"
```

This writes:

- `my-model/mercurio-project.json`
- `my-model/src/main.sysml`

Use `--force` to write the scaffold files into an existing non-empty directory, and `--quiet` to suppress the creation summary.

The project descriptor is the root-level `mercurio-project.json` file. The generated descriptor is intentionally small:

```json
{
  "version": 1,
  "name": "My Model",
  "baseline_libraries": [],
  "libraries": []
}
```

Descriptor fields:

- `version`: descriptor schema version. The current version is `1`; omitted values default to `1`.
- `name`: optional display name for the project.
- `baseline_libraries`: foundational libraries used as the baseline semantic context. If this array is empty or omitted, Mercurio uses the bundled standard library.
- `libraries`: ordinary read-only dependency libraries added after the baseline context.

Each entry in `baseline_libraries` or `libraries` has this shape:

```json
{
  "id": "domain-lib",
  "provider": {
    "kind": "kpar_file",
    "path": "libs/domain.kpar"
  }
}
```

Supported provider `kind` values:

- `bundled_stdlib`: use Mercurio's bundled standard library; no extra fields.
- `precompiled_kir_artifact`: load a KIR JSON file with `path`.
- `sysml_directory`: load all SysML/KerML sources under `path`.
- `kpar_file`: load one `.kpar` package file from `path`.
- `package_set_directory`: load a package from a local package-set directory using `path` and `entry`.

Relative provider paths are resolved from the directory containing `mercurio-project.json`.

Semantic CLI commands discover this descriptor automatically:

- `compile --file PATH` looks for `mercurio-project.json` from `PATH` upward.
- `lint --file PATH` uses the first input path as the project anchor.
- `package build --file PATH` validates the package against the descriptor discovered from the first input path.
- Inline `--text` commands use the current working directory as the project anchor.

Passing `--stdlib PATH` skips descriptor discovery for that command and uses the provided KIR document as the semantic library context.

### Shell Completions

Generate completion scripts for your shell:

```powershell
mercurio completions powershell
```

Supported shells are `bash`, `elvish`, `fish`, `powershell`, and `zsh`.

For PowerShell, add the generated script to your profile or dot-source it from a file:

```powershell
mercurio completions powershell > mercurio-completions.ps1
. .\mercurio-completions.ps1
```

For Bash or Zsh, write the generated script to a directory loaded by your shell's completion system:

```bash
mercurio completions bash > ~/.local/share/bash-completion/completions/mercurio
mercurio completions zsh > ~/.zfunc/_mercurio
```

### Parse SysML or KerML

Parse one file and print a syntax summary:

```powershell
mercurio parse --file "examples/src/examples/Simple Tests/PartTest.sysml"
```

Parse inline SysML:

```powershell
mercurio parse --text "package Demo { part def Vehicle; }"
```

Emit the syntax AST as JSON:

```powershell
mercurio parse --file "examples/src/examples/Simple Tests/PartTest.sysml" --format json
```

### Compile to KIR

Compile a file to KIR using the default stdlib:

```powershell
mercurio compile --file "examples/src/examples/Simple Tests/PartTest.sysml"
```

Compile inline KerML with an explicit language:

```powershell
mercurio compile --text "package Demo { classifier Vehicle; }" --language kerml
```

Emit the KIR document as JSON:

```powershell
mercurio compile --text "package Demo { part def Vehicle; }" --format json
```

Override the stdlib:

```powershell
mercurio compile --file model.sysml --stdlib resources/stdlib.full.kir.json
```

### Evaluate Runtime Expressions

Evaluate a derived feature from source by compiling the model first:

```powershell
mercurio evaluate --file model.sysml --feature totalMass --owner Demo.Vehicle
```

Evaluate directly from a precompiled KIR document:

```powershell
mercurio evaluate --kir model.kir.json --feature Demo.Vehicle.totalMass --owner Demo.Vehicle
```

Evaluate an inline expression model:

```powershell
mercurio evaluate --text "package Demo { part def Vehicle { attribute mass = 40+(2); } }" --feature mass --owner Demo.Vehicle
```

Provide overlay values for runtime context:

```powershell
mercurio evaluate --kir model.kir.json --feature totalMass --owner Demo.Vehicle --value assembly.Vehicle.mass=42
```

For larger overlays, use nested JSON where the first key is owner name and the second key is feature name:

```powershell
mercurio evaluate --kir model.kir.json --feature totalMass --owner Demo.Vehicle --context-json '{ "assembly.Vehicle": { "mass": 42 } }'
```

User-facing evaluation arguments use model qualified names. Existing KIR ids such as `type.Demo.Vehicle` and `feature.Demo.Vehicle.totalMass` are still accepted for diagnostics and low-level workflows. Add `--explain` to include runtime explanation steps in text output, or `--format json` for structured output.

### Lint SysML or KerML

Lint one file:

```powershell
mercurio lint --file "examples/src/examples/Simple Tests/PartTest.sysml"
```

Lint every `.sysml` and `.kerml` file under a directory:

```powershell
mercurio lint --file "examples/src/examples/Simple Tests"
```

Emit JSON diagnostics:

```powershell
mercurio lint --file "examples/src/examples/Simple Tests" --format json
```

Fail when warnings are present, useful for CI:

```powershell
mercurio lint --file "examples/src/examples/Simple Tests" --warnings-as-errors
```

### Build KPAR Packages

Build a source-backed `.kpar` package from a model file:

```powershell
mercurio package build --file model.sysml --out model.kpar
```

Build a package from every `.sysml` and `.kerml` file under a directory:

```powershell
mercurio package build --file examples/src/examples --out examples.kpar
```

Override the package metadata:

```powershell
mercurio package build --file model.sysml --out model.kpar --name Demo --version 0.1.0
```

## Developer Tools

The `mercurio-tools` crate contains diagnostics, benchmark, demo, and Pilot comparison binaries. These are useful for maintainers, but they are separate from the public CLI surface.

### Inspect Connection Resolution

Dump parsed connection declarations and resolved usages for a SysML file:

```powershell
cargo run -p mercurio-tools --bin inspect_connection -- "examples/src/examples/Simple Tests/ConnectionTest.sysml"
```

### Run the Runtime Demo

Run graph subtype queries, feature queries, and a derived value calculation against the vehicle example model:

```powershell
cargo run -p mercurio-tools --bin runtime_demo
```

### Diagnose Example Corpus

Compile the default example corpus and emit a JSON diagnostic summary:

```powershell
cargo run -p mercurio-tools --bin diagnose_examples
```

Diagnose each top-level folder separately:

```powershell
cargo run -p mercurio-tools --bin diagnose_examples -- --folders --root examples/src/examples --out target/example-diagnostics.json
```

### Benchmark Example Compilation

Benchmark each top-level example folder:

```powershell
cargo run -p mercurio-tools --bin benchmark_examples -- --folders
```

Benchmark the full examples tree as one workspace:

```powershell
cargo run -p mercurio-tools --bin benchmark_examples -- --all --root examples/src/examples
```

Benchmark incremental edited-file behavior:

```powershell
cargo run -p mercurio-tools --bin benchmark_examples -- --edited --root examples/src/examples
```

## Pilot Comparison Tools

Several binaries compare Mercurio output against the Pilot implementation. These tools expect a Pilot checkout or exported Pilot artifacts.

Audit a Pilot corpus:

```powershell
cargo run -p mercurio-tools --bin audit_pilot_corpus -- --corpus small --pilot-root path/to/pilot --out target/pilot-audit.json
```

Compare one KerML example:

```powershell
cargo run -p mercurio-tools --bin compare_kerml_examples -- --examples-root examples/kerml/examples --relative-path "Vehicle Example/VehicleDefinitions.kerml" --pilot-root path/to/pilot --out target/kerml-compare.json
```

Compare Pilot AST, compile diagnostics, or semantics for one case:

```powershell
cargo run -p mercurio-tools --bin compare_pilot_ast -- --pilot-root path/to/pilot --relative-path "examples/Simple Tests/PartTest.sysml" --out target/pilot-ast.json
cargo run -p mercurio-tools --bin compare_pilot_compile_errors -- --pilot-root path/to/pilot --relative-path "examples/Simple Tests/PartTest.sysml" --out target/pilot-errors.json
cargo run -p mercurio-tools --bin compare_pilot_semantics -- --pilot-root path/to/pilot --relative-path "examples/Simple Tests/PartTest.sysml" --out target/pilot-semantics.json
```

Import Pilot standard library export data into KIR:

```powershell
cargo run -p mercurio-tools --bin import_pilot_stdlib -- --from-export path/to/pilot-stdlib-export.json --out resources/stdlib.kir.json
```

Or export directly from a Pilot checkout:

```powershell
cargo run -p mercurio-tools --bin import_pilot_stdlib -- --pilot-root path/to/pilot --out resources/stdlib.kir.json
```

## Repository Layout

- `Cargo.toml` - workspace manifest
- `crates/mercurio-core/` - library crate
- `crates/mercurio-cli/` - public command-line binaries
- `crates/mercurio-tools/` - maintainer diagnostics, benchmarks, demos, and Pilot comparison tools
- `crates/mercurio-core/src/frontend/` - SysML, KerML, linting, formatting, and resolver code
- `examples/` - KIR JSON models and SysML/KerML example corpora
- `resources/` - bundled runtime and library resources
- `docs/` - deeper architecture and implementation notes
- `crates/mercurio-core/tests/` - integration and corpus tests
- `tools/pilot-exporter/` - Java helper used by Pilot comparison workflows

## More Documentation

See [docs/README.md](docs/README.md) for architecture notes, language support plans, runtime details, server plans, and semantic service documentation.
