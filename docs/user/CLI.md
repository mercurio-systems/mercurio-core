# Mercurio CLI Guide

## Overview

The public CLI is one cohesive `mercurio` binary with `project`, `parse`, `compile`, `query`, `evaluate`, `lint`, `package`, and `completions` subcommands.

Source-based commands can read model input from:

- `--file PATH`
- `--text TEXT`
- `--url URL`

`compile`, `query`, and `evaluate` commands can also read prebuilt inputs where supported:

- `--kir PATH` for KIR JSON input
- `--kpar PATH` for KPAR package input

Inline text defaults to SysML. File and URL input infer language from `.sysml` or `.kerml`. Use `--language kerml` for inline KerML.

## Quick Commands

Show CLI help:

```powershell
mercurio --help
```

Parse inline SysML:

```powershell
mercurio parse --text "package Demo { part def Vehicle; }"
```

Compile inline KerML:

```powershell
mercurio compile --text "package Demo { classifier Vehicle; }" --language kerml
```

Create a project scaffold:

```powershell
mercurio project new my-model --name "My Model"
```

Lint a file:

```powershell
mercurio lint --file "test_files/examples/src/examples/Simple Tests/PartTest.sysml"
```

Build and stage a KPAR package:

```powershell
mercurio package build --file src --name domain-lib --version 0.1.0
```

## Project Scaffolding

Create a new project directory with `.mercurio-project.json` and `src/main.sysml`:

```powershell
mercurio project new my-model --name "My Model"
```

Use `--force` to scaffold into an existing non-empty directory and `--quiet` to suppress the creation summary. New descriptors use a single `libraries` array; baseline libraries are entries with `role: "baseline"` and ordinary dependencies use `role: "dependency"`.

## Parse SysML Or KerML

Parse one file and print a syntax summary:

```powershell
mercurio parse --file "test_files/examples/src/examples/Simple Tests/PartTest.sysml"
```

Parse inline SysML:

```powershell
mercurio parse --text "package Demo { part def Vehicle; }"
```

Emit the syntax AST as JSON:

```powershell
mercurio parse --file "test_files/examples/src/examples/Simple Tests/PartTest.sysml" --format json
```

## Compile To KIR

Compile a file to KIR using the default standard library:

```powershell
mercurio compile --file "test_files/examples/src/examples/Simple Tests/PartTest.sysml"
```

Emit the KIR document as JSON:

```powershell
mercurio compile --text "package Demo { part def Vehicle; }" --format json
```

Override the standard library:

```powershell
mercurio compile --file model.sysml --stdlib resources/stdlib-sources/sysml-2.0-pilot-0.57.0/stdlib.full.kir.json
```

Compile source from a network URL:

```powershell
mercurio compile --url https://example.com/models/vehicle.sysml
```

Compile a KPAR package:

```powershell
mercurio compile --kpar model.kpar --format json
```

## Query And Evaluate

Query a source model:

```powershell
mercurio query --file model.sysml --query 'from elements where kind = "SysML::Systems::PartDefinition" select id, qualified_name'
```

Query from a file:

```powershell
mercurio query --kpar model.kpar --query-file queries/requirements.mq --format json
```

Evaluate a derived feature:

```powershell
mercurio evaluate --file model.sysml --owner Demo.Vehicle --feature totalMass
```

Provide runtime overlay values and explanation output:

```powershell
mercurio evaluate --kir model.kir.json --owner Demo.Vehicle --feature totalMass --value Demo.Vehicle.mass=42 --explain
```

Higher-level reasoning capabilities, such as behavioral simulation and requirement coverage reports, live in the sibling `mercurio-reasoning` repository.


## Lint SysML Or KerML

Lint one file:

```powershell
mercurio lint --file "test_files/examples/src/examples/Simple Tests/PartTest.sysml"
```

Lint every `.sysml` and `.kerml` file under a directory:

```powershell
mercurio lint --file "test_files/examples/src/examples/Simple Tests"
```

Emit JSON diagnostics:

```powershell
mercurio lint --file "test_files/examples/src/examples/Simple Tests" --format json
```

Fail when warnings are present, useful for CI:

```powershell
mercurio lint --file "test_files/examples/src/examples/Simple Tests" --warnings-as-errors
```

## KPAR Package Workflows

Build a package from source files and write it to a specific path:

```powershell
mercurio package build --file src --out dist/domain-lib-0.1.0.kpar --name domain-lib --version 0.1.0
```

Build and stage a package in the default local package repository:

```powershell
mercurio package build --file src --name domain-lib --version 0.1.0
```

Include precompiled KIR in the package:

```powershell
mercurio package build --file src --name domain-lib --version 0.1.0 --include-kir
```

List, inspect, verify, and compile staged packages:

```powershell
mercurio package list
mercurio package inspect domain-lib --version 0.1.0
mercurio package verify domain-lib --version 0.1.0
mercurio package compile domain-lib --version 0.1.0 --format json
```

Move packages between package repositories:

```powershell
mercurio package publish domain-lib --version 0.1.0 --to C:/work/published-packages
mercurio package pull domain-lib --version 0.1.0 --from C:/work/published-packages
```

Publish to an indexless HTTP package manager:

```powershell
mercurio package publish domain-lib --version 0.1.0 --to https://packages.example.com/mercurio
```

Install by coordinate from a package repository or indexless HTTP package manager:

```powershell
mercurio package install kpar:domain-lib:0.1.0 --from https://packages.example.com/mercurio
```

For HTTP(S), Mercurio resolves the coordinate to the package repository layout, fetches `manifest.json`, verifies the downloaded KPAR digest, and stages it locally. Use `--repo` with package repository commands to use a non-default source or target repository. `publish`, `pull`, and `install` keep existing versions immutable unless `--force` is provided.

## Plugin Registry

Install and inspect plugin manifests through the default `mercurio` CLI:

```powershell
mercurio plugin install .\extension.json
mercurio plugin install .\org.mercurio.semantic-impact-0.1.0.mpack
mercurio plugin install mpack:org.mercurio.semantic-impact:0.1.0 --from .\plugin-repo
mercurio plugin publish .\org.mercurio.semantic-impact-0.1.0.mpack --to .\plugin-repo
mercurio plugin list
mercurio plugin inspect org.mercurio.requirements --version 0.1.0
```

The install command accepts either a raw `extension.json` manifest or a packaged `.mpack` archive containing `extension.json`. Packaged installs preserve the original archive as `plugin.mpack` beside the normalized installed manifest.

Without an index, coordinate installs resolve against a local plugin repository layout:

```text
plugin-repo/
  org.mercurio.semantic-impact/
    0.1.0/
      plugin.mpack
```

`plugin publish` writes that layout and reports the package digest. Project plugin pins can use the digest to bind execution to an exact `.mpack` archive.

Reasoning services are implemented in the sibling `mercurio-reasoning` repository. `mercurio-reason invoke` can resolve service declarations from this same local plugin registry, while the default `mercurio` CLI remains the package and registry management surface.

## Shell Completions

Generate completion scripts:

```powershell
mercurio completions powershell
```

Supported shells are `bash`, `elvish`, `fish`, `powershell`, and `zsh`.

For PowerShell:

```powershell
mercurio completions powershell > mercurio-completions.ps1
. .\mercurio-completions.ps1
```

For Bash or Zsh:

```bash
mercurio completions bash > ~/.local/share/bash-completion/completions/mercurio
mercurio completions zsh > ~/.zfunc/_mercurio
```
