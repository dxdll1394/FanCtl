//! `fanctl` – command line front end for the fanctl daemon.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use fancore::client::{Client, ClientError};
use fancore::config::Paths;
use fancore::protocol::{Request, Response, Snapshot};
use fancore::smc::Smc;
use fancore::types::{ControlMode, SensorGroup};

mod daemon_install;

const SOCKET_ENV: &str = "FANCTL_SOCKET";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("错误：{e}");
            ExitCode::FAILURE
        }
    }
}

fn socket_path() -> PathBuf {
    std::env::var_os(SOCKET_ENV).map(PathBuf::from).unwrap_or_else(|| Paths::system().socket)
}

fn client() -> Client {
    Client::new(socket_path(), Duration::from_secs(5))
}

fn usage() {
    println!(
        r#"fanctl {version} — Apple Silicon 风扇控制

用法: fanctl <命令> [参数]

命令:
  status [--json]              显示风扇转速、温度与控制状态
  sensors [--json]             列出所有温度传感器分组
  set <rpm> [--fan N]          固定转速（立即生效）
  auto                         交还系统自动控制
  curve [--sensor 组]          启用温度曲线控制
  history [--seconds N]        打印历史采样（JSON 行）
  log [--flush]                查看日志大小/轮转状态；--flush 立即落盘
  doctor                       自检：SMC、守护进程、权限、冲突程序
  selftest                     实机验证写入：小幅升降转速并自动还原

配置:
  config show [--json]         显示当前配置
  config set key=value ...     修改配置，例如 sensor=cpu mode=curve
  config reload                让守护进程重新读取配置文件

守护进程:
  daemon run [--dry-run]       前台运行（需要 root 才能真正写风扇）
  daemon install               安装为 launchd 服务（需要 root）
  daemon uninstall             卸载 launchd 服务（需要 root）
  daemon status                查看服务是否在运行

环境变量:
  {env}        自定义 socket 路径（调试用）
"#,
        version = fancore::VERSION,
        env = SOCKET_ENV
    );
}

fn run(args: &[String]) -> Result<(), String> {
    let Some(cmd) = args.first().map(|s| s.as_str()) else {
        usage();
        return Ok(());
    };
    match cmd {
        "status" => cmd_status(args),
        "sensors" => cmd_sensors(args),
        "set" => cmd_set(args),
        "auto" => cmd_auto(),
        "curve" => cmd_curve(args),
        "history" => cmd_history(args),
        "log" => cmd_log(args),
        "doctor" => cmd_doctor(),
        "selftest" => cmd_selftest(args),
        "config" => cmd_config(args),
        "daemon" => daemon_install::cmd_daemon(&args[1..], &socket_path()),
        "--help" | "-h" | "help" => {
            usage();
            Ok(())
        }
        "--version" | "-V" | "version" => {
            println!("fanctl {}", fancore::VERSION);
            Ok(())
        }
        other => Err(format!("未知命令 {other}，用 fanctl --help 查看用法")),
    }
}

/// Is something actually listening on this unix socket?
///
/// A socket *file* only proves that someone once listened there: the kernel
/// keeps the inode after the owning process dies unless it unlinks on exit
/// (`fand` does, most scripts do not). Connecting is the honest test — a live
/// daemon accepts, a stale file is refused immediately.
fn socket_is_live(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn cmd_status(args: &[String]) -> Result<(), String> {
    let snap = fetch_snapshot()?;
    if has_flag(args, "--json") {
        println!("{}", serde_json::to_string_pretty(&snap).map_err(|e| e.to_string())?);
        return Ok(());
    }
    let permission = if snap.dry_run {
        "（dry-run 只读）"
    } else if snap.privileged {
        ""
    } else {
        "（无写权限）"
    };
    println!("模式      {}{permission}", snap.mode.label());
    println!("状态      {}", snap.status);
    for fan in &snap.fans {
        println!(
            "风扇 {}    {:.0} rpm（目标 {:.0}，范围 {:.0}–{:.0}，{}）",
            fan.index,
            fan.actual_rpm,
            fan.target_rpm,
            fan.min_rpm,
            fan.max_rpm,
            fan.mode.label()
        );
    }
    for s in &snap.sensors {
        match s.temp {
            Some(v) => println!(
                "温度 {:<8} {:5.1}°C（峰值 {:>5.1}°C，{} 个传感器）",
                s.name,
                v,
                s.peak.unwrap_or(v),
                s.keys.len()
            ),
            None => println!("温度 {:<8} 不可用", s.name),
        }
    }
    for w in &snap.warnings {
        println!("注意      {w}");
    }
    Ok(())
}

fn cmd_sensors(args: &[String]) -> Result<(), String> {
    let snap = fetch_snapshot()?;
    if has_flag(args, "--json") {
        println!("{}", serde_json::to_string_pretty(&snap.sensors).map_err(|e| e.to_string())?);
        return Ok(());
    }
    for s in &snap.sensors {
        let temp = s.temp.map(|v| format!("{v:.1}°C")).unwrap_or_else(|| "—".into());
        println!("{:<10} {:>7}  {}", s.name, temp, s.keys.join(" "));
    }
    Ok(())
}

fn cmd_set(args: &[String]) -> Result<(), String> {
    let rpm_text = args.get(1).ok_or("用法: fanctl set <rpm> [--fan N]")?;
    let rpm: f64 = rpm_text.parse().map_err(|_| format!("转速 {rpm_text:?} 不是数字"))?;
    let Some(fan) = parse_fan_flag(args)? else {
        let resp = client()
            .call(&Request::SetFixed { rpm, apply: true, fan: None })
            .map_err(|e| e.to_string())?;
        return finish_set(resp);
    };
    let resp = client()
        .call(&Request::SetFixed { rpm, apply: true, fan: Some(fan) })
        .map_err(|e| e.to_string())?;
    finish_set(resp)
}

/// Parse `--fan N` from a subcommand's arguments.
fn parse_fan_flag(args: &[String]) -> Result<Option<u8>, String> {
    let mut fan = None;
    let mut i = args.len().min(2);
    while i < args.len() {
        match args[i].as_str() {
            "--fan" => {
                i += 1;
                let value = args.get(i).ok_or("--fan 需要一个编号")?;
                fan = Some(value.parse::<u8>().map_err(|_| format!("风扇编号 {value:?} 无效"))?);
            }
            other => return Err(format!("set: 未知参数 {other}")),
        }
        i += 1;
    }
    Ok(fan)
}

fn finish_set(resp: Response) -> Result<(), String> {
    if !resp.ok {
        return Err(resp.error.unwrap_or_else(|| "设置失败".into()));
    }
    println!("{}", resp.message.unwrap_or_default());
    if let Some(s) = resp.snapshot {
        print_fan_line(&s);
    }
    Ok(())
}

fn cmd_auto() -> Result<(), String> {
    let resp = client().call(&Request::SetAuto).map_err(|e| e.to_string())?;
    if !resp.ok {
        return Err(resp.error.unwrap_or_else(|| "操作失败".into()));
    }
    println!("{}", resp.message.unwrap_or_default());
    Ok(())
}

fn cmd_curve(args: &[String]) -> Result<(), String> {
    let mut group = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--sensor" => {
                i += 1;
                let name = args.get(i).ok_or("--sensor 需要一个值")?;
                group = Some(parse_group(name)?);
            }
            other => return Err(format!("curve: 未知参数 {other}")),
        }
        i += 1;
    }
    let c = client();
    let snap = c.snapshot().map_err(|e| e.to_string())?;
    let mut cfg = snap.config.clone();
    cfg.mode = ControlMode::Curve;
    if let Some(g) = group {
        cfg.sensor = g;
    }
    cfg.normalize();
    let resp = c.call(&Request::SetConfig { config: cfg }).map_err(|e| e.to_string())?;
    if !resp.ok {
        return Err(resp.error.unwrap_or_else(|| "切换失败".into()));
    }
    let snap = resp.snapshot.unwrap_or(snap);
    println!(
        "已启用温度曲线：{} · 平滑 {:.0}s · 回差 {:.0} rpm",
        snap.config.sensor.label(),
        snap.config.curve.smoothing_secs,
        snap.config.curve.rpm_deadband
    );
    Ok(())
}

fn parse_group(name: &str) -> Result<SensorGroup, String> {
    match name.to_ascii_lowercase().replace('-', "_").as_str() {
        "cpu" => Ok(SensorGroup::Cpu),
        "pcore" | "p_core" | "p" => Ok(SensorGroup::PCore),
        "ecore" | "e_core" | "e" => Ok(SensorGroup::ECore),
        "gpu" => Ok(SensorGroup::Gpu),
        "soc" => Ok(SensorGroup::Soc),
        "storage" | "ssd" => Ok(SensorGroup::Storage),
        "ambient" | "env" => Ok(SensorGroup::Ambient),
        other => Err(format!("未知传感器组 {other}（cpu/pcore/ecore/gpu/soc/storage/ambient）")),
    }
}

fn cmd_history(args: &[String]) -> Result<(), String> {
    let mut seconds = 300u64;
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--seconds" {
            i += 1;
            seconds = args.get(i).and_then(|s| s.parse().ok()).ok_or("--seconds 需要一个整数")?;
        }
        i += 1;
    }
    let resp = client().call(&Request::History { seconds }).map_err(|e| e.to_string())?;
    if !resp.ok {
        return Err(resp.error.unwrap_or_else(|| "读取历史失败".into()));
    }
    for s in resp.history.unwrap_or_default() {
        println!("{}", serde_json::to_string(&s).map_err(|e| e.to_string())?);
    }
    Ok(())
}

fn cmd_log(args: &[String]) -> Result<(), String> {
    if has_flag(args, "--flush") {
        let resp = client().call(&Request::FlushLog).map_err(|e| e.to_string())?;
        if !resp.ok {
            return Err(resp.error.unwrap_or_else(|| "刷盘失败".into()));
        }
        println!("{}", resp.message.unwrap_or_default());
    }
    // Follow the socket's sandbox: a dry-run daemon keeps its log under
    // `FANCTL_CONFIG_DIR`, not under /Library/Logs.
    let path = if std::env::var_os("FANCTL_CONFIG_DIR").is_some() {
        Paths::user().log_file()
    } else {
        Paths::system().log_file()
    };
    let live = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!("{}  {:.1} KB", path.display(), live as f64 / 1024.0);
    let mut total = live;
    for i in 0..16u32 {
        let archive = PathBuf::from(format!("{}.{i}", path.display()));
        match std::fs::metadata(&archive) {
            Ok(m) => {
                println!("{}  {:.1} KB", archive.display(), m.len() as f64 / 1024.0);
                total += m.len();
            }
            Err(_) => break,
        }
    }
    println!("合计 {:.1} KB（含归档）", total as f64 / 1024.0);
    if let Ok(snap) = fetch_snapshot() {
        let c = &snap.config;
        let flush = if c.log_flush_secs == 0 {
            "每行立即写".to_string()
        } else {
            format!("每 {} 秒", c.log_flush_secs)
        };
        println!("刷盘间隔  {flush}");
        if c.log_max_kb == 0 {
            println!("轮转      关闭");
        } else {
            println!("轮转      {} KB × 保留 {} 份", c.log_max_kb, c.log_keep);
        }
        println!("合并      {}", if c.log_coalesce { "开" } else { "关" });
    }
    Ok(())
}

fn cmd_config(args: &[String]) -> Result<(), String> {
    let sub = args.get(1).map(|s| s.as_str()).unwrap_or("show");
    match sub {
        "show" => {
            let snap = fetch_snapshot()?;
            if has_flag(args, "--json") {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&snap.config).map_err(|e| e.to_string())?
                );
            } else {
                println!("模式        {}", snap.config.mode.label());
                println!("传感器      {}", snap.config.sensor.label());
                println!("固定转速    {:.0} rpm", snap.config.fixed_rpm);
                println!("采样间隔    {} ms", snap.config.poll_ms);
                println!("紧急温度    {:.0}°C", snap.config.emergency_temp);
                let flush = if snap.config.log_flush_secs == 0 {
                    "每行立即写".to_string()
                } else {
                    format!("{} 秒", snap.config.log_flush_secs)
                };
                println!("日志刷盘    {}", flush);
                if snap.config.log_max_kb == 0 {
                    println!("日志轮转    关闭");
                } else {
                    println!(
                        "日志轮转    {} KB × 保留 {} 份",
                        snap.config.log_max_kb, snap.config.log_keep
                    );
                }
                println!(
                    "日志合并    {}",
                    if snap.config.log_coalesce { "开（转速变化折叠成一行）" } else { "关" }
                );
                println!(
                    "退出恢复    {}",
                    if snap.config.restore_auto_on_exit { "是" } else { "否" }
                );
                println!("曲线点      {} 个", snap.config.curve.points.len());
                for p in &snap.config.curve.points {
                    println!("  {:>5.1}°C → {:>5.0} rpm", p.temp, p.rpm);
                }
            }
            Ok(())
        }
        "reload" => {
            let resp = client().call(&Request::Reload).map_err(|e| e.to_string())?;
            if !resp.ok {
                return Err(resp.error.unwrap_or_else(|| "重载失败".into()));
            }
            println!("{}", resp.message.unwrap_or_default());
            Ok(())
        }
        "set" => {
            let c = client();
            let snap = c.snapshot().map_err(|e| e.to_string())?;
            let mut cfg = snap.config.clone();
            for pair in &args[2..] {
                let (key, value) =
                    pair.split_once('=').ok_or_else(|| format!("{pair:?} 不是 key=value 形式"))?;
                apply_setting(&mut cfg, key, value)?;
            }
            cfg.normalize();
            let resp = c.call(&Request::SetConfig { config: cfg }).map_err(|e| e.to_string())?;
            if !resp.ok {
                return Err(resp.error.unwrap_or_else(|| "保存失败".into()));
            }
            println!("{}", resp.message.unwrap_or_default());
            Ok(())
        }
        other => Err(format!("config: 未知子命令 {other}（show/set/reload）")),
    }
}

fn apply_setting(cfg: &mut fancore::Config, key: &str, value: &str) -> Result<(), String> {
    match key {
        "mode" => {
            cfg.mode = match value {
                "auto" => ControlMode::Auto,
                "fixed" => ControlMode::Fixed,
                "curve" => ControlMode::Curve,
                other => return Err(format!("未知模式 {other}")),
            }
        }
        "sensor" => cfg.sensor = parse_group(value)?,
        "rpm" | "fixed_rpm" => {
            cfg.fixed_rpm = value.parse().map_err(|_| "rpm 需要数字".to_string())?
        }
        "emergency" | "emergency_temp" => {
            cfg.emergency_temp = value.parse().map_err(|_| "温度需要数字".to_string())?
        }
        "poll_ms" => cfg.poll_ms = value.parse().map_err(|_| "poll_ms 需要整数".to_string())?,
        "log_flush" | "log_flush_secs" => {
            cfg.log_flush_secs =
                value.parse().map_err(|_| "log_flush 需要整秒数（0=每行立即写）".to_string())?
        }
        "log_max_kb" => {
            cfg.log_max_kb =
                value.parse().map_err(|_| "log_max_kb 需要整数（0=不轮转）".to_string())?
        }
        "log_keep" => {
            cfg.log_keep = value.parse().map_err(|_| "log_keep 需要整数份数".to_string())?
        }
        "log_coalesce" => cfg.log_coalesce = parse_bool(value)?,
        "restore_auto_on_exit" => cfg.restore_auto_on_exit = parse_bool(value)?,
        "smoothing" => {
            cfg.curve.smoothing_secs =
                value.parse().map_err(|_| "smoothing 需要数字".to_string())?
        }
        "deadband" => {
            cfg.curve.rpm_deadband = value.parse().map_err(|_| "deadband 需要数字".to_string())?
        }
        "max_rate" => {
            cfg.curve.max_rpm_per_sec =
                value.parse().map_err(|_| "max_rate 需要数字".to_string())?
        }
        "max_temp" => {
            cfg.curve.max_temp = value.parse().map_err(|_| "max_temp 需要数字".to_string())?
        }
        other => return Err(format!("未知配置项 {other}")),
    }
    Ok(())
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(format!("{other:?} 不是布尔值")),
    }
}

fn fetch_snapshot() -> Result<Snapshot, String> {
    match client().snapshot() {
        Ok(s) => Ok(s),
        Err(ClientError::NotRunning(_)) => direct_snapshot(),
        Err(e) => Err(e.to_string()),
    }
}

fn direct_snapshot() -> Result<Snapshot, String> {
    let cfg = Paths::system().load_config();
    let smc = Smc::open().map_err(|e| e.to_string())?;
    let fans = smc.read_fans();
    let all = smc.all_keys();
    let temp_keys: Vec<String> =
        all.iter().filter(|k| k.starts_with('T') && k.len() == 4).cloned().collect();
    let raw = smc.read_temperatures(&temp_keys);
    let sensors = SensorGroup::ALL
        .iter()
        .map(|g| {
            let keys = g.keys(&all);
            fancore::protocol::SensorReading {
                group: *g,
                name: g.label().to_string(),
                temp: fancore::group_temp(&keys, &raw),
                peak: raw
                    .iter()
                    .filter(|(k, _)| keys.iter().any(|x| x == k))
                    .map(|(_, v)| *v as f32)
                    .fold(None, |acc: Option<f32>, v| Some(acc.map_or(v, |a| a.max(v)))),
                keys,
            }
        })
        .collect();
    Ok(Snapshot {
        version: fancore::protocol::PROTOCOL_VERSION,
        pid: 0,
        uptime_secs: 0,
        config: cfg.clone(),
        fans,
        sensors,
        control_temp: None,
        target_rpm: None,
        status: "后台服务未运行（以下为直接读取的硬件数据）".into(),
        warnings: vec!["后台服务未运行：只能查看，不能改转速".into()],
        privileged: false,
        dry_run: true,
        mode: cfg.mode,
    })
}

fn print_fan_line(s: &Snapshot) {
    if let Some(f) = s.fans.first() {
        println!("当前 {:.0} rpm（目标 {:.0}）", f.actual_rpm, f.target_rpm);
    }
}

/// Prove the write path works on this machine: nudge the fan up, confirm the
/// hardware followed, then hand control back exactly where it started.
fn cmd_selftest(args: &[String]) -> Result<(), String> {
    let step: f64 = {
        let mut value = 300.0;
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--step" => {
                    i += 1;
                    value =
                        args.get(i).and_then(|s| s.parse().ok()).ok_or("--step 需要一个数字")?;
                }
                other => return Err(format!("selftest: 未知参数 {other}")),
            }
            i += 1;
        }
        value
    };

    let c = client();
    let before = c.snapshot().map_err(|e| e.to_string())?;
    if before.dry_run {
        return Err("后台服务运行在只读预览模式，无法验证写入；请用 sudo 正常启动".into());
    }
    let fan = *before.fans.first().ok_or("本机没有检测到风扇，无法自检")?;
    let original_mode = before.mode;
    let start = fan.actual_rpm;
    let bump = (start + step).clamp(fan.min_rpm, fan.max_rpm);
    if (bump - start).abs() < 50.0 {
        println!("风扇已经在边界（{start:.0} rpm），改测降低 {} rpm", step as i64);
    }

    println!("1/3 记录当前状态：{:.0} rpm，模式 {}", start, original_mode.label());
    println!("2/3 请求 {:.0} rpm 并观察 5 秒…", bump);
    let resp = c
        .call(&Request::SetFixed { rpm: bump, apply: true, fan: None })
        .map_err(|e| e.to_string())?;
    if !resp.ok {
        return Err(resp.error.unwrap_or_else(|| "写入被拒绝".into()));
    }
    std::thread::sleep(Duration::from_secs(5));
    let after = c.snapshot().map_err(|e| e.to_string())?;
    let actual = after.fans.first().map(|f| f.actual_rpm).unwrap_or(0.0);
    let moved = (actual - start).abs();
    let target = after.fans.first().map(|f| f.target_rpm).unwrap_or(0.0);
    println!("    实际 {actual:.0} rpm（目标 {target:.0}），变化 {moved:.0} rpm");

    println!("3/3 还原为{}", original_mode.label());
    let restore = match original_mode {
        ControlMode::Auto => c.call(&Request::SetAuto),
        ControlMode::Fixed => {
            c.call(&Request::SetFixed { rpm: before.config.fixed_rpm, apply: true, fan: None })
        }
        ControlMode::Curve => c.call(&Request::SetConfig { config: before.config.clone() }),
    }
    .map_err(|e| e.to_string())?;
    if !restore.ok {
        eprintln!("还原失败：{}", restore.error.unwrap_or_default());
    }

    if moved >= 50.0 && (target - bump).abs() < 100.0 {
        println!("\n✔ 写入链路正常：目标转速被 SMC 接受，风扇确实跟着变化。");
        Ok(())
    } else if (target - bump).abs() >= 100.0 {
        Err(format!(
            "目标转速被改回去了（请求 {bump:.0}，实际目标 {target:.0}）。\n             通常是有别的风扇软件在抢控制权，先退掉它再试：fanctl doctor"
        ))
    } else {
        Err(format!(
            "目标转速正确但风扇转速没跟上（{start:.0} → {actual:.0} rpm）。\n             可能被系统限制，或多等几秒；也可能是风扇被禁用（FOff/FEna）。"
        ))
    }
}

fn cmd_doctor() -> Result<(), String> {
    println!("fanctl {}", fancore::VERSION);
    let mut ok = true;

    match Smc::open() {
        Ok(smc) => {
            let fans = smc.read_fans();
            println!("✔ AppleSMC 可访问，风扇 {} 个", fans.len());
            if fans.is_empty() {
                println!("  注意：没有检测到风扇（可能是无风扇机型）");
            }
        }
        Err(e) => {
            ok = false;
            println!("✘ AppleSMC 不可访问：{e}");
        }
    }

    let socket = socket_path();
    if socket.exists() {
        match client().call(&Request::Ping) {
            Ok(r) if r.ok => println!("✔ 后台服务在线：{}", socket.display()),
            Ok(_) => {
                ok = false;
                println!("✘ 后台服务响应异常");
            }
            Err(e) => {
                ok = false;
                println!("✘ 无法连接 {}：{e}", socket.display());
            }
        }
    } else {
        println!("· 后台服务未运行（{} 不存在）", socket.display());
        println!("  启动：sudo fanctl daemon install   或   sudo fand");
    }

    if fancore::smc::is_root() {
        println!("✔ 当前是 root，可以写入风扇");
    } else {
        println!("· 当前不是 root（查看正常，写入需要后台服务）");
    }

    for other in ["/var/run/fankit.sock"] {
        let path = Path::new(other);
        if !path.exists() {
            continue;
        }
        if socket_is_live(path) {
            println!("! 发现另一个风扇守护进程 {other}，同时运行会互相抢风扇");
            ok = false;
        } else {
            println!("· 发现残留的 socket 文件 {other}（没有进程在监听，可安全删除）");
            println!("  sudo rm {other}");
        }
    }
    // Third-party fan apps rewrite F0Tg behind our back; flag the usual suspects.
    const KNOWN_APPS: &[(&str, &str)] = &[
        ("/Applications/Macs Fan Control.app", "Macs Fan Control"),
        ("/Applications/iStat Menus.app", "iStat Menus"),
        ("/Applications/TG Pro.app", "TG Pro"),
        ("/Applications/Fanny.app", "Fanny"),
        ("/Applications/Stats.app", "Stats"),
    ];
    for (path, name) in KNOWN_APPS {
        if Path::new(path).exists() {
            println!("· 检测到 {name}（{path}）");
            println!("  如果它的风扇控制也开着，会和 fanctl 抢 F0Tg，请只保留一个");
        }
    }
    if ok {
        Ok(())
    } else {
        Err("自检发现问题".into())
    }
}

#[allow(dead_code)]
fn print_response(resp: Response) {
    if let Some(m) = resp.message {
        println!("{m}");
    }
    if let Some(e) = resp.error {
        eprintln!("{e}");
    }
}
