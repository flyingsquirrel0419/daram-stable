#!/bin/sh
set -eu

REPO="${DR_REPO:-flyingsquirrel0419/daram-stable}"
REQUESTED_VERSION="${DR_VERSION:-}"
REQUESTED_INSTALL_DIR="${DR_INSTALL_DIR:-}"
TRUSTED_SIGNING_KEY_ID="${DRPM_TRUSTED_SIGNING_KEY_ID:-local-dev}"
DEFAULT_TRUSTED_SIGNING_PUBLIC_KEY_PEM='-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEA6yOVMh5UY+KH9Y5Y/Tu2i93a2Lmdsn8/+odW8qCPs8w=\n-----END PUBLIC KEY-----'
TRUSTED_SIGNING_PUBLIC_KEY_PEM="${DRPM_TRUSTED_SIGNING_PUBLIC_KEY_PEM:-$DEFAULT_TRUSTED_SIGNING_PUBLIC_KEY_PEM}"

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

path_contains_dir() {
  dir="$1"
  case ":$PATH:" in
    *":$dir:"*) return 0 ;;
    *) return 1 ;;
  esac
}

ensure_dir() {
  dir="$1"
  if [ -d "$dir" ]; then
    [ -w "$dir" ]
    return
  fi

  mkdir -p "$dir" 2>/dev/null || return 1
  [ -w "$dir" ]
}

choose_install_dir() {
  if [ -n "$REQUESTED_INSTALL_DIR" ]; then
    printf '%s' "$REQUESTED_INSTALL_DIR"
    return
  fi

  for candidate in "$HOME/.local/bin" "$HOME/bin" "$HOME/.cargo/bin" "/usr/local/bin"; do
    if path_contains_dir "$candidate" && ensure_dir "$candidate"; then
      printf '%s' "$candidate"
      return
    fi
  done

  printf '%s' "$HOME/.local/bin"
}

profile_targets() {
  shell_name="${SHELL##*/}"
  if [ -z "$shell_name" ]; then
    shell_name="sh"
  fi

  printf '%s\n' "$HOME/.profile"

  if [ "$shell_name" = "bash" ] || [ -f "$HOME/.bashrc" ]; then
    printf '%s\n' "$HOME/.bashrc"
  fi
  if [ -f "$HOME/.bash_profile" ]; then
    printf '%s\n' "$HOME/.bash_profile"
  elif [ -f "$HOME/.bash_login" ]; then
    printf '%s\n' "$HOME/.bash_login"
  elif [ "$shell_name" = "bash" ]; then
    printf '%s\n' "$HOME/.bash_profile"
  fi
  if [ "$shell_name" = "zsh" ] || [ -f "$HOME/.zshrc" ]; then
    printf '%s\n' "$HOME/.zshrc"
  fi
  if [ -f "$HOME/.zprofile" ] || [ "$shell_name" = "zsh" ]; then
    printf '%s\n' "$HOME/.zprofile"
  fi
}

persist_path_update() {
  install_dir="$1"
  export_line="export PATH=\"$install_dir:\$PATH\""
  added_profiles=""

  for profile in $(profile_targets); do
    [ -n "$profile" ] || continue
    if [ -f "$profile" ] && grep -F "$export_line" "$profile" >/dev/null 2>&1; then
      continue
    fi

    {
      printf '\n# added by daram installer\n'
      printf '%s\n' "$export_line"
    } >> "$profile" || fail "failed to update shell profile: $profile"
    if [ -z "$added_profiles" ]; then
      added_profiles="$profile"
    else
      added_profiles="$added_profiles, $profile"
    fi
  done

  printf '%s' "$added_profiles"
}

persist_registry_trust() {
  key_id_line="export DRPM_TRUSTED_SIGNING_KEY_ID=\"$TRUSTED_SIGNING_KEY_ID\""
  key_pem_line="export DRPM_TRUSTED_SIGNING_PUBLIC_KEY_PEM=\"$TRUSTED_SIGNING_PUBLIC_KEY_PEM\""
  added_profiles=""

  for profile in $(profile_targets); do
    [ -n "$profile" ] || continue
    needs_update=0
    if [ ! -f "$profile" ] || ! grep -F "$key_id_line" "$profile" >/dev/null 2>&1; then
      needs_update=1
    fi
    if [ ! -f "$profile" ] || ! grep -F "DRPM_TRUSTED_SIGNING_PUBLIC_KEY_PEM=" "$profile" >/dev/null 2>&1; then
      needs_update=1
    fi
    [ "$needs_update" -eq 1 ] || continue

    {
      printf '\n# added by daram installer\n'
      printf '%s\n' "$key_id_line"
      printf '%s\n' "$key_pem_line"
    } >> "$profile" || fail "failed to update shell profile: $profile"
    if [ -z "$added_profiles" ]; then
      added_profiles="$profile"
    else
      added_profiles="$added_profiles, $profile"
    fi
  done

  printf '%s' "$added_profiles"
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

  INSTALL_DIR="$(choose_install_dir)"
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
  if version_output="$("$install_path" --version 2>&1)"; then
    log "verified binary: $version_output"
  else
    fail "installed binary failed to execute: $version_output"
  fi

  if path_contains_dir "$INSTALL_DIR"; then
    log "run 'dr --version' to verify the installation"
  else
    updated_profiles="$(persist_path_update "$INSTALL_DIR")"
    if [ -n "$updated_profiles" ]; then
      log "added $INSTALL_DIR to your shell profiles: $updated_profiles"
    fi
    log "restart your shell, or run: export PATH=\"$INSTALL_DIR:\$PATH\""
    log "then run 'dr --version'"
  fi
  trust_profiles="$(persist_registry_trust)"
  if [ -n "$trust_profiles" ]; then
    log "configured trusted registry signing key in: $trust_profiles"
  fi
  log "Rust is not required to use dr; native builds may require a system C compiler"
}

main "$@"
