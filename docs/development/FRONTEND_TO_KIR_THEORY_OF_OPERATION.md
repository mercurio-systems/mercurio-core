# Frontend To KIR Theory Of Operation

Status: current architecture and implementation guide.

## Purpose

This document explains how Mercurio turns textual SysML/KerML source into KIR, and how the SysML/KerML metamodel participates in that flow.

The short version is:

```text
source text
  -> tokens
  -> syntax AST
  -> resolved semantic model
  -> mapping-driven KIR elements
  -> validated KIR document
```

KIR is the semantic handoff point. Once KIR exists, downstream graph, runtime, view, and API code should not need the original source syntax.

## Main Code Path

For SysML source, the normal compile path is:

```text
compile_sysml_text
  -> parse_sysml
       -> lex
       -> Parser::parse
  -> MappingBundle::load
       -> resources/language-profiles/<profile-id>/mappings/pilot_constructs.seed.json
       -> resources/language-profiles/<profile-id>/mappings/kir_emission.seed.json
  -> resolve_module
  -> transpile_module
  -> KirDocument
```

Implementation anchors:

- `crates/mercurio-core/src/frontend/lexer.rs`
- `crates/mercurio-core/src/frontend/sysml.rs`
- `crates/mercurio-core/src/frontend/kerml.rs`
- `crates/mercurio-core/src/frontend/ast.rs`
- `crates/mercurio-core/src/frontend/resolver.rs`
- `crates/mercurio-core/src/frontend/transpile.rs`
- `crates/mercurio-core/src/ir.rs`
- `resources/language-profiles/<profile-id>/mappings/pilot_constructs.seed.json`
- `resources/language-profiles/<profile-id>/mappings/kir_emission.seed.json`

## Step 1: Tokenize Source Text

The lexer reads source text one character at a time and produces a flat token stream. Tokens keep source spans so later diagnostics and KIR metadata can point back to the source file.

Example source:

```sysml
package Demo {
  part def Engine;
  part def Vehicle {
    part engine: Engine;
  }
}
```

Representative tokens:

```text
Package
Identifier("Demo")
LBrace
Part
Def
Identifier("Engine")
Semicolon
Part
Def
Identifier("Vehicle")
LBrace
Part
Identifier("engine")
Colon
Identifier("Engine")
Semicolon
RBrace
RBrace
Eof
```

The lexer does not decide whether `Vehicle` is a classifier, whether `Engine` resolves, or what KIR id will be emitted. It only recognizes keywords, identifiers, punctuation, literals, operators, comments, and source locations.

Important lexer behavior:

- comments and whitespace are skipped except `doc` blocks, which become `Doc(...)` tokens
- quoted identifiers are accepted as identifier tokens
- relation shorthands such as `:>` and `:>>` become semantic punctuation tokens
- line and column tracking is preserved in every `Token.span`

## Step 2: Parse Tokens Into Syntax AST

The parser consumes tokens and builds a syntax-oriented AST from `frontend/ast.rs`.

For the example above, the useful AST shape is:

```text
SysmlModule
  package: PackageDecl "Demo"
    members:
      PartDefinitionDecl "Engine"
      PartDefinitionDecl "Vehicle"
        members:
          PartUsageDecl "engine"
            ty: QualifiedName ["Engine"]
```

The AST still represents source syntax, not final semantics. For example:

- `part def Vehicle` is a `PartDefinitionDecl`
- `part engine: Engine` is a `PartUsageDecl`
- `Engine` is a `QualifiedName`, not yet a resolved KIR id
- docs, modifiers, multiplicity, expression syntax, and source spans remain attached to AST nodes

The parser can operate in strict mode through `parse_sysml`, or in recovering mode through `parse_sysml_recovering`. Recovering mode keeps valid sibling declarations when one declaration has a parse error, which is useful for editor and lint workflows.

## Step 3: Load Metamodel And Emission Policy

Mercurio keeps metamodel policy out of the parser where practical. The parser recognizes source forms, but mapping files decide how those forms correspond to SysML/KerML metaclasses and KIR output.

There are two mapping layers:

```text
textual construct -> SysML/KerML metaclass -> KIR element shape
```

### Construct To Metaclass

`resources/language-profiles/<profile-id>/mappings/pilot_constructs.seed.json` maps parser construct names to SysML/KerML metaclasses. Examples:

```json
{
  "construct": "PartDefinition",
  "metaclass": "SysML::PartDefinition"
}
```

```json
{
  "construct": "PartUsage",
  "metaclass": "SysML::PartUsage"
}
```

The same file also keeps keyword registries. For example, the keyword `part` maps to:

```text
definition keyword: part -> PartDefinition
usage keyword:     part -> PartUsage
```

That means grammar-specific spelling is separated from semantic class identity.

### Metaclass To KIR

`resources/language-profiles/<profile-id>/mappings/kir_emission.seed.json` maps metaclasses to KIR kind, id template, and emitted properties. Example for part definitions:

```json
{
  "kir_kind": "SysML::Systems::PartDefinition",
  "id_template": "type.{qualified_name}",
  "emit": {
    "properties": {
      "declared_name": "{declared_name}",
      "name": "{name}",
      "owner": "{owner_id}",
      "members": "{member_ids}",
      "specializes": "{specializes_refs}",
      "features": "{owned_feature_ids}",
      "metatype": "{metatype_ref}"
    }
  }
}
```

Example for part usages:

```json
{
  "kir_kind": "SysML::PartUsage",
  "id_template": "feature.{owner_path}.{declared_name}",
  "emit": {
    "properties": {
      "owner": "{owner_id}",
      "type": "{type_ref}",
      "declared_name": "{declared_name}",
      "features": "{owned_feature_ids}",
      "specializes": "{specializes_refs}",
      "subsetted_features": "{subsetted_feature_refs}",
      "redefined_features": "{redefined_feature_refs}"
    }
  }
}
```

The metamodel files describe semantic class identity and default semantic anchors. The emission file describes Mercurio's canonical KIR projection of those concepts.

## Step 4: Resolve Names And Semantic References

The resolver converts AST declarations into a resolved semantic model. It collects packages, definitions, usages, imports, and aliases, then resolves references against:

- local definitions in the current module set
- imported namespaces and imported values
- aliases
- the loaded stdlib KIR document
- stdlib aliases from `pilot_constructs.seed.json`

For the example:

```sysml
package Demo {
  part def Engine;
  part def Vehicle {
    part engine: Engine;
  }
}
```

Resolution produces the semantic facts:

```text
Package:
  qualified_name: Demo

Definition:
  construct: PartDefinition
  qualified_name: Demo.Engine
  declared_name: Engine
  specializes: Parts::Part

Definition:
  construct: PartDefinition
  qualified_name: Demo.Vehicle
  declared_name: Vehicle
  specializes: Parts::Part

Usage:
  construct: PartUsage
  owner_qualified_name: Demo.Vehicle
  qualified_name: Demo.Vehicle.engine
  declared_name: engine
  type_ref: type.Demo.Engine
  subsetted_features: Items::Item::subparts
```

The important transition is that `Engine` changes from source text to the semantic id `type.Demo.Engine`. If a required reference cannot be resolved, strict SysML compilation reports a diagnostic and does not emit invalid KIR for that affected reference.

KerML compilation uses a more permissive policy for some unresolved references, because KerML examples often need to preserve kernel-level names that are not represented as local L2 definitions.

## Step 5: Emit KIR Elements

The transpiler renders KIR from the resolved model and the emission mappings.

For each resolved package, definition, import, or usage, it:

1. Finds the construct's metaclass through `MappingBundle::metaclass_for`.
2. Finds the metaclass emission rule through `MappingBundle::emission_for`.
3. Renders the id template.
4. Renders property templates from a context map.
5. Drops null, empty string, and empty array properties.
6. Adds source documentation and source metadata.
7. Produces a `KirElement`.

Representative KIR for the example:

```json
{
  "metadata": {
    "source": "sysml",
    "parsed_from": "examples/demo.sysml"
  },
  "elements": [
    {
      "id": "pkg.Demo",
      "kind": "SysML::Package",
      "layer": 2,
      "properties": {
        "declared_name": "Demo",
        "name": "Demo",
        "members": [
          "type.Demo.Engine",
          "type.Demo.Vehicle"
        ],
        "metatype": "KerML::Kernel::Package"
      }
    },
    {
      "id": "type.Demo.Engine",
      "kind": "SysML::Systems::PartDefinition",
      "layer": 2,
      "properties": {
        "declared_name": "Engine",
        "name": "Engine",
        "specializes": [
          "Parts::Part"
        ]
      }
    },
    {
      "id": "type.Demo.Vehicle",
      "kind": "SysML::Systems::PartDefinition",
      "layer": 2,
      "properties": {
        "declared_name": "Vehicle",
        "name": "Vehicle",
        "features": [
          "feature.Demo.Vehicle.engine"
        ],
        "specializes": [
          "Parts::Part"
        ]
      }
    },
    {
      "id": "feature.Demo.Vehicle.engine",
      "kind": "SysML::PartUsage",
      "layer": 2,
      "properties": {
        "owner": "type.Demo.Vehicle",
        "type": "type.Demo.Engine",
        "declared_name": "engine",
        "name": "engine",
        "subsetted_features": [
          "Items::Item::subparts"
        ]
      }
    }
  ]
}
```

Actual output also includes `properties.metadata.source_file` and `properties.metadata.source_span` on emitted elements. Elements may contain additional defaults such as `metatype`, `is_abstract`, `is_derived`, `is_end`, or semantic specialization fields depending on construct family and source syntax.

## Step 6: Validate And Merge KIR

KIR is represented by:

```rust
pub struct KirDocument {
    pub metadata: BTreeMap<String, Value>,
    pub elements: Vec<KirElement>,
}

pub struct KirElement {
    pub id: String,
    pub kind: String,
    pub layer: u8,
    pub properties: BTreeMap<String, Value>,
}
```

Validation rejects:

- empty ids
- ids with leading or trailing whitespace
- empty kinds
- unsupported layers outside `0`, `1`, and `2`
- duplicate ids

When loading a source file as a model stack, Mercurio compiles the user source to layer-2 KIR and merges it with the stdlib KIR document. The merged document is then available to graph and runtime services.

## Example: Import Resolution

Source:

```sysml
package Demo {
  import ISQ::TorqueValue;

  part def Engine {
    attribute maxTorque: TorqueValue;
  }
}
```

Tokenization sees only syntax:

```text
Import Identifier("ISQ") ScopeSep Identifier("TorqueValue") Semicolon
Attribute Identifier("maxTorque") Colon Identifier("TorqueValue") Semicolon
```

Parsing creates:

```text
ImportDecl path: ISQ::TorqueValue
GenericUsageDecl keyword: attribute, name: maxTorque, ty: TorqueValue
```

Resolution uses stdlib aliases and imports:

```text
ISQ::TorqueValue -> ISQMechanics::TorqueValue
TorqueValue      -> ISQMechanics::TorqueValue
```

KIR emission creates an attribute usage under `type.Demo.Engine`:

```json
{
  "id": "feature.Demo.Engine.maxTorque",
  "kind": "SysML::AttributeUsage",
  "layer": 2,
  "properties": {
    "owner": "type.Demo.Engine",
    "type": "ISQMechanics::TorqueValue",
    "declared_name": "maxTorque",
    "name": "maxTorque",
    "subsetted_features": [
      "Base::dataValues"
    ]
  }
}
```

The important point is that imports do not stay parser trivia. They affect name resolution, and emitted KIR stores resolved references where possible.

## Example: Expression Lowering

Source:

```sysml
package Demo {
  part def Wheel {
    attribute mass: MassValue;
  }

  part def Vehicle {
    part wheel: Wheel;
    attribute totalMass: MassValue = wheel.mass;
  }
}
```

Parsing keeps the expression as AST:

```text
Expr::Path
  root: Expr::Name("wheel")
  segment: "mass"
```

Resolution maps the path to known feature ids where the feature path can be resolved:

```text
wheel.mass -> feature.Demo.Wheel.mass
```

KIR emission prefers structured expression data. Current path lowering stores the resolved terminal feature segment:

```json
{
  "id": "feature.Demo.Vehicle.totalMass",
  "kind": "SysML::AttributeUsage",
  "properties": {
    "owner": "type.Demo.Vehicle",
    "type": "ISQBase::MassValue",
    "expression_ir": {
      "kind": "path",
      "root": "self",
      "segments": [
        {
          "name": "mass",
          "feature": "feature.Demo.Wheel.mass"
        }
      ]
    }
  }
}
```

The runtime should evaluate `expression_ir` from KIR, not reparse the original SysML expression.

## Relationship To The Runtime Graph

KIR properties are semantic data. The graph builder creates edges from registered reference fields in the KIR field contract. It does not infer edges from every string that happens to match a known element id.

Example:

```json
{
  "id": "feature.Demo.Vehicle.engine",
  "properties": {
    "owner": "type.Demo.Vehicle",
    "type": "type.Demo.Engine"
  }
}
```

If the merged KIR document contains `type.Demo.Vehicle` and `type.Demo.Engine`, graph construction can create:

```text
feature.Demo.Vehicle.engine --owner--> type.Demo.Vehicle
feature.Demo.Vehicle.engine --type-->  type.Demo.Engine
```

This is why frontend resolution matters. Unresolved text remains scalar JSON and does not become a graph edge. Non-reference fields such as documentation and names also remain scalar data, even when their text matches an element id.

## Frontend And Metamodel Responsibilities

The frontend owns:

- tokenization and parse diagnostics
- syntax AST construction
- source spans and docs
- local/import/stdlib name resolution
- deterministic KIR id generation
- emitting direct semantic references
- preserving enough source metadata for editor workflows

The mapping files own:

- source construct to SysML/KerML metaclass identity
- keyword-to-construct policy
- default semantic anchors
- metaclass-to-KIR kind mapping
- id templates and property templates

KIR owns:

- stable element identity
- semantic kind
- model layer
- normalized semantic properties
- source provenance as metadata

The runtime owns:

- KIR validation and merge
- graph edge construction from resolved KIR references
- semantic queries
- expression evaluation over KIR data
- derived views and API responses

## Invariants

- KIR ids must be deterministic for unchanged source.
- Frontends should emit semantic property names, not parser-specific AST names.
- Required semantic references should be resolved before KIR emission.
- Source spans should survive into `properties.metadata.source_span`.
- The runtime should consume KIR without caring whether it came from `.sysml`, `.kerml`, a precompiled stdlib artifact, or hand-authored JSON.
