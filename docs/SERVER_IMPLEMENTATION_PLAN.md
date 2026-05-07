# Server Implementation Plan

Historical note: Mercurio Core is now library-only. The privileged HTTP/console API
implementation lives in `mercurio-product/mercurio-console-api`. Keep reusable
semantic and packaging logic in `mercurio-core`; keep deployment and HTTP route
policy in the product repository.

## Goal

Build Mercurio Server as a semantic reasoning service over external source authorities.

The server should support:

- external Git projects
- semantic artifact caching keyed by immutable commits and semantic environment
- consolidated KIR generation
- validation and diagnostics
- semantic graph and diff APIs
- lightweight browser proposals
- package registry and package release from validated commits
- provider integrations for checks, comments, branches, and PRs

This document is the implementation companion to [SERVER_WORKSPACE_PLAN.md](SERVER_WORKSPACE_PLAN.md).

## Product Boundary

Keep these workflows distinct:

```text
Desktop local Git project -> user commits and pushes through normal Git
External Git project      -> Mercurio indexes and reasons over external commits
Web proposal              -> Mercurio validates an overlay, then exports patch or opens PR
Package release           -> Mercurio publishes immutable artifact from validated source
```

Do not make Mercurio Server the default source authority. Do not create per-user clones for normal client actions. The server may maintain mirrors and caches for project repositories, but those mirrors are derived from external Git.

## Proposed Module Shape

The server binary lives in the product repository alongside the deployable console surface.

Recommended shape:

```text
mercurio-product/
  mercurio-console-api/
    src/
      main.rs
      lib.rs
      api/
      workspace.rs
```

The existing server project/file API can remain as a prototype or compatibility slice while the project repository model is introduced, but new work should favor repository authority records and semantic artifacts.

## Storage Model

Use server-local storage for mirrors, semantic artifacts, proposals, packages, and audit data:

```text
server_data/
  repositories/
    {repository_id}/
      metadata.json
      mirror.git
      watch_state.json
  semantic_artifacts/
    {repository_id}/
      {artifact_key}/
        kir.json
        diagnostics.json
        graph_index.json
        summary.json
        provenance.json
  proposals/
    {proposal_id}/
      metadata.json
      overlays/
      validation.json
      semantic_diff.json
  packages/
    {package_name}/
      {version}/
        package.kpar
        manifest.json
        checksums.json
  audit/
```

Repository mirrors are caches of external Git state. They are not client workspaces and should not be mutated as the accepted source of truth.

## Semantic Artifact Keys

A semantic artifact key should include:

```text
repository_id
commit_sha
compiler_version_or_digest
kir_schema_version
stdlib_digest
dependency_package_digests
mapping_rules_digest
```

The same commit may compile differently under a different compiler, stdlib, dependency set, or mapping file. The cache key must reflect that.

## Core Services

### Repository Tracker

Responsibilities:

- register project repositories
- expose project repositories as a clone/discovery catalog for desktop clients
- store indexed branches, tags, or PR refs
- fetch new refs into a mirror/cache
- enqueue semantic compilation for new commits
- track last indexed commit per ref

### Provider Integration

Responsibilities:

- authenticate through provider-native app or OAuth flows
- fetch repository metadata and PR metadata
- post validation checks/statuses
- post review comments where configured
- create branches and PRs from proposals when allowed

### Semantic Compiler

Responsibilities:

- materialize or stream a source tree at a commit
- apply optional proposal overlays
- compile source files through `mercurio-core`
- merge library and user KIR into consolidated KIR
- return diagnostics with source spans
- record provenance needed for cache keys

### Semantic Artifact Store

Responsibilities:

- persist consolidated KIR
- persist diagnostics, graph indexes, and source span indexes
- cache summaries and semantic diff inputs
- invalidate or bypass stale artifacts when semantic environment fingerprints change

### Semantic Diff Service

Responsibilities:

- compare two semantic artifacts
- report added, removed, changed, and unchanged model elements
- identify changed properties and relationships
- link semantic changes back to source spans where possible

### Proposal Service

Responsibilities:

- create proposal overlays against a base commit
- store changed file content without mutating the indexed mirror
- validate virtual source trees
- compute proposal semantic diffs
- export patches
- submit external PRs through provider integration

### Package Registry

Responsibilities:

- publish package version from validated commit/tag or proposal result
- accept package artifact published from desktop local Git
- validate package manifest
- store package artifact and checksums
- record source repository and commit provenance
- resolve package by name/version
- expose package download or mount metadata

### Reasoning History

Responsibilities:

- record decisions, assumptions, critiques, and review notes
- link reasoning records to repositories, commits, semantic artifacts, proposals, PRs, and package releases
- preserve rationale outside the source repository unless explicitly exported

## API Plan

### Mercurio Extension API

Start here because it supports the web UI and semantic server role directly.

Minimum project repository endpoints:

```text
GET  /api/health

GET  /api/repositories
POST /api/repositories
GET  /api/repositories/{repository_id}
PATCH /api/repositories/{repository_id}
GET  /api/repositories/{repository_id}/clone-info

GET  /api/repositories/{repository_id}/refs
POST /api/repositories/{repository_id}/fetch

GET  /api/repositories/{repository_id}/commits/{commit_sha}/validation
POST /api/repositories/{repository_id}/commits/{commit_sha}/validate
GET  /api/repositories/{repository_id}/commits/{commit_sha}/kir
GET  /api/repositories/{repository_id}/commits/{commit_sha}/graph

GET  /api/repositories/{repository_id}/diff?left=...&right=...

GET  /api/proposals
POST /api/proposals
GET  /api/proposals/{proposal_id}
PUT  /api/proposals/{proposal_id}/files/{path}
POST /api/proposals/{proposal_id}/validate
GET  /api/proposals/{proposal_id}/diff
POST /api/proposals/{proposal_id}/export-patch
POST /api/proposals/{proposal_id}/submit-pr

GET  /api/packages
POST /api/packages
GET  /api/packages/{name}
GET  /api/packages/{name}/versions/{version}
GET  /api/packages/{name}/versions/{version}/download
POST /api/packages/{name}/versions/{version}/publish
```

Legacy server-owned project endpoints can be kept temporarily while the product direction changes:

```text
GET  /api/projects
POST /api/projects
GET  /api/projects/{project_id}
...
```

They should not drive new core architecture unless Mercurio-hosted authority is reintroduced deliberately.

### SysML v2 API

Add after semantic artifact and repository concepts stabilize.

Implement a compatibility layer that maps repositories, commits, and compiled semantic elements onto SysML v2 concepts where natural. Keep project repository configuration, proposal overlays, diagnostics, semantic diffs, package release, and reasoning history in explicit Mercurio extension endpoints.

## Browser UI Plan

The web UI should be a semantic review and proposal cockpit.

Recommended first views:

- project repository list
- repository catalog details including clone URLs for desktop
- repository detail with refs and validation status
- commit semantic status
- semantic file browser
- graph/explorer for a selected commit
- commit compare view
- proposal editor
- package release view

The web UI should show source authority prominently:

```text
GitHub / acme/brake-system
branch: main
commit: abc123
Mercurio artifact: current
```

## Implementation Phases

### Phase 1: Semantic Server Foundation

- add or keep server binary/crate
- load config and data directory
- expose `/api/health`
- add structured error DTOs
- define repository authority records
- define semantic artifact provenance and cache keys
- compile one local source snapshot into consolidated KIR through the server path

Exit condition:

- server can produce and store a semantic artifact for a known source snapshot

### Phase 2: Project Repository Mirror

- register external repository metadata
- expose repository catalog records with clone URL, provider, default branch, and semantic status
- create a bare mirror/cache per repository
- fetch indexed refs
- list refs and commits
- avoid per-user clones

Exit condition:

- server can track an external Git repository and identify new commits

### Phase 3: Commit Validation And Cache

- compile a commit from the mirror
- store consolidated KIR and diagnostics under a semantic artifact key
- expose validation status and KIR lookup endpoints
- post provider checks if credentials are configured

Exit condition:

- an indexed commit has cached validation status and KIR artifacts

### Phase 4: Semantic Diff

- compare cached artifacts for two commits
- expose semantic diff endpoint
- add web compare view
- link changes to source spans where possible

Exit condition:

- user can compare two commits semantically in the browser

### Phase 5: Proposals

- create proposal records against a base commit
- store file overlays
- validate `base commit + overlay`
- show proposal diagnostics and semantic diff
- export patch

Exit condition:

- user can make and validate a browser proposal without mutating external Git

### Phase 6: Provider PR Integration

- support provider app credentials
- optionally support user-delegated credentials
- create branch and PR from proposal
- post validation summary to PR

Exit condition:

- a validated proposal can be submitted as an external PR

### Phase 7: Package Registry

- define package manifest DTO
- publish package from validated commit or tag
- publish package artifact from desktop local Git project
- store checksums and provenance
- resolve package dependencies by name/version

Exit condition:

- an external Git commit can produce a reusable package, and another workspace can depend on it

### Phase 8: Reasoning History And Governance

- store decisions, assumptions, critiques, and review notes
- link reasoning to commits, proposals, PRs, validation results, and package releases
- add authz policy for projects, proposals, and package publish

Exit condition:

- semantic review and package release are traceable to source commits and decisions

## Current Prototype Status

The current code includes an initial server-owned project API slice:

- project create/list/open
- project file read/write
- validation over server project files
- browser shell around server projects

This remains useful as a prototype and may support future Mercurio-hosted authority. New strategic work should shift toward project repositories and semantic artifacts instead of expanding server-owned project revisioning as the default path.

## First Implementation Recommendation

Implement these next:

1. semantic artifact key/provenance model
2. project repository registration and mirror fetch
3. commit validation into consolidated KIR cache
4. semantic diff endpoint for two cached commits

Package registry and browser proposals should follow once commit-indexed artifacts are reliable.

## Risks

### Rebuilding Git Hosting

Avoid implementing repository authority, branching, merges, and user worktrees as the default server model. External providers already own that well.

### Cache Staleness

Do not key semantic artifacts only by commit SHA. Include compiler, stdlib, dependency, mapping, and KIR schema fingerprints.

### Credential Risk

Prefer provider app credentials and scoped OAuth over raw PATs. Store credentials encrypted and keep scopes narrow.

### Proposal Drift

Proposals must be revalidated when their target branch moves or when semantic environment fingerprints change.

### API Drift

Keep Mercurio extension endpoints explicit. Do not force proposal overlays, semantic diffs, diagnostics, and package release into SysML v2 endpoints if the standard does not naturally represent them.
