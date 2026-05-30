# Stdlib Release Pipeline

Mercurio treats the Pilot export as the first locked source artifact for the
stdlib release pipeline. Extraction from Pilot can depend on the exact Pilot
checkout, built interactive jar, and exporter behavior, so downstream artifacts
must be reproducible from a captured export plus provenance rather than from a
floating Pilot workspace.

## Version Boundaries

- Spec/library version: the OMG-facing package version, for example
  `org.omg/sysml-stdlib:2.0.0`.
- Source id: the Pilot source identity, for example a tag, commit, or explicit
  release label such as `pilot-0.57.0-g5694b8a813c3`.
- Profile id: the language profile binding, for example
  `sysml-2.0-pilot-0.57.0`.
- KIR schema version: the normalized KIR contract version.
- MPack version: the Mercurio support package version containing the KPAR,
  profile, rulepack, and generated wrappers.

Do not collapse these into one version string. The release provenance binds them
together with artifact digests.

## Build

From an already captured Pilot export:

```powershell
cargo run -p mercurio-tools --bin build_stdlib_release -- `
  --from-export resources\stdlib-sources\sysml-2.0-pilot-0.57.0\pilot-stdlib-export.json `
  --source-id pilot-0.57.0-g5694b8a813c3 `
  --out artifacts\stdlib\sysml-2.0.0\pilot-0.57.0-g5694b8a813c3 `
  --check-reproducible `
  --audit-profile
```

From a Pilot checkout:

```powershell
cargo run -p mercurio-tools --bin build_stdlib_release -- `
  --pilot-root ..\SysML-v2-Pilot-Implementation `
  --source-id pilot-0.57.0-g5694b8a813c3 `
  --out artifacts\stdlib\sysml-2.0.0\pilot-0.57.0-g5694b8a813c3 `
  --check-reproducible `
  --audit-profile
```

To update the checked-in runtime stdlib resources after review, add
`--promote`. Promotion copies the locked raw export, generated full KIR,
generated rulepack, and consolidated release lock into
`resources/stdlib-sources/<profile-id>/`. Release packages, wrappers, and MPack
archives remain under `artifacts/`.

The output directory contains:

- `source.lock.json`
- `raw/pilot-stdlib-export.json`
- `kir/stdlib.full.kir.json`
- `rules/stdlib.rulepack.json`
- `kpar/sysml-stdlib-<version>.kpar`
- `profiles/<profile-id>/profile.json`
- `python/<wrapper-module>/...`
- `mpack/<mpack-id>-<version>.mpack`
- `build.provenance.json`
- `release.lock.json`

## Repeatability Rule

For release review, compare `release.lock.json`. It binds the locked raw export,
profile, mapping files, generated KIR, generated rulepack, wrappers, KPAR, and
MPack with artifact digests. The raw export digest is the boundary for Pilot
stdlib extraction. All downstream artifact digests should remain stable when the
same export, profile, mappings, KIR schema, and Mercurio tool revision are used.
