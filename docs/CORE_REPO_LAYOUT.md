# Mercurio Core Repository Layout

This document defines the target boundary for the open `mercurio-core` repository.

The core repository owns the deterministic semantic substrate:

- KIR schema, validation, metadata, and artifact IO
- SysML/KerML language parsing, formatting, and linting
- SysML/KerML to KIR compilation
- bundled standard library artifacts and compiler mappings
- deterministic graph, query, and runtime indexes over KIR
- thin developer interfaces for parse/compile/query/package workflows

The core repository does not own product reasoning, AI orchestration, private services,
domain rulepacks, or UI workflows.

## Target Repository Shape

```text
mercurio-core/
  Cargo.toml
  README.md
  LICENSE

  crates/
    mercurio-kir/
      src/
        lib.rs
        document.rs
        element.rs
        fields.rs
        metadata.rs
        validation.rs
        artifact.rs

    mercurio-sysml/
      src/
        lib.rs
        lexer.rs
        ast.rs
        parser/
        formatter.rs
        lint.rs
        language_profile.rs

    mercurio-compiler/
      src/
        lib.rs
        resolver.rs
        transpile.rs
        source_set.rs
        mappings.rs
        diagnostics.rs

    mercurio-runtime/
      src/
        lib.rs
        graph.rs
        query.rs
        datalog.rs
        derived.rs
        expression.rs
        runtime.rs
        cache.rs

    mercurio-stdlib/
      src/
        lib.rs
        library.rs
      resources/
        stdlib/
        mappings/
        rulepacks/

    mercurio-cli/
      src/
        main.rs

    mercurio-wasm/
      src/
        lib.rs

    mercurio-python/
      src/
        lib.rs

  test_files/
    examples/
    kerml/
    l2/

  docs/
    CORE_REPO_LAYOUT.md
    KIR_SCHEMA.md
    LANGUAGE_SUPPORT.md
    COMPILER_PIPELINE.md
    QUERY_RUNTIME.md
    MIGRATION_BOUNDARIES.md
```

## Workspace Membership

The eventual core workspace should contain only crates that support KIR, language,
compiler, stdlib, graph/query/runtime, and thin developer access.

```toml
[workspace]
members = [
    "crates/mercurio-kir",
    "crates/mercurio-sysml",
    "crates/mercurio-compiler",
    "crates/mercurio-runtime",
    "crates/mercurio-stdlib",
    "crates/mercurio-cli",
    "crates/mercurio-wasm",
    "crates/mercurio-python",
]
resolver = "2"
```

## Crate Responsibilities

### `mercurio-kir`

Canonical KIR types and artifact behavior.

Owns:

- `KirDocument`
- `KirElement`
- KIR schema versioning
- field registry
- metadata annotation helpers
- KIR validation
- JSON/binary artifact IO boundaries

Does not own:

- SysML parsing
- graph materialization
- reasoning findings
- simulation execution

### `mercurio-sysml`

Language frontend for SysML/KerML source text.

Owns:

- lexer
- AST
- parser
- formatter
- language lint diagnostics
- language profile / metamodel concept lookup

Does not own:

- KIR graph runtime
- reasoning capabilities
- AI workflows

### `mercurio-compiler`

Compilation from language ASTs into KIR.

Owns:

- name resolution
- import/context resolution
- semantic diagnostics
- SysML/KerML to KIR transpilation
- source set compilation
- compiler mapping tables

Depends on:

- `mercurio-kir`
- `mercurio-sysml`
- `mercurio-stdlib`

### `mercurio-runtime`

Deterministic graph/query/runtime substrate over KIR.

Owns:

- semantic graph
- query engine
- Datalog/index materialization
- expression IR and deterministic evaluation
- derived property indexes
- persistent compile/runtime cache

Does not own:

- domain-specific reasoning reports
- simulations whose semantics exceed standard KIR/runtime meaning
- AI explanations

### `mercurio-stdlib`

Bundled standard library and compiler support resources.

Owns:

- stdlib document loading
- KPAR/package library loading
- compiler mappings
- core rulepacks required for deterministic graph/query behavior

### `mercurio-cli`

Developer CLI for core workflows.

Owns commands for:

- parse
- compile
- lint
- query
- evaluate deterministic expressions
- package/build KPAR artifacts
- inspect KIR/runtime artifacts

Does not own:

- private reasoning service orchestration
- AI chat
- product UI commands

### `mercurio-wasm` and `mercurio-python`

Thin bindings over core capabilities.

Own:

- parse/compile/query/KIR IO bindings

Do not own:

- reasoning services
- AI orchestration
- private simulation backends

## Moved Out Of Core

The following crates have moved to the sibling `mercurio-reasoning` repository:

```text
mercurio-reasoning/crates/mercurio-ai
mercurio-reasoning/crates/mercurio-plugin-api
mercurio-reasoning/crates/mercurio-reasoner-api
mercurio-reasoning/crates/mercurio-reference-capabilities
```

Reasoning and simulation material should move to a separate repository:

```text
mercurio-reasoning/
  crates/
    mercurio-reasoner-api/
    mercurio-plugin-api/
    mercurio-reference-capabilities/
    mercurio-behavior/
    mercurio-simulation/
    mercurio-analysis/
```

Private/product material should remain outside core:

```text
mercurio-product/
  apps/
  services/
  packages/
```

## Boundary Rules

1. Core truth is KIR.
2. Core behavior must be deterministic and testable.
3. Core must not depend on LLM providers, private services, or product UI.
4. Core may expose semantic projections only when they reflect SysML/KerML/KIR semantics.
5. Reasoning consumes KIR/runtime artifacts and core projections.
6. Product UI displays and orchestrates; it does not implement semantic analysis.
7. Problems caused by incomplete KIR/compiler/runtime semantics must be fixed in core, not patched in reasoning or UI.

## Migration Map From Current Repo

Current module/crate ownership target:

```text
crates/mercurio-core/src/ir.rs                 -> mercurio-kir
crates/mercurio-core/src/metadata.rs           -> mercurio-kir
crates/mercurio-core/src/frontend/lexer.rs     -> mercurio-sysml
crates/mercurio-core/src/frontend/ast.rs       -> mercurio-sysml
crates/mercurio-core/src/frontend/sysml.rs     -> mercurio-sysml + mercurio-compiler split
crates/mercurio-core/src/frontend/kerml.rs     -> mercurio-sysml + mercurio-compiler split
crates/mercurio-core/src/frontend/format.rs    -> mercurio-sysml
crates/mercurio-core/src/frontend/lint.rs      -> mercurio-sysml
crates/mercurio-core/src/frontend/resolver.rs  -> mercurio-compiler
crates/mercurio-core/src/frontend/transpile.rs -> mercurio-compiler
crates/mercurio-core/src/source_set.rs         -> mercurio-compiler
crates/mercurio-core/src/language.rs           -> mercurio-sysml
crates/mercurio-core/src/metamodel.rs          -> mercurio-sysml or mercurio-runtime by use
crates/mercurio-core/src/library.rs            -> mercurio-stdlib
crates/mercurio-core/src/project.rs            -> mercurio-stdlib / mercurio-compiler
crates/mercurio-core/src/graph.rs              -> mercurio-runtime
crates/mercurio-core/src/query.rs              -> mercurio-runtime
crates/mercurio-core/src/datalog.rs            -> mercurio-runtime
crates/mercurio-core/src/derived.rs            -> mercurio-runtime
crates/mercurio-core/src/expression.rs         -> mercurio-runtime
crates/mercurio-core/src/runtime.rs            -> mercurio-runtime
crates/mercurio-core/src/project_cache.rs      -> mercurio-runtime
```

Modules to move out or split carefully:

```text
crates/mercurio-core/src/assessment.rs
crates/mercurio-core/src/behavior.rs
crates/mercurio-core/src/constraints.rs
crates/mercurio-core/src/feasibility.rs
crates/mercurio-core/src/goal.rs
crates/mercurio-core/src/mutation.rs
crates/mercurio-core/src/proposal.rs
mercurio-reasoning/crates/mercurio-reference-capabilities/
mercurio-reasoning/crates/mercurio-reasoner-api/
mercurio-reasoning/crates/mercurio-plugin-api/
mercurio-reasoning/crates/mercurio-ai/
```

Some of these contain useful deterministic substrate. The rule is:

- keep KIR/schema/compiler/runtime-neutral pieces in core
- move reasoning reports, findings, decision logic, simulation execution, and AI integration out

## Phased Migration

### Phase 1: Boundary Freeze

- Add this layout document.
- Mark reasoning/AI/plugin crates as non-core candidates.
- Stop adding new reasoning services to `mercurio-core`.
- Keep current build working.

### Phase 2: Extract KIR Crate

- Created `mercurio-kir`.
- Moved KIR document/element/field/validation APIs.
- Re-exported from existing `mercurio-core` temporarily for compatibility.
- Kept core-specific model-stack loading in `mercurio-core::ir`.

### Phase 3: Extract Language and Compiler

- Create `mercurio-sysml`.
- Move lexer/AST/parser/format/lint.
- Create `mercurio-compiler`.
- Move resolver/transpiler/source-set compilation.

### Phase 4: Extract Runtime

- Create `mercurio-runtime`.
- Move graph/query/datalog/expression/runtime/cache.
- Keep compile/query CLI commands passing.

### Phase 5: Move Reasoning Out

- Move reasoner API, plugin API, reference capabilities, AI, behavior execution, and simulation reports to `mercurio-reasoning`.
- Core may retain only semantic projections that are direct KIR/SysML normalization.

### Phase 6: Tighten Public API

- Remove broad compatibility re-exports.
- Stabilize crate-level APIs.
- Add dependency direction checks.

## Target Dependency Direction

```text
mercurio-kir
  ↑
mercurio-stdlib
  ↑
mercurio-sysml
  ↑
mercurio-compiler
  ↑
mercurio-runtime
  ↑
mercurio-cli / mercurio-wasm / mercurio-python
```

Reasoning repositories may depend on core crates:

```text
mercurio-reasoning -> mercurio-kir / mercurio-runtime / mercurio-compiler
```

Core crates must not depend on reasoning repositories.
