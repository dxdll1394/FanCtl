#!/bin/bash
# Build FanCtl.app.
#
# The app is a thin wrapper: the UI talks to the root daemon over a unix
# socket, so the bundle itself needs no special entitlements.
#
#   ./packaging/build_app.sh [target.app]     # default /Applications/FanCtl.app
#   ./packaging/build_app.sh --unsigned /tmp/FanCtl.app
#   ./packaging/build_app.sh --egui /tmp/FanCtl.app   # legacy egui front end
#
# By default this builds the SwiftUI front end from `fanui-mac/` (≈37 MB RSS).
# `--egui` keeps the old eframe binary available for comparison.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="$(cd "$HERE/.." && pwd)"
SIGN=1
FRONTEND="swiftui"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --unsigned) SIGN=0; shift ;;
    --egui) FRONTEND="egui"; shift ;;
    -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
    *) break ;;
  esac
done
APP="${1:-/Applications/FanCtl.app}"

if [[ "$FRONTEND" == "swiftui" ]]; then
  # The SwiftUI build produces a complete .app with its own Info.plist, icon
  # and ad-hoc signature, so just build it straight into the destination.
  exec "$SRC/fanui-mac/build.sh" "$APP"
fi

BIN="$SRC/target/release/fanui"

[[ -x "$BIN" ]] || { echo "先运行 cargo build --release -p fanui" >&2; exit 1; }

echo "→ 生成 ${APP}（egui 旧前端）"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$HERE/Info.plist" "$APP/Contents/Info.plist"
cp "$BIN" "$APP/Contents/MacOS/fanui"
chmod 755 "$APP/Contents/MacOS/fanui"

# Icon: draw a simple fan, then let iconutil assemble the .icns.
if command -v python3 >/dev/null && python3 -c "import PIL" 2>/dev/null; then
  TMP="$(mktemp -d)"
  if python3 "$HERE/make_icon.py" "$TMP/icon.png"; then
    ICONSET="$TMP/AppIcon.iconset"
    mkdir -p "$ICONSET"
    for size in 16 32 128 256 512; do
      sips -z $size $size "$TMP/icon.png" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
      sips -z $((size * 2)) $((size * 2)) "$TMP/icon.png" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
    done
    iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
  fi
  rm -rf "$TMP"
fi

if [[ $SIGN -eq 1 ]]; then
  # Ad-hoc signature keeps macOS happy without a developer account.
  if codesign --force --deep --sign - "$APP" 2>/dev/null; then
    echo "  ✓ 已临时签名"
  else
    echo "  ! 临时签名失败（不影响本机使用）"
  fi
fi

echo "  ✓ 完成：$APP"
echo "    双击打开，或 open -a FanCtl"
