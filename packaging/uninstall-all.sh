#!/bin/bash
# FanCtl 彻底清理：停止服务、交还风扇、删除所有历史版本残留
set -uo pipefail

echo "=== 1. 停止服务 ==="
launchctl bootout system/local.fanctl.daemon 2>/dev/null && echo "  ✓ 已 bootout launchd 作业"
launchctl bootout system /Library/LaunchDaemons/local.fanctl.daemon.plist 2>/dev/null
# Do NOT `launchctl disable` here. It persists into
# /var/db/com.apple.xpc.launchd/disabled.plist, and the next install's
# `launchctl bootstrap` then fails ("Operation not permitted"), aborting the
# whole .pkg with PKInstallErrorDomain 112. Deleting the plist below is enough
# to keep the job from coming back.
echo "  ✓ 作业已卸载（不写 disabled 标记，避免后续安装失败）"

echo
echo "=== 2. 交还风扇给 macOS ==="
if [[ -x /usr/local/libexec/fanctl/fand ]]; then
  /usr/local/libexec/fanctl/fand --restore-auto && echo "  ✓ 风扇已交还系统自动控制"
else
  echo "  （fand 不存在，跳过；风扇本来就由系统管）"
fi

echo
echo "=== 3. 删除 launchd 配置 ==="
rm -f /Library/LaunchDaemons/local.fanctl.daemon.plist && echo "  ✓ local.fanctl.daemon.plist"
rm -f /Library/LaunchAgents/local.fanctl.ui.plist 2>/dev/null

echo
echo "=== 4. 删除程序文件 ==="
rm -rf /usr/local/libexec/fanctl          && echo "  ✓ /usr/local/libexec/fanctl（当前守护进程）"
rm -f  /usr/local/bin/fanctl              && echo "  ✓ /usr/local/bin/fanctl（当前 CLI）"
rm -rf /usr/local/lib/fanrs               && echo "  ✓ /usr/local/lib/fanrs（fanrs 中间版）"
rm -f  /usr/local/bin/fand                && echo "  ✓ /usr/local/bin/fand（fanrs 软链）"
rm -rf /usr/local/lib/fankit              && echo "  ✓ /usr/local/lib/fankit（Python 最早版）"
rm -f  /usr/local/bin/fankit              && echo "  ✓ /usr/local/bin/fankit（Python 版入口）"
rm -rf /Applications/FanCtl.app           && echo "  ✓ /Applications/FanCtl.app（egui 版 UI）"
# PackageKit 在 bundle 标识不匹配时会把旧 app 挪到 FanCtl.localized/ 下，
# 安装回滚后这份副本会变成孤儿，留在 /Applications 里看起来像装了两份。
rm -rf /Applications/FanCtl.localized     && echo "  ✓ /Applications/FanCtl.localized（重定位残骸）"
rm -f  /usr/local/bin/fanui               && echo "  ✓ /usr/local/bin/fanui（若有）"

echo
echo "=== 5. 删除配置与日志 ==="
rm -rf "/Library/Application Support/FanCtl" && echo "  ✓ /Library/Application Support/FanCtl"
rm -rf /Library/Logs/FanCtl                  && echo "  ✓ /Library/Logs/FanCtl"
rm -f  /var/run/fanctl.sock /var/run/fankit.sock 2>/dev/null && echo "  ✓ 残留 socket"

echo
echo "=== 6. 清理 pkg 收据 ==="
pkgutil --forget local.fanctl.pkg 2>/dev/null && echo "  ✓ local.fanctl.pkg 收据" || echo "  （无收据）"

echo
echo "=== 完成 ==="
