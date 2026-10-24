#!/bin/bash
# Build a double-clickable FanCtl.pkg.
#
#   ./packaging/build_pkg.sh                # -> ./dist/FanCtl-<version>.pkg
#   ./packaging/build_pkg.sh --skip-build   # reuse existing build artifacts
#   ./packaging/build_pkg.sh --unsigned /tmp/FanCtl.pkg
#
# The package installs, as root:
#   /Applications/FanCtl.app                       窗口 + 菜单栏 UI
#   /usr/local/libexec/fanctl/fand                 root 守护进程
#   /usr/local/bin/fanctl                          CLI
#   /Library/LaunchDaemons/local.fanctl.daemon.plist
#
# Installer runs packaging/pkg/preinstall (stop old daemon, hand fans back)
# then packaging/pkg/postinstall (launchctl bootstrap + wait for the socket).
#
# An unsigned .pkg works on the build machine but Gatekeeper blocks it
# elsewhere. For distribution, pass --sign "Developer ID Installer: …" so
# productsign can seal it.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="$(cd "$HERE/.." && pwd)"
PKG_SRC="$HERE/pkg"
# The UI ships its own version (fanui-mac/build.sh writes it into Info.plist);
# the package version follows the app so Installer shows something meaningful.
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' \
  "$SRC/fanui-mac/build/FanCtl.app/Contents/Info.plist" 2>/dev/null)" || true
VERSION="${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$SRC/Cargo.toml" | head -1)}"
IDENT="local.fanctl.pkg"

SKIP_BUILD=0
SIGN_ID=""
OUT=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    --sign) SIGN_ID="${2:?--sign 需要证书名}"; shift 2 ;;
    --out) OUT="${2:?--out 需要路径}"; shift 2 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "未知参数 $1" >&2; exit 2 ;;
  esac
done

OUT="${OUT:-$SRC/dist/FanCtl-$VERSION.pkg}"
WORK="$(mktemp -d)"
STAGE="$WORK/root"
SCRIPTS="$WORK/scripts"
COMPONENT="$WORK/component"
mkdir -p "$STAGE" "$SCRIPTS" "$COMPONENT"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

step() { echo; echo "→ $*"; }

if [[ $SKIP_BUILD -eq 0 ]]; then
  step "编译（release）"
  ( cd "$SRC" && cargo build --release -p fand -p fanctl )
fi

step "编译 SwiftUI 界面"
"$SRC/fanui-mac/build.sh" "$STAGE/Applications/FanCtl.app"

step "装配 payload"
mkdir -p "$STAGE/usr/local/libexec/fanctl" "$STAGE/usr/local/bin"
mkdir -p "$STAGE/Library/LaunchDaemons"

# Note: `pkgbuild` records xattrs as AppleDouble `._name` siblings, and macOS
# 15+ stamps every copied file with `com.apple.provenance` (which `xattr -d`
# refuses to remove). Those entries are harmless — Apple's own packages have
# them — so the payload is left as-is.
install -m 755 "$SRC/target/release/fand"   "$STAGE/usr/local/libexec/fanctl/fand"
install -m 755 "$SRC/target/release/fanctl" "$STAGE/usr/local/bin/fanctl"
install -m 644 "$HERE/local.fanctl.daemon.plist" \
  "$STAGE/Library/LaunchDaemons/local.fanctl.daemon.plist"

# `pkgbuild` archives exactly what is on disk, so a 0700 binary would land
# unreadable inside the system package.
chmod -R a+rX "$STAGE"
chmod 755 "$STAGE/usr/local/libexec/fanctl/fand" "$STAGE/usr/local/bin/fanctl"

step "写入安装脚本"
cp "$PKG_SRC/preinstall" "$PKG_SRC/postinstall" "$SCRIPTS/"
chmod 755 "$SCRIPTS/preinstall" "$SCRIPTS/postinstall"

# Preflight: two situations make PackageKit refuse to overwrite
# /Applications/FanCtl.app, and both end with the user seeing two copies.
#   * an installed bundle with a different CFBundleIdentifier
#   * an orphaned *.localized/ directory from an earlier rolled-back install
APP_BUNDLE="/Applications/FanCtl.app"
PKG_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' \
  "$STAGE/Applications/FanCtl.app/Contents/Info.plist" 2>/dev/null || true)"
if [[ -d "$APP_BUNDLE" ]]; then
  INSTALLED_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' \
    "$APP_BUNDLE/Contents/Info.plist" 2>/dev/null || true)"
  if [[ -n "$INSTALLED_ID" && "$INSTALLED_ID" != "$PKG_ID" ]]; then
    echo
    echo "  ! 已安装的 FanCtl.app 标识是 $INSTALLED_ID，与本包（$PKG_ID）不一致"
    echo "    PackageKit 不会覆盖它，而会挪到 FanCtl.localized/，看起来就像装了两份"
    echo "    安装后的 preinstall 会清理这些残留，但建议先： sudo rm -rf $APP_BUNDLE"
  fi
fi
for stale in /Applications/FanCtl*.localized; do
  [[ -d "$stale" ]] || continue
  echo
  echo "  ! 发现残留副本 $stale"
  echo "    安装脚本会删除它；也可以手动： sudo rm -rf \"$stale\""
done

step "pkgbuild"
pkgbuild \
  --root "$STAGE" \
  --identifier "$IDENT" \
  --version "$VERSION" \
  --install-location "/" \
  --scripts "$SCRIPTS" \
  --ownership recommended \
  "$COMPONENT/FanCtl.pkg"

step "productbuild"
mkdir -p "$(dirname "$OUT")"
sed "s/__VERSION__/$VERSION/" "$PKG_SRC/Distribution.xml" > "$SCRIPTS/Distribution.xml"
productbuild \
  --distribution "$SCRIPTS/Distribution.xml" \
  --resources "$PKG_SRC/resources" \
  --package-path "$COMPONENT" \
  "$OUT"

if [[ -n "$SIGN_ID" ]]; then
  step "签名（$SIGN_ID）"
  productsign --sign "$SIGN_ID" "$OUT" "$OUT.signed"
  mv -f "$OUT.signed" "$OUT"
else
  echo
  echo "  ! 未签名：本机可用，分发到别的机器会被 Gatekeeper 拦截"
  echo "    签名： ./packaging/build_pkg.sh --sign \"Developer ID Installer: 你的名字 (TEAMID)\""
fi

step "完成"
echo "  $OUT  ($(du -h "$OUT" | cut -f1))"
echo "  安装：双击，或 sudo installer -pkg \"$OUT\" -target /"
echo "  查看内容：pkgutil --payload-files \"$OUT\""
