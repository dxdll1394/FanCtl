#!/bin/bash
# Remove leftovers from the abandoned fankit / fanrs daemons.
#
#   sudo ./cleanup-legacy.sh            # 只清理残留
#   sudo ./cleanup-legacy.sh --dry-run  # 先看会删什么
#
# 依据：这三个 socket/plist/日志都属于已经废弃的 fankit(Python) 与 fanrs(Rust)
# 版本，launchctl 里已经没有对应服务，风扇由 fanctl 独占控制。

set -euo pipefail

DRY=0
[[ "${1:-}" == "--dry-run" ]] && DRY=1

# 每一项都是绝对路径，逐条列出便于核对。
TARGETS=(
  "/var/run/fankit.sock"                              # fankit 遗留的无主 socket
  "/Library/LaunchDaemons/local.fankit.daemon.plist"  # 已废弃服务定义
  "/Library/LaunchDaemons/local.fanrs.daemon.plist"   # 已废弃服务定义
  "/var/log/fankit.log"                               # 旧守护进程日志
  "/var/log/fanrs.out.log"
  "/var/log/fanrs.err.log"
)

if [[ $EUID -ne 0 && $DRY -eq 0 ]]; then
  echo "需要管理员权限：sudo $0" >&2
  exit 1
fi

# 删除前先确认没有进程真的在监听，避免误删活着的服务。
live_socket() {
  local p="$1"
  [[ -S "$p" ]] || return 1
  # 能用 python3 连上就说明有守护进程在跑，此时不该删。
  python3 - "$p" <<'PY' >/dev/null 2>&1
import socket, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
try:
    s.connect(sys.argv[1])
except OSError:
    sys.exit(1)
finally:
    s.close()
PY
}

echo "→ 检查残留"
for f in "${TARGETS[@]}"; do
  if [[ ! -e "$f" ]]; then
    echo "  跳过（不存在）：$f"
    continue
  fi
  if live_socket "$f"; then
    echo "  ✗ 跳过（仍有进程监听）：$f" >&2
    continue
  fi
  if [[ $DRY -eq 1 ]]; then
    echo "  将删除：$f"
  else
    rm -f -- "$f"
    echo "  已删除：$f"
  fi
done

if [[ $DRY -eq 1 ]]; then
  echo
  echo "（dry-run，未做任何改动）"
  exit 0
fi

echo
echo "→ 确认 launchd 不再加载旧服务"
for label in local.fankit.daemon local.fanrs.daemon; do
  if launchctl print "system/$label" >/dev/null 2>&1; then
    launchctl bootout "system/$label" 2>/dev/null || true
    launchctl disable "system/$label" 2>/dev/null || true
    echo "  已卸载：$label"
  else
    echo "  未加载：$label"
  fi
done

echo
echo "→ 当前风扇服务状态"
fanctl doctor 2>/dev/null | head -5 || true
