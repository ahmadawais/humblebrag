#!/bin/sh
# Release build, installed to ~/.local/bin/npmstat (on PATH via ~/.dotfiles zsh/core.zsh).
set -eu
cd "$(dirname "$0")/.."
CARGO="$(command -v cargo || echo /opt/homebrew/opt/rustup/bin/cargo)"
"$CARGO" build --release --locked --quiet
DEST="${NPMSTAT_PREFIX:-$HOME/.local/bin}"
mkdir -p "$DEST"
install -m 0755 target/release/npmstat "$DEST/npmstat"
echo "installed $DEST/npmstat"
