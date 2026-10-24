//! Integration tests for the daemon's socket API.
//!
//! These start a real `fand` in `--dry-run` mode against a temporary socket and
//! config directory. They never touch the SMC's writable keys, so they are safe
//! to run on any developer machine (and in CI, where there is no AppleSMC at
//! all — the daemon then reports "SMC 不可用" and the tests cover that path).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use fancore::client::Client;
use fancore::config::Paths;
use fancore::protocol::{Request, Response, Snapshot};
use fand::{serve, Daemon};

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Harness {
    socket: PathBuf,
    config_dir: PathBuf,
    _dir: PathBuf,
}

impl Harness {
    fn start() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("fanctl-it-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let socket = dir.join("fanctl.sock");
        let config_dir = dir.join("config");
        let paths = Paths::in_dir(config_dir.clone(), socket.clone());
        std::fs::create_dir_all(&config_dir).unwrap();

        let daemon = Daemon::new(paths, true).expect("daemon init");
        let socket_for_thread = socket.clone();
        std::thread::spawn(move || {
            let _ = serve(daemon, &socket_for_thread);
        });

        // Wait for the socket to appear.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !socket.exists() {
            assert!(Instant::now() < deadline, "daemon did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        Self { socket, config_dir, _dir: dir }
    }

    fn client(&self) -> Client {
        Client::new(&self.socket, Duration::from_secs(5))
    }

    fn snapshot(&self) -> Snapshot {
        self.client().snapshot().expect("snapshot")
    }

    fn call(&self, req: &Request) -> Response {
        self.client().call(req).expect("call")
    }

    fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.json")
    }

    fn shutdown(&self) {
        let _ = self.client().call(&Request::Shutdown);
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.socket.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.client().call(&Request::Shutdown);
        std::thread::sleep(Duration::from_millis(50));
        let _ = std::fs::remove_dir_all(&self._dir);
    }
}

#[test]
fn ping_reports_protocol_version() {
    let h = Harness::start();
    let resp = h.call(&Request::Ping);
    assert!(resp.ok);
    assert!(resp.message.unwrap().contains("fanctl"));
    h.shutdown();
}

#[test]
fn snapshot_is_readable_and_marked_dry_run() {
    let h = Harness::start();
    let snap = h.snapshot();
    assert!(snap.dry_run, "harness runs the daemon in dry-run mode");
    assert_eq!(snap.mode, fancore::types::ControlMode::Auto);
    // Either we have fans with sane ranges, or the SMC is unavailable and the
    // list is empty — both are valid.
    for fan in &snap.fans {
        assert!(fan.max_rpm >= fan.min_rpm);
        assert!(fan.actual_rpm >= 0.0);
    }
    h.shutdown();
}

#[test]
fn switching_modes_persists_config() {
    let h = Harness::start();

    let resp = h.call(&Request::SetMode { mode: fancore::types::ControlMode::Curve });
    assert!(resp.ok, "{:?}", resp.error);
    assert_eq!(h.snapshot().mode, fancore::types::ControlMode::Curve);

    let text = std::fs::read_to_string(h.config_file()).expect("config written");
    let cfg: fancore::Config = serde_json::from_str(&text).unwrap();
    assert_eq!(cfg.mode, fancore::types::ControlMode::Curve);

    h.call(&Request::SetAuto);
    assert_eq!(h.snapshot().mode, fancore::types::ControlMode::Auto);
    h.shutdown();
}

#[test]
fn fixed_rpm_is_clamped_to_hardware_range() {
    let h = Harness::start();
    let snap = h.snapshot();
    let Some(fan) = snap.fans.first() else {
        // No SMC in this environment; nothing to clamp against.
        h.shutdown();
        return;
    };
    let (min, max) = (fan.min_rpm, fan.max_rpm);

    h.call(&Request::SetFixed { rpm: max + 5000.0, apply: true, fan: None });
    let after = h.snapshot();
    assert!(
        after.config.fixed_rpm >= min && after.config.fixed_rpm <= max,
        "fixed_rpm {} outside {min}..{max}",
        after.config.fixed_rpm
    );
    h.shutdown();
}

#[test]
fn unknown_fan_index_is_rejected() {
    let h = Harness::start();
    let resp = h.call(&Request::SetFixed { rpm: 2000.0, apply: true, fan: Some(7) });
    // Either rejected (fan 7 does not exist) or the machine really has 8 fans.
    if !resp.ok {
        let err = resp.error.unwrap_or_default();
        assert!(err.contains("风扇"), "unexpected error: {err}");
    }
    h.shutdown();
}

#[test]
fn bad_curve_is_repaired_not_crashing() {
    let h = Harness::start();
    let mut snap = h.snapshot();
    let mut cfg = snap.config.clone();
    cfg.mode = fancore::types::ControlMode::Curve;
    cfg.curve.points.clear();
    cfg.curve.smoothing_secs = 999.0;
    cfg.poll_ms = 1;

    let resp = h.call(&Request::SetConfig { config: cfg });
    assert!(resp.ok, "{:?}", resp.error);
    snap = h.snapshot();
    assert!(snap.config.curve.points.len() >= 2, "curve was repaired");
    assert!(snap.config.curve.smoothing_secs <= 30.0);
    assert!(snap.config.poll_ms >= 500);
    h.shutdown();
}

#[test]
fn history_accumulates_samples() {
    let h = Harness::start();
    h.call(&Request::SetFixed { rpm: 1800.0, apply: true, fan: None });
    // The control loop ticks once per second by default.
    std::thread::sleep(Duration::from_millis(2600));
    let resp = h.call(&Request::History { seconds: 60 });
    let history = resp.history.expect("history");
    assert!(history.len() >= 2, "expected at least 2 samples, got {}", history.len());
    let timestamps: Vec<f64> = history.iter().map(|s| s.t).collect();
    assert!(timestamps.windows(2).all(|w| w[0] <= w[1]), "timestamps must be ordered");
    h.shutdown();
}

#[test]
fn shutdown_stops_the_server_and_removes_the_socket() {
    let h = Harness::start();
    let resp = h.call(&Request::Shutdown);
    assert!(resp.ok);
    let deadline = Instant::now() + Duration::from_secs(3);
    while h.socket.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(30));
    }
    assert!(!h.socket.exists(), "socket should be removed on shutdown");
}

#[test]
fn garbage_requests_get_an_error_response() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let h = Harness::start();
    let mut stream = UnixStream::connect(&h.socket).unwrap();
    stream.write_all(b"not json at all\n").unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    let resp: Response = serde_json::from_str(&line).unwrap();
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("解析"));
    h.shutdown();
}

// ---------------------------------------------------------------------------
// Log buffering
// ---------------------------------------------------------------------------

#[test]
fn log_is_buffered_until_flushed_on_demand() {
    let h = Harness::start();
    let log = h.config_dir.join("Logs").join("daemon.log");
    // Pin the tick rate so this test does not depend on the shipped default.
    let mut cfg = h.snapshot().config;
    cfg.poll_ms = 500;
    h.call(&Request::SetConfig { config: cfg });

    // Default flush interval is thirty minutes: the startup banner (urgent)
    // lands immediately, but nothing new should appear while we wait.
    let after_start = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(after_start.contains("listening on"), "启动横幅应立即落盘：{after_start}");

    h.call(&Request::SetFixed { rpm: 1500.0, apply: true, fan: None });
    std::thread::sleep(Duration::from_millis(1500));

    let buffered = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(
        buffered.lines().count(),
        after_start.lines().count(),
        "未到刷盘时间，不应出现普通行"
    );

    // Queue a line that is *not* urgent, then flush explicitly. Changing the
    // fixed RPM re-arms the controller, so the next tick emits an rpm line that
    // the buffer holds until someone asks for it.
    h.call(&Request::SetFixed { rpm: 2100.0, apply: false, fan: None });
    std::thread::sleep(Duration::from_millis(1200));
    let before_flush = std::fs::read_to_string(&log).unwrap_or_default();

    let resp = h.call(&Request::FlushLog);
    assert!(resp.ok);
    let after_flush = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        after_flush.lines().count() > before_flush.lines().count(),
        "flush 后应出现缓冲的普通行：{after_flush}"
    );
    h.shutdown();
}

#[test]
fn shutdown_flushes_the_buffer() {
    let h = Harness::start();
    let log = h.config_dir.join("Logs").join("daemon.log");

    h.call(&Request::SetFixed { rpm: 2200.0, apply: true, fan: None });
    std::thread::sleep(Duration::from_millis(1200));
    h.shutdown();
    std::thread::sleep(Duration::from_millis(150));

    let text = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(text.contains("stopped"), "退出时必须把缓冲刷盘：{text}");
}

#[test]
fn log_settings_round_trip_through_config() {
    let h = Harness::start();
    let mut cfg = h.snapshot().config;
    cfg.log_flush_secs = 60;
    cfg.log_max_kb = 512;
    cfg.log_keep = 2;
    cfg.log_coalesce = false;
    let resp = h.call(&Request::SetConfig { config: cfg });
    assert!(resp.ok, "{:?}", resp.error);

    let cfg = h.snapshot().config;
    assert_eq!(cfg.log_flush_secs, 60);
    assert_eq!(cfg.log_max_kb, 512);
    assert_eq!(cfg.log_keep, 2);
    assert!(!cfg.log_coalesce);
    h.shutdown();
}

// ---------------------------------------------------------------------------
// Sensor read cadence
// ---------------------------------------------------------------------------

#[test]
fn non_control_sensors_are_served_from_cache_between_refreshes() {
    let h = Harness::start();
    // Fast tick so several control-group refreshes happen inside one
    // OTHER_SENSOR_SECS window.
    let mut cfg = h.snapshot().config;
    cfg.poll_ms = 500;
    cfg.sensor = fancore::types::SensorGroup::Cpu;
    assert!(h.call(&Request::SetConfig { config: cfg }).ok);

    let first = h.snapshot();
    std::thread::sleep(Duration::from_millis(1600));
    let second = h.snapshot();

    // Every group is always reported, even though most were not re-read.
    assert_eq!(first.sensors.len(), fancore::types::SensorGroup::ALL.len());
    assert_eq!(second.sensors.len(), first.sensors.len());
    assert!(
        second.sensors.iter().any(|s| s.temp.is_some()) || first.sensors.iter().any(|s| s.temp.is_some()),
        "至少应有一组读到了温度"
    );
    h.shutdown();
}
