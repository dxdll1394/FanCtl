#!/bin/bash
# Build FanCtl.app from the Swift sources in this directory.
#
# The UI talks to the root daemon over its existing unix socket, so the bundle
# needs no entitlements and no privileged helper.
#
#   ./build.sh                      # -> ./build/FanCtl.app
#   ./build.sh /tmp/FanCtl.app      # custom destination
#   ./build.sh /tmp/FanCtl.app --run
#
# Only Apple Command Line Tools are required (`swiftc`); Xcode is not.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
APP="${1:-$HERE/build/FanCtl.app}"
RUN=0
for arg in "$@"; do [[ "$arg" == "--run" ]] && RUN=1; done

command -v swiftc >/dev/null || {
  echo "缺少 swiftc：xcode-select --install" >&2
  exit 1
}

echo "→ 编译 Swift 源码"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
BIN="$APP/Contents/MacOS/FanCtl"

# `-parse-as-library` is required because we use @main.
swiftc \
  -O -whole-module-optimization \
  -parse-as-library \
  -target arm64-apple-macos14.0 \
  -o "$BIN" \
  "$HERE"/Sources/*.swift

echo "→ 写入 Info.plist"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>FanCtl</string>
	<key>CFBundleDisplayName</key>
	<string>FanCtl</string>
	<key>CFBundleIdentifier</key>
	<string>local.fanctl.ui.swift</string>
	<key>CFBundleVersion</key>
	<string>0.2.0</string>
	<key>CFBundleShortVersionString</key>
	<string>0.2.0</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleExecutable</key>
	<string>FanCtl</string>
	<key>CFBundleIconFile</key>
	<string>AppIcon</string>
	<key>LSMinimumSystemVersion</key>
	<string>14.0</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.utilities</string>
	<key>LSUIElement</key>
	<true/>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSHumanReadableCopyright</key>
	<string>FanCtl — Apple Silicon fan control</string>
</dict>
</plist>
PLIST

# Reuse the icon the Rust build already generates, if present.
if [[ -f "$ROOT/packaging/make_icon.py" ]] && command -v python3 >/dev/null; then
  TMP="$(mktemp -d)"
  if python3 "$ROOT/packaging/make_icon.py" "$TMP/icon.png" >/dev/null 2>&1; then
    ICONSET="$TMP/AppIcon.iconset"
    mkdir -p "$ICONSET"
    for size in 16 32 128 256 512; do
      sips -z $size $size "$TMP/icon.png" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
      sips -z $((size * 2)) $((size * 2)) "$TMP/icon.png" \
        --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
    done
    iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns" 2>/dev/null || true
  fi
  rm -rf "$TMP"
fi

if codesign --force --sign - "$APP" >/dev/null 2>&1; then
  echo "  ✓ 已临时签名"
fi

echo "  ✓ 完成：$APP"
echo "    $(du -sh "$APP" | cut -f1) · 双击打开，或 open \"$APP\""

if [[ $RUN -eq 1 ]]; then
  open "$APP"
fi
