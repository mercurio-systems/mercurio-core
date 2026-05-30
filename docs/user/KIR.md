# KIR User Guide

## What KIR Is

KIR is Mercurio's compiled semantic JSON format. SysML and KerML source files compile into KIR, and Mercurio's runtime uses KIR to build model graphs, answer queries, evaluate expressions, package models, and run future verification or simulation workflows.

You usually author `.sysml` or `.kerml` files. KIR is the compiled form you may inspect, save, package, or pass to lower-level commands.

```text
.sysml / .kerml
        |
        v
      KIR
        |
        v
 graph + runtime queries
```

## When You Will See KIR

You may encounter KIR when you:

- compile source with JSON output
- evaluate runtime expressions from a precompiled model
- query model elements directly
- package a model as a KPAR artifact
- debug resolved model relationships
- compare semantic output across commits or tools

Most normal editing workflows should use source files. Use KIR when you need the compiled semantic model.

## Basic Shape

A KIR document is a JSON object with optional `metadata` and an `elements` array.

```json
{
  "metadata": {
    "source": "model.sysml"
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

Each element has:

- `id`: stable semantic identity used for references
- `kind`: semantic type of the element
- `layer`: model layer, where user-authored model elements are usually `2`
- `properties`: semantic data such as owner, type, features, specialization, documentation, or expressions

## Common IDs

KIR ids are stable strings. Common prefixes include:

- `pkg.` for packages
- `type.` for definitions, classifiers, and types
- `feature.` for usages and features
- `part.` for part-like elements in older examples

Example:

```text
pkg.Demo
type.Demo.Vehicle
feature.Demo.Vehicle.engine
```

CLI commands often accept user-facing qualified names such as `Demo.Vehicle`. Low-level workflows may also accept full KIR ids such as `type.Demo.Vehicle`.

## Compile Source To KIR

Compile a SysML file and print JSON:

```powershell
mercurio compile --file model.sysml --format json
```

Compile with the bundled standard library:

```powershell
mercurio compile --file model.sysml --stdlib resources/stdlib-sources/sysml-2.0-pilot-0.57.0/stdlib.full.kir.json --format json
```

Compile a KPAR package:

```powershell
mercurio compile --kpar model.kpar --format json
```

## Evaluate From KIR

Evaluate a derived feature from a precompiled KIR document:

```powershell
mercurio evaluate --kir model.kir.json --feature Demo.Vehicle.totalMass --owner Demo.Vehicle
```

Provide runtime values:

```powershell
mercurio evaluate --kir model.kir.json --feature totalMass --owner Demo.Vehicle --value assembly.Vehicle.mass=42
```

Add explanations:

```powershell
mercurio evaluate --kir model.kir.json --feature totalMass --owner Demo.Vehicle --explain
```

## Query KIR-backed Models

Query all part definitions:

```powershell
mercurio query --kir model.kir.json --query 'from elements where kind = "SysML::Systems::PartDefinition" select id, qualified_name'
```

Query type-feature relationships:

```powershell
mercurio query --kir model.kir.json --query 'match ?type features ?feature select ?type.qualified_name, ?feature.qualified_name'
```

The same query language can also compile from source directly:

```powershell
mercurio query --file model.sysml --query 'from elements select id, kind'
```

## Expressions In KIR

Newer compiled expressions use `expression_ir`, a structured JSON representation. Mercurio evaluates `expression_ir` from KIR rather than reparsing source text.

Example shape:

```json
{
  "kind": "call",
  "function": "sum",
  "args": [
    {
      "kind": "path",
      "root": "self",
      "segments": ["parts", "mass"]
    }
  ]
}
```

This lets the runtime evaluate derived values, constraints, guards, and future simulation assertions from compiled semantics.

## Source Provenance

KIR elements may include source metadata:

```json
{
  "metadata": {
    "source_file": "model.sysml",
    "source_span": {
      "start_line": 1,
      "start_col": 1,
      "end_line": 5,
      "end_col": 1
    }
  }
}
```

Mercurio uses this metadata to connect runtime elements, diagnostics, and future verification evidence back to authored source.

## Validation

Mercurio validates KIR before building the runtime graph. Common structural failures include:

- duplicate element ids
- empty ids
- ids with leading or trailing whitespace
- empty `kind` values
- unsupported layer values

Semantic errors such as unresolved required references are usually reported during source compilation before KIR is emitted.

## User Guidance

Prefer source files for normal modeling. Use KIR when you need to:

- inspect compiled semantics
- run low-level runtime commands
- exchange a compiled model artifact
- debug references and generated ids
- compare semantic output across versions

For implementation-level details, see the developer-facing [KIR Spec](../development/KIR_SPEC.md).
