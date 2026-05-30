# SysML 2.0 Pilot 0.57.0 Mappings

These files are part of the `sysml-2.0-pilot-0.57.0` language profile.

`pilot_constructs.seed.json` maps textual parser constructs and keywords to
Pilot-derived SysML/KerML metaclasses.

`kir_emission.seed.json` maps those metaclasses to Mercurio KIR emission rules:
KIR kind, id template, emitted properties, relationships, and metadata policy.

They are compiler/profile inputs, not runtime workspace files. Stdlib release
builds include their digests in provenance and package them with the profile.
