#!/usr/bin/env bash

set -euo pipefail

REPO="nvrmnd-png/coinfetch"
BIN_NAME="coinfetch"
INSTALL_DIR="${COINFETCH_PREFIX:-$HOME/.local/bin}"
BINARY="$INSTALL_DIR/$BIN_NAME"
STRATEGY="${COINFETCH_SOURCE:-}"

VERSION_URL="https://raw.githubusercontent.com/$REPO/master/VERSION"
RELEASES_API="https://api.github.com/repos/$REPO/releases/latest"
REPO_URL="https://github.com/$REPO.git"

log()   { printf '\033[36m>>>\033[0m %s\n' "$*"; }
warn()  { printf '\033[33m!!!\033[0m %s\n' "$*" >&2; }
die()   { printf '\033[31mXXX\033[0m %s\n' "$*" >&2; exit 1; }

require() { command -v "$1" >/dev/null 2>&1 || die "'$1' is required"; }

installed_version() {
  if [ -x "$BINARY" ]; then
    "$BINARY" --version 2>/dev/null | awk '{print $NF}'
  else
    echo "none"
  fi
}

remote_version() {
  require curl
  curl -fsSL "$VERSION_URL" | head -n1 | tr -d '[:space:]'
}

install_from_source() {
  require cargo
  local src="$1"
  log "Building from source in $src"
  ( cd "$src" && cargo install --path . --root "$(dirname "$INSTALL_DIR")" --locked )
}

install_from_release() {
  require curl
  local os arch asset_url
  os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  arch="$(uname -m)"
  asset_url=$(curl -fsSL "$RELEASES_API" \
    | grep -oE '"browser_download_url":[[:space:]]*"[^"]+"' \
    | sed -E 's/.*"(https[^"]+)".*/\1/' \
    | grep -iE "$os.*$arch|$arch.*$os" | head -n1) || true
  if [ -z "${asset_url:-}" ]; then
    return 1
  fi
  log "Downloading $asset_url"
  mkdir -p "$INSTALL_DIR"
  curl -fsSL "$asset_url" -o "$BINARY.tmp"
  chmod +x "$BINARY.tmp"
  mv "$BINARY.tmp" "$BINARY"
}

install_via_clone() {
  require git
  require cargo
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  log "Cloning $REPO_URL"
  git clone --depth 1 "$REPO_URL" "$tmp"
  install_from_source "$tmp"
}

is_local_source() {
  [ -f "Cargo.toml" ] && grep -q '^name = "coinfetch"' Cargo.toml
}

choose_strategy() {
  if [ -n "$STRATEGY" ]; then return; fi
  if ! { exec 3</dev/tty; } 2>/dev/null; then
    warn "no controlling tty, defaulting to prebuilt release"
    STRATEGY="release"
    return
  fi
  printf '\nHow would you like to install coinfetch?\n'
  printf '  1) build from source  (needs cargo, rust 1.88+)\n'
  printf '  2) prebuilt release   (downloads a binary from GitHub, recommended)\n'
  printf 'Choice [1/2, default 2]: '
  local answer=""
  IFS= read -r answer <&3 || true
  exec 3<&-
  case "$answer" in
    1|s|source)   STRATEGY="source" ;;
    ""|2|r|release) STRATEGY="release" ;;
    *) die "invalid choice: $answer" ;;
  esac
  log "using strategy: $STRATEGY"
}

do_install() {
  choose_strategy
  mkdir -p "$INSTALL_DIR"

  case "$STRATEGY" in
    source)
      if is_local_source; then install_from_source .
      else install_via_clone
      fi
      ;;
    release)
      install_from_release || die "no release asset found for $(uname -s)/$(uname -m)"
      ;;
    auto)
      if is_local_source; then
        install_from_source .
      elif install_from_release; then
        :
      else
        log "no release asset yet, falling back to source build"
        install_via_clone
      fi
      ;;
    *)
      die "COINFETCH_SOURCE must be one of: source, release, auto"
      ;;
  esac

  case ":$PATH:" in
    *":$INSTALL_DIR:"*) : ;;
    *) warn "$INSTALL_DIR is not on \$PATH, add it to your shell rc" ;;
  esac

  log "Installed $("$BINARY" --version) at $BINARY"
}

do_update() {
  local have want
  have="$(installed_version)"
  want="$(remote_version)"
  echo "installed: $have"
  echo "latest:    $want"
  if [ "$have" = "$want" ]; then
    log "Already up to date"
    return
  fi
  log "Updating $have -> $want"
  do_install
}

uninstall_paths() {
  local os
  os="$(uname -s)"
  if [ "$os" = "Darwin" ]; then
    printf '%s\n' \
      "$HOME/Library/Application Support/coinfetch" \
      "$HOME/Library/Caches/coinfetch" \
      "$HOME/Library/Preferences/coinfetch"
  else
    printf '%s\n' \
      "${XDG_CONFIG_HOME:-$HOME/.config}/coinfetch" \
      "${XDG_CACHE_HOME:-$HOME/.cache}/coinfetch" \
      "${XDG_DATA_HOME:-$HOME/.local/share}/coinfetch"
  fi
}

clear_keyring() {
  if command -v secret-tool >/dev/null 2>&1; then
    if secret-tool clear service coinfetch account coingecko-api-key 2>/dev/null; then
      log "Cleared keyring entry (secret-tool)"
    fi
  fi
  if command -v security >/dev/null 2>&1; then
    if security delete-generic-password -s coinfetch -a coingecko-api-key >/dev/null 2>&1; then
      log "Cleared keychain entry (security)"
    fi
  fi
}

do_uninstall() {
  local paths=()
  while IFS= read -r p; do paths+=("$p"); done < <(uninstall_paths)

  echo "This will remove:"
  echo "  binary : $BINARY"
  for p in "${paths[@]}"; do
    if [ -e "$p" ]; then echo "  data   : $p"; fi
  done
  echo "  keyring: service=coinfetch account=coingecko-api-key (if present)"

  if [ "${COINFETCH_UNINSTALL_YES:-}" != "1" ]; then
    if ! { exec 3</dev/tty; } 2>/dev/null; then
      die "non-interactive uninstall requires COINFETCH_UNINSTALL_YES=1"
    fi
    printf 'Proceed? [y/N]: '
    local ans=""
    IFS= read -r ans <&3 || true
    exec 3<&-
    case "$ans" in
      y|Y|yes|YES) : ;;
      *) log "Aborted."; return ;;
    esac
  fi

  if [ -f "$BINARY" ]; then
    rm -f "$BINARY" && log "Removed $BINARY"
  else
    log "No binary at $BINARY"
  fi

  for p in "${paths[@]}"; do
    if [ -e "$p" ]; then
      rm -rf -- "$p" && log "Removed $p"
    fi
  done

  clear_keyring
  log "Uninstall complete."
}

do_check() {
  echo "installed: $(installed_version)"
  echo "latest:    $(remote_version 2>/dev/null || echo 'unavailable')"
}

usage() {
  cat <<EOF
Usage: bash install.sh <command>

Commands:
  install     Install coinfetch (asks: build from source or prebuilt release)
  update      Reinstall if the remote VERSION is newer than the local one
  uninstall   Remove binary, config, cache, data and keyring entry
  check       Print installed and latest published version

Environment:
  COINFETCH_PREFIX          Install directory (default: ~/.local/bin)
  COINFETCH_SOURCE          Skip the prompt: source | release | auto
  COINFETCH_UNINSTALL_YES=1 Skip the uninstall confirmation prompt
EOF
}

case "${1:-help}" in
  install)              do_install ;;
  update)               do_update ;;
  uninstall|remove)     do_uninstall ;;
  check|status)         do_check ;;
  help|--help|-h|"")    usage ;;
  *) usage; exit 1 ;;
esac
