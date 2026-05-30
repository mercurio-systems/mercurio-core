# Troubleshooting

## Command Not Found

If `mercurio` is not found, build or install the CLI and make sure the binary directory is on `PATH`.

From the repository root:

```powershell
cargo build
```

You can also run the CLI through Cargo while developing:

```powershell
cargo run -p mercurio-cli -- --help
```

## Wrong Source Language

Files ending in `.sysml` are parsed as SysML. Files ending in `.kerml` are parsed as KerML.

Inline text defaults to SysML. For inline KerML, pass `--language kerml`:

```powershell
mercurio compile --text "package Demo { classifier Vehicle; }" --language kerml
```

## Descriptor Not Found Or Wrong Libraries

Semantic commands discover `.mercurio-project.json` by walking upward from the input path. If a command appears to use the wrong libraries:

- run the command from the expected project root
- check which file path is used as the command input
- verify relative provider paths are relative to `.mercurio-project.json`
- pass `--stdlib PATH` when you deliberately want to skip descriptor discovery

See [Project Descriptors](PROJECTS.md).

## Standard Library Override

Most commands use the bundled standard library by default.

Use `--stdlib PATH` to override it:

```powershell
mercurio compile --file model.sysml --stdlib resources/stdlib-sources/sysml-2.0-pilot-0.57.0/stdlib.full.kir.json
```

If results differ between runs, check whether the command is using a project descriptor, a bundled stdlib, or an explicit `--stdlib` path.

## KPAR Input Problems

For `--kpar PATH`, verify:

- the path exists
- the file is a `.kpar`
- package metadata is present
- source files inside the package are valid SysML or KerML

For project descriptor KPAR dependencies, verify the provider is `kpar_file` and that relative paths resolve from the descriptor directory.

See [KPAR Packages](KPAR.md).

## Query Returns No Rows

If a query returns no rows:

- compile with `--format json` and inspect element `kind`, `qualified_name`, and `metatype`
- check whether the model uses a different SysML/KerML kind than expected
- use a broader query first, then add filters

Example broad query:

```powershell
mercurio query --file model.sysml --query 'from elements select id, kind, qualified_name'
```

## Evaluation Cannot Find Owner Or Feature

Evaluation accepts user-facing qualified names and low-level KIR ids.

If lookup fails:

- query the model for candidate ids and qualified names
- make sure the feature is owned by the owner you passed
- use `--format json` or `--explain` for more diagnostic detail

## Pilot Tools Require Java And Pilot Inputs

The public CLI does not require Java. Java is only needed for Pilot comparison/export tools under `tools/pilot-exporter`.

Pilot comparison commands also need a Pilot checkout or exported Pilot artifacts.
