#!/bin/sh
# meow installer — https://meow.style/install
#
#   curl -fsSL https://meow.style/install | sh
#
# Downloads the prebuilt `meow` binary for your platform from the latest GitHub
# release, installs it to ~/.meow/bin (relocatable via MEOW_HOME), verifies its
# SHA-256, and adds it to your PATH. No sudo, no system files touched.
#
# Env knobs:
#   MEOW_HOME     install root (default: $HOME/.meow)
#   MEOW_VERSION  release tag to install, e.g. v0.1.0 (default: latest)
#   MEOW_NO_MODIFY_PATH=1  install the binary but do not edit shell profiles
set -eu

REPO="0xchasercat/meow"
MEOW_HOME="${MEOW_HOME:-$HOME/.meow}"
BIN_DIR="$MEOW_HOME/bin"
VERSION="${MEOW_VERSION:-latest}"

# --- tiny UX (the "floof"): colored only on a TTY --------------------------
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  C_PINK='\033[38;5;218m'; C_MINT='\033[38;5;121m'; C_DIM='\033[2m'; C_RED='\033[31m'; C_OFF='\033[0m'
else
  C_PINK=''; C_MINT=''; C_DIM=''; C_RED=''; C_OFF=''
fi
purr()  { printf "%b🐾 %s%b\n" "$C_MINT" "$1" "$C_OFF"; }
pounce(){ printf "%b   %s%b\n" "$C_DIM"  "$1" "$C_OFF"; }
hiss()  { printf "%b✗ %s%b\n"  "$C_RED"  "$1" "$C_OFF" >&2; }
die()   { hiss "$1"; exit 1; }

# --- detect platform -> release target triple ------------------------------
detect_target() {
  _os="$(uname -s)"
  _arch="$(uname -m)"
  case "$_os" in
    Linux)  _os_part="unknown-linux-gnu" ;;
    Darwin) _os_part="apple-darwin" ;;
    *) die "unsupported OS: $_os. On Windows use the PowerShell installer or grab a release zip from https://github.com/$REPO/releases." ;;
  esac
  case "$_arch" in
    x86_64|amd64)   _arch_part="x86_64" ;;
    arm64|aarch64)  _arch_part="aarch64" ;;
    *) die "unsupported architecture: $_arch" ;;
  esac
  TARGET="${_arch_part}-${_os_part}"
}

# --- download helper (curl or wget) ----------------------------------------
fetch() { # fetch <url> <dest>
  if command -v curl >/dev/null 2>&1; then
    curl -fSL "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$2" "$1"
  else
    die "need either curl or wget installed"
  fi
}

verify_checksum() { # verify_checksum <file> <sha256_file>
  [ -f "$2" ] || { pounce "no checksum published; skipping verification"; return 0; }
  _expected="$(awk '{print $1}' "$2" 2>/dev/null | head -n1)"
  [ -n "$_expected" ] || return 0
  if command -v sha256sum >/dev/null 2>&1; then
    _actual="$(sha256sum "$1" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    _actual="$(shasum -a 256 "$1" | awk '{print $1}')"
  else
    pounce "no sha256 tool found; skipping verification"; return 0
  fi
  [ "$_expected" = "$_actual" ] || die "checksum mismatch (expected $_expected, got $_actual) — aborting"
  pounce "checksum verified"
}

# --- PATH wiring -----------------------------------------------------------
add_to_path() {
  case ":${PATH}:" in *":$BIN_DIR:"*) return 0 ;; esac
  [ "${MEOW_NO_MODIFY_PATH:-0}" = "1" ] && return 0

  _line="export PATH=\"$BIN_DIR:\$PATH\""
  _shell_name="$(basename "${SHELL:-sh}")"
  _profiles=""
  case "$_shell_name" in
    zsh)  _profiles="${ZDOTDIR:-$HOME}/.zshrc" ;;
    bash) _profiles="$HOME/.bashrc $HOME/.bash_profile" ;;
    fish) _profiles="$HOME/.config/fish/config.fish"; _line="set -gx PATH $BIN_DIR \$PATH" ;;
    *)    _profiles="$HOME/.profile" ;;
  esac

  _wrote=""
  for _p in $_profiles; do
    [ -e "$_p" ] || { [ "$_shell_name" = "fish" ] && mkdir -p "$(dirname "$_p")" 2>/dev/null || true; }
    if [ -w "$_p" ] || [ ! -e "$_p" ]; then
      if ! { [ -f "$_p" ] && grep -Fq "$BIN_DIR" "$_p"; }; then
        printf '\n# meow\n%s\n' "$_line" >> "$_p" 2>/dev/null && _wrote="$_p"
      else
        _wrote="$_p"
      fi
      break
    fi
  done

  if [ -n "$_wrote" ]; then
    pounce "added $BIN_DIR to PATH in $_wrote"
    PATH_HINT="Restart your shell or run:  export PATH=\"$BIN_DIR:\$PATH\""
  else
    PATH_HINT="Add this to your shell profile:  export PATH=\"$BIN_DIR:\$PATH\""
  fi
}

main() {
  detect_target

  if [ "$VERSION" = "latest" ]; then
    _base="https://github.com/$REPO/releases/latest/download"
  else
    _base="https://github.com/$REPO/releases/download/$VERSION"
  fi
  _asset="meow-${TARGET}.tar.gz"
  _url="$_base/$_asset"

  purr "Installing meow ($TARGET, $VERSION)"

  _tmp="$(mktemp -d 2>/dev/null || mktemp -d -t meow)"
  trap 'rm -rf "$_tmp"' EXIT INT TERM

  pounce "downloading $_asset"
  fetch "$_url" "$_tmp/$_asset" || die "download failed: $_url (is there a published release for $TARGET?)"
  fetch "$_url.sha256" "$_tmp/$_asset.sha256" 2>/dev/null || true
  verify_checksum "$_tmp/$_asset" "$_tmp/$_asset.sha256"

  pounce "unpacking"
  tar -xzf "$_tmp/$_asset" -C "$_tmp" || die "failed to extract $_asset"

  _bin="$_tmp/meow-${TARGET}/meow"
  [ -f "$_bin" ] || _bin="$(find "$_tmp" -type f -name meow -perm -u+x 2>/dev/null | head -n1)"
  [ -n "$_bin" ] && [ -f "$_bin" ] || die "could not find the meow binary inside the archive"

  mkdir -p "$BIN_DIR"
  if command -v install >/dev/null 2>&1; then
    install -m 0755 "$_bin" "$BIN_DIR/meow"
  else
    cp "$_bin" "$BIN_DIR/meow" && chmod 0755 "$BIN_DIR/meow"
  fi

  add_to_path

  purr "meow installed to $BIN_DIR/meow"
  "$BIN_DIR/meow" --version 2>/dev/null || true
  [ -n "${PATH_HINT:-}" ] && pounce "$PATH_HINT"
  printf "%b   Get started:  meow init%b\n" "$C_PINK" "$C_OFF"
}

main "$@"
