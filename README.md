# Mercurio

Mercurio is the open source Rust library and CLI workspace for working with SysML v2, KerML, and Mercurio's KIR JSON model representation.

The goal of this repository is to make the modeling kernel useful on its own: parse source models, compile them into semantic KIR, lint them, package them, and expose reusable library APIs that private products and external tools can build on.

## Objectives

- Provide a reusable Rust library for SysML v2 and KerML model processing.
- Keep the core model semantics independent from any particular server, desktop app, or hosted product.
- Offer a small public CLI that demonstrates the library without requiring the private product repo.
- Use KIR as the stable semantic interchange format for graph queries, derived values, package loading, and downstream applications.
- Optimize for high-performance model loading, compilation, and runtime use.
- Keep maintainer-only diagnostics, benchmarks, and Pilot comparison workflows separate from the public CLI.

## What Lives Here

- `mercurio-core` parses, compiles, lints, loads libraries, builds runtime graphs, and computes derived values.
- `mercurio-reasoner-api` defines product-neutral reasoning, capability, finding, and evidence DTOs for services and plugins.
- `mercurio-plugin-api` defines product-neutral plugin manifests, permissions, service declarations, and capability declarations.
- `mercurio-reference-capabilities` contains open deterministic reference capabilities built on core semantics.
- `mercurio-ai` contains provider adapters and semantic agent workflows that depend on core mutation, feasibility, and goal contracts without becoming part of the core library crate.
- `mercurio-cli` provides the public `mercurio` command for parse, compile, lint, query, evaluate, and package workflows.
- `mercurio-tools` contains maintainer tools for diagnostics, benchmarks, demos, and Pilot comparison/export workflows.
- `resources/` contains bundled runtime and standard library artifacts.
- `examples/` and `fixtures/` provide SysML, KerML, and KIR models for tests and demonstrations.

The hosted product, UI, and privileged console API live in the private `mercurio-product` repository. They depend on `mercurio-core` for domain behavior.

## Core Concepts

- Source languages: Mercurio reads `.sysml` and `.kerml` files. Inline CLI text defaults to SysML unless `--language kerml` is provided.
- KIR: Mercurio's validated semantic JSON format, used by graph queries, derived values, requirements views, package loading, and product hosts.
- Standard library: semantic compilation and linting use the bundled default standard library unless a command is given `--stdlib PATH`.
- KPAR packages: source-backed zip packages containing SysML/KerML sources plus package metadata.
- Reasoning reports: deterministic reference capabilities can return product-neutral findings and evidence through `mercurio-reasoner-api`; the first reference capability is requirement coverage.

## Requirements

- Rust toolchain with Cargo
- Java, only for Pilot comparison/export tools under `tools/pilot-exporter`

Most commands assume you are running them from the repository root.

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

## User Documentation

- [CLI Guide](docs/user/CLI.md): public `mercurio` command examples for parse, compile, lint, completions, and common input forms.
- [Project Descriptors](docs/user/PROJECTS.md): `mercurio-project.json`, provider kinds, and descriptor discovery.
- [KIR User Guide](docs/user/KIR.md): compiled semantic JSON, ids, provenance, validation, and low-level workflows.
- [Querying And Evaluation](docs/user/QUERY_EVALUATE.md): model queries, derived values, runtime context, and explanations.
- [KPAR Packages](docs/user/KPAR.md): building and consuming `.kpar` model packages.
- [Troubleshooting](docs/user/TROUBLESHOOTING.md): common command, descriptor, stdlib, KPAR, and Pilot-tool issues.

## Developer Documentation

- [Development Docs](docs/development/README.md): architecture notes, implementation plans, roadmap, runtime design, server plans, and semantic-service references.
- [Maintainer Tools](docs/development/MAINTAINER_TOOLS.md): diagnostics, benchmarks, demos, and Pilot comparison/export workflows.

## Repository Layout

- `Cargo.toml` - workspace manifest
- `crates/mercurio-core/` - library crate
- `crates/mercurio-reasoner-api/` - product-neutral reasoning service contracts
- `crates/mercurio-plugin-api/` - product-neutral plugin manifest and capability contracts
- `crates/mercurio-reference-capabilities/` - open deterministic reference capabilities over core semantics
- `crates/mercurio-cli/` - public command-line binary
- `crates/mercurio-tools/` - maintainer diagnostics, benchmarks, demos, and Pilot comparison tools
- `crates/mercurio-core/src/frontend/` - SysML, KerML, linting, formatting, and resolver code
- `examples/` - KIR JSON models and SysML/KerML example corpora
- `fixtures/` - test fixtures
- `resources/` - bundled runtime and library resources
- `docs/` - user docs plus development architecture and implementation notes
- `tools/pilot-exporter/` - Java helper used by Pilot comparison workflows

## Performance

Mercurio keeps the default standard library precompiled as KIR, uses bounded semantic caches for warm project workflows, and tracks load speed and memory use in benchmark runs.

The current benchmark snapshot is in [Compile Performance Benchmark](docs/development/COMPILE_PERFORMANCE_BENCHMARK.md).
