#!/usr/bin/env bash
# Install the extracted Lahza binary bundle for the current user.
set -euo pipefail

bundle_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
install_prefix="${LAHZA_PREFIX:-$HOME/.local}"

if [[ "${1:-}" == "--help" ]]; then
  echo 'Usage: ./install.sh'
  echo 'Installs Lahza into ~/.local. Set LAHZA_PREFIX to choose another prefix.'
  echo 'System libraries must be installed separately; see README.md.'
  exit 0
fi
if [[ $# -ne 0 ]]; then
  echo 'Unknown argument. Use --help for usage.' >&2
  exit 2
fi
if [[ ! -x "$bundle_dir/bin/screendrop" || ! -d "$bundle_dir/share/screendrop/assets" ]]; then
  echo 'Run this installer from the extracted Lahza release bundle.' >&2
  exit 1
fi

install -Dm755 "$bundle_dir/bin/screendrop" "$install_prefix/bin/screendrop"
install -d "$install_prefix/share/screendrop/assets"
cp -R "$bundle_dir/share/screendrop/assets/." "$install_prefix/share/screendrop/assets/"
install -Dm644 "$bundle_dir/share/applications/com.screendrop.Screendrop.desktop" \
  "$install_prefix/share/applications/com.screendrop.Screendrop.desktop"
install -Dm644 "$bundle_dir/share/icons/hicolor/512x512/apps/com.screendrop.Screendrop.png" \
  "$install_prefix/share/icons/hicolor/512x512/apps/com.screendrop.Screendrop.png"
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$install_prefix/share/applications" || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -t "$install_prefix/share/icons/hicolor" || true
fi
printf 'Installed Lahza in %s\n' "$install_prefix"
printf 'Ensure %s/bin is on PATH, then launch Lahza from your app menu or run screendrop.\n' "$install_prefix"
