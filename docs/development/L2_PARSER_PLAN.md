# L2 Text Parser Plan

Status: historical baseline. Prefer [Frontend To KIR Theory Of Operation](FRONTEND_TO_KIR_THEORY_OF_OPERATION.md) for the current compiler path.

## Goal

Load user `.sysml` files as L2 models without depending on a Java subprocess at runtime.

The runtime path remains:

```text
resources/stdlib-sources/sysml-2.0-pilot-0.57.0/stdlib.full.kir.json + user_model.sysml
                    ->
          Rust parser + transpiler
                    ->
              canonical KIR
                    ->
          existing runtime / UI / API
```

L0 and L1 continue to come from the committed stdlib artifact generated from the pilot implementation.

## Key Decision

Keep the parser grammar in Rust code, but keep the SysML/KerML construct-to-KIR mapping in files.

That means:

- Rust owns tokenization, parsing, diagnostics, and name resolution.
- Pilot-derived files describe textual construct -> SysML/KerML metaclass.
- Repo-owned files describe metaclass -> KIR normalization.

This avoids baking most language mapping into Rust logic.

## What We Can Pull From The Pilot

The pilot implementation already exposes two recoverable layers:

1. Xtext grammar rules map textual constructs to metaclasses.

Examples from the pilot:

- `Package returns SysML::Package`
- `Import returns SysML::Import`
- `ItemDefinition returns SysML::ItemDefinition`
- `ItemUsage returns SysML::ItemUsage`
- `PartDefinition returns SysML::PartDefinition`
- `PartUsage returns SysML::PartUsage`

Those come from:

- `../external/SysML-v2-Pilot-Implementation/org.omg.sysml.xtext/src/org/omg/sysml/xtext/SysML.xtext`
- `../external/SysML-v2-Pilot-Implementation/org.omg.kerml.xtext/src/org/omg/kerml/xtext/KerML.xtext`

2. The Ecore metamodel defines supertypes, structural features, and documentation.

Examples from the pilot:

- `PartDefinition` extends `ItemDefinition`
- `PartUsage` extends `ItemUsage`
- `ItemDefinition` extends `OccurrenceDefinition` and `Structure`
- `Type` has derived `ownedFeature`
- `Feature` has derived `type`

Those come from:

- `../external/SysML-v2-Pilot-Implementation/org.omg.sysml/model/SysML.ecore`
- `../external/SysML-v2-Pilot-Implementation/org.omg.sysml/model/kerml.ecore`

## What We Cannot Pull Directly From The Pilot

The pilot does not know our KIR shape. It does not define:

- canonical KIR ids
- flattened KIR property names
- which derived EMF features should become direct KIR edges
- which pilot details should become metadata only

So the final metaclass -> KIR mapping remains a Mercurio-owned file.

## Mapping Layers

Use two mapping layers under the active language profile's `mappings/` directory:

1. `pilot_constructs.seed.json`

Purpose:

- textual construct name
- pilot metaclass
- pilot grammar source

This is pilot-derived and should be generated or refreshed from the pilot grammar.

2. `kir_emission.seed.json`

Purpose:

- metaclass
- KIR kind
- KIR id template
- direct property extraction rules
- metadata capture rules

This is repo-owned and defines how parsed AST nodes become canonical KIR.

## Recommended File Layout

```text
src/
  frontend/
    mod.rs
    lexer.rs
    ast.rs
    sysml.rs
    resolver.rs
    transpile.rs
    diagnostics.rs
resources/language-profiles/<profile-id>/
  mappings/
    README.md
    pilot_constructs.seed.json
    kir_emission.seed.json
docs/
  development/
    L2_PARSER_PLAN.md
test_files/
  l2/
    minimal_vehicle.sysml
    minimal_vehicle.kir.json
src/bin/
  parse_l2.rs
```

## Parser Scope For V1

Only support a narrow structural subset:

- `package`
- `import`
- `part def`
- owned `part` members
- simple typing like `part engine: Engine;`
- direct specialization
- doc comments when cheap

Defer:

- constraints
- expression lowering
- actions / behaviors
- full grammar coverage
- round-trip formatting

## AST Strategy

Use a small Rust AST that models syntax, not semantics.

Suggested first nodes:

- `PackageDecl`
- `ImportDecl`
- `PartDefinitionDecl`
- `PartUsageDecl`
- `TypeRef`
- `DocBlock`

The parser should preserve:

- source file
- line/column span
- doc comments
- raw names

It should not evaluate inheritance or implied relationships.

## Name Resolution Strategy

Keep name resolution intentionally narrow in v1:

- local package scope
- explicit imports
- stdlib lookup against loaded L0/L1 KIR ids

Unresolved names should produce diagnostics and block KIR emission for the affected element.

## KIR Emission Strategy

The current KIR contract is documented in [KIR_SPEC.md](KIR_SPEC.md). This section describes how the L2 parser feeds that contract.

The transpiler should be generic over mapping files:

1. Parse `.sysml` into AST.
2. Convert AST node kind -> pilot metaclass using `pilot_constructs.seed.json`.
3. Convert metaclass -> KIR output using `kir_emission.seed.json`.
4. Emit KIR with source metadata and docs.

Example target outputs:

- `part def Vehicle`
  - KIR id like `type.Vehicle`
  - KIR kind `SysML::Systems::PartDefinition`

- `part engine: Engine;`
  - KIR id like `feature.Vehicle.engine`
  - KIR kind `KerML::Core::Feature`
  - properties:
    - `owner`
    - `type`

## Why Use Owner-Qualified Feature IDs

Current demo ids like `feature.engine` are simple, but they do not scale across files.

For parsed L2, prefer:

- `feature.Vehicle.engine`
- `feature.Vehicle.powertrain`

or a fully qualified package-aware variant later.

That avoids collisions and gives deterministic ids.

## Delivery Phases

### Phase 1

- Add mapping files under the active language profile's `mappings/` directory
- Add parser design docs
- Add AST and lexer skeletons
- Add `parse_l2` binary

### Phase 2

- Parse `package`, `import`, `part def`, `part`
- Resolve local names and stdlib type refs
- Emit KIR for minimal fixtures

### Phase 3

- Integrate `.sysml` loading into the existing server/runtime path
- Load parsed L2 through the public CLI, reusable library APIs, or a product-hosted backend.
- Show parsed L2 in the graph UI

### Phase 4

- Expand grammar coverage
- Add mapping extraction tooling from the pilot grammar
- Replace seed mapping files with generated artifacts

## Concrete First Milestone

The first milestone should prove this end-to-end:

1. Parse:

```sysml
package Demo {
  part def Vehicle {
    part engine: Engine;
  }
}
```

2. Emit L2 KIR in memory.
3. Merge with `resources/stdlib-sources/sysml-2.0-pilot-0.57.0/stdlib.full.kir.json`.
4. Load in the existing runtime.
5. Display the resulting L2 model in the current UI.

## Test Plan

- lexer tests for identifiers, keywords, punctuation
- parser tests for `package`, `part def`, and typed `part`
- transpiler snapshot tests against `test_files/l2/*.kir.json`
- resolution tests for imported stdlib names
- runtime integration tests loading `.sysml`

## Recommendation

Implement the parser as a small Rust-native frontend, but treat the construct-to-KIR policy as data loaded from files. Use the pilot implementation only to derive the construct -> metaclass seed data, not as a runtime parser dependency.
