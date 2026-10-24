//! `fanctl daemon …`: install/uninstall the launchd service and run in foreground.

use std::path::{Path, PathBuf};
use std::process::Command;

use fancore::config::Paths;

const LABEL: &str = "local.fanctl.daemon";
const PLIST_PATH: &str = "/Library/LaunchDaemons/local.fanctl.daemon.plist";
const INSTALL_BIN: &str = "/usr/local/libexec/fanctl/fand";
const LEGACY_SOCKET: &str = "/var/run/fankit.sock";

pub fn cmd_daemon(args: &[String], _socket: &Path) -> Result<(), String> {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("status");
    match sub {
        "run" => run_foreground(args),
        "install" => install(),
        "uninstall" => uninstall(),
        "status" => status(),
        other => Err(format!("daemon: 未知子命令 {other}（run/install/uninstall/status）")),
    }
}

fn run_foreground(args: &[String]) -> Result<(), String> {
    let mut cmd = Command::new(fand_binary()?);
    cmd.args(args.iter().skip(1));
    let status = cmd.status().map_err(|e| format!("无法启动 fand：{e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("fand 退出码 {status}"))
    }
}

fn fand_binary() -> Result<PathBuf, String> {
    if Path::new(INSTALL_BIN).exists() {
        return Ok(PathBuf::from(INSTALL_BIN));
    }
    // Running from `target/debug/fanctl`: look next to ourselves.
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let sibling = exe.with_file_name("fand");
    if sibling.exists() {
        return Ok(sibling);
    }
    Err("找不到 fand 可执行文件（先 cargo build，或 sudo ./install.sh）".into())
}

fn is_root() -> bool {
    fancore::smc::is_root()
}

/// Same test as `main::socket_is_live`: a stale socket file from a dead daemon
/// is not a conflict.
fn socket_is_live(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

fn run_cmd(program: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("执行 {program} 失败：{e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("{program} {} 失败：{}", args.join(" "), stderr.trim()))
    }
}

fn install() -> Result<(), String> {
    if !is_root() {
        return Err("安装需要 root：sudo fanctl daemon install".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let fand = fand_binary()?;

    std::fs::create_dir_all("/usr/local/libexec/fanctl")
        .map_err(|e| format!("创建安装目录失败：{e}"))?;
    copy_file(&fand, Path::new(INSTALL_BIN))?;
    if exe != Path::new("/usr/local/bin/fanctl") {
        let _ = copy_file(&exe, Path::new("/usr/local/bin/fanctl"));
    }
    let _ = copy_file(&exe.with_file_name("fanui"), Path::new("/usr/local/bin/fanui"));

    let plist = plist_contents();
    write_root_file(Path::new(PLIST_PATH), plist.as_bytes())?;

    // Replace any previous instance.
    let _ = Command::new("launchctl").args(["bootout", "system", PLIST_PATH]).output();
    run_cmd("launchctl", &["bootstrap", "system", PLIST_PATH])?;
    run_cmd("launchctl", &["enable", &format!("system/{LABEL}")])?;
    run_cmd("launchctl", &["kickstart", "-k", &format!("system/{LABEL}")])?;
    println!("已安装并启动 {LABEL}");
    println!("  程序：{INSTALL_BIN}");
    println!("  plist：{PLIST_PATH}");
    println!("  socket：/var/run/fanctl.sock");
    println!("  日志：/Library/Logs/FanCtl/daemon.log");
    // Only warn when the old daemon is really alive; a leftover socket file is
    // not a conflict and there is no service to boot out.
    if socket_is_live(Path::new(LEGACY_SOCKET)) {
        println!();
        println!("! 检测到旧的 fankit 守护进程（{LEGACY_SOCKET}）仍在运行，两者会互相抢风扇：");
        println!("  sudo launchctl bootout system/local.fankit.daemon");
    }
    Ok(())
}

fn uninstall() -> Result<(), String> {
    if !is_root() {
        return Err("卸载需要 root：sudo fanctl daemon uninstall".into());
    }
    let _ = Command::new("launchctl").args(["bootout", "system", PLIST_PATH]).output();
    let _ = Command::new("launchctl").args(["disable", &format!("system/{LABEL}")]).output();
    let _ = std::fs::remove_file(PLIST_PATH);
    let _ = std::fs::remove_file(INSTALL_BIN);
    println!("已卸载 {LABEL}（风扇已交还系统）");
    println!("配置和日志保留在 /Library/Application Support/FanCtl 与 /Library/Logs/FanCtl");
    Ok(())
}

fn status() -> Result<(), String> {
    let loaded = Command::new("launchctl")
        .args(["print", &format!("system/{LABEL}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if loaded {
        println!("✔ launchd 服务 {LABEL} 已加载");
    } else {
        println!("· launchd 服务 {LABEL} 未加载");
    }
    let sock = Paths::system().socket;
    if sock.exists() {
        println!("✔ socket 存在：{}", sock.display());
    } else {
        println!("· socket 不存在：{}", sock.display());
    }
    let log = Paths::system().log_file();
    if let Ok(text) = std::fs::read_to_string(&log) {
        println!("\n最近日志（{}）：", log.display());
        for line in text.lines().rev().take(10).collect::<Vec<_>>().iter().rev() {
            println!("  {line}");
        }
    }
    if loaded {
        Ok(())
    } else {
        Err("服务未加载".into())
    }
}

fn plist_contents() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{INSTALL_BIN}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ProcessType</key>
    <string>Background</string>
    <key>LowPriorityIO</key>
    <true/>
    <key>Nice</key>
    <integer>0</integer>
    <key>StandardOutPath</key>
    <string>/Library/Logs/FanCtl/daemon.log</string>
    <key>StandardErrorPath</key>
    <string>/Library/Logs/FanCtl/daemon.log</string>
</dict>
</plist>
"#
    )
}

fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
    if let (Ok(a), Ok(b)) = (std::fs::canonicalize(from), std::fs::canonicalize(to)) {
        if a == b {
            return Ok(());
        }
    }
    std::fs::copy(from, to)
        .map_err(|e| format!("复制 {} → {} 失败：{e}", from.display(), to.display()))?;
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o755));
    Ok(())
}

fn write_root_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, contents).map_err(|e| format!("写入 {} 失败：{e}", path.display()))?;
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644));
    Ok(())
}
