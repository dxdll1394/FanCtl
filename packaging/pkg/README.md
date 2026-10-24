# FanCtl 安装包

`build_pkg.sh` 产出双击可用的 `FanCtl.pkg`。安装内容：

| 路径 | 内容 | 属主 |
| -- | -- | -- |
| `/Applications/FanCtl.app` | 窗口 + 菜单栏 UI | root:wheel |
| `/usr/local/libexec/fanctl/fand` | root 守护进程 | root:wheel |
| `/usr/local/bin/fanctl` | 命令行 | root:wheel |
| `/Library/LaunchDaemons/local.fanctl.daemon.plist` | 开机启动 | root:wheel |

## 脚本

- **preinstall**：停掉旧守护进程 → `fand --restore-auto` 把风扇交还 macOS →
  删掉 egui 时代的 `/usr/local/bin/fanui`（现在 UI 在 app 里，留着会变成第二个前端）。
- **postinstall**：建好 `Logs` / `Application Support` 目录 → `launchctl bootstrap`
  → 轮询等 `/var/run/fanctl.sock` 出现。守护进程起不来时**不**让安装失败：
  app 仍然可以打开，界面里会显示未连接和修复入口。

两个脚本都支持 `FANCTL_PKG_ROOT`，可以指向一个临时目录做演练，不会碰系统。

## 分发

未签名的包在本机可用，拷给别人会被 Gatekeeper 拦。要分发需要 Developer ID：

```bash
./packaging/build_pkg.sh --sign "Developer ID Installer: 你的名字 (TEAMID)"
```

`.pkg` 的签名和 app 的签名是两回事：`productsign` 签的是安装包本身，
app 内部还需要 `codesign`（`build.sh` 已做 ad-hoc），
彻底合规还要 `notarytool submit` 公证。
