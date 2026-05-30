# mercurio-wasm

Browser-facing WebAssembly adapter for `mercurio-core`.

The design keeps Rust domain logic in `mercurio-core` and exposes a compact JSON API at the JS boundary. Browser callers can use one-shot functions for simple workflows or `MercurioSession` to keep compiled sources and derived indexes in memory.

## Build

```powershell
cargo check -p mercurio-wasm --target wasm32-unknown-unknown
wasm-pack build crates/mercurio-wasm --target web
```

## API Shape

Every exported operation returns:

```json
{
  "ok": true,
  "value": {},
  "diagnostics": [],
  "errors": [],
  "metadata": {}
}
```

Main exports:

- `compileSysml(input, options)`
- `compileKerml(input, options)`
- `lint(input, language, options)`
- `formatText(input, language)`
- `renderDiagram(document, request)`
- `requirementsTable(document)`
- `queryRuntime(document, query)`
- `runAssessment(document, spec)`
- `new MercurioSession(options)`

`options.stdlib` may provide a KIR stdlib document. If omitted, the module uses the embedded lightweight `resources/stdlib-sources/sysml-2.0-pilot-0.57.0/stdlib.kir.json`.
