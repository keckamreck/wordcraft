#!/usr/bin/env bash
# Vercel's build step (see vercel.json): install the pinned Rust toolchain and Trunk, then build the
# browser version into dist/web, the folder Vercel serves. Hosting notes: packaging/web/README.md.
#
# Vercel's build image (Amazon Linux 2023, x86_64) has no Rust, so each build installs it first.
# Anywhere else, tools already installed at the pinned versions are used as they are:
#
#   packaging/web/vercel-build.sh [OUT]
#
# OUT (relative to the folder the script starts in) also gets a copy of the site: a Vercel project
# whose Root Directory is apps/wordcraft-web serves its own dist folder (apps/wordcraft-web/vercel.json).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT=""
if [ -n "${1:-}" ]; then
  case "$1" in
    /*) OUT="$1" ;;
    *) OUT="$PWD/$1" ;;
  esac
fi
APP=wordcraft

# Pinned, so a deploy builds with what the repository was tested with. Bump them together.
RUST_TOOLCHAIN=1.97.0
RUSTUP_VERSION=1.29.1
RUSTUP_SHA256=dda7234360b7f578ca8b0ddcb80145646fa61a67c1720a5abc7051b35c9fcb71 # x86_64-unknown-linux-gnu/rustup-init
TRUNK_VERSION=0.21.14
TRUNK_SHA256=a67f4054b249fe9acc5fabc25de1aebf19783aca3ad6ff64bf34d7da44d0ea20 # trunk-x86_64-unknown-linux-musl.tar.gz

export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
export PATH="$CARGO_HOME/bin:$PATH"
TOOLS="$(mktemp -d)"
trap 'rm -rf "$TOOLS"' EXIT

fetch() { curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$2" "$1"; }
verify() {
  printf '%s  %s\n' "$1" "$2" | sha256sum -c --quiet - || { echo "error: $2 doesn't match its pinned SHA-256" >&2; exit 1; }
}
linux_x64() { [ "$(uname -s)-$(uname -m)" = "Linux-x86_64" ]; }

if ! command -v rustup >/dev/null; then
  linux_x64 || { echo "error: rustup not found; install it from https://rustup.rs" >&2; exit 1; }
  echo "installing rustup $RUSTUP_VERSION"
  fetch "https://static.rust-lang.org/rustup/archive/$RUSTUP_VERSION/x86_64-unknown-linux-gnu/rustup-init" "$TOOLS/rustup-init"
  verify "$RUSTUP_SHA256" "$TOOLS/rustup-init"
  chmod 755 "$TOOLS/rustup-init"
  "$TOOLS/rustup-init" -y --no-modify-path --profile minimal --default-toolchain none
fi
rustup toolchain install "$RUST_TOOLCHAIN" --profile minimal --target wasm32-unknown-unknown
export RUSTUP_TOOLCHAIN="$RUST_TOOLCHAIN"

if [ "$(trunk --version 2>/dev/null || true)" != "trunk $TRUNK_VERSION" ]; then
  linux_x64 || { echo "error: trunk $TRUNK_VERSION not found (cargo install trunk --locked --version $TRUNK_VERSION)" >&2; exit 1; }
  echo "installing trunk $TRUNK_VERSION"
  fetch "https://github.com/trunk-rs/trunk/releases/download/v$TRUNK_VERSION/trunk-x86_64-unknown-linux-musl.tar.gz" "$TOOLS/trunk.tar.gz"
  verify "$TRUNK_SHA256" "$TOOLS/trunk.tar.gz"
  mkdir -p "$TOOLS/trunk" "$CARGO_HOME/bin"
  tar -xzf "$TOOLS/trunk.tar.gz" -C "$TOOLS/trunk"
  BIN=""
  for f in "$TOOLS/trunk/trunk" "$TOOLS"/trunk/*/trunk; do
    if [ -f "$f" ]; then BIN="$f" && break; fi
  done
  [ -n "$BIN" ] || { echo "error: no trunk binary in the release archive" >&2; exit 1; }
  install -m 755 "$BIN" "$CARGO_HOME/bin/trunk"
fi

# Trunk fetches the wasm-bindgen CLI that matches Cargo.lock, and wasm-opt, by itself.
(cd "$ROOT/apps/$APP-web" && trunk build --release --locked)
# Same check as package.sh: the site must work from any path.
if grep -Eq '(src|href)="/[^/]' "$ROOT/dist/web/index.html"; then
  echo "error: dist/web/index.html has root-absolute URLs" >&2
  exit 1
fi
if [ -n "$OUT" ]; then
  mkdir -p "$OUT"
  cp -R "$ROOT/dist/web/." "$OUT/"
  echo "copied the site to $OUT"
fi
echo "built $ROOT/dist/web"
