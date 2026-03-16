#!/bin/sh
set -eu

REPO="${DR_REPO:-flyingsquirrel0419/daram-stable}"
INSTALL_DIR="${DR_INSTALL_DIR:-$HOME/.local/bin}"
REQUESTED_VERSION="${DR_VERSION:-}"

log() {
  printf '[daram-install] %s\n' "$*"
}

fail() {
  printf '[daram-install] error: %s\n' "$*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

normalize_version() {
  version="$1"
  if [ -z "$version" ]; then
    printf 'latest'
  elif printf '%s' "$version" | grep -q '^v'; then
    printf '%s' "$version"
  else
    printf 'v%s' "$version"
  fi
}

resolve_tag() {
  version="$1"
  normalized="$(normalize_version "$version")"
  if [ "$normalized" != "latest" ]; then
    printf '%s' "$normalized"
    return
  fi

  json="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | tr -d '\n')"
  tag="$(printf '%s' "$json" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
  [ -n "$tag" ] || fail "failed to resolve latest release tag"
  printf '%s' "$tag"
}

asset_version() {
  tag="$1"
  printf '%s' "${tag#v}"
}

detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Linux) os_part="unknown-linux-gnu" ;;
    Darwin) os_part="apple-darwin" ;;
    *)
      fail "unsupported operating system: $os"
      ;;
  esac

  case "$arch" in
    x86_64|amd64) arch_part="x86_64" ;;
    arm64|aarch64)
      if [ "$os" = "Darwin" ]; then
        arch_part="aarch64"
      else
        fail "unsupported architecture on $os: $arch"
      fi
      ;;
    *)
      fail "unsupported architecture: $arch"
      ;;
  esac

  printf '%s-%s' "$arch_part" "$os_part"
}

download_url() {
  tag="$1"
  asset="$2"
  if [ "$tag" = "latest" ]; then
    printf 'https://github.com/%s/releases/latest/download/%s' "$REPO" "$asset"
  else
    printf 'https://github.com/%s/releases/download/%s/%s' "$REPO" "$tag" "$asset"
  fi
}

checksum_cmd() {
  if command -v sha256sum >/dev/null 2>&1; then
    printf 'sha256sum'
  elif command -v shasum >/dev/null 2>&1; then
    printf 'shasum -a 256'
  else
    fail "required command not found: sha256sum or shasum"
  fi
}

verify_checksum() {
  sums_file="$1"
  asset_name="$2"
  asset_path="$3"
  expected="$(awk -v asset="$asset_name" '$2 == asset { print $1; exit }' "$sums_file")"
  [ -n "$expected" ] || fail "checksum entry not found for $asset_name"

  cmd="$(checksum_cmd)"
  actual="$($cmd "$asset_path" | awk '{print $1}')"
  [ "$expected" = "$actual" ] || fail "checksum mismatch for $asset_name"
}

main() {
  need_cmd curl
  need_cmd tar
  need_cmd mktemp
  need_cmd uname
  need_cmd grep
  need_cmd awk
  need_cmd chmod
  need_cmd cp
  need_cmd mkdir
  need_cmd rm
  need_cmd sed
  need_cmd tr

  target="$(detect_target)"
  tag="$(resolve_tag "$REQUESTED_VERSION")"
  version="$(asset_version "$tag")"
  asset="dr-${version}-${target}.tar.gz"

  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT INT TERM

  asset_path="$tmpdir/$asset"
  sums_path="$tmpdir/SHA256SUMS"

  log "downloading $asset"
  curl -fsSL "$(download_url "$tag" "$asset")" -o "$asset_path"
  log "downloading SHA256SUMS"
  curl -fsSL "$(download_url "$tag" "SHA256SUMS")" -o "$sums_path"
  verify_checksum "$sums_path" "$asset" "$asset_path"

  extract_dir="$tmpdir/extract"
  mkdir -p "$extract_dir"
  tar -xzf "$asset_path" -C "$extract_dir"

  mkdir -p "$INSTALL_DIR"
  install_path="$INSTALL_DIR/dr"
  cp "$extract_dir/dr" "$install_path"
  chmod +x "$install_path"

  log "installed dr to $install_path"
  case ":$PATH:" in
    *":$INSTALL_DIR:"*)
      log "run 'dr --version' to verify the installation"
      ;;
    *)
      log "add $INSTALL_DIR to PATH, then run 'dr --version'"
      ;;
  esac
  log "Rust is not required to use dr; native builds may require a system C compiler"
}

main "$@"
