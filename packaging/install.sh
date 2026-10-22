#!/bin/sh
# Builds Linamp and installs it for the current user (~/.local).
set -eu
cd "$(dirname "$0")/.."
cargo build --release -p linamp
prefix="${PREFIX:-$HOME/.local}"
install -Dm755 target/release/linamp "$prefix/bin/linamp"
install -Dm644 packaging/linamp.desktop "$prefix/share/applications/linamp.desktop"
install -Dm644 packaging/linamp-skin.xml "$prefix/share/mime/packages/linamp-skin.xml"
mkdir -p "$prefix/share/linamp/skins"
command -v update-mime-database >/dev/null && update-mime-database "$prefix/share/mime" || true
command -v update-desktop-database >/dev/null && update-desktop-database "$prefix/share/applications" || true
echo "Installed to $prefix/bin/linamp"
