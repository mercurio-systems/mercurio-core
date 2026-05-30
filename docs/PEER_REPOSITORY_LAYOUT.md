# Peer Repository Layout

This document defines the recommended placement for Mercurio peer repositories,
upstream reference repositories, and example corpora.

The goal is to keep `mercurio-core` clean while still making reference
implementations and large example sets easy to use for conformance, comparison,
and demos.

## Current Local Workspace

The current local workspace is:

```text
C:\dev\git\mercurio\
  mercurio-core\
  mercurio-examples\
  mercurio-product\
  mercurio-reasoning\

  external\
    SysML-v2-Pilot-Implementation\
    SysMLv2Example\
    sysml-project-test1\
```

Upstream/reference checkouts live under `external/`, not as vendored subtrees
inside `mercurio-core`.

## Recommended Target Workspace

```text
C:\dev\git\mercurio\
  mercurio-core\
  mercurio-reasoning\
  mercurio-services\
  mercurio-product\
  mercurio-examples\

  external\
    SysML-v2-Pilot-Implementation\
```

The `external/` directory is optional. If it is not used, upstream reference
repositories may remain direct siblings:

```text
C:\dev\git\mercurio\
  mercurio-core\
  mercurio-product\
  SysML-v2-Pilot-Implementation\
```

Mercurio code should support both layouts through configuration.

## Repository Roles

### `mercurio-core`

Open semantic kernel.

Owns:

- KIR
- SysML/KerML parser
- compiler
- stdlib support
- deterministic graph/query/runtime
- small fixtures required for unit and integration tests

Does not own:

- full upstream reference implementations
- large public example corpus
- generated comparison artifacts
- product UI
- private reasoning services

### `mercurio-examples`

Public example and corpus repository.

Recommended shape:

```text
mercurio-examples/
  sysml/
    training/
    validation/
    tutorials/
    domain/

  kerml/
    training/
    validation/

  kir/
    snapshots/
    test_files/

  scenarios/
    state_machine/
    mission/
    trade_study/

  expected/
    projections/
    query_results/
    reasoning_reports/

  docs/
    tutorials/
    walkthroughs/
```

Owns:

- larger public model corpus
- tutorials
- validation examples
- demo scenarios
- expected outputs for cross-repo regression checks

Core may keep a small subset of examples in `mercurio-core/test_files/examples` when those
examples are required for local tests, docs, or CLI smoke checks.

### `SysML-v2-Pilot-Implementation`

Upstream/reference checkout.

Recommended placement:

```text
C:\dev\git\mercurio\external\SysML-v2-Pilot-Implementation\
```

Legacy accepted placement:

```text
C:\dev\git\mercurio\SysML-v2-Pilot-Implementation\
```

Owns:

- upstream SysML v2 reference implementation
- upstream examples and validation corpus
- reference exporter behavior

Mercurio should not vendor this repository into `mercurio-core`.

## Configuration

Mercurio tooling should locate peer repositories through explicit configuration.

Preferred environment variables:

```powershell
$env:MERCURIO_WORKSPACE_ROOT = "C:\dev\git\mercurio"
$env:MERCURIO_PILOT_ROOT = "C:\dev\git\mercurio\external\SysML-v2-Pilot-Implementation"
$env:MERCURIO_EXAMPLES_ROOT = "C:\dev\git\mercurio\mercurio-examples"
```

If an `external/` folder is adopted:

```powershell
$env:MERCURIO_PILOT_ROOT = "C:\dev\git\mercurio\external\SysML-v2-Pilot-Implementation"
```

The `mercurio-tools` Pilot-facing binaries now honor these variables. If
`MERCURIO_PILOT_ROOT` is not set, they resolve the Pilot checkout in this order:

1. `MERCURIO_WORKSPACE_ROOT\SysML-v2-Pilot-Implementation`
2. `MERCURIO_WORKSPACE_ROOT\external\SysML-v2-Pilot-Implementation`
3. `../external/SysML-v2-Pilot-Implementation`
4. `../SysML-v2-Pilot-Implementation`

This preserves legacy sibling checkouts while preferring the cleaner
`external/` placement.

Local project config may also point to peers:

```toml
[workspace]
root = ".."

[pilot]
root = "../external/SysML-v2-Pilot-Implementation"

[examples]
root = "../mercurio-examples"
```

## Core Rules

1. Do not vendor `SysML-v2-Pilot-Implementation` into `mercurio-core`.
2. Do not commit generated Pilot comparison artifacts to core.
3. Keep only small, stable test files in `mercurio-core/test_files`.
4. Put larger example corpora in `mercurio-examples`.
5. Use environment variables or local config to locate peer repositories.
6. Generated outputs belong in ignored directories such as `target/` or `.mercurio/cache/`.
7. Upstream reference behavior may inform tests, but Mercurio semantics must be represented through KIR/compiler/runtime code, not copied ad hoc into reasoning or UI layers.

## Migration Steps

1. Keep current sibling Pilot checkout working.
2. Use `MERCURIO_PILOT_ROOT` or `MERCURIO_WORKSPACE_ROOT` for tools that compare against Pilot.
3. Create `mercurio-examples` when the example corpus becomes too large or noisy for core.
4. Move large examples and expected outputs from core to `mercurio-examples`.
5. Keep a minimal smoke-test set in `mercurio-core/test_files/examples`.
6. Keep upstream checkouts under `external/` once all tools use configured paths.

## Dependency Direction

Reference repositories and example corpora are inputs to Mercurio tooling:

```text
SysML-v2-Pilot-Implementation -> comparison input
mercurio-examples             -> examples and regression input
mercurio-core                 -> parser/compiler/runtime implementation
```

They should not become compile-time dependencies of `mercurio-core`.
