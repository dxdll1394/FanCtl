#!/bin/bash
# FanCtl installer.
#
#   ./install.sh              install CLI + privileged daemon (needs sudo)
#   ./install.sh --app        additionally build FanCtl.app into /Applications
#   ./install.sh --uninstall  remove everything and hand the fans back to macOS
#
# Everything lives under /usr/local and /Library; no Homebrew, no kernel
# extensions, no SIP changes.

set -euo pipefail

SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LABEL="local.fanctl.daemon"
PLIST="/Library/LaunchDaemons/${LABEL}.plist"
PREFIX="/usr/local/libexec/fanctl"
BINDIR="/usr/local/bin"
SUPPORT="/Library/Application Support/FanCtl"
LOGDIR="/Library/Logs/FanCtl"
OLD_SOCKET="/var/run/fankit.sock"

die() { echo "错误：$*" >&2; exit 1; }
step() { echo; echo "→ $*"; }

WITH_APP=0
UNINSTALL=0
for arg in "$@"; do
  case "$arg" in
    --app) WITH_APP=1 ;;
    --uninstall) UNINSTALL=1 ;;
    -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
    *) die "未知参数 $arg" ;;
  esac
done

[[ $EUID -eq 0 ]] || die "需要管理员权限：sudo ./install.sh $*"

if [[ $UNINSTALL -eq 1 ]]; then
  step "停止并移除守护进程"
  launchctl bootout system "$PLIST" 2>/dev/null || true
  launchctl disable "system/${LABEL}" 2>/dev/null || true
  rm -f "$PLIST"

  # Make sure the fans are not left in manual mode.
  if [[ -x "$PREFIX/fand" ]]; then
    "$PREFIX/fand" --restore-auto >/dev/null 2>&1 || true
  fi

  step "删除程序文件"
  rm -rf "$PREFIX"
  rm -f "$BINDIR/fanctl" "$BINDIR/fanui"
  rm -rf "/Applications/FanCtl.app"
  rm -f "/Library/LaunchAgents/local.fanctl.ui.plist"
  echo
  echo "已卸载。配置和日志保留在："
  echo "  $SUPPORT"
  echo "  $LOGDIR"
  echo "如需彻底清理：sudo rm -rf \"$SUPPORT\" \"$LOGDIR\""
  exit 0
fi

step "编译（release）"
if [[ "${FANCTL_SKIP_BUILD:-0}" == "1" ]]; then
  echo "  使用已有构建产物"
else
  command -v cargo >/dev/null || die "找不到 cargo，请先安装 Rust 工具链"
  # `sudo ./install.sh` runs this as root; building as root would leave
  # root-owned artifacts in the user's target/ directory and break the next
  # unprivileged build. Switch back to the invoking user instead.
  if [[ $EUID -eq 0 && -n "${SUDO_UID:-}" && "${SUDO_UID:-0}" -ne 0 ]]; then
    sudo -u "#${SUDO_UID}" -H env PATH="$PATH"       CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"       bash -c "cd \"$SRC\" && cargo build --release -p fand -p fanctl -p fanui"
  else
    ( cd "$SRC" && cargo build --release -p fand -p fanctl -p fanui )
  fi
fi

step "安装到 $PREFIX"
install -d -m 755 -o root -g wheel "$PREFIX"
install -m 755 -o root -g wheel "$SRC/target/release/fand"   "$PREFIX/fand"
install -m 755 -o root -g wheel "$SRC/target/release/fanctl" "$BINDIR/fanctl"
install -m 755 -o root -g wheel "$SRC/target/release/fanui"  "$BINDIR/fanui"

install -d -m 700 -o root -g wheel "$SUPPORT"
install -d -m 755 -o root -g wheel "$LOGDIR"
install -m 644 -o root -g wheel "$SRC/packaging/${LABEL}.plist" "$PLIST"

# Old versions wrote one line per RPM change and never rotated. Fold that
# history into a single archive so the new self-rotating sink starts clean
# instead of inheriting a multi-megabyte file.
if [[ -f "$LOGDIR/daemon.log" ]] && (( $(stat -f%z "$LOGDIR/daemon.log") > 1048576 )); then
  echo
  echo "→ 压缩旧日志（$(du -h "$LOGDIR/daemon.log" | cut -f1)）"
  mv -f "$LOGDIR/daemon.log" "$LOGDIR/daemon.log.0"
fi

step "注册 launchd 服务"
launchctl bootout system "$PLIST" 2>/dev/null || true
launchctl bootstrap system "$PLIST"
launchctl enable "system/${LABEL}"
launchctl kickstart -k "system/${LABEL}"

for _ in $(seq 1 20); do
  [[ -S /var/run/fanctl.sock ]] && break
  sleep 0.2
done

if [[ -S /var/run/fanctl.sock ]]; then
  echo "  ✓ 守护进程已启动"
else
  echo "  ✗ 守护进程没有起来，检查 $LOGDIR/daemon.log" >&2
  tail -20 "$LOGDIR/daemon.log" 2>/dev/null || true
  exit 1
fi

if [[ -S "$OLD_SOCKET" ]]; then
  echo
  echo "  ! 检测到旧的 fankit 守护进程仍在运行，两个程序会互相抢风扇。"
  echo "    建议停掉它： sudo launchctl bootout system/local.fankit.daemon"
fi

if [[ $WITH_APP -eq 1 ]]; then
  step "打包 FanCtl.app"
  FANCTL_SKIP_BUILD=1 "$SRC/packaging/build_app.sh"
fi

step "完成"
"$BINDIR/fanctl" status || true
echo
echo "常用命令："
echo "  fanctl status          查看转速与温度"
echo "  fanctl set 2200        固定 2200 rpm"
echo "  fanctl curve           交给温度曲线"
echo "  fanctl auto            交还 macOS"
echo "  open -a FanCtl         打开图形界面"
