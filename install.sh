#!/usr/bin/env bash
# One-shot setup: install build/runtime dependencies, build the whole
# workspace, then build and install both GUI apps as pacman packages.
#
# Usage: ./install.sh [sender|receiver|both]
#   (defaults to "both"; forwarded to packaging/pacman/install.sh)
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
target="${1:-both}"

if ! command -v pacman >/dev/null 2>&1; then
  echo "error: this script targets Arch-based distros (pacman not found)." >&2
  echo "See README.md for manual build steps on other distros." >&2
  exit 1
fi

echo "==> Installing system dependencies (pacman)"
sudo pacman -S --needed \
  base-devel rust pkgconf \
  webkit2gtk-4.1 gtk3 libappindicator-gtk3 librsvg alsa-lib \
  curl wget file openssl appmenu-gtk-module

if command -v npm >/dev/null 2>&1; then
  for app in sender receiver; do
    echo "==> npm install ($app GUI)"
    ( cd "$script_dir/apps/$app/gui" && npm install )
  done
else
  echo "warning: npm not found; skipping 'npm install' for the GUI dev toolchains (not required to build/install the packages below)" >&2
fi

echo "==> Building the full workspace (cargo build --release)"
( cd "$script_dir" && cargo build --release --locked --workspace )

echo "==> Building and installing GUI package(s): $target"
"$script_dir/packaging/pacman/install.sh" "$target"

echo "==> Done."
echo "    Native CLIs built at target/release/{sender,receiver} (not installed system-wide)."
echo "    GUI(s) installed as audio-network-sender / audio-network-receiver."
