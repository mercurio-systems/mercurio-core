# Project Descriptor And Library Provider Plan

Status: partially implemented architecture and remaining work.

## Goal

Introduce a root-level project descriptor that defines:

- editable project sources
- read-only library dependencies
- baseline library inputs
- cache and refresh policy

For local and external-Git-authoritative projects, the descriptor belongs with the source repository. Mercurio Server may read descriptor metadata while indexing commits, but the descriptor does not make the server authoritative for the project.

The key point is that not every external input is the stdlib. Many `kpar` and API-backed inputs will be ordinary L2 libraries. The built-in stdlib should be treated as one baseline library source, not as the central abstraction for the whole design.

## 1. Define The Model

Create a root project descriptor as the single place that declares:

- project metadata
- included and excluded editable local files
- library dependency sources
- baseline library configuration
- cache policy and refresh policy

Keep the descriptor focused on intent, not runtime state. Runtime state such as last refresh time, resolved digests, cache paths, and resolved snapshots should live in generated metadata.

## 2. Separate Three Concepts

Use three distinct layers in the design:

- `ProjectDescriptor`: declarative config checked into the project
- `Provider`: filesystem, `kpar`, package-set directory, API snapshot, bundled baseline library
- `ResolvedArtifacts`: normalized local outputs, especially cached KIR

This avoids mixing "what the project is" with "how inputs are fetched" and "what the compiler consumes."

## 3. Distinguish Source Categories

The design should explicitly separate:

- editable project sources
- read-only library dependencies
- baseline libraries

Editable project sources are usually local filesystem content.
Library dependencies are usually read-only inputs from `kpar`, package sets, directories, or repository APIs.
Baseline libraries are the default foundational libraries we currently inject through the built-in KIR stdlib path.

This keeps the built-in stdlib in the design, but as a special baseline dependency rather than the definition of the whole mechanism.

## 4. Keep The Compiler Boundary Stable

Do not change the core compiler to consume arbitrary providers directly.

Instead:

- project sources continue to resolve to logical `.sysml` files
- library dependencies resolve to preprocessed cached artifacts
- baseline libraries resolve to preprocessed cached artifacts
- the compiler continues consuming local logical source files plus one or more `KirDocument` library artifacts

This preserves the current `WorkspaceService` shape and limits risk.

## 5. Introduce A Library Provider Pipeline

Add a library resolver with provider kinds such as:

- bundled baseline library
- precompiled KIR artifact
- local SysML directory
- local `kpar`
- local package-set directory
- remote API snapshot
- project repository snapshot

Every dependency or baseline provider must produce:

- normalized cached artifact, usually KIR
- provenance metadata
- cache key based on source identity, revision or version, and importer version

The compiler should consume the cached artifact, not the provider directly.

When the descriptor must point directly at an already-normalized artifact, that should be modeled explicitly as a precompiled artifact reference such as `precompiled_kir_artifact`, not as a source-style provider name. KIR remains an intermediate artifact, not a conceptual source format.

## 6. Start With Read-Only External Inputs

In v1, support:

- editable filesystem project files
- read-only `kpar` libraries
- read-only package-set libraries
- read-only API snapshot libraries
- read-only baseline library providers

Do not support in-place editing of mounted remote or package content yet.

## 7. Descriptor Shape

Add fields for:

- `version`
- `name`
- `meta`
- `project_files`
- `libraries`
- `cache`

Each library entry carries a `role`:

- `role: "dependency"` is the general mechanism for ordinary dependencies, including many L2 libraries
- `role: "baseline"` is the special mechanism for the foundational set we currently treat as stdlib

This avoids making every package look like a stdlib override.

## 8. Cache Design

Implement a local cache directory that stores:

- preprocessed library KIR
- preprocessed baseline library KIR
- provenance manifests with source URI, version or revision, digest, importer version, and timestamp

Support three cache modes:

- use existing cache
- refresh on demand
- fail if source is unavailable and no cache exists

## 9. Migration Path

Roll this out in phases:

1. Add descriptor discovery with a legacy fallback when no descriptor exists.
2. Move bundled stdlib loading behind a baseline library provider interface.
3. Add preprocessing and cache metadata for baseline libraries.
4. Add local `kpar` and package-set library providers for ordinary dependencies.
5. Add API-backed library providers with pinned revision support.
6. Generalize project source selection when the dependency model is stable.

## 10. UI And API Impact

Expose enough metadata for the UI to distinguish:

- editable local project files
- read-only dependency files from packages or APIs
- baseline library provenance and cache status

Do not surface every dependency as if it were part of the editable project tree unless there is a clear user need. Read-only dependencies and baseline libraries should be visibly different from project sources.

## 11. Key Risks To Control

Watch for:

- non-reproducible API inputs without revision pinning
- cache invalidation bugs
- path collisions across providers
- implicit precedence rules between project files and dependencies
- descriptor creep into runtime state
- overloading the word "stdlib" so that ordinary libraries get modeled incorrectly

## 12. Current Implementation Status

Current implemented provider kinds in `mercurio-core`:

- `bundled_stdlib`
- `precompiled_kir_artifact`
- `sysml_directory`
- `kpar_file`
- `package_set_directory`

The current gap is package-set and API-backed resolution:

- `kpar_file` works for one package archive
- `package_set_directory` now resolves one root package plus its local `.project.json` `usage` dependency closure from a directory like [resources/stdlib-sources/sysml-2.0-pilot-0.57.0/sysml.library.kpar](/C:/dev/git/mercurio/ideation-m2/resources/stdlib-sources/sysml-2.0-pilot-0.57.0/sysml.library.kpar)
- resolved descriptor/library metadata is surfaced through workspace status and semantic workspace session APIs for GUI testing
- API-backed snapshot providers are still future work

## 13. Observed KPAR Structure

The example package set at [resources/stdlib-sources/sysml-2.0-pilot-0.57.0/sysml.library.kpar](/C:/dev/git/mercurio/ideation-m2/resources/stdlib-sources/sysml-2.0-pilot-0.57.0/sysml.library.kpar) is not a single archive. It is a directory containing multiple package files such as:

- `Kernel_Data_Type_Library-1.0.0.kpar`
- `Kernel_Function_Library-1.0.0.kpar`
- `Kernel_Semantic_Library-1.0.0.kpar`
- `SysML_Systems_Library-2.0.0.kpar`
- `SysML_Quantities_and_Units_Library-2.0.0.kpar`

Each inner `.kpar` is a zip archive containing:

- `.project.json`
- `.meta.json`
- one or more `.sysml` source files

For example, `SysML_Systems_Library-2.0.0.kpar` contains package metadata plus source files such as:

- `SysML.sysml`
- `Parts.sysml`
- `Actions.sysml`
- `Requirements.sysml`

The manifests provide useful structure:

- `.project.json` includes package identity, version, description, and package dependencies under `usage`
- `.meta.json` includes an index of logical source names, metamodel URI, creation timestamp, and per-file checksums

## 14. KPAR Design Implications

This observed structure implies:

- a `kpar` provider is more than archive reading; it is package resolution plus source extraction
- a top-level library input may be a package-set directory rather than one monolithic package file
- dependencies are explicit and should be resolved deterministically
- package references are expressed as canonical resource URIs plus version constraints
- local package-set resolution should be sufficient for v1, without requiring remote resolution

This strengthens the case for treating `kpar` as a library dependency provider rather than a generic filesystem-style mount.

For v1, the likely shape is:

- descriptor points to a local package-set directory or specific package file
- resolver reads `.project.json` dependency declarations
- resolver matches required packages from the local package set
- resolver extracts or streams `.sysml` sources into a normalized local preprocessing step
- preprocessing generates cached KIR plus provenance metadata

## 15. Example Providers

Two concrete examples now anchor the provider design:

- local `kpar` package-set example:
  [resources/stdlib-sources/sysml-2.0-pilot-0.57.0/sysml.library.kpar](/C:/dev/git/mercurio/ideation-m2/resources/stdlib-sources/sysml-2.0-pilot-0.57.0/sysml.library.kpar)
- remote API repository example:
  [Intercax API docs](http://sysml2.intercax.com:9000/docs/)

These examples suggest two distinct provider categories:

- `kpar` or package-set library provider
  - resolves package files from a local package set
  - reads `.project.json` and `.meta.json`
  - extracts or streams `.sysml` content
  - resolves package dependencies locally
- repository API library provider
  - connects to a remote SysML repository implementation
  - resolves a pinned model, branch, package, or revision snapshot
  - materializes a stable preprocessing input
  - then produces cached local KIR

The API provider should be treated as a snapshot source, not as a live mutable filesystem equivalent.

An external Git project provider follows the same rule: resolve a specific commit or tag, materialize a stable source snapshot, then produce cached KIR with provenance.
