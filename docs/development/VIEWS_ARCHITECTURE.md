# Views Architecture

Status: partially implemented architecture.

## Purpose

Mercurio views are semantic projections over compiled KIR, not independent model artifacts.

The same model graph should support diagrams, tables, matrices, trees, dashboards, and reports:

```text
SysML / KerML source
        |
        v
      KIR
        |
        v
semantic graph + indexes
        |
        v
view query -> projection DTO -> renderer
```

A diagram is one kind of view. A requirements table is another. Both must derive from the same source authority and semantic graph.

## Product Objectives

- Make design truth inspectable through multiple projections without duplicating truth.
- Let users move between source, semantic elements, tables, diagrams, trace matrices, and review surfaces.
- Support model reasoning workflows such as coverage checks, impact analysis, trace review, and verification planning.
- Keep renderers thin and deterministic by giving them normalized projection DTOs.
- Allow future runtime-defined views without requiring every view to be hardcoded in product code.
- Support proposal and draft-change workflows where views can render against `base + overlay`, not only accepted source.

## Core Principle

Views describe intent. They do not own semantics.

```text
View definition: what to select, project, group, sort, and render.
Semantic graph: what the model means.
Renderer: how the projection appears to a user.
```

Presentation state may be saved with a view, but semantic facts must remain in source-derived KIR or an explicit proposal/change overlay.

## View Layers

### Semantic Query

The query selects elements and relationships from a compiled graph.

Examples:

- all authored requirements
- all requirements satisfied by a selected subsystem
- all parts within a composition subtree
- all verification cases linked to a requirement set
- all elements reachable from a seed through dependency relationships

### Projection Model

The projection converts graph data into a renderer-neutral DTO.

Examples:

- requirement table rows
- graph nodes and edges
- trace matrix rows, columns, and cells
- tree nodes
- dashboard metrics

The projection should include source anchors when available so users can navigate back to model text.

### Presentation State

Presentation state controls display without changing semantics.

Examples:

- visible columns
- sorting and grouping
- collapsed rows or tree branches
- diagram layout options
- pinned graph positions, later
- filters and search terms

This state may be saved in project-local artifacts such as `.view` or `.diagram` files.

### Renderer

Renderers consume projection DTOs and should not reimplement model semantics.

Initial renderers:

- table
- graph / diagram
- matrix
- tree
- inspector

Later renderers:

- dashboard
- timeline
- report
- form

## Existing Slice

The first concrete non-diagram view is the requirements table projection:

- implementation: `mercurio-core/src/views.rs`
- example KIR: `test_files/examples/requirements_table_model.json`
- exported API: `requirements_table_view(graph)`
- persisted view kind: `table`, with the specific preset in `table.kind` such as `requirements`

The current DTO includes:

- requirement id
- declared name
- text/documentation
- owner
- satisfied-by sources
- verified-by sources
- source file and line span
- warnings

This proves the intended direction:

```text
KIR graph -> requirements table projection -> UI table renderer
```

The table is not manually authored. It is derived from requirements and trace relationships in the model graph.

## Relationship To Diagrams

Diagrams should be treated as graphical views.

```text
Requirement table:
  query: authored requirements and trace links
  projection: rows and columns
  renderer: table

Requirement trace diagram:
  query: same requirements and trace links
  projection: nodes and edges
  renderer: graph
```

The view system should eventually unify diagrams and non-diagram projections behind a common view abstraction. The existing diagram system can remain specialized while this abstraction is proven.

The diagram implementation plan is documented in [DIAGRAM_IMPLEMENTATION_PLAN.md](DIAGRAM_IMPLEMENTATION_PLAN.md).

## Runtime-Defined Views

The long-term direction is a declarative view definition language or structured view IR.

The language should be safe, declarative, and query-oriented:

```text
view RequirementTable {
  from Requirement as req

  columns {
    id: req.element_id
    name: req.declared_name
    text: req.text
    owner: req.owner
    satisfiedBy: incoming(req, satisfy).source
    verifiedBy: incoming(req, verify).source
  }

  sort by id asc
  render table
}
```

This should compile into internal view IR rather than execute arbitrary user code.

Required properties:

- type-checkable against the semantic graph schema
- bounded traversal and predictable cost
- usable in server, desktop, web, CI, and proposal contexts
- able to render against immutable artifacts and draft overlays
- deterministic enough for tests and review workflows

Runtime-defined views should not be the first implementation step. They should emerge after several hardcoded projections expose the shared IR shape.

## Editability

Views may support two categories of changes.

Presentation-only edits:

- sort rows
- hide columns
- resize panes
- collapse groups
- change diagram layout

These update saved view state only.

Semantic edits:

- change requirement text
- add a satisfy relationship
- move a requirement to another package
- rename an element
- set priority metadata

These must emit semantic operations against a draft change set or proposal overlay. The UI should not patch source text directly.

```text
table cell edit
    -> semantic operation
    -> overlay graph
    -> validation
    -> source patch preview
    -> apply / discard / commit
```

The shared lifecycle for semantic view edits, draft overlays, proposals, and PR bindings is documented in [Proposal And Draft Overlay Lifecycle](PROPOSAL_DRAFT_LIFECYCLE.md).

## Draft And Proposal Support

Every view API should eventually accept an explicit semantic context:

```text
artifact id
proposal id
draft change set id
branch/commit pair
local workspace snapshot
```

This enables:

- render current accepted source
- render a proposed change before applying it
- compare before/after projections
- reason over cumulative edits
- validate table and diagram changes against the same overlay model

View-result caching should use the shared key model in [Semantic Artifact Keys](SEMANTIC_ARTIFACT_KEYS.md), with the view spec digest and semantic context added to the base artifact key.

## API Direction

Near-term hardcoded endpoint shape:

```text
GET  /api/views/kinds
POST /api/views/requirements-table
POST /api/views/render
```

Durable endpoint shape:

```text
POST /api/views/compile
POST /api/views/evaluate
GET  /api/views/files
GET  /api/views/file?path=...
PUT  /api/views/file?path=...
```

Example evaluate request:

```json
{
  "view": {
    "version": 1,
    "kind": "table",
    "table": {
      "version": 1,
      "kind": "requirements"
    }
  },
  "context": {
    "artifactId": "current"
  }
}
```

Example response:

```json
{
  "renderer": "table",
  "title": "Requirements",
  "columns": [],
  "rows": [],
  "warnings": []
}
```

## File Format Direction

Saved views should describe intent and presentation preferences, not cached results.

Possible `.view` shape:

```json
{
  "schema": "mercurio.view.v1",
  "version": 1,
  "kind": "table",
  "mode": "visualization",
  "table": {
    "version": 1,
    "kind": "requirements",
    "title": "Safety Requirements",
    "query": {
      "root": "pkg.VehicleSafety",
      "includeLibraries": false
    },
    "columns": [
      {"key": "id", "label": "ID"},
      {"key": "name", "label": "Name"},
      {"key": "text", "label": "Text"},
      {"key": "satisfied_by", "label": "Satisfied By"},
      {"key": "verified_by", "label": "Verified By"}
    ]
  }
}
```

`.diagram` can remain a specialized view file while diagram features mature. A later migration can either keep `.diagram` as a renderer-specific view file or generalize it under `.view`.

## Staged Development

### Stage 1: Hardcoded Projection Slice

Status: started.

Objectives:

- Add requirements table DTOs.
- Extract authored requirements from the KIR graph.
- Resolve satisfy and verify relationships.
- Return a renderer-neutral table projection.
- Cover with core tests and a small example KIR document.

Exit criteria:

- `requirements_table_view(graph)` returns stable rows for `test_files/examples/requirements_table_model.json`.
- Projection code has no dependency on source syntax or UI state.

### Stage 2: Server API

Objectives:

- Add a requirements table endpoint over the current workspace semantic graph.
- Include warnings and source anchors.
- Add request context for current artifact/workspace.
- Mirror the Rust DTO in TypeScript API types.

Exit criteria:

- Web and desktop clients can fetch a requirements table projection from the server.
- The endpoint can be exercised with the example KIR model.

### Stage 3: Table Renderer

Objectives:

- Add a table renderer component that consumes the projection DTO.
- Support sorting, filtering, column visibility, and source navigation.
- Keep renderer behavior presentation-only.

Exit criteria:

- Users can inspect requirements in a table without leaving the semantic model.
- Selecting a row can reveal the related source element when source metadata exists.

### Stage 4: Trace And Coverage Views

Objectives:

- Add trace matrix projection for requirement-to-satisfier and requirement-to-verifier coverage.
- Add coverage diagnostics for missing satisfy/verify links.
- Add filters for package, owner, kind, and verification status.

Exit criteria:

- Users can identify unverified or unsatisfied requirements from derived views.
- The same trace data can feed both a table/matrix and a graph projection.

### Stage 5: View Spec IR

Objectives:

- Define a `ViewSpec` and internal view IR that can represent requirements table, trace matrix, and selected diagram queries.
- Make hardcoded views compile into this IR.
- Add serde round-trip tests.

Exit criteria:

- At least three hardcoded views share the same internal query/projection concepts.
- The IR is stable enough to expose as JSON before designing a custom language.

### Stage 6: Saved View Files

Objectives:

- Add `.view` file discovery and persistence.
- Save query intent and presentation state.
- Re-evaluate saved views against current semantic artifacts.

Exit criteria:

- A saved requirements view can be reopened and refreshed after model changes.
- Saved view files do not contain stale semantic result data.

### Stage 7: Draft Overlay Support

Objectives:

- Let view evaluation run against proposal and draft change contexts.
- Show before/after or base/overlay projections.
- Keep semantic edits routed through change-set operations.

Exit criteria:

- A user can edit a requirement in a draft, re-render the table against the draft, and preview the eventual source patch.

### Stage 8: Runtime View Definitions

Objectives:

- Add a restricted view-definition syntax or JSON DSL over the view IR.
- Type-check view definitions against available semantic properties and relationships.
- Enforce traversal and cost limits.
- Add editable field declarations that emit semantic operations.

Exit criteria:

- A user-defined requirements-like table can be loaded at runtime and evaluated without code changes.
- Invalid view definitions produce actionable diagnostics.

### Stage 9: View Packages And Governance

Objectives:

- Allow organizations to package approved view definitions with model libraries.
- Version view definitions alongside KerML-derived language packages.
- Record which official views were used during reviews, validations, and releases.

Exit criteria:

- A project can depend on a shared view package and use those views in desktop, web, server, and CI workflows.

## Test Strategy

Core tests:

- projection from hand-authored KIR
- empty view warnings
- relationship extraction through graph edges and relationship elements
- library filtering
- source metadata preservation
- stable sorting

API tests:

- request/response serde
- invalid view kind
- artifact context missing
- stale artifact handling

Frontend tests:

- table renders rows and empty states
- sort/filter/column visibility
- source navigation action
- error and loading states

Integration tests:

- compile SysML source to KIR
- evaluate requirements table
- apply draft semantic operation
- re-evaluate against overlay
- confirm source patch round-trip

## Risks

### Hardcoding Too Much

Hardcoded projections are useful early, but they should be implemented in a way that exposes a future view IR. Avoid view-specific logic leaking into UI renderers.

### Treating Views As Source

Saved view files are important, but they must not become a second model. They describe how to inspect the model, not what the model is.

### Renderer Semantics Drift

Tables, diagrams, and matrices must not independently compute trace semantics. The server projection should compute relationships and coverage consistently.

### Runtime DSL Too Early

A view-definition language before the semantic graph is stable would likely freeze the wrong abstractions. Build hardcoded views first, then lift common structure into IR.

### Large Model Cost

Views need bounded traversal, pagination, filtering, and caching. Runtime-defined views must be cost-aware before they are exposed to shared servers.

## Near-Term Recommendation

Finish the requirements table as a vertical slice:

1. core projection, done
2. server endpoint
3. TypeScript DTOs
4. web/desktop table renderer
5. trace matrix projection
6. shared `ViewSpec` IR

This proves the main product thesis:

```text
Every view is a semantic projection of the design truth.
```
