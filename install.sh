#!/usr/bin/env bash
# Installs bungkus-mc from a GitHub release.
#
# bungkus-cli's install.sh with REPO/BIN_NAME changed (ARCHITECTURE §11):
#   curl -fsSL https://raw.githubusercontent.com/osbrjp/bungkus-mc/main/install.sh | bash
# It reads the latest release from the public GitHub API, downloads the
# binary and checksums.txt with curl and verifies the binary before it is
# installed. BUNGKUS_MC_VERSION picks a release (`bungkus-mc update` sets it).
#
# Never requires sudo for a fresh install (like uv, rustup, bun, deno and
# Claude Code). The folder is, in order:
#   1. BUNGKUS_INSTALL_DIR, if set;
#   2. the folder of the existing binary: BUNGKUS_CURRENT_BIN (passed by
#      `bungkus-mc update`), else `command -v bungkus-mc`, symlinks resolved;
#   3. ~/.local/bin.
# If that folder is not writable and ~/.local/bin comes earlier on PATH, the
# new copy goes to ~/.local/bin (it shadows the old one); otherwise sudo is
# used in place. The folder logic lives here, so older clients get it on
# their next update. BUNGKUS_INSTALL_DRY_RUN=1 prints the decision and
# exits before any network call.
set -euo pipefail

REPO="osbrjp/bungkus-mc"
BIN_NAME="bungkus-mc"
SHORT_NAME="bkmc"
LOCAL_BIN="${HOME}/.local/bin"
INSTALL_DIR=""
USE_SUDO=0
SHADOWED=""

err() { printf 'install: %s\n' "$*" >&2; exit 1; }
log() { printf '==> %s\n' "$*"; }

detect_os() {
  case "$(uname -s)" in
    Darwin) echo darwin ;;
    Linux)  echo linux ;;
    *) err "unsupported OS: $(uname -s). ${BIN_NAME} supports darwin and linux." ;;
  esac
}

detect_arch() {
  case "$(uname -m)" in
    arm64|aarch64) echo arm64 ;;
    x86_64|amd64)  echo amd64 ;;
    *) err "unsupported architecture: $(uname -m). ${BIN_NAME} supports arm64 and amd64." ;;
  esac
}

sha256_of() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    err "neither shasum nor sha256sum found; cannot verify download"
  fi
}

resolve_tag() {
  if [ -n "${BUNGKUS_MC_VERSION:-}" ]; then
    echo "$BUNGKUS_MC_VERSION"
  else
    curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
      | grep -m1 '"tag_name":' \
      | sed -E 's/.*"tag_name":[[:space:]]*"([^"]+)".*/\1/'
  fi
}

download() {
  local tag="$1" asset="$2" dir="$3"
  curl -fSL "https://github.com/${REPO}/releases/download/${tag}/${asset}" -o "${dir}/${asset}"
  curl -fSL "https://github.com/${REPO}/releases/download/${tag}/checksums.txt" -o "${dir}/checksums.txt"
}

# Prints $1 with every symlink in its last component resolved.
resolve_link() {
  local p="$1" t
  while [ -L "$p" ]; do
    t=$(readlink "$p")
    case "$t" in
      /*) p="$t" ;;
      *) p="$(dirname "$p")/$t" ;;
    esac
  done
  printf '%s\n' "$p"
}

# Prints the physical path of folder $1, or $1 without a trailing slash
# when it does not exist (so ~/.local/bin/ on PATH still matches).
physical() {
  (cd "$1" 2>/dev/null && pwd -P) || printf '%s\n' "${1%/}"
}

# Succeeds when folder $1 is writable, or does not exist yet and its nearest
# existing parent is writable.
writable() {
  local d="$1"
  while [ ! -e "$d" ]; do
    d=$(dirname "$d")
  done
  [ -d "$d" ] && [ -w "$d" ]
}

# Succeeds when ~/.local/bin comes before folder $1 on PATH.
local_bin_first() {
  local want there entry
  want=$(physical "$LOCAL_BIN")
  there=$(physical "$1")
  local IFS=:
  for entry in $PATH; do
    [ -n "$entry" ] || continue
    entry=$(physical "$entry")
    [ "$entry" = "$want" ] && return 0
    [ "$entry" = "$there" ] && return 1
  done
  return 1
}

# Sets INSTALL_DIR, USE_SUDO and SHADOWED (see the header).
choose_dir() {
  local current=""
  if [ -n "${BUNGKUS_INSTALL_DIR:-}" ]; then
    INSTALL_DIR="$BUNGKUS_INSTALL_DIR"
  else
    current="${BUNGKUS_CURRENT_BIN:-$(command -v "$BIN_NAME" 2>/dev/null || true)}"
    if [ -n "$current" ] && [ -e "$current" ]; then
      INSTALL_DIR=$(physical "$(dirname "$(resolve_link "$current")")")
    else
      INSTALL_DIR="$LOCAL_BIN"
    fi
  fi
  writable "$INSTALL_DIR" && return
  if [ -z "${BUNGKUS_INSTALL_DIR:-}" ] && local_bin_first "$INSTALL_DIR" && writable "$LOCAL_BIN"; then
    SHADOWED="${INSTALL_DIR}/${BIN_NAME}"
    INSTALL_DIR="$LOCAL_BIN"
  else
    USE_SUDO=1
  fi
}

# Runs "$@" with sudo when the folder needs it.
as_needed() {
  if [ "$USE_SUDO" = 1 ]; then
    sudo "$@"
  else
    "$@"
  fi
}

install_file() {
  local src="$1" dest="$2"
  [ "$USE_SUDO" = 1 ] && log "writing to ${INSTALL_DIR} requires sudo"
  as_needed mkdir -p "$INSTALL_DIR"
  as_needed mv "$src" "$dest"
}

link_short_name() {
  local dest="$1" link="${INSTALL_DIR}/${SHORT_NAME}"
  if command -v "$SHORT_NAME" >/dev/null 2>&1 || [ -e "$link" ]; then
    log "${SHORT_NAME} already exists; add an alias instead: alias ${SHORT_NAME}=${BIN_NAME}"
    return
  fi
  as_needed ln -s "$dest" "$link"
  log "short command: ${SHORT_NAME}"
}

# Prints how to put INSTALL_DIR on PATH when it is not there.
path_hint() {
  local entry want
  want=$(physical "$INSTALL_DIR")
  local IFS=:
  for entry in $PATH; do
    [ "$(physical "$entry")" = "$want" ] && return
  done
  log "${INSTALL_DIR} is not on your PATH; add it:"
  printf '    fish: fish_add_path %s\n' "$INSTALL_DIR"
  printf '    zsh:  echo '\''export PATH="%s:$PATH"'\'' >> ~/.zshrc\n' "$INSTALL_DIR"
  printf '    bash: echo '\''export PATH="%s:$PATH"'\'' >> ~/.bashrc\n' "$INSTALL_DIR"
}

main() {
  command -v uname >/dev/null 2>&1 || err "required command not found: uname"
  choose_dir
  if [ -n "${BUNGKUS_INSTALL_DRY_RUN:-}" ]; then
    printf 'dir=%s\nsudo=%s\nshadowed=%s\n' "$INSTALL_DIR" "$USE_SUDO" "$SHADOWED"
    exit 0
  fi
  command -v curl >/dev/null 2>&1 || err "required command not found: curl"

  local os arch tag asset tmp expected actual dest
  os=$(detect_os)
  arch=$(detect_arch)
  tag=$(resolve_tag)
  [ -n "$tag" ] || err "could not resolve the latest release (is there a published release?)"

  asset="${BIN_NAME}-${os}-${arch}"
  tmp=$(mktemp -d -t "${BIN_NAME}.XXXXXX")
  trap 'rm -rf "$tmp"' EXIT
  log "installing ${BIN_NAME} ${tag} (${os}/${arch})"
  download "$tag" "$asset" "$tmp"

  log "verifying checksum"
  expected=$(awk -v f="$asset" '$2==f {print $1}' "${tmp}/checksums.txt")
  [ -n "$expected" ] || err "no checksum entry for ${asset} in checksums.txt"
  actual=$(sha256_of "${tmp}/${asset}")
  [ "$expected" = "$actual" ] || err "checksum mismatch for ${asset} (expected ${expected}, got ${actual})"

  chmod +x "${tmp}/${asset}"
  if [ "$os" = "darwin" ]; then
    xattr -d com.apple.quarantine "${tmp}/${asset}" 2>/dev/null || true
  fi
  dest="${INSTALL_DIR}/${BIN_NAME}"
  install_file "${tmp}/${asset}" "$dest"
  link_short_name "$dest"
  log "installed ${BIN_NAME} ${tag} -> ${dest}"
  if [ -n "$SHADOWED" ]; then
    log "old copy at ${SHADOWED} is now shadowed — remove it with: sudo rm ${SHADOWED}"
  fi
  path_hint
}

main "$@"
