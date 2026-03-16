# Daram 1.0 Release Notes

## Stable Scope

`1.0.0` is intended to cover the following repository surface:

- Daram language frontend accepted by the compiler tests
- interpreter-first execution path used by `dr check`, `dr test`, and CLI validation
- `dr` command surface exercised by `scripts/release-gate.sh`
- package installation flow validated by the release smoke workspace

## Non-Goals For 1.0.0

These areas are shipped, but should not be described as full-platform stability guarantees:

- native backend parity for every accepted program
- full stdlib runtime coverage outside the bundled stable subset
- placeholder packages in `packages/daram-*`

## Release Checklist

- run `bash scripts/release-gate.sh all`
- run `cd packages/daram-vscode && npm run lint`
- run `cd packages/daram-vscode && npm run package`
- verify `dr new <name>` followed by `dr build` succeeds
- verify version output is `dr 1.0.0`
- verify install scripts download the current release assets
- review open TODOs in backend and unstable stdlib areas before tagging

## Release Assets

Each tagged release should publish:

- `dr-<version>-x86_64-unknown-linux-gnu.tar.gz`
- `dr-<version>-x86_64-apple-darwin.tar.gz`
- `dr-<version>-aarch64-apple-darwin.tar.gz`
- `dr-<version>-x86_64-pc-windows-msvc.zip`
- `SHA256SUMS`
- `install.sh`
- `install.ps1`
- `daram-vscode-<version>.vsix`

## Install Commands

Unix:

```bash
curl -fsSL https://github.com/flyingsquirrel0419/daram-stable/releases/latest/download/install.sh | sh
```

Windows:

```powershell
powershell -ExecutionPolicy Bypass -Command "irm https://github.com/flyingsquirrel0419/daram-stable/releases/latest/download/install.ps1 | iex"
```
