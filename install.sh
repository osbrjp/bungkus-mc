#!/usr/bin/env bash
# Installs bungkus-mc from a GitHub release.
#
# bungkus-cli's install.sh with REPO/BIN_NAME changed (ARCHITECTURE §11).
# The repository is private, so the release is read with the user's
# authenticated gh CLI; plain curl is used only when gh is missing and the
# repository is public. The binary is verified against the release's
# checksums.txt before it is installed.
set -euo pipefail

REPO="osbrjp/bungkus-mc"
BIN_NAME="bungkus-mc"
SHORT_NAME="bkmc"
INSTALL_DIR="${BUNGKUS_INSTALL_DIR:-/usr/local/bin}"

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

have_gh() {
  command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1
}

resolve_tag() {
  if [ -n "${BUNGKUS_MC_VERSION:-}" ]; then
    echo "$BUNGKUS_MC_VERSION"
  elif have_gh; then
    gh release view --repo "$REPO" --json tagName --jq .tagName
  else
    curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
      | grep -m1 '"tag_name":' \
      | sed -E 's/.*"tag_name":[[:space:]]*"([^"]+)".*/\1/'
  fi
}

download() {
  local tag="$1" asset="$2" dir="$3"
  if have_gh; then
    gh release download "$tag" --repo "$REPO" --pattern "$asset" --pattern checksums.txt --dir "$dir"
  else
    curl -fsSL "https://github.com/${REPO}/releases/download/${tag}/${asset}" -o "${dir}/${asset}"
    curl -fsSL "https://github.com/${REPO}/releases/download/${tag}/checksums.txt" -o "${dir}/checksums.txt"
  fi
}

install_file() {
  local src="$1" dest="$2"
  if [ -w "$INSTALL_DIR" ] || { [ ! -e "$INSTALL_DIR" ] && mkdir -p "$INSTALL_DIR" 2>/dev/null; }; then
    mv "$src" "$dest"
  else
    log "writing to ${INSTALL_DIR} requires sudo"
    sudo mv "$src" "$dest"
  fi
}

link_short_name() {
  local dest="$1" link="${INSTALL_DIR}/${SHORT_NAME}"
  if command -v "$SHORT_NAME" >/dev/null 2>&1 || [ -e "$link" ]; then
    log "${SHORT_NAME} already exists; add an alias instead: alias ${SHORT_NAME}=${BIN_NAME}"
    return
  fi
  if [ -w "$INSTALL_DIR" ]; then
    ln -s "$dest" "$link"
  else
    sudo ln -s "$dest" "$link"
  fi
  log "short command: ${SHORT_NAME}"
}

main() {
  command -v uname >/dev/null 2>&1 || err "required command not found: uname"
  have_gh || command -v curl >/dev/null 2>&1 || err "install gh (and run gh auth login) or curl"

  local os arch tag asset tmp expected actual dest
  os=$(detect_os)
  arch=$(detect_arch)
  tag=$(resolve_tag)
  [ -n "$tag" ] || err "could not resolve the release (is gh logged in to an account that can see ${REPO}?)"

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
}

main "$@"
