# KPAR Packages

## What A KPAR Is

A `.kpar` package is a source-backed model package. It contains SysML/KerML sources plus package metadata and can be used later as a model input or library dependency.

KPARs are useful when you want to:

- distribute a reusable model library
- compile or evaluate a packaged model
- mount read-only dependencies through a project descriptor
- preserve package metadata alongside source

## Build A Package

Build a source-backed `.kpar` package from a model file:

```powershell
mercurio package build --file model.sysml --out model.kpar
```

Build a package from every `.sysml` and `.kerml` file under a directory:

```powershell
mercurio package build --file examples/src/examples --out examples.kpar
```

Override package metadata:

```powershell
mercurio package build --file model.sysml --out model.kpar --name Demo --version 0.1.0
```

Embed the compiled KIR document in the KPAR:

```powershell
mercurio package build --file src --out domain-lib-0.1.0.kpar --name domain-lib --version 0.1.0 --include-kir
```

When `document.kir.json` is present in a KPAR, Mercurio loads that precompiled KIR instead of recompiling the package sources. The source files still remain in the archive for inspection and future rebuilds.

Package an existing KIR document directly:

```powershell
mercurio package build --kir resources/stdlib.full.kir.json --name org.omg/sysml-stdlib --version 2.0.0
```

## Local Package Repository

Mercurio supports a Maven-like local package repository for staged KPAR packages. In this workflow, `package build` can write to a local package repository first, and a later `package publish` command can push that staged package to a remote registry.

Default local repository:

```text
~/.mercurio/packages/
```

Package layout:

```text
~/.mercurio/packages/
  domain-lib/
    0.1.0/
      domain-lib-0.1.0.kpar
      manifest.json
```

Build and stage a package:

```powershell
mercurio package build --file src --name domain-lib --version 0.1.0
```

That command stages the package locally. The existing `--out` form remains useful when the caller wants to write a package to an explicit path:

```powershell
mercurio package build --file src --out dist/domain-lib-0.1.0.kpar --name domain-lib --version 0.1.0
```

The local package manifest records the package identity, file name, digest, creation time, and source path:

```json
{
  "schema": "dev.mercurio.local-package.v1",
  "name": "domain-lib",
  "version": "0.1.0",
  "kind": "kpar",
  "file": "domain-lib-0.1.0.kpar",
  "digest": "fnv1a64:...",
  "created_at": "unix:1780000000",
  "source": {
    "kind": "directory",
    "path": "C:/work/domain-lib/src"
  }
}
```

List staged packages:

```powershell
mercurio package list
```

Inspect a staged package:

```powershell
mercurio package inspect domain-lib --version 0.1.0
```

Verify a staged package:

```powershell
mercurio package verify domain-lib --version 0.1.0
```

Verification checks the local manifest, package digest, `.project.json`, source/precompiled payload presence, and parses `document.kir.json` when present.

Compile a staged package:

```powershell
mercurio package compile domain-lib --version 0.1.0 --format json
```

Publish a staged package into another package repository:

```powershell
mercurio package publish domain-lib --version 0.1.0 --to C:/work/published-packages
```

Use `--repo` to publish from a non-default source repository:

```powershell
mercurio package publish domain-lib --version 0.1.0 --repo C:/work/staged-packages --to C:/work/published-packages
```

Published package versions are immutable by default. Use `--force` to overwrite an existing package in the target repository:

```powershell
mercurio package publish domain-lib --version 0.1.0 --to C:/work/published-packages --force
```

Pull a package from another repository into the local package repository:

```powershell
mercurio package pull domain-lib --version 0.1.0 --from C:/work/published-packages
```

Use `--repo` to pull into a non-default target repository:

```powershell
mercurio package pull domain-lib --version 0.1.0 --from C:/work/published-packages --repo C:/work/staged-packages
```

Pulled package versions are immutable by default. Use `--force` to overwrite an existing package in the target repository.

## Compile A KPAR

Compile a KPAR package directly as a model input:

```powershell
mercurio compile --kpar model.kpar --format json
```

Compile a package from a URL when the URL ends in `.kpar`:

```powershell
mercurio compile --url https://example.com/packages/domain.kpar --format json
```

## Evaluate From A KPAR

Evaluate a derived feature from a KPAR package:

```powershell
mercurio evaluate --kpar model.kpar --feature totalMass --owner Demo.Vehicle
```

## Use A KPAR As A Library

Add a KPAR dependency in `.mercurio-project.json`:

```json
{
  "version": 1,
  "name": "My Model",
  "libraries": [
    {
      "id": "domain-lib",
      "provider": {
        "kind": "kpar_file",
        "path": "libs/domain.kpar"
      }
    }
  ]
}
```

Relative paths are resolved from the descriptor location.

## Package Locators

Project descriptors can use a locator-based provider. A locator describes the package coordinate, while Mercurio decides whether to load it from the local package repository, configured package repositories, or a bundled repository.

Example:

```json
{
  "version": 1,
  "name": "Vehicle Model",
  "libraries": [
    {
      "id": "domain-lib",
      "provider": {
        "kind": "kpar_locator",
        "locator": "kpar:domain-lib:0.1.0"
      }
    }
  ]
}
```

Supported locator forms in the first implementation:

```text
kpar:domain-lib:0.1.0
kpar:com.acme/domain-lib:0.1.0
file:libs/domain-lib-0.1.0.kpar
```

Planned later locator forms:

```text
kpar:domain-lib@sha256:abc123...
oci:ghcr.io/acme/mercurio/domain-lib:0.1.0
```

For a `kpar:` locator, resolution should try:

1. Local user package repository.
2. Configured package repositories.
3. Bundled package repository.

Configured repositories are exact package repository roots. They do not require an index.

Use an environment variable for ad hoc configuration:

```powershell
$env:MERCURIO_PACKAGE_REPOSITORIES = "C:/work/published-packages;D:/shared/mercurio-packages"
```

Or configure repository roots in `~/.mercurio/config.json`:

```json
{
  "package_repositories": [
    "C:/work/published-packages",
    "D:/shared/mercurio-packages"
  ]
}
```

Use `MERCURIO_CONFIG_PATH` to point Mercurio at a different config file.

If the package is found in a configured repository, Mercurio verifies the staged package manifest digest and then loads it through the existing KPAR library path.

If a `kpar:` locator cannot be resolved, Mercurio reports the package coordinate and every repository root it searched.

## Standard Library Package Locator

When a project omits `baseline_libraries`, Mercurio uses the default standard library locator:

```text
kpar:org.omg/sysml-stdlib:2.0.0
```

That locator now follows the package convention too. Resolution first checks local, configured, and bundled package repositories. Mercurio ships a bundled package for `org.omg/sysml-stdlib:2.0.0` that contains `document.kir.json`. If no staged package is present, Mercurio resolves the bundled OMG package set at `examples/sysml.library.kpar`, using `Systems-Library.kpar` as the entry package. During migration, the legacy bundled KIR at `resources/stdlib.full.kir.json` remains the final fallback.

## Compiled KIR Cache

KPAR is the package distribution format. Mercurio compiles KPAR sources into KIR before using them as semantic context.

For `kpar:` locators, Mercurio caches compiled KIR documents so repeated commands do not need to recompile unchanged packages:

```text
~/.mercurio/cache/kir/
  domain-lib/
    0.1.0/
      fnv1a64_.../
        fnv1a64_.../
          document.kir.json
          manifest.json
```

The cache key includes:

- package name
- package version
- KPAR digest
- importer version
- library context digest

The library context digest matters because non-baseline packages can compile differently depending on already-loaded baseline or dependency libraries.

## Planned OCI Publish

After a KPAR has been staged locally, the publish command should later support pushing it to an OCI registry:

```powershell
mercurio package publish domain-lib --version 0.1.0 --to oci://ghcr.io/acme/mercurio/domain-lib:0.1.0
```

The OCI artifact should use this media type:

```text
application/vnd.mercurio.kpar.v1+zip
```

The published artifact should include annotations for package name, version, kind, and digest.
