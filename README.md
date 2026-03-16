# Daram

Daram is a Rust-based language toolchain with a CLI, compiler, bundled stdlib, example packages, and a VS Code extension.

## What Is In This Repository

- `compiler/`: lexer, parser, type checker, MIR, interpreter, and native backends
- `cli/`: the `dr` executable and language server entrypoint
- `stdlib/`: bundled Daram standard library sources
- `packages/`: sample and placeholder Daram packages plus the VS Code extension
- `scripts/release-gate.sh`: workspace verification gate

## Stability

The `1.0.0` promise in this repository applies to the language frontend, interpreter-first execution path, and the `dr` command surface that is exercised by the release gate.

The following areas are still intentionally narrower in scope:

- native backend support is a subset of the full accepted language surface
- `packages/daram-*` packages are placeholders, not fully implemented libraries

## Toolchain

Validated in this repository:

- Rust `1.94.0`
- Cargo `1.94.0`
- Node `20.20.1`
- npm `10.8.2`

## CLI Commands

`dr` currently provides:

- `new`, `init`
- `add`, `remove`, `install`
- `build`, `run`, `test`, `bench`
- `fmt`, `lint`, `check`, `doc`
- `lsp`

Show the full CLI help with:

```bash
cargo run -q -p dr -- --help
```

Language grammar reference: [grammar.md](grammar.md)

If you want the easy version first, read the `Quick Tour` section at the top of `grammar.md`.

## Install

`dr` itself does not require Rust on the target machine. Install the prebuilt CLI from GitHub Releases:

macOS and Linux:

```bash
curl -fsSL https://github.com/flyingsquirrel0419/daram-stable/releases/latest/download/install.sh | sh
```

Windows PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -Command "irm https://github.com/flyingsquirrel0419/daram-stable/releases/latest/download/install.ps1 | iex"
```

Version-pinned install:

```bash
curl -fsSL https://github.com/flyingsquirrel0419/daram-stable/releases/latest/download/install.sh | env DR_VERSION=1.0.0 sh
```

```powershell
$env:DR_VERSION='1.0.0'; irm https://github.com/flyingsquirrel0419/daram-stable/releases/latest/download/install.ps1 | iex
```

After installation, verify with:

```bash
dr --version
```

Native `dr build` output may still require a system C toolchain such as `cc`, `clang`, or `gcc`.

## Packages

The default Daram package registry is:

```text
https://daram.flyingsquirrel.me
```

`dr add` updates `daram.toml`. `dr install` actually resolves and downloads packages.

Try the package flow like this:

```bash
dr new sample-app
cd sample-app
dr add <package-name>@<version>
dr install
```

If you want to force the registry explicitly:

```bash
DRPM_REGISTRY=https://daram.flyingsquirrel.me dr install
```

You can inspect the manifest with:

```bash
cat daram.toml
```

## Development

Rust workspace:

```bash
cargo test -q -p daram-compiler
cargo test -q -p dr
bash scripts/release-gate.sh workspace
```

VS Code extension:

```bash
cd packages/daram-vscode
npm ci
npm run lint
npm run package
```

## Release Gate

Run the combined verification before release work:

```bash
bash scripts/release-gate.sh all
```

This gate checks compiler tests, CLI tests, a temporary workspace smoke run, and dependency bundling.

Release scope and sign-off notes live in `RELEASE.md`.
