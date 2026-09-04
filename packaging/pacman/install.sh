#!/usr/bin/env bash
# Build and install the audio-network GUI(s) as pacman packages.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

usage() {
  cat >&2 <<EOF
Usage: $(basename "$0") [sender|receiver|both]

  sender    build and install the sender GUI only
  receiver  build and install the receiver GUI only
  both      build and install both (default)

Runs 'makepkg -f' for each selected app, then 'sudo pacman -U' on the
resulting package(s). You'll be prompted for your sudo password.
EOF
}

case "${1:-}" in
  sender)          apps=(audio-network-sender) ;;
  receiver)        apps=(audio-network-receiver) ;;
  both|"")         apps=(audio-network-sender audio-network-receiver) ;;
  -h|--help)       usage; exit 0 ;;
  *)               usage; exit 1 ;;
esac

pkgfiles=()
for app in "${apps[@]}"; do
  echo "==> Building $app"
  ( cd "$script_dir/$app" && makepkg -f )
  pkgfile="$(ls -t "$script_dir/$app/$app"-*-x86_64.pkg.tar.zst | head -n1)"
  pkgfiles+=("$pkgfile")
done

echo "==> Installing: ${pkgfiles[*]##*/}"
sudo pacman -U "${pkgfiles[@]}"
