# Mercurio Development Docs

This directory keeps architecture notes, implementation plans, compiler/runtime design, benchmarks, and semantic-service references.

For user-facing guides, see [../user/README.md](../user/README.md).

Start with the [development roadmap](DEVELOPMENT_ROADMAP.md) for delivery order and the [developer-docs corpus review](DOCS_CORPUS_REVIEW.md) when deciding where new material belongs or when reconciling overlapping plans.

## Canonical Architecture

These are the highest-level documents. Other plans should reinforce these boundaries.

- [Development Roadmap](DEVELOPMENT_ROADMAP.md): cross-plan delivery order and coordination rules.
- [Architecture Model](ARCHITECTURE_MODEL.md): source authority, KIR, server, desktop, web, proposals, PRs, packages, and canonical nouns.
- [KIR Spec](KIR_SPEC.md): current KIR document contract and open schema-tightening work.
- [KIR Schema Roadmap](KIR_SCHEMA_ROADMAP.md): staged plan for schema versioning, provenance, references, and `expression_ir`.
- [Frontend To KIR Theory Of Operation](FRONTEND_TO_KIR_THEORY_OF_OPERATION.md): source-to-KIR pipeline and implementation anchors.
- [Project Descriptor and Library Provider Plan](PROJECT_DESCRIPTOR_AND_MOUNT_PLAN.md): project descriptors, libraries, baseline libraries, providers, and caches.
- [Plugin Architecture](PLUGIN_ARCHITECTURE.md): plugin packages, registries, WASM services, verification, loading, and caches.
- [Semantic Artifact Keys](SEMANTIC_ARTIFACT_KEYS.md): shared cache/evidence key model for compiled and derived semantic artifacts.
- [Proposal And Draft Overlay Lifecycle](PROPOSAL_DRAFT_LIFECYCLE.md): shared lifecycle for drafts, proposals, overlays, and PR bindings.

Public reasoning service contracts live in `crates/mercurio-reasoner-api`. That crate defines DTOs
for semantic contexts, capabilities, findings, artifacts, and evidence graphs without implementing
private reasoning services or product workflow.

Public plugin contracts live in `crates/mercurio-plugin-api`. That crate defines manifest,
permission, service, verification-action, and capability-declaration DTOs without implementing
plugin discovery, installation, sandboxing, or execution.

Open deterministic reference capabilities live in `crates/mercurio-reference-capabilities`. The
first reference capability is requirement coverage, which turns the core requirements view and
derived indexes into a `ReasoningReport` without adding orchestration, plugins, or private services
to the semantic kernel crate.

## Active Runtime And Semantic-Service Architecture

These describe services downstream of compiled KIR. They should not redefine source authority or the KIR contract.

- [Datalog Reasoning Engine Plan](DATALOG_REASONING_ENGINE_PLAN.md): derived fact/rule layer over graph state.
- [Views Architecture](VIEWS_ARCHITECTURE.md): semantic projections for tables, diagrams, matrices, dashboards, and runtime-defined views.
- [Simulation Architecture](SIMULATION_ARCHITECTURE.md): behavioral simulation over KIR, runtime expressions, traces, and scenarios.
- [Simulation Implementation Plan](SIMULATION_IMPLEMENTATION_PLAN.md): phased implementation plan for the first event-based simulation slice.
- [Verification Pipeline Architecture](VERIFICATION_PIPELINE_ARCHITECTURE.md): CI/CD actions, requirement compliance, behavioral simulation, and evidence.

## Active Language And Compiler Plans

- [KerML Support Plan](KERML_SUPPORT_PLAN.md): first-class `.kerml` support and shared KerML/SysML frontend shape.
- [L2 Parser Plan](L2_PARSER_PLAN.md): original Rust-native SysML parser plan. Treat as historical baseline where current code has advanced past it.
- [SysML Expression Implementation Plan](SYSML_EXPRESSION_IMPLEMENTATION_PLAN.md): expression parsing and `expression_ir`; partly implemented in current core code.

## Product, API, And Distribution Plans

These docs describe integration surfaces. Core-reusable behavior belongs here only when it remains product-neutral.

- [Server Workspace Plan](SERVER_WORKSPACE_PLAN.md): strategic server role as semantic reasoner over source authorities.
- [Server Implementation Plan](SERVER_IMPLEMENTATION_PLAN.md): implementation companion and product-repo boundary.
- [Python Frontend Plan](PYTHON_FRONTEND_PLAN.md): Python client/process-manager contract over a local backend.
- [Windows Installer Plan](WINDOWS_INSTALLER_PLAN.md): Windows install and release packaging path.
- [Maintainer Tools](MAINTAINER_TOOLS.md): diagnostics, benchmarks, demos, and Pilot comparison/export workflows.

## Feature Plans And Benchmarks

- [Diagram Implementation Plan](DIAGRAM_IMPLEMENTATION_PLAN.md): diagram/view lifecycle. This should be reconciled with the broader views architecture as diagram APIs mature.
- [Compile Performance Benchmark](COMPILE_PERFORMANCE_BENCHMARK.md): benchmark snapshot and performance notes. Refresh dates when results are updated.
