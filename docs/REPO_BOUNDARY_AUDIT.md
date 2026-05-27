# Repo Boundary Audit

This audit classifies the current `mercurio-core` workspace before the physical
restructure. The goal is to keep the open core repository focused on:

```text
KIR + parser + compiler + stdlib + deterministic graph/query/runtime
```

## Current Workspace Classification

| Current crate | Target repo | Target crate / package | Action |
| --- | --- | --- | --- |
| `crates/mercurio-core` | `mercurio-core` | split into `mercurio-kir`, `mercurio-sysml`, `mercurio-compiler`, `mercurio-runtime`, `mercurio-stdlib` | split |
| `crates/mercurio-cli` | `mercurio-core` | `mercurio-cli` | keep, then trim reasoning commands |
| `crates/mercurio-wasm` | `mercurio-core` | `mercurio-wasm` | keep, core bindings only |
| `crates/mercurio-python` | `mercurio-core` | `mercurio-python` | keep, core bindings only |
| `crates/mercurio-tools` | `mercurio-core` | `mercurio-tools` or `xtask` | keep temporarily; split public audit tools from migration-only tools |
| `crates/mercurio-reasoner-api` | `mercurio-reasoning` | `mercurio-reasoner-api` | moved |
| `crates/mercurio-plugin-api` | `mercurio-reasoning` | `mercurio-plugin-api` | moved |
| `crates/mercurio-reference-capabilities` | `mercurio-reasoning` | `mercurio-reference-capabilities` | moved |
| `crates/mercurio-ai` | `mercurio-reasoning` or `mercurio-product` | `mercurio-ai-orchestration` | moved |

## Current `mercurio-core` Module Classification

| Module | Target | Action |
| --- | --- | --- |
| `ir.rs` | `mercurio-kir` | move |
| `metadata.rs` | `mercurio-kir` | move |
| `frontend/ast.rs` | `mercurio-sysml` | move |
| `frontend/lexer.rs` | `mercurio-sysml` | move |
| `frontend/format.rs` | `mercurio-sysml` | move |
| `frontend/lint.rs` | `mercurio-sysml` | move |
| `frontend/sysml.rs` | `mercurio-sysml` + `mercurio-compiler` | split parser from compile wrapper |
| `frontend/kerml.rs` | `mercurio-sysml` + `mercurio-compiler` | split parser from compile wrapper |
| `frontend/resolver.rs` | `mercurio-compiler` | move |
| `frontend/transpile.rs` | `mercurio-compiler` | move |
| `source_set.rs` | `mercurio-compiler` | move |
| `language.rs` | `mercurio-sysml` | move |
| `metamodel.rs` | `mercurio-sysml` or `mercurio-runtime` | split by usage |
| `library.rs` | `mercurio-stdlib` | move |
| `project.rs` | `mercurio-compiler` / `mercurio-stdlib` | split |
| `project_cache.rs` | `mercurio-runtime` | move |
| `graph.rs` | `mercurio-runtime` | move |
| `query.rs` | `mercurio-runtime` | move |
| `datalog.rs` | `mercurio-runtime` | move |
| `derived.rs` | `mercurio-runtime` | move |
| `expression.rs` | `mercurio-runtime` | move |
| `runtime.rs` | `mercurio-runtime` | move |
| `views.rs` | `mercurio-runtime` or product | split core data views from UI-shaped DTOs |
| `diagrams.rs` | `mercurio-reasoning` or product visualization | move unless retained as core graph DTO only |
| `assessment.rs` | `mercurio-reasoning` | move |
| `behavior.rs` | split | keep projection in core, move execution/scenarios/traces to reasoning |
| `constraints.rs` | `mercurio-reasoning` / `mercurio-services` | move |
| `feasibility.rs` | `mercurio-reasoning` | move |
| `goal.rs` | `mercurio-reasoning` | move |
| `mutation.rs` | `mercurio-reasoning` or product authoring | split |
| `proposal.rs` | `mercurio-product` or `mercurio-reasoning` | move |
| `authoring.rs` | product authoring or separate authoring repo | split after compiler/runtime extraction |
| `outline.rs` | product/editor support | move or split |
| `semantic_compare.rs` | `mercurio-runtime` / `mercurio-reasoning` | split snapshot substrate from impact reasoning |
| `syntax_compare.rs` | `mercurio-sysml` | move if retained |

## Boundary Rules For New Work

1. New parsing, KIR, compile, stdlib, graph, query, runtime, and cache work may stay in core.
2. New reasoning reports, evidence graphs, findings, simulation execution, decision logic, AI orchestration, and plugin/service contracts must not be added to core.
3. If a higher-level feature needs better core semantics, fix the parser/compiler/KIR/runtime layer first.
4. Product UI must consume artifacts and reports; it must not implement semantic analysis.
5. Every major capability must be runnable headlessly before receiving a custom UI.

## Boundary Check

The mechanical boundary manifest lives at `repo-boundaries.json`.

Run the non-strict check during the transition:

```powershell
cargo run -p mercurio-tools --bin check_repo_boundaries
```

This fails on unclassified crates or root directories that belong in peer repositories.

Run the strict check to also fail if any transitional migration crates are added back:

```powershell
cargo run -p mercurio-tools --bin check_repo_boundaries -- --strict
```

## Restructure Order

1. Clean working trees and ignore generated cache outputs.
2. Add boundary docs and crate/module inventory.
3. Remove or relocate reasoning commands from `mercurio-cli`.
4. Extract `mercurio-kir`.
5. Extract `mercurio-sysml`.
6. Extract `mercurio-compiler`.
7. Extract `mercurio-runtime`.
8. Extract `mercurio-stdlib`.
9. Move reasoning/API/plugin/AI crates to `mercurio-reasoning`.
10. Update `mercurio-product` dependencies.

## Dependency Direction

Allowed:

```text
mercurio-product -> mercurio-services -> mercurio-reasoning -> mercurio-core
```

Disallowed:

```text
mercurio-core -> mercurio-reasoning
mercurio-core -> mercurio-product
mercurio-core -> mercurio-services
```
