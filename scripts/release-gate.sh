#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-all}"

log() {
  printf '[release-gate] %s\n' "$*"
}

run_workspace_gate() {
  log "checking Rust formatting"
  cargo fmt --all --check --manifest-path "$ROOT_DIR/Cargo.toml"

  log "running compiler regression suites"
  CARGO_BUILD_JOBS=1 cargo test -q -p daram-compiler --lib --manifest-path "$ROOT_DIR/Cargo.toml"
  CARGO_BUILD_JOBS=1 cargo test -q -p daram-compiler --test frontend_regression --manifest-path "$ROOT_DIR/Cargo.toml"
  CARGO_BUILD_JOBS=1 cargo test -q -p daram-compiler --test interpreter_regression --manifest-path "$ROOT_DIR/Cargo.toml"
  CARGO_BUILD_JOBS=1 cargo test -q -p daram-compiler --test mir_regression --manifest-path "$ROOT_DIR/Cargo.toml"
  CARGO_BUILD_JOBS=1 cargo test -q -p daram-compiler --test cranelift_backend_regression --manifest-path "$ROOT_DIR/Cargo.toml"

  log "running CLI test suite"
  CARGO_BUILD_JOBS=1 cargo test -q -p dr --manifest-path "$ROOT_DIR/Cargo.toml"

  log "type-checking compiler crate"
  CARGO_BUILD_JOBS=1 cargo check -q -p daram-compiler --manifest-path "$ROOT_DIR/Cargo.toml"

  run_cli_smoke
}

run_cli_smoke() {
  local tmpdir
  local generated_dir=""
  tmpdir="$(mktemp -d)"
  trap 'rm -rf -- "${tmpdir-}" "${generated_dir-}"' RETURN

  mkdir -p "$tmpdir/src"
  cat > "$tmpdir/daram.toml" <<'EOF'
[package]
name = "release-smoke"
version = "1.0.0"
EOF

  mkdir -p "$tmpdir/.drpm/expanded/greeter/1.0.0"
  mkdir -p "$tmpdir/.drpm/metadata/greeter/1.0.0"

  cat > "$tmpdir/dr.lock" <<'EOF'
[[package]]
name = "release-smoke"
version = "1.0.0"
dependencies = ["greeter 1.0.0"]
dev_dependencies = []

[[package]]
name = "greeter"
version = "1.0.0"
source = "registry+https://registry.example.test"
checksum = "sha256:deadbeef"
registry_url = "https://registry.example.test"
signing_key_id = "release-gate"
dependencies = []
EOF

  cat > "$tmpdir/.drpm/expanded/greeter/1.0.0/main.dr" <<'EOF'
import { answer_impl } from "./util";

export fun answer(): i32 {
    answer_impl()
}
EOF

  cat > "$tmpdir/.drpm/expanded/greeter/1.0.0/util.dr" <<'EOF'
export fun answer_impl(): i32 {
    42
}
EOF

  cat > "$tmpdir/.drpm/metadata/greeter/1.0.0/manifest.json" <<'EOF'
{
  "name": "greeter",
  "version": "1.0.0",
  "module_name": "greeter",
  "main": "main.dr",
  "source_files": [
    "main.dr",
    "util.dr"
  ],
  "dependencies": []
}
EOF

  cat > "$tmpdir/src/main.dr" <<'EOF'
/// release smoke binary
fun double(value: i32): i32 {
    value * 2
}

fun main(): i32 {
    println("release gate ok");
    double(21)
}
EOF

  log "running CLI smoke checks in $tmpdir"
  (
    cd "$tmpdir"
    cargo run -q --manifest-path "$ROOT_DIR/cli/Cargo.toml" -- lint
    cargo run -q --manifest-path "$ROOT_DIR/cli/Cargo.toml" -- doc
    cat > "$tmpdir/src/main.dr" <<'EOF'
/// release smoke binary
import { answer } from greeter;

fun double(value: i32): i32 {
    value * 2
}

fun main(): i32 {
    println("release gate ok");
    double(answer() / 2)
}
EOF
    cargo run -q --manifest-path "$ROOT_DIR/cli/Cargo.toml" -- check
    cargo run -q --manifest-path "$ROOT_DIR/cli/Cargo.toml" -- build
    cat > "$tmpdir/src/smoke_test.dr" <<'EOF'
import { answer } from greeter;

#[test]
fun doubles_numbers() {
    assert_eq(answer(), 42);
}

#[test]
fun imported_dependency_feeds_local_logic() {
    assert_eq(answer() / 2, 21);
}
EOF
    cargo run -q --manifest-path "$ROOT_DIR/cli/Cargo.toml" -- test --verbose
    local binary="./target/debug/release-smoke"
    if [[ ! -x "$binary" ]]; then
      printf 'expected native artifact %s to exist\n' "$binary" >&2
      exit 1
    fi
    local output
    local status
    set +e
    output="$("$binary")"
    status=$?
    set -e
    if [[ "$output" != "release gate ok" ]]; then
      printf 'unexpected binary output: %s\n' "$output" >&2
      exit 1
    fi
    if [[ "$status" -ne 42 ]]; then
      printf 'unexpected binary exit code: %s\n' "$status" >&2
      exit 1
    fi
    if [[ ! -f "$tmpdir/docs/index.html" ]]; then
      printf 'documentation output missing: %s\n' "$tmpdir/docs/index.html" >&2
      exit 1
    fi
  )

  generated_dir="$(mktemp -d)"

  log "verifying \`dr new\` generated project build in $generated_dir"
  (
    cd "$generated_dir"
    cargo run -q --manifest-path "$ROOT_DIR/cli/Cargo.toml" -- new release-template >/dev/null
    cd "$generated_dir/release-template"
    cargo run -q --manifest-path "$ROOT_DIR/cli/Cargo.toml" -- check
    cargo run -q --manifest-path "$ROOT_DIR/cli/Cargo.toml" -- build
  )
}

case "$MODE" in
  workspace)
    run_workspace_gate
    ;;
  all)
    run_workspace_gate
    ;;
  *)
    printf 'usage: %s [workspace|all]\n' "${BASH_SOURCE[0]}" >&2
    exit 1
    ;;
esac
