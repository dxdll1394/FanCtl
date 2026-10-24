//! Privileged daemon: talks to the SMC, runs the control loop, serves the UI.
//!
//! Two ways to run:
//! * `sudo fand run` – foreground, useful while developing.
//! * launchd (`local.fanctl.daemon`) – installed by `sudo fanctl daemon install`.
//!
//! `fand --dry-run` does everything except write to the SMC, which lets the
//! control loop be exercised safely on any machine.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fancore::config::Paths;
use fancore::controller::Controller;
use fancore::protocol::{
    Request, Response, Sample, SensorReading, Snapshot, TickReason, PROTOCOL_VERSION,
};
use fancore::rotation::History;
use fancore::smc::{self, Smc};
use fancore::types::{ControlMode, FanReading, SensorGroup};

pub mod logbuf;
pub use logbuf::{LogSettings, LogSink};

const MAX_REQUEST_BYTES: usize = 64 * 1024;
const SOCKET_GROUP: &str = "admin";

/// How often the non-control sensor groups are refreshed. They only feed the
/// UI, and thermal mass moves far slower than this, so 5 s is plenty while it
/// cuts the per-tick SMC traffic by an order of magnitude.
const OTHER_SENSOR_SECS: f32 = 5.0;

/// Shared state between the control loop and socket handlers.
pub struct Daemon {
    started: Instant,
    dry_run: bool,
    paths: Paths,
    config: Mutex<fancore::Config>,
    controller: Mutex<Controller>,
    /// Serializes SMC access (one IOKit connection, one writer at a time).
    smc: Mutex<Option<Smc>>,
    state: Mutex<LiveState>,
    /// Buffered, self-rotating log. Lines are held in memory and written at
    /// most once per `log_flush_secs`; state changes bypass the buffer.
    log: Mutex<LogSink>,
    /// SMC topology (key list + readable temperature keys + per-group keys).
    /// It is fixed for the lifetime of the machine, so enumerating 1369 keys
    /// on every tick is pure waste.
    topology: Mutex<Option<Arc<Topology>>>,
}

/// SMC key layout, discovered once and then shared read-only.
///
/// Held behind an `Arc` rather than cloned per tick: it carries every group's
/// key strings, and the control loop used to deep-copy all of it once a second
/// just to read a single temperature.
struct Topology {
    /// Per sensor group, the resolved key list (immutable once scanned).
    group_keys: Vec<(SensorGroup, Arc<Vec<String>>)>,
    /// Most recent per-group readings.
    cache: Mutex<SensorCache>,
}

/// Per-group temperatures plus the bookkeeping for the two refresh cadences.
#[derive(Default)]
struct SensorCache {
    /// `(group, average °C, peak °C)`, one entry per `SensorGroup::ALL`.
    readings: Vec<(SensorGroup, Option<f32>, Option<f32>)>,
    /// When the non-control groups were last refreshed.
    others_at: Option<Instant>,
}

impl Topology {
    fn scan(smc: &Smc) -> Self {
        let all_keys = smc.all_keys();
        let group_keys: Vec<(SensorGroup, Arc<Vec<String>>)> = SensorGroup::ALL
            .iter()
            .map(|g| (*g, Arc::new(g.keys(&all_keys))))
            .collect();
        let readings = SensorGroup::ALL.iter().map(|g| (*g, None, None)).collect();
        Self {
            group_keys,
            cache: Mutex::new(SensorCache { readings, others_at: None }),
        }
    }
}

#[derive(Default)]
struct LiveState {
    fans: Vec<FanReading>,
    sensors: Vec<SensorReading>,
    control_temp: Option<f32>,
    target_rpm: Option<f64>,
    status: String,
    warnings: Vec<String>,
    history: History,
    /// Last applied write, used to suppress redundant SMC writes.
    last_applied: Option<(u8, f64)>,
}

fn now_unix() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

impl Daemon {
    pub fn new(paths: Paths, dry_run: bool) -> std::io::Result<Arc<Self>> {
        // Buffered sink: opens the file ourselves instead of letting launchd's
        // inherited stdout fd do the writing, so we control when bytes hit the
        // disk and can rotate the file in place.
        let log = match paths.ensure_dirs().and_then(|()| LogSink::open(&paths.log_file())) {
            Ok(sink) => sink,
            Err(e) if dry_run => {
                eprintln!("警告：无法写入 {}（{e}），日志改到终端", paths.log_dir.display());
                LogSink::null()
            }
            Err(e) => return Err(e),
        };
        let config = paths.load_config();
        let smc = match Smc::open() {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("SMC open failed: {e}");
                None
            }
        };
        Ok(Arc::new(Self {
            started: Instant::now(),
            dry_run,
            paths,
            config: Mutex::new(config),
            controller: Mutex::new(Controller::new()),
            smc: Mutex::new(smc),
            state: Mutex::new(LiveState {
                status: "正在初始化…".into(), ..Default::default()
            }),
            log: Mutex::new(log),
            topology: Mutex::new(None),
        }))
    }

    /// Current log knobs, read fresh so `fanctl config set` applies live.
    fn log_settings(&self) -> LogSettings {
        let cfg = self.config();
        LogSettings {
            flush_secs: cfg.log_flush_secs,
            max_kb: cfg.log_max_kb,
            keep: cfg.log_keep,
            coalesce: cfg.log_coalesce,
        }
    }

    /// Queue a line that records a state change worth surviving a crash:
    /// mode switches, config saves, write failures, start/stop. Written now.
    fn log_urgent(&self, msg: &str) {
        self.log_with(msg, true);
    }

    fn log_with(&self, msg: &str, urgent: bool) {
        let settings = self.log_settings();
        match self.log.lock() {
            Ok(mut sink) => sink.log(msg, urgent, &settings),
            Err(e) => e.into_inner().log(msg, urgent, &settings),
        }
        // Interactive `fand --dry-run` writes its log under the user's home;
        // mirror to the terminal too so the developer sees the stream live.
        if self.dry_run {
            let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            eprintln!("{stamp} {msg}");
        }
    }

    /// Queue a fan RPM change; consecutive changes fold into one summary line.
    fn log_rpm(&self, fan: u8, rpm: f64, reason: &str) {
        let settings = self.log_settings();
        if let Ok(mut sink) = self.log.lock() {
            sink.log_rpm(fan, rpm, reason, &settings);
        }
    }

    /// Write anything buffered to disk right now.
    pub fn flush_log(&self) {
        let settings = self.log_settings();
        if let Ok(mut sink) = self.log.lock() {
            sink.flush(&settings);
        }
    }

    pub fn dry_run(&self) -> bool {
        self.dry_run
    }

    pub fn config(&self) -> fancore::Config {
        self.config.lock().map(|c| c.clone()).unwrap_or_default()
    }

    fn set_config(&self, cfg: fancore::Config) {
        if let Ok(mut c) = self.config.lock() {
            *c = cfg;
        }
    }

    fn with_smc<T>(&self, f: impl FnOnce(&Smc) -> T) -> Option<T> {
        let guard = self.smc.lock().ok()?;
        guard.as_ref().map(f)
    }

    /// Number of fans reported by the SMC (0 if it is unreachable).
    fn fan_count(&self) -> Option<u8> {
        self.with_smc(|smc| smc.fan_count())
    }

    /// Reject requests that name a fan this machine does not have.
    fn check_fan(&self, fan: u8) -> Result<(), String> {
        match self.fan_count() {
            Some(n) if fan < n => Ok(()),
            Some(n) => Err(format!("本机有 {n} 个风扇，没有编号 {fan}")),
            None => Err("读不到风扇数量（SMC 不可用）".to_string()),
        }
    }

    fn controller(&self) -> MutexGuard<'_, Controller> {
        self.controller.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Cached SMC topology, scanned once on first use. Returns an `Arc` clone
    /// rather than a deep copy: the old version duplicated every key string
    /// plus all per-group lists on every single tick.
    fn topology(&self) -> Option<Arc<Topology>> {
        if let Ok(cache) = self.topology.lock() {
            if let Some(topo) = cache.as_ref() {
                return Some(Arc::clone(topo));
            }
        }
        let scanned = Arc::new(self.with_smc(Topology::scan)?);
        if let Ok(mut cache) = self.topology.lock() {
            *cache = Some(Arc::clone(&scanned));
        }
        Some(scanned)
    }

    /// Read fans and temperatures.
    ///
    /// Two cadences: the group the curve controls is refreshed every tick (its
    /// latency decides how fast the fan reacts), while the other six are
    /// refreshed every `OTHER_SENSOR_SECS` and served from cache in between.
    /// Those exist only to be displayed, and at 5 s they are still far fresher
    /// than the thermal mass they describe.
    fn sample_hardware(&self, control: SensorGroup) -> (Vec<FanReading>, Vec<SensorReading>, Vec<String>) {
        let Some(topo) = self.topology() else {
            return (
                Vec::new(),
                Vec::new(),
                vec!["读不到 SMC（风扇与温度数据不可用）".to_string()],
            );
        };

        let now = Instant::now();
        // Decide what to read *before* taking the SMC lock.
        // The control group is read every tick (its latency is what the fan
        // follows); the others only once per OTHER_SENSOR_SECS.
        let read_others = {
            let cache = topo.cache.lock().unwrap_or_else(|e| e.into_inner());
            cache
                .others_at
                .map(|t| now.duration_since(t).as_secs_f32() >= OTHER_SENSOR_SECS)
                .unwrap_or(true)
        };

        // Keys to fetch this pass: the control group always, the rest only when
        // their slower deadline has come due.
        let mut want: Vec<String> = Vec::new();
        for (group, keys) in &topo.group_keys {
            if *group != control && !read_others {
                continue;
            }
            for k in keys.iter() {
                if !want.iter().any(|x| x == k) {
                    want.push(k.clone());
                }
            }
        }

        let scanned = self.with_smc(|smc| {
            let fans = smc.read_fans();
            let raw = smc.read_temperatures(&want);
            (fans, raw)
        });

        let Some((fans, raw)) = scanned else {
            return (
                Vec::new(),
                Vec::new(),
                vec!["读不到 SMC（风扇与温度数据不可用）".to_string()],
            );
        };

        // Fold the fresh readings into the cache, leaving untouched groups alone.
        let mut cache = topo.cache.lock().unwrap_or_else(|e| e.into_inner());
        for (group, keys) in &topo.group_keys {
            if *group != control && !read_others {
                continue;
            }
            let temp = fancore::group_temp(keys, &raw);
            let peak = raw
                .iter()
                .filter(|(k, _)| keys.iter().any(|x| x == k))
                .map(|(_, v)| *v as f32)
                .fold(None, |acc: Option<f32>, v| Some(acc.map_or(v, |a| a.max(v))));
            if let Some(slot) = cache.readings.iter_mut().find(|(g, _, _)| g == group) {
                slot.1 = temp;
                slot.2 = peak;
            }
        }
        if read_others {
            cache.others_at = Some(now);
        }

        // Build the snapshot-facing list from the cache. `keys` is an `Arc`
        // clone, not the per-tick deep copy the old code did for all 7 groups.
        let sensors: Vec<SensorReading> = topo
            .group_keys
            .iter()
            .map(|(group, keys)| {
                let (temp, peak) = cache
                    .readings
                    .iter()
                    .find(|(g, _, _)| g == group)
                    .map(|(_, t, p)| (*t, *p))
                    .unwrap_or((None, None));
                SensorReading {
                    group: *group,
                    name: group.label().to_string(),
                    temp,
                    peak,
                    keys: keys.as_ref().clone(),
                }
            })
            .collect();
        drop(cache);

        let mut warnings = Vec::new();
        if sensors.iter().all(|s| s.temp.is_none()) {
            warnings.push("所有温度传感器都读不到值".to_string());
        }
        (fans, sensors, warnings)
    }

    /// One control-loop iteration. Returns how long to sleep before the next.
    pub fn tick(&self) -> Duration {
        let cfg = self.config();
        let (fans, sensors, base_warnings) = self.sample_hardware(cfg.sensor);
        let control_temp =
            sensors.iter().find(|s| s.group == cfg.sensor).and_then(|s| s.temp).or_else(|| {
                sensors.iter().find(|s| s.group == SensorGroup::Cpu).and_then(|s| s.temp)
            });

        // Bounds of the fan we actually drive, falling back to fan 0 (most
        // Apple Silicon Macs only have one).
        let fan = fans.iter().find(|f| f.index == cfg.fan).or_else(|| fans.first());
        let (min_rpm, max_rpm) = fan.map(|f| (f.min_rpm, f.max_rpm)).unwrap_or((1000.0, 4900.0));

        let dt = (cfg.poll_ms as f32 / 1000.0).max(0.2);
        let plan = self.controller().tick(&cfg, control_temp, min_rpm, max_rpm, dt);

        let mut warnings = base_warnings;
        let mut mode_now = cfg.mode;
        if matches!(plan.reason, TickReason::SensorLost | TickReason::Fault) {
            warnings.push(plan.reason.label().to_string());
            mode_now = ControlMode::Auto;
        }

        let mut applied: Option<f64> = None;
        if plan.release {
            // Only hand control back if we were the ones who took it. Writing
            // F0Md=0 every second would fight the SMC's own curve.
            let we_held_control =
                self.state.lock().map(|st| st.last_applied.is_some()).unwrap_or(false);
            if !self.dry_run && we_held_control {
                if let Some(Err(e)) = self.with_smc(|smc| smc.reset_all()) {
                    warnings.push(format!("恢复系统自动失败：{e}"));
                } else {
                    self.log_urgent("released: fans back to automatic");
                }
            }
            if let Ok(mut st) = self.state.lock() {
                st.last_applied = None;
            }
        } else if let Some(target) = plan.targets.first() {
            applied = Some(target.rpm);
            let changed = self
                .state
                .lock()
                .ok()
                .and_then(|st| st.last_applied)
                .map(|(i, rpm)| i != target.index || (rpm - target.rpm).abs() >= 1.0)
                .unwrap_or(true);
            if changed {
                if self.dry_run {
                    // Same buffered/coalesced path as a real run so dry-run
                    // exercises it, but also echo each step to the terminal.
                    self.log_rpm(target.index, target.rpm, plan.reason.label());
                    eprintln!(
                        "{stamp} [dry-run] fan {} → {:.0} rpm ({})",
                        target.index,
                        target.rpm,
                        plan.reason.label(),
                        stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
                    );
                    // Track it anyway so dry-run exercises the same de-dup path
                    // as a real run.
                    if let Ok(mut st) = self.state.lock() {
                        st.last_applied = Some((target.index, target.rpm));
                    }
                } else {
                    match self.with_smc(|smc| smc.set_fan_rpm(target.index, target.rpm)) {
                        Some(Ok(())) => {
                            // Buffered: consecutive changes for the same reason
                            // collapse into one "1000~1400 rpm (温度曲线, N 次调整)".
                            self.log_rpm(target.index, target.rpm, plan.reason.label());
                            if let Ok(mut st) = self.state.lock() {
                                st.last_applied = Some((target.index, target.rpm));
                            }
                        }
                        Some(Err(e)) => {
                            warnings.push(format!("写入风扇转速失败：{e}"));
                            self.log_urgent(&format!("write failed: {e}"));
                        }
                        None => warnings.push("SMC 连接不可用".to_string()),
                    }
                }
            }
        }

        let target_now = applied.or(plan.targets.first().map(|t| t.rpm));
        let status = match plan.reason {
            TickReason::Auto => "系统自动控制".to_string(),
            TickReason::Fixed => format!("固定 {:.0} rpm", target_now.unwrap_or(0.0)),
            TickReason::Curve => match control_temp {
                Some(t) => format!(
                    "{} · {:.1}°C → {:.0} rpm",
                    cfg.sensor.label(),
                    t,
                    target_now.unwrap_or(0.0)
                ),
                None => "等待温度数据".to_string(),
            },
            TickReason::Emergency => {
                format!("温度过高（{:.0}°C）→ 全速", control_temp.unwrap_or(0.0))
            }
            other => other.label().to_string(),
        };

        let sample = Sample {
            t: now_unix(),
            control_temp,
            fan_rpm: fan.map(|f| f.actual_rpm as f32),
            target_rpm: target_now.map(|v| v as f32),
            mode: mode_now,
        };

        if let Ok(mut st) = self.state.lock() {
            st.fans = fans;
            st.sensors = sensors;
            st.control_temp = control_temp;
            st.target_rpm = target_now;
            st.status = status;
            st.warnings = warnings;
            st.history.push(sample);
        }

        // Piggyback the log flush deadline on the control loop: it already runs
        // once per poll interval, so no extra thread or timer is needed. One
        // relaxed load plus a timestamp compare when nothing is buffered.
        if let Ok(mut sink) = self.log.lock() {
            sink.maybe_flush(&self.log_settings());
        }

        Duration::from_millis(cfg.poll_ms.clamp(500, 5_000))
    }

    pub fn snapshot(&self) -> Snapshot {
        let cfg = self.config();
        let (fans, sensors, control_temp, target_rpm, status, warnings) = match self.state.lock() {
            Ok(s) => (
                s.fans.clone(),
                s.sensors.clone(),
                s.control_temp,
                s.target_rpm,
                s.status.clone(),
                s.warnings.clone(),
            ),
            Err(_) => (vec![], vec![], None, None, "状态不可用".into(), vec![]),
        };
        Snapshot {
            version: PROTOCOL_VERSION,
            pid: std::process::id(),
            uptime_secs: self.started.elapsed().as_secs(),
            config: cfg.clone(),
            fans,
            sensors,
            control_temp,
            target_rpm,
            status,
            warnings,
            privileged: !self.dry_run && smc::is_root(),
            dry_run: self.dry_run,
            mode: cfg.mode,
        }
    }

    /// Apply a request. The bool is `true` when the daemon should stop.
    pub fn handle(&self, req: &Request) -> (Response, bool) {
        match req {
            Request::Ping => {
                (Response::ok_msg(format!("fanctl protocol {PROTOCOL_VERSION}")), false)
            }
            Request::Snapshot => (Response::ok().with_snapshot(self.snapshot()), false),
            Request::History { seconds } => {
                let hist = self
                    .state
                    .lock()
                    .map(|s| s.history.tail((*seconds).min(3600)))
                    .unwrap_or_default();
                (Response::ok().with_history(hist), false)
            }
            Request::SetConfig { config } => {
                if let Err(e) = self.check_fan(config.fan) {
                    return (Response::err(e), false);
                }
                let mut cfg = config.clone();
                cfg.normalize();
                self.set_config(cfg.clone());
                let saved = self.paths.save_config(&cfg).map_err(|e| format!("保存配置失败：{e}"));
                self.reset_controller(cfg.fan);
                if let Err(e) = saved {
                    return (Response::err(e), false);
                }
                self.log_urgent(&format!("config saved (mode={:?})", cfg.mode));
                (Response::ok_msg("配置已保存").with_snapshot(self.snapshot()), false)
            }
            Request::SetMode { mode } => {
                if let Err(e) = self.check_fan(self.config().fan) {
                    return (Response::err(e), false);
                }
                let mut cfg = self.config();
                cfg.mode = *mode;
                self.set_config(cfg.clone());
                let _ = self.paths.save_config(&cfg);
                self.reset_controller(cfg.fan);
                self.log_urgent(&format!("mode → {mode:?}"));
                (
                    Response::ok_msg(format!("已切换到{}", mode.label()))
                        .with_snapshot(self.snapshot()),
                    false,
                )
            }
            Request::SetFixed { rpm, apply, fan } => {
                if let Some(index) = fan {
                    if let Err(e) = self.check_fan(*index) {
                        return (Response::err(e), false);
                    }
                }
                let mut cfg = self.config();
                cfg.fixed_rpm = *rpm;
                if let Some(index) = fan {
                    cfg.fan = *index;
                }
                if *apply {
                    cfg.mode = ControlMode::Fixed;
                }
                cfg.normalize();
                self.set_config(cfg.clone());
                let _ = self.paths.save_config(&cfg);
                self.reset_controller(cfg.fan);
                (
                    Response::ok_msg(format!("固定转速 {:.0} rpm", cfg.fixed_rpm))
                        .with_snapshot(self.snapshot()),
                    false,
                )
            }
            Request::SetAuto => {
                let mut cfg = self.config();
                cfg.mode = ControlMode::Auto;
                self.set_config(cfg.clone());
                let _ = self.paths.save_config(&cfg);
                self.reset_controller(cfg.fan);
                if !self.dry_run {
                    if let Some(Err(e)) = self.with_smc(|smc| smc.reset_all()) {
                        return (Response::err(format!("恢复自动失败：{e}")), false);
                    }
                }
                self.log_urgent("mode → auto");
                (Response::ok_msg("已交还系统自动控制").with_snapshot(self.snapshot()), false)
            }
            Request::Manual { fan, rpm } => {
                if let Err(e) = self.check_fan(*fan) {
                    return (Response::err(e), false);
                }
                if self.dry_run {
                    return (
                        Response::ok_msg("dry-run：未写入 SMC").with_snapshot(self.snapshot()),
                        false,
                    );
                }
                match self.with_smc(|smc| smc.set_fan_rpm(*fan, *rpm)) {
                    Some(Ok(())) => (Response::ok_msg(format!("fan {fan} → {rpm:.0} rpm")), false),
                    Some(Err(e)) => (Response::err(e.to_string()), false),
                    None => (Response::err("SMC 不可用"), false),
                }
            }
            Request::Reload => {
                let cfg = self.paths.load_config();
                self.set_config(cfg.clone());
                self.reset_controller(cfg.fan);
                (Response::ok_msg("配置已重新载入").with_snapshot(self.snapshot()), false)
            }
            Request::FlushLog => {
                self.flush_log();
                (Response::ok_msg("日志已写入"), false)
            }
            Request::Shutdown => (Response::ok_msg("bye"), true),
        }
    }

    fn reset_controller(&self, _fan: u8) {
        self.controller().reset();
        if let Ok(mut st) = self.state.lock() {
            st.last_applied = None;
        }
    }

    pub fn restore_auto(&self) {
        if self.dry_run || !self.config().restore_auto_on_exit {
            return;
        }
        if let Some(Ok(())) = self.with_smc(|smc| smc.reset_all()) {
            self.log_urgent("exit: fans restored to automatic");
        }
    }
}

pub fn serve(daemon: Arc<Daemon>, socket: &Path) -> std::io::Result<()> {
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    if let Some(gid) = group_gid(SOCKET_GROUP) {
        let _ = chown(socket, 0, gid);
        let _ = std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o660));
    }
    listener.set_nonblocking(true)?;
    daemon.log_urgent(&format!(
        "fand {} listening on {} (dry-run={})",
        fancore::VERSION,
        socket.display(),
        daemon.dry_run()
    ));

    let shutdown = Arc::new(AtomicBool::new(false));

    // launchd stops a job with SIGTERM, and a bare SIGTERM would kill us before
    // the buffer reached the disk. Install a handler that only flips a flag
    // (async-signal-safe), then let the accept loop exit through the normal
    // path so `restore_auto` and the final flush still run.
    install_terminate_handler();

    // Control loop on its own thread so the socket stays responsive.
    {
        let daemon = Arc::clone(&daemon);
        let shutdown = Arc::clone(&shutdown);
        std::thread::spawn(move || {
            while !shutdown.load(Ordering::Relaxed) {
                let sleep_for =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| daemon.tick()))
                        .unwrap_or_else(|_| {
                            daemon.log_urgent("control loop panicked, continuing");
                            Duration::from_secs(2)
                        });
                std::thread::sleep(sleep_for);
            }
        });
    }

    // Non-blocking accept so a `shutdown` request can stop the loop without
    // waiting for another connection to wake `incoming()`.
    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                // Accepted sockets can inherit the listener's non-blocking flag
                // on macOS; the handler wants blocking reads.
                let _ = stream.set_nonblocking(false);
                let daemon = Arc::clone(&daemon);
                let conn_shutdown = Arc::clone(&shutdown);
                std::thread::spawn(move || {
                    if let Err(e) = handle_conn(&daemon, stream, &conn_shutdown) {
                        daemon.log_urgent(&format!("connection error: {e}"));
                    }
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // This 50 ms poll is also where SIGTERM is noticed, so the
                // shutdown path (flush + restore-auto) still runs on `kill`.
                if terminate_requested() {
                    shutdown.store(true, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                daemon.log_urgent(&format!("accept error: {e}"));
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }

    shutdown.store(true, Ordering::Relaxed);
    daemon.restore_auto();
    let _ = std::fs::remove_file(socket);
    // Make sure the last buffer (including "stopped") reaches the disk.
    daemon.log_urgent("stopped");
    daemon.flush_log();
    Ok(())
}

/// Process-wide flag the SIGTERM/SIGINT handler sets. A `static` because a
/// signal handler cannot capture locals; it is only ever written from the
/// handler and read by the accept loop.
static TERMINATE: AtomicBool = AtomicBool::new(false);

extern "C" fn on_terminate(_sig: libc::c_int) {
    // Only async-signal-safe work here: a relaxed store.
    TERMINATE.store(true, Ordering::Relaxed);
}

/// Route SIGTERM/SIGINT into the normal shutdown path instead of letting the
/// default action kill the process on the spot.
///
/// No watcher thread: the accept loop already wakes every 50 ms, so it polls
/// `terminate_requested()` itself. The handler stays down to a single store,
/// which is all that is async-signal-safe.
fn install_terminate_handler() {
    unsafe {
        libc::signal(libc::SIGTERM, on_terminate as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_terminate as *const () as libc::sighandler_t);
    }
}

/// Has the process been asked to stop via SIGTERM/SIGINT?
fn terminate_requested() -> bool {
    TERMINATE.load(Ordering::Relaxed)
}

pub fn handle_conn(
    daemon: &Arc<Daemon>,
    stream: UnixStream,
    shutdown: &AtomicBool,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let reply = |stream: &UnixStream, resp: &Response| -> std::io::Result<()> {
        let mut s = stream;
        s.write_all(format!("{}\n", serde_json::to_string(resp)?).as_bytes())?;
        s.flush()
    };
    if line.len() > MAX_REQUEST_BYTES {
        return reply(&stream, &Response::err("请求过大"));
    }
    let req: Request = match serde_json::from_str(line.trim()) {
        Ok(r) => r,
        Err(e) => return reply(&stream, &Response::err(format!("无法解析请求：{e}"))),
    };
    let (resp, stop) = daemon.handle(&req);
    reply(&stream, &resp)?;
    if stop {
        shutdown.store(true, Ordering::Relaxed);
    }
    Ok(())
}

pub fn group_gid(name: &str) -> Option<u32> {
    use std::ffi::CString;
    let cname = CString::new(name).ok()?;
    unsafe {
        let gr = libc::getgrnam(cname.as_ptr());
        if gr.is_null() {
            None
        } else {
            Some((*gr).gr_gid)
        }
    }
}

fn chown(path: &Path, uid: u32, gid: u32) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes())?;
    let rc = unsafe { libc::chown(c.as_ptr(), uid, gid) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// `fand --restore-auto`: one-shot hand-back used by the uninstaller so fans
/// are never left pinned in manual mode.
pub fn restore_auto_standalone() {
    if !smc::is_root() {
        eprintln!("需要 root：sudo fand --restore-auto");
        std::process::exit(1);
    }
    match Smc::open() {
        Ok(smc) => match smc.reset_all() {
            Ok(()) => println!("风扇已交还系统自动控制"),
            Err(e) => {
                eprintln!("恢复自动失败：{e}");
                std::process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("无法打开 AppleSMC：{e}");
            std::process::exit(1);
        }
    }
}
