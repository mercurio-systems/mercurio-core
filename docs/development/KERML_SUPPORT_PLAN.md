# Robust KerML Support Plan

Status: partially implemented plan.

## Goal

Make `.kerml` a first-class Mercurio input format while preserving the existing architecture:

```text
.kerml / .sysml / KIR
          |
          v
  frontend parser + resolver + transpiler
          |
          v
        KIR
          |
          v
  generic semantic graph runtime
```

Robust KerML support should strengthen the shared foundation that SysML depends on. It should not create a second runtime, a syntax-bound semantic path, or a parallel model representation.

## Current Baseline

The repo already has the pieces needed to evolve toward this:

- `docs/development/KIR_SPEC.md` defines KIR as the semantic source of truth and records the current KIR contract.
- `docs/development/L2_PARSER_PLAN.md` defines the Rust-native text frontend approach.
- `mercurio-core/src/frontend/sysml.rs` parses a growing SysML subset.
- `mercurio-core/src/frontend/ast.rs` contains generic declaration, feature, package, import, alias, and expression nodes.
- `mercurio-core/src/frontend/resolver.rs` resolves local names, imports, aliases, stdlib references, and expression paths.
- `mercurio-core/src/frontend/transpile.rs` emits KIR through mapping data.
- `resources/language-profiles/<profile-id>/mappings/` separates construct and KIR emission policy from parser code.
- `docs/development/PROJECT_DESCRIPTOR_AND_MOUNT_PLAN.md` already treats KPARs, libraries, baseline libraries, and source providers as separate concerns.

The main gap is that the frontend is still organized as a SysML parser that happens to know some KerML-shaped concepts. Robust `.kerml` support requires making the KerML layer explicit and shared.

## Design Principles

1. Keep KIR canonical.

   `.kerml` and `.sysml` should both compile into the same KIR graph shape. Runtime queries should not need to know which source syntax produced an element.

2. Make KerML the lower language layer.

   SysML parsing and lowering should reuse KerML package, namespace, classifier, feature, relationship, import, alias, expression, and metadata behavior wherever possible.

3. Keep syntax and semantics separate.

   The frontend may parse and normalize syntax, but semantic closure, inherited feature lookup, relationship traversal, and evaluation must remain runtime or resolver responsibilities.

4. Treat mappings as data.

   Pilot-derived construct-to-metaclass data and Mercurio-owned metaclass-to-KIR emission data should remain outside hardcoded parser logic as much as practical.

5. Prefer staged compatibility over a rewrite.

   The current SysML parser has useful coverage. The plan should extract shared infrastructure incrementally, keeping existing `.sysml` behavior green.

## What Changes Fundamentally

Robust KerML support changes Mercurio from a SysML-first text compiler into a layered KerML/SysML compiler:

- The frontend boundary changes from `parse_sysml` as the central entrypoint to a source-language-aware compiler that can parse `.kerml` or `.sysml`.
- The AST becomes explicitly language-neutral where the languages overlap.
- Resolver behavior becomes KerML-first: namespace ownership, membership, imports, aliases, specialization, subsetting, redefinition, typing, and feature paths are handled as core model mechanics.
- SysML constructs lower through SysML mappings on top of that core instead of embedding KerML-like behavior in SysML-specific branches.
- Library handling must support Kernel libraries, SysML libraries, user KerML libraries, and mixed projects.

## Scope

### In Scope

- Parse `.kerml` files as project sources and library sources.
- Support KerML packages, imports, aliases, classifiers, features, memberships, specializations, subsettings, redefinitions, references, annotations, and basic expressions.
- Resolve names across local files, imports, baseline libraries, and mounted libraries.
- Emit KIR compatible with the existing runtime.
- Allow mixed `.kerml` and `.sysml` source sets.
- Add corpus and fixture tests against pilot examples and committed KPAR/package-set sources.

### Out Of Scope For The First Robust Slice

- Full language conformance.
- Round-trip formatting.
- Complete expression evaluation.
- Full constraint solving.
- Editing mounted read-only library sources.
- Replacing the existing KIR runtime.

## Target Architecture

### Frontend Modules

Move toward this structure:

```text
mercurio-core/src/frontend/
  ast.rs
  diagnostics.rs
  lexer.rs
  language.rs
  kerml.rs
  sysml.rs
  resolver.rs
  transpile.rs
```

`language.rs` should define:

- `SourceLanguage` with `Kerml` and `Sysml`.
- `ParsedModule`, replacing the SysML-specific public module type over time.
- shared compile entrypoints such as `compile_source_text`.
- extension detection for `.kerml` and `.sysml`.

`kerml.rs` should own KerML grammar rules. `sysml.rs` should keep SysML-specific syntax and delegate shared grammar to common helpers where possible.

### AST

Keep the existing `Declaration` shape but generalize names that are currently SysML-specific:

- Rename or wrap `SysmlModule` as `ParsedModule`.
- Add KerML-specific declarations only where generic definitions/usages are insufficient.
- Preserve spans, docs, modifiers, raw names, and source language.
- Avoid introducing runtime semantic structs into the AST.

The AST should be able to represent:

- packages and nested packages
- namespace imports
- membership/import visibility
- aliases
- classifiers
- features
- relationships
- specialization, subsetting, redefinition
- annotations and metadata
- expression syntax that is already supported by the current expression AST

### Resolver

The resolver should become language-neutral. It should operate over `ParsedModule` and mapping data, not `SysmlModule`.

Required capabilities:

- package and namespace indexing
- local definition and feature indexing
- alias expansion
- wildcard and namespace import handling
- stdlib and baseline library lookup
- cross-file context resolution
- mixed `.kerml` / `.sysml` source resolution
- diagnostics with source spans

Short-term, keep the current resolver structure and rename only at module boundaries. Long-term, split large responsibilities into collection, import resolution, type resolution, feature resolution, and expression path resolution.

### Transpiler

The transpiler should continue emitting KIR through mappings.

Add or extend mapping data for:

- KerML constructs
- KerML metaclasses
- default KIR kind mapping
- relationship property emission
- owner and member edge emission
- feature typing and specialization properties
- metadata/provenance

SysML mappings should become overlays that build on KerML defaults where possible.

### Library And Project Loading

Project loading should accept both `.kerml` and `.sysml` files:

- editable project sources: `.kerml`, `.sysml`
- read-only package/KPAR sources: `.kerml`, `.sysml`
- baseline libraries: precompiled KIR, package-set sources, or generated artifacts

The project descriptor and provider pipeline should treat `.kerml` as a normal source extension, not a special stdlib-only path.

## Delivery Phases

### Phase 1: Inventory And Compatibility Baseline

Deliverables:

- Add a KerML support matrix covering syntax forms, semantic behavior, and test fixtures.
- Identify which existing `sysml.rs` grammar helpers are already KerML-compatible.
- Add `.kerml` fixture files for minimal packages, imports, classifiers, features, and relationships.
- Add tests proving current `.sysml` behavior remains unchanged.

Exit criteria:

- A documented coverage table exists.
- Minimal `.kerml` examples are committed as fixtures.
- Existing parser, resolver, transpiler, and runtime tests pass.

### Phase 2: Source Language Entry Point

Deliverables:

- Add `SourceLanguage`.
- Add file-extension-based dispatch for `.kerml` and `.sysml`.
- Add public `parse_source_text` / `compile_source_text` entrypoints.
- Keep `parse_sysml` and `compile_sysml_text` as compatibility wrappers.
- Update project/source-set loading to include `.kerml`.

Exit criteria:

- `.kerml` files are recognized by loader and project APIs.
- Existing `.sysml` callers still compile without behavioral changes.
- A minimal `.kerml` file can reach parser dispatch, even if coverage is narrow.

### Phase 3: Minimal KerML Parser

Deliverables:

- Add `frontend/kerml.rs`.
- Parse minimal KerML:
  - `package`
  - `import`
  - `alias`
  - classifier declarations
  - feature declarations
  - specialization, subsetting, and redefinition syntax
  - nested memberships
- Reuse lexer, spans, diagnostics, expression AST, and shared AST nodes.

Exit criteria:

- Minimal KerML fixtures parse into AST.
- Parser diagnostics include useful span data.
- No SysML parser regression.

### Phase 4: KerML Resolution

Deliverables:

- Generalize resolver entrypoints from `SysmlModule` to `ParsedModule`.
- Resolve KerML packages, classifiers, features, imports, aliases, and relationships.
- Support mixed context modules where `.sysml` imports `.kerml` and `.kerml` imports `.kerml`.
- Improve ambiguity diagnostics for duplicate short names and wildcard imports.

Exit criteria:

- Cross-file `.kerml` resolution works.
- Mixed `.sysml` / `.kerml` resolution works for a narrow fixture.
- Invalid references produce deterministic diagnostics.

### Phase 5: KerML KIR Emission

Deliverables:

- Add KerML construct mappings under the active language profile's `mappings/` directory.
- Emit KIR for KerML packages, classifiers, features, relationships, ownership, memberships, specialization, subsetting, redefinition, and metadata.
- Ensure emitted ids are stable and package-qualified.
- Preserve source provenance in KIR metadata.

Exit criteria:

- Minimal `.kerml` fixtures compile to KIR.
- KIR loads in `Graph` and `Runtime`.
- Snapshot tests cover expected element ids, kinds, owners, and reference properties.

### Phase 6: Library Integration

Deliverables:

- Include `.kerml` in project descriptor source discovery.
- Include `.kerml` in KPAR/package-set extraction and preprocessing.
- Cache compiled KerML library artifacts with provenance.
- Validate Kernel library package sets from local fixtures or pilot-derived exports.

Exit criteria:

- A project can depend on a read-only KerML library source.
- A project can compile with baseline KIR plus local `.kerml` and `.sysml` sources.
- Workspace status distinguishes editable KerML project files from read-only library files.

### Phase 7: Corpus And Conformance Growth

Deliverables:

- Add pilot-comparison tooling for KerML similar to existing SysML pilot comparison tools.
- Track pass/fail coverage by grammar family.
- Expand coverage in priority order:
  - namespaces/imports
  - classifiers/features
  - relationships
  - expressions
  - annotations/metadata
  - library packages
- Add negative tests for invalid names, ambiguous imports, invalid redefinitions, and unresolved features.

Exit criteria:

- KerML corpus coverage is measurable.
- New syntax support is added through failing fixtures first.
- Comparison tooling reports deltas against pilot output or known expected KIR.

## Testing Strategy

Use layered tests:

- lexer tests for KerML tokens and punctuation
- parser tests for AST shape and diagnostics
- resolver tests for names, imports, aliases, and cross-file context
- transpiler snapshot tests for KIR
- graph/runtime tests for loaded KerML-derived models
- project/provider tests for `.kerml` discovery and library preprocessing
- corpus audit tests for pilot/library examples

Every phase should preserve the current `.sysml` test suite. KerML work should expand the shared foundation, not trade away SysML behavior.

## Risks

- Parser drift: duplicating SysML and KerML grammar logic could create inconsistent behavior.
- Mapping drift: hardcoding KerML semantics in Rust would undermine the mapping-based design.
- Resolver growth: the current resolver is large and may become difficult to evolve without staged internal splits.
- Library ambiguity: Kernel, SysML, user libraries, and package sources need deterministic precedence.
- False conformance: accepting syntax without correct KIR/reference behavior can be worse than rejecting it.

## Recommended First Milestone

Implement a narrow end-to-end KerML slice:

```kerml
package Demo {
  classifier Vehicle;
  feature engine : Engine;
  classifier Engine;
}
```

The milestone is complete when Mercurio can:

1. parse the file as `.kerml`;
2. resolve `Engine`;
3. emit KIR with stable package-qualified ids;
4. merge that KIR with the baseline library document;
5. load it into the existing graph/runtime;
6. run a snapshot test proving the emitted owner/type/reference properties.

This first slice should be deliberately small. Its purpose is to prove the shared frontend boundary, not to claim broad KerML conformance.

## Recommended Implementation Order

1. Add the plan and coverage matrix.
2. Add source language dispatch and `.kerml` file discovery.
3. Add minimal `kerml.rs` parser using existing AST nodes.
4. Generalize resolver entrypoints while keeping compatibility wrappers.
5. Add KerML mapping data and KIR snapshot tests.
6. Integrate `.kerml` into project/library providers.
7. Expand coverage from corpus failures.

## Definition Of Done

Robust `.kerml` support is credible when:

- `.kerml` is accepted anywhere Mercurio accepts project model sources.
- mixed `.kerml` and `.sysml` projects compile through one source-set path.
- KerML packages, imports, classifiers, features, relationships, and core relationship modifiers emit stable KIR.
- resolver diagnostics are source-span aware and deterministic.
- library/KPAR/package-set preprocessing handles `.kerml`.
- runtime behavior remains source-format agnostic.
- coverage is tracked against committed fixtures and pilot-derived examples.
