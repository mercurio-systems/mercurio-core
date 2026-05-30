# KIR Specification

Status: canonical implementation contract.

## Purpose

KIR is the KerML Intermediate Representation used by Mercurio as the canonical semantic JSON form.

Source frontends such as `.sysml` and `.kerml` compile into KIR. The runtime consumes KIR, builds a semantic graph, resolves relationships, evaluates supported expressions, and serves model queries. Source syntax should not be required after KIR has been produced.

The key rule is:

> KIR is the semantic contract between frontends and the runtime.

## Status

This document describes the current implementation contract. Some conventions are intentionally broader than a formal schema because the current Rust representation stores most element data as JSON properties.

Implementation anchor:

- `mercurio-core/src/ir.rs`: KIR document and element structs
- `mercurio-core/src/graph.rs`: graph construction and reference discovery
- `mercurio-core/src/frontend/transpile.rs`: source AST to KIR emission
- `resources/language-profiles/<profile-id>/mappings/kir_emission.seed.json`: metaclass-to-KIR emission rules for the L2 subset

## Document Shape

A KIR document is a JSON object with:

- `metadata`: optional object for document-level provenance and processing metadata
- `elements`: required array of KIR elements

```json
{
  "metadata": {
    "source": "test_files/l2/minimal_vehicle.sysml"
  },
  "elements": [
    {
      "id": "type.Demo.Vehicle",
      "kind": "SysML::Systems::PartDefinition",
      "layer": 2,
      "properties": {
        "declared_name": "Vehicle"
      }
    }
  ]
}
```

When multiple KIR documents are merged, element ids must be unique. The merge path sorts elements by `id` and preserves source document metadata under `metadata.merged_sources`.

KIR documents are validated when loaded and after merge. Invalid documents fail before graph/runtime construction.

## Element Shape

Each KIR element has:

- `id`: required stable external identity
- `kind`: required semantic kind or model reference
- `layer`: optional numeric model layer, defaulting to `0`
- `properties`: optional object carrying graph-relevant semantic data, defaulting to `{}`

```json
{
  "id": "feature.Demo.Vehicle.engine",
  "kind": "SysML::PartUsage",
  "layer": 2,
  "properties": {
    "declared_name": "engine",
    "owner": "type.Demo.Vehicle",
    "type": "type.Demo.Engine"
  }
}
```

The runtime adds `element_id` to each graph element's properties from the canonical `id`. If an input `properties.element_id` is present and disagrees with `id`, the graph overwrites it with the canonical `id`.

## Validation Rules

Current validation rejects:

- empty element ids
- ids with leading or trailing whitespace
- empty semantic kinds
- unsupported layers outside `0`, `1`, or `2`
- duplicate ids inside a loaded or merged document

Validation is intentionally structural. Semantic diagnostics such as unresolved required references belong to frontend compilation or semantic services.

## Layers

Layer values identify where an element belongs in the model stack:

- `0`: KerML kernel or foundational metamodel concepts
- `1`: SysML library or reusable baseline concepts
- `2`: user-authored model elements

The same KIR element structure is used for all layers. Frontends should normally emit user-authored elements as layer `2`; library artifacts should preserve their source layer.

## Id Conventions

KIR ids are stable strings used for references. They are not Rust object ids and should not depend on parse order.

Current common prefixes include:

- `pkg.` for packages
- `type.` for definitions/classifiers/types
- `feature.` for features/usages
- `part.` for part instances or part-like elements in examples
- `df.` for derived features in older examples

For source-derived L2 elements, prefer package-qualified ids:

```text
pkg.Demo
type.Demo.Vehicle
feature.Demo.Vehicle.engine
```

Ids should be deterministic across recompiles for unchanged source. If a source declaration has an explicit stable identifier in the future, the frontend may use it, but the resulting id still must be unique in the merged KIR document.

## Kind Conventions

`kind` names the semantic classification of the element. Current emitted values include SysML and KerML-style model references such as:

```text
SysML::Package
SysML::Systems::PartDefinition
SysML::PartUsage
KerML::Core::Feature
KerML::Root::Dependency
```

Older hand-authored examples may use shorter names such as `sysml.PartDefinition` or may set `kind` to another element id such as `type.Vehicle`. New frontend output should prefer the canonical names emitted by the mapping files and standard library artifacts.

## Property Conventions

`properties` is the main semantic payload. Values may be JSON strings, numbers, booleans, arrays, objects, or nulls.

Common properties:

- `declared_name`: source-level declared name
- `qualified_name`: source-level qualified name when available
- `owner`: owning element id
- `owning_type`: type or classifier that owns a feature
- `type`: referenced type element id
- `features`: ordered feature element ids owned or exposed by a type
- `specializes`: direct specialization target ids
- `subsets`: direct subsetting target ids
- `redefines`: direct redefinition target ids
- `imports`: imported package or namespace ids
- `documentation`: source documentation text
- `metadata`: element-level provenance object
- `expression`: legacy string expression
- `expression_ir`: structured expression representation

Property names are intentionally semantic, not parser-specific. Frontends should avoid storing raw AST node shapes in KIR properties.

## Reference And Edge Rules

The graph builder discovers references from element properties through the KIR field contract.

For each registered reference field:

- if the value is a string equal to a known element id, it creates an edge
- if the value is an array, each string item equal to a known element id creates an edge
- objects are not recursively scanned for references
- strings that do not match known ids remain scalar data and do not create edges
- unregistered fields never create graph edges

The edge relation is the property name.

Example:

```json
{
  "id": "type.Demo.Vehicle",
  "kind": "SysML::Systems::PartDefinition",
  "layer": 2,
  "properties": {
    "specializes": ["SysML::Systems::PartDefinition"],
    "features": ["feature.Demo.Vehicle.engine"]
  }
}
```

This can create outgoing `specializes` and `features` edges if those target ids are present in the merged document.

Because unresolved string references do not become edges, source frontends should diagnose unresolved semantic references before or during KIR emission when that reference is required for correctness.

Non-reference fields do not create edges even if their text happens to equal an element id. For example, `documentation: "type.Demo.Vehicle"` remains documentation text and is not treated as a semantic relationship.

## Metadata

Document-level `metadata` describes the KIR document as a whole. Element-level `properties.metadata` describes a specific element.

Current source provenance commonly uses:

```json
{
  "metadata": {
    "source_file": "test_files/l2/minimal_vehicle.sysml",
    "source_span": {
      "start_line": 1,
      "start_col": 1,
      "end_line": 5,
      "end_col": 1
    }
  }
}
```

Line and column values are 1-based. Frontends should preserve source provenance for user-authored elements so editor diagnostics, outline navigation, and semantic inspection can link runtime elements back to source.

## Expressions

There are two expression representations:

- `expression`: legacy string expressions, currently used by older examples
- `expression_ir`: structured expression data emitted by newer SysML expression support

The runtime historically supports a small string subset such as:

```text
count(self.parts)
sum(self.parts.mass)
```

New expression work should prefer `expression_ir` so expressions are data, not parser-specific strings. Expression IR is still evolving and should be documented in this file once its shape is stable enough to treat as a contract.

### `expression_ir` Contract

`expression_ir` is a JSON object with a required string `kind` field. Consumers must reject unknown kinds or malformed required fields with diagnostics rather than guessing at source syntax. Producers may preserve unsupported source expressions as legacy `expression` strings, but must only emit `expression_ir` for the supported forms below.

Supported expression kinds:

- `literal`: JSON scalar literal.

  ```json
  { "kind": "literal", "value": 42 }
  ```

- `self`: the runtime owner context.

  ```json
  { "kind": "self" }
  ```

- `path`: a feature path rooted at `self`. New frontends should emit each segment as an object with a required source `name` and optional resolved semantic `feature` id. Runtime consumers may also read legacy string segments such as `"parts"` for older KIR artifacts. When a segment cannot be resolved, frontends should report a diagnostic before KIR emission.

  ```json
  {
    "kind": "path",
    "root": "self",
    "segments": [
      { "name": "parts", "feature": "feature.Demo.vehicle.parts" },
      { "name": "mass", "feature": "feature.Demo.Engine.mass" }
    ]
  }
  ```

- `tuple`: ordered expression items. Tuple runtime semantics are limited and consumers may reject tuple values where a scalar is required.

  ```json
  {
    "kind": "tuple",
    "items": [
      { "kind": "literal", "value": 1 },
      { "kind": "literal", "value": 2 }
    ]
  }
  ```

- `unary`: unary operator plus operand in `expr`. Supported operators are `negate` and `not`.

  ```json
  {
    "kind": "unary",
    "op": "not",
    "expr": { "kind": "literal", "value": false }
  }
  ```

- `binary`: binary operator plus `left` and `right` operands. Supported operators are `add`, `subtract`, `multiply`, `divide`, `power`, `equal`, `not_equal`, `less`, `less_equal`, `greater`, `greater_equal`, `and`, and `or`.

  ```json
  {
    "kind": "binary",
    "op": "greater",
    "left": {
      "kind": "binary",
      "op": "multiply",
      "left": { "kind": "literal", "value": 2 },
      "right": { "kind": "literal", "value": 3 }
    },
    "right": { "kind": "literal", "value": 5 }
  }
  ```

- `call`: pure function call with ordered `args`. The runtime currently supports one-argument `count`, `sum`, `min`, `max`, and `avg`. Other functions may be carried for inspection, but runtime consumers should reject unsupported functions explicitly.

  ```json
  {
    "kind": "call",
    "function": "sum",
    "args": [
      {
        "kind": "path",
        "root": "self",
        "segments": [
          { "name": "parts", "feature": "feature.Demo.vehicle.parts" },
          { "name": "mass", "feature": "feature.Demo.Engine.mass" }
        ]
      }
    ]
  }
  ```

Runtime evaluation of `expression_ir` must be pure. It may read the graph and execution context, but it must not mutate model state.

## Frontend Responsibilities

Frontends that emit KIR should:

- parse source syntax
- normalize identifiers and references
- emit deterministic ids
- emit semantic `kind` values from mapping policy
- preserve source provenance in metadata
- emit direct semantic relationships such as ownership, specialization, typing, subsetting, and redefinition
- report diagnostics for unresolved required references

Frontends should not:

- compute specialization closure
- evaluate expressions
- encode source AST trivia as semantic data
- make runtime-only semantic decisions
- emit graph edges indirectly through parser-specific property shapes

## Runtime Responsibilities

The runtime and graph layer should:

- load KIR JSON
- reject duplicate element ids during merge or graph construction
- build reference edges from recognized property values
- answer semantic queries from the graph
- evaluate supported expression data
- preserve KIR properties for inspection

The runtime should not depend on whether an element came from `.sysml`, `.kerml`, a precompiled library artifact, or hand-authored KIR.

## Minimal Example

```json
{
  "elements": [
    {
      "id": "type.Demo.Engine",
      "kind": "SysML::Systems::PartDefinition",
      "layer": 2,
      "properties": {
        "declared_name": "Engine",
        "specializes": ["SysML::Systems::PartDefinition"]
      }
    },
    {
      "id": "feature.Demo.Vehicle.engine",
      "kind": "SysML::PartUsage",
      "layer": 2,
      "properties": {
        "declared_name": "engine",
        "owner": "type.Demo.Vehicle",
        "type": "type.Demo.Engine"
      }
    },
    {
      "id": "type.Demo.Vehicle",
      "kind": "SysML::Systems::PartDefinition",
      "layer": 2,
      "properties": {
        "declared_name": "Vehicle",
        "features": ["feature.Demo.Vehicle.engine"],
        "specializes": ["SysML::Systems::PartDefinition"]
      }
    }
  ]
}
```

## Compatibility Rules

KIR should remain stable enough that:

- precompiled library artifacts can be cached and reused
- `.sysml` and `.kerml` frontends can produce equivalent semantic graph shapes
- UI features can inspect elements without knowing the source language
- project repositories and server workflows can validate and compare semantic changes across revisions

Breaking changes to KIR should be handled deliberately. A future stricter schema should add explicit version metadata, define field contracts for semantic properties, and invalidate generated artifacts and caches when the schema changes. During the current tightening phase, old KIR artifacts should be regenerated from source rather than migrated.

## Open Tightening Work

The current implementation allows flexible JSON properties. The next specification work should define:

- a formal JSON Schema for `KirDocument`
- a field contract that distinguishes scalar fields, reference fields, expression data, metadata, and extension data
- canonical id templates for all supported construct classes
- complete `expression_ir` schema
- recursive or typed reference fields, if needed
- document-level schema/version metadata
- validation rules for precompiled KIR artifacts
- metamodel-derived capability validation instead of hand-maintained kind-profile tables

The staged schema plan is tracked in [KIR Schema Roadmap](KIR_SCHEMA_ROADMAP.md).
