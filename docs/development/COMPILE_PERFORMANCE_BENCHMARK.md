# Compile Performance Benchmark

Status: benchmark snapshot. Refresh the date and command context when adding new measurements.

Date: 2026-04-29

Branch: `codex/performance-improv`

Corpus root:

```text
test_files/examples/src/examples
```

Benchmark command:

```powershell
cargo run -q --bin benchmark_examples -- --folders --root test_files/examples/src/examples
```

Whole-tree stress command:

```powershell
cargo run -q --bin benchmark_examples -- --all --root test_files/examples/src/examples
```

Edited-file stress command:

```powershell
cargo run -q --bin benchmark_examples -- --edited --root test_files/examples/src/examples
```

The benchmark reports:

- `cold_diagnostics_ms`: `WorkspaceService::from_workspace_root_diagnostics_only` plus `compile_project_scope_diagnostics_only(".", [])`, skipping compiled app-state construction and semantic outlines.
- `cold_workspace_ms`: `WorkspaceService::from_workspace_root_compiled`, including project discovery, source compile, library setup, and app-state graph construction.
- `warm_scope_ms`: `compile_project_scope(".", [])` on the already-loaded service, which approximates editor/project recompile after caches are warm.

## Latest Focused Results

These runs include stdlib index caching, partial recovery capping, parse/context reuse, workspace source-document caching, the diagnostics-only compile path, semantic result caching, and direct-import dependency-aware invalidation.

| Target | Files | Bytes | Cold diagnostics ms | Cold workspace ms | Warm scope ms | Success | Failure |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Simple Tests | 34 | 21,506 | 3,232 | 3,596 | 32 | 7 | 27 |
| Vehicle Example | 4 | 81,730 | 3,992 | 3,963 | 27 | 1 | 3 |
| test_files/examples/src/examples | 95 | 228,994 | 17,452 | 17,240 | 97 | 22 | 73 |

The warm scope path now reuses semantic compile results when every source path/content in the compile scope is unchanged. Focused runs reported semantic cache hits for every file on warm scope calls.

## Edited-File Results

The edited-file benchmark loads a compiled workspace, runs an unchanged warm project compile, then stages a one-file text edit and recompiles the same scope.

| Target | Files | Edited path | Cold workspace ms | Unchanged warm ms | Unchanged hits | Unchanged misses | Edited warm ms | Edited hits | Edited misses | Cache entries | Cache capacity |
| --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Simple Tests | 34 | ActionTest.sysml | 4,173 | 35 | 34 | 0 | 98 | 33 | 1 | 35 | 512 |
| Vehicle Example | 4 | SysML v2 Spec Annex A SimpleVehicleModel.sysml | 4,839 | 31 | 4 | 0 | 3,311 | 2 | 2 | 6 | 512 |
| test_files/examples/src/examples | 95 | Analysis Examples/AnalysisAnnotation.sysml | 18,213 | 98 | 95 | 0 | 107 | 94 | 1 | 96 | 512 |

The semantic cache key now includes a direct-import dependency fingerprint instead of a whole-scope hash. Unrelated files keep hitting after a staged edit, while files that provide or directly depend on the edited package miss.

## Per-Folder Results

The per-folder table below is the earlier post parse/context-reuse baseline before the source-document cache and diagnostics-only path were added. It is retained for folder-to-folder shape and hotspot comparison.

| Target | Files | Bytes | Cold workspace ms | Warm scope ms | Success | Failure |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Analysis Examples | 4 | 17,080 | 3,417 | 368 | 3 | 1 |
| Arrowhead Framework Example | 4 | 14,978 | 7,989 | 435 | 0 | 4 |
| Association Examples | 3 | 2,837 | 3,149 | 134 | 0 | 3 |
| Camera Example | 2 | 545 | 3,249 | 17 | 2 | 0 |
| Cause and Effect Examples | 2 | 1,766 | 3,583 | 28 | 0 | 2 |
| Comment Examples | 1 | 398 | 3,129 | 3 | 0 | 1 |
| Flashlight Example | 1 | 1,307 | 2,619 | 20 | 0 | 1 |
| Geometry Examples | 5 | 19,858 | 2,550 | 118 | 3 | 2 |
| Import Tests | 4 | 1,411 | 2,111 | 35 | 1 | 3 |
| Individuals Examples | 2 | 4,402 | 2,818 | 37 | 0 | 2 |
| Interaction Sequencing Examples | 6 | 18,683 | 3,109 | 147 | 0 | 6 |
| Mass Roll-up Example | 3 | 3,424 | 2,532 | 76 | 0 | 3 |
| Metadata Examples | 5 | 3,741 | 2,700 | 29 | 5 | 0 |
| Packet Example | 2 | 1,412 | 2,269 | 35 | 0 | 2 |
| Requirements Examples | 3 | 2,848 | 2,094 | 21 | 3 | 0 |
| Room Model | 1 | 2,923 | 1,936 | 11 | 1 | 0 |
| Simple Tests | 34 | 21,506 | 2,777 | 739 | 7 | 27 |
| State Space Representation Examples | 2 | 13,212 | 2,176 | 212 | 1 | 1 |
| Timeslice and Snapshot Examples | 1 | 1,447 | 2,967 | 6 | 1 | 0 |
| Variability Examples | 1 | 4,882 | 2,067 | 19 | 0 | 1 |
| Vehicle Example | 4 | 81,730 | 3,085 | 1,186 | 1 | 3 |
| v1 Spec Examples | 5 | 8,604 | 2,079 | 208 | 1 | 4 |

## Whole-Tree Stress Result

The current whole-tree result is included in the latest focused table above. The older post parse/context-reuse result is retained here for comparison.

| Target | Files | Bytes | Cold workspace ms | Warm scope ms | Success | Failure |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| test_files/examples/src/examples | 95 | 228,994 | 19,939 | 15,896 | 22 | 73 |

The whole-tree result is a stress test, not a realistic project shape. It places independent example models into one shared context, which inflates cross-file context work and changes diagnostics.

## Observations

- The stdlib index cache removes the repeated stdlib index rebuild from warm compiles. In the focused API staged compile test, warm project-scope compile dropped from roughly seconds to single-digit milliseconds.
- Large source partial recovery is now capped. `Vehicle Example` warm scope time dropped from about `7.1s` to `1.4s`, and `SysML v2 Spec Annex A SimpleVehicleModel.sysml` dropped from about `6.7s` to `1.1s` in folder mode.
- In the whole-tree stress run, warm scope time dropped from about `33.0s` to `17.1s`; the Annex A file dropped from about `12.3s` to `1.5s` in that context.
- Reusing parsed modules and building resolver context indexes once per compile scope reduced `Simple Tests` warm scope time from about `1.2s` to `0.7s`.
- After parse/context reuse, the whole-tree stress run is at about `15.9s` warm scope time.
- The source-document cache eliminates repeated file parse work on warm scope compiles, but current measurements show that parsing is not the dominant cost for the large corpus.
- The diagnostics-only path avoids app-state graph construction for project diagnostics. It is most useful for API/editor calls that only need diagnostics, not runtime app state.
- Semantic result caching reduces unchanged warm project-scope compile from `1.378s` to `32ms` for `Simple Tests`, from `1.204s` to `27ms` for `Vehicle Example`, and from `21.481s` to `97ms` for the whole examples stress corpus.
- The semantic cache is bounded to `512` entries per workspace service to avoid unbounded long-session growth.
- Direct-import dependency-aware invalidation reduces the full-corpus one-file edit case from `15.663s` with `0/95` hits to `107ms` with `94/95` hits. In `Simple Tests`, the same scenario moves from `966ms` with `0/34` hits to `98ms` with `33/34` hits.
- `Vehicle Example` still has dependent misses when editing the large Annex file (`2/4` hits, `3.311s`), which is expected because other vehicle files import from or otherwise depend on that model surface.

Large files currently use a smaller partial semantic recovery budget. When that budget is exhausted, diagnostics include an extra message noting that recovery stopped early.
