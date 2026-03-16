# Daram VS Code Extension

This extension provides syntax support and starts the Daram language server by running `dr lsp`.

## Requirements

- VS Code `1.89.0` or newer
- a `dr` binary available on `PATH`, or a custom path set in extension settings

## Install

Download the release `.vsix` from GitHub Releases and install it with:

```bash
code --install-extension daram-vscode-<version>.vsix
```

## Settings

- `daram.server.path`: executable path, default `dr`
- `daram.server.args`: server arguments, default `["lsp"]`
- `daram.server.trace`: trace level, one of `off`, `messages`, `verbose`

## Development

```bash
cd packages/daram-vscode
npm ci
npm run lint
npm run package
```
