#!/usr/bin/env bash
# Regenerate every app icon file from assets/app-icon/lightcraft.svg (the Fotofficina icon, a 1024x1024 SVG).
# Same outputs and file names as packaging/icons.sh (which assumes a 512 viewBox), so nothing else changes.
#
# Needs: resvg (brew install resvg); on macOS iconutil writes the .icns.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/assets/app-icon"
SVG="$DIR/lightcraft.svg"
ID="ai.storyteller.lightcraft"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
command -v resvg >/dev/null || { echo "error: resvg not found (brew install resvg)" >&2; exit 1; }

# macOS icons use an 824/1024 body with a transparent margin: widen the viewBox (1024 / 0.805 = 1272).
MAC="$TMP/macos.svg"
sed 's/viewBox="0 0 1024 1024"/viewBox="-124 -124 1272 1272"/' "$SVG" >"$MAC"
grep -q 'viewBox="-124 -124 1272 1272"' "$MAC" || { echo "error: unexpected viewBox in $SVG" >&2; exit 1; }

render() { resvg -w "$2" -h "$2" "$1" "$3" </dev/null; }

render "$SVG" 1024 "$DIR/lightcraft-1024.png"
render "$MAC" 512 "$DIR/lightcraft-macos-512.png"

for s in 16 24 32 48 64 128 256 512; do
  mkdir -p "$DIR/hicolor/${s}x${s}/apps"
  render "$SVG" "$s" "$DIR/hicolor/${s}x${s}/apps/$ID.png"
done
mkdir -p "$DIR/hicolor/scalable/apps"
cp "$DIR/lightcraft-small.svg" "$DIR/hicolor/scalable/apps/$ID.svg"

ICO_PNGS=()
for s in 16 20 24 32 40 48 64 128 256; do
  render "$SVG" "$s" "$TMP/ico-$s.png"
  ICO_PNGS+=("$TMP/ico-$s.png")
done
(cd "$ROOT" && cargo run -q -p xtask -- ico "$DIR/lightcraft.ico" "${ICO_PNGS[@]}")

if command -v iconutil >/dev/null; then
  SET="$TMP/fotofficina.iconset"
  mkdir -p "$SET"
  for s in 16 32 128 256 512; do
    render "$MAC" "$s" "$SET/icon_${s}x${s}.png"
    render "$MAC" $((s * 2)) "$SET/icon_${s}x${s}@2x.png"
  done
  iconutil -c icns -o "$DIR/lightcraft.icns" "$SET"
fi
echo "icons written to $DIR"
