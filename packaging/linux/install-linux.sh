#!/usr/bin/env bash
set -euo pipefail

# Single-click Linux installer for EcConn (Receiver mode).
# Builds app-host in release mode, installs ~/.local/bin/ecconn, and creates
# Application Menu + Desktop shortcuts for single-click launch without a terminal.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

echo "Building EcConn for Linux..."
cd "$ROOT_DIR"
cargo build --release -p app-host

mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications"
cp "$ROOT_DIR/target/release/app-host" "$HOME/.local/bin/ecconn"
chmod +x "$HOME/.local/bin/ecconn"

sed "s|^Exec=.*|Exec=$HOME/.local/bin/ecconn|" "$SCRIPT_DIR/ecconn.desktop" > "$HOME/.local/share/applications/ecconn.desktop"
chmod +x "$HOME/.local/share/applications/ecconn.desktop"

if [ -d "$HOME/Desktop" ]; then
    cp "$HOME/.local/share/applications/ecconn.desktop" "$HOME/Desktop/EcConn.desktop"
    chmod +x "$HOME/Desktop/EcConn.desktop"
fi

if ! command -v ffmpeg >/dev/null 2>&1; then
    echo "Warning: ffmpeg not found. Install it with your package manager (e.g. sudo apt install ffmpeg)."
fi

echo "EcConn installed! You can now launch 'EcConn' with a single click from your Application Menu or Desktop."
