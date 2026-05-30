# Resources

`resources/` contains versioned inputs and compatibility artifacts used by the
current bundled stdlib path.

## `stdlib-sources/<source-id>/`

Locked stdlib source and derived compatibility artifacts for one stdlib lineage.
For `sysml-2.0-pilot-0.57.0` this includes:

- `pilot-stdlib-export.json`: raw Pilot export, the repeatable source boundary
- `source.lock.json`: source identity and export digest
- `stdlib.full.kir.json`: full precompiled stdlib KIR used by native defaults
- `stdlib.kir.json`: lightweight stdlib KIR embedded by the WASM crate
- `stdlib.rulepack.json`: generated stdlib metamodel adapter rulepack
- `sysml.library.kpar/`: bundled OMG package-set directory fallback

## `language-profiles/<profile-id>/`

Language/profile binding for a compiler profile, including:

- `profile.json`
- `provenance.json`
- `mappings/`: construct and KIR-emission mapping files for that profile

Longer term, native defaults should resolve stdlib content through bundled
KPAR/MPack packages. The unpackaged KIR/rulepack files remain here as explicit
versioned compatibility artifacts during that migration.
