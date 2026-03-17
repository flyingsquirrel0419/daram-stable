# Daram Packages

`packages/` contains two kinds of content:

- placeholder Daram packages used as examples
- the VS Code extension in `daram-vscode/`

## Placeholder Packages

Most packages here still expose only a minimal `package_name()` function:

- `daram-cli`
- `daram-db`
- `daram-http`
- `daram-serde`
- `daram-test`

The rest are versioned with the repository, but they should be treated as stubs until their APIs are expanded.

## VS Code Extension

`daram-vscode/` provides language support by spawning `dr lsp`.
