# Maintainer Tools

Status: maintainer reference.

## Overview

The `mercurio-tools` crate contains diagnostics, benchmark, demo, and Pilot comparison binaries. These are useful for maintainers, but they are separate from the public CLI surface.

Pilot comparison tools expect a Pilot checkout or exported Pilot artifacts. Java is required only for the Pilot helper under `tools/pilot-exporter`.

Peer repository roots can be supplied either by command-line flags or environment variables:

```powershell
$env:MERCURIO_WORKSPACE_ROOT = "C:\dev\git\mercurio"
$env:MERCURIO_PILOT_ROOT = "C:\dev\git\mercurio\SysML-v2-Pilot-Implementation"
$env:MERCURIO_EXAMPLES_ROOT = "C:\dev\git\mercurio\mercurio-examples"
```

If `MERCURIO_PILOT_ROOT` is unset, Pilot-facing tools look under `MERCURIO_WORKSPACE_ROOT\SysML-v2-Pilot-Implementation`, then `MERCURIO_WORKSPACE_ROOT\external\SysML-v2-Pilot-Implementation`. Without `MERCURIO_WORKSPACE_ROOT`, they fall back to `../external/SysML-v2-Pilot-Implementation` and then `../SysML-v2-Pilot-Implementation`.

## Inspect Connection Resolution

Dump parsed connection declarations and resolved usages for a SysML file:

```powershell
cargo run -p mercurio-tools --bin inspect_connection -- "examples/src/examples/Simple Tests/ConnectionTest.sysml"
```

## Run The Runtime Demo

Run graph subtype queries, feature queries, and a derived value calculation against the vehicle example model:

```powershell
cargo run -p mercurio-tools --bin runtime_demo
```

## Check Repository Boundaries

Check that crates and root directories are classified by the core repository boundary manifest:

```powershell
cargo run -p mercurio-tools --bin check_repo_boundaries
```

Use `--strict` to fail if transitional migration crates are added back to `mercurio-core`.

## Diagnose Example Corpus

Compile the default example corpus and emit a JSON diagnostic summary:

```powershell
cargo run -p mercurio-tools --bin diagnose_examples
```

Diagnose each top-level folder separately:

```powershell
cargo run -p mercurio-tools --bin diagnose_examples -- --folders --root examples/src/examples --out target/example-diagnostics.json
```

## Benchmark Example Compilation

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

Audit a Pilot corpus:

```powershell
cargo run -p mercurio-tools --bin audit_pilot_corpus -- --corpus small --pilot-root path/to/pilot --out target/pilot-audit.json
```

With `MERCURIO_PILOT_ROOT` configured, `--pilot-root` may be omitted:

```powershell
cargo run -p mercurio-tools --bin audit_pilot_corpus -- --corpus small --out target/pilot-audit.json
```

Compare one KerML example:

```powershell
cargo run -p mercurio-tools --bin compare_kerml_examples -- --examples-root examples/kerml/examples --relative-path "Vehicle Example/VehicleDefinitions.kerml" --pilot-root path/to/pilot --out target/kerml-compare.json
```

`compare_kerml_examples` also honors `MERCURIO_EXAMPLES_ROOT`. If that variable points at the `mercurio-examples` repository root, the tool uses its `kerml/examples` folder.

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
