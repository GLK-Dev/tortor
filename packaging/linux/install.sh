#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_SRC=""
for candidate in "$SCRIPT_DIR/tortor" "$SCRIPT_DIR/../../target/release/tortor"; do
  if [[ -f "$candidate" ]]; then
    BIN_SRC="$candidate"
    break
  fi
done
if [[ -z "$BIN_SRC" ]]; then
  echo "Error: tortor binary not found next to install.sh or in target/release/tortor" >&2
  exit 1
fi

ICON_SRC=""
for candidate in "$SCRIPT_DIR/tortor_icon.png" "$SCRIPT_DIR/../../Images/tortor_icon.png"; do
  if [[ -f "$candidate" ]]; then
    ICON_SRC="$candidate"
    break
  fi
done

DESKTOP_SRC="$SCRIPT_DIR/tortor.desktop"
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
APP_DIR="$PREFIX/share/applications"
ICON_DIR="$PREFIX/share/icons/hicolor/128x128/apps"

mkdir -p "$BIN_DIR" "$APP_DIR" "$ICON_DIR"
install -m 755 "$BIN_SRC" "$BIN_DIR/tortor"
if [[ -n "$ICON_SRC" ]]; then
  install -m 644 "$ICON_SRC" "$ICON_DIR/tortor.png"
fi
if [[ -f "$DESKTOP_SRC" ]]; then
  sed "s|^Exec=.*|Exec=$BIN_DIR/tortor %U|" "$DESKTOP_SRC" > "$APP_DIR/tortor.desktop"
  chmod 644 "$APP_DIR/tortor.desktop"
fi

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t "$PREFIX/share/icons/hicolor" >/dev/null 2>&1 || true
fi

echo "Installed TorTor GUI to $BIN_DIR/tortor and desktop launcher to $APP_DIR/tortor.desktop"