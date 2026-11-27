#!/usr/bin/env bash
set -euo pipefail

# Build a macOS .app bundle from an already compiled release binary.
# Usage: ./packaging/macos/build_app.sh <binary-path> <output-dir> [version]

BIN_PATH="${1:-target/release/tortor}"
OUT_DIR="${2:-dist}"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
VERSION="${3:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -n1)}"

if [[ ! -f "$BIN_PATH" ]]; then
  echo "Error: binary not found at $BIN_PATH" >&2
  exit 1
fi

APP_DIR="$OUT_DIR/TorTor.app"
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"

cp "$BIN_PATH" "$APP_DIR/Contents/MacOS/tortor"
chmod 755 "$APP_DIR/Contents/MacOS/tortor"

sed "s/__VERSION__/$VERSION/g" "$ROOT_DIR/packaging/macos/Info.plist" > "$APP_DIR/Contents/Info.plist"

ICON_PNG="$ROOT_DIR/Images/tortor_icon.png"
if [[ -f "$ICON_PNG" ]] && command -v sips >/dev/null 2>&1 && command -v iconutil >/dev/null 2>&1; then
  ICONSET_DIR="$(mktemp -d)/tortor.iconset"
  mkdir -p "$ICONSET_DIR"
  sips -z 16 16     "$ICON_PNG" --out "$ICONSET_DIR/icon_16x16.png"      >/dev/null
  sips -z 32 32     "$ICON_PNG" --out "$ICONSET_DIR/icon_16x16@2x.png"   >/dev/null
  sips -z 32 32     "$ICON_PNG" --out "$ICONSET_DIR/icon_32x32.png"      >/dev/null
  sips -z 64 64     "$ICON_PNG" --out "$ICONSET_DIR/icon_32x32@2x.png"   >/dev/null
  sips -z 128 128   "$ICON_PNG" --out "$ICONSET_DIR/icon_128x128.png"    >/dev/null
  sips -z 256 256   "$ICON_PNG" --out "$ICONSET_DIR/icon_128x128@2x.png" >/dev/null
  sips -z 256 256   "$ICON_PNG" --out "$ICONSET_DIR/icon_256x256.png"    >/dev/null
  sips -z 512 512   "$ICON_PNG" --out "$ICONSET_DIR/icon_256x256@2x.png" >/dev/null
  sips -z 512 512   "$ICON_PNG" --out "$ICONSET_DIR/icon_512x512.png"    >/dev/null
  sips -z 1024 1024 "$ICON_PNG" --out "$ICONSET_DIR/icon_512x512@2x.png" >/dev/null
  iconutil -c icns "$ICONSET_DIR" -o "$APP_DIR/Contents/Resources/tortor.icns"
  rm -rf "$(dirname "$ICONSET_DIR")"
fi

if command -v codesign >/dev/null 2>&1; then
  codesign --force --deep --sign - "$APP_DIR"
fi

echo "Created macOS bundle: $APP_DIR"