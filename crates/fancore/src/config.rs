//! Persistent configuration and filesystem locations.
//!
//! Daemon-owned settings live here. UI-only preferences (menu bar behaviour)
//! are kept separately by `fanui` in `ui.json` because they are per-user.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::{ControlMode, Curve, SensorGroup};

/// Default poll interval for the control loop.
///
/// 2 s rather than 1 s: the SMC round trip dominates the daemon's CPU, and the
/// curve input is already exponentially smoothed over ~3 s, so halving the tick
/// rate halves the cost with no visible change in fan behaviour. Lower it with
/// `fanctl config set poll_ms=1000` if you want snappier response.
pub const DEFAULT_POLL_MS: u64 = 2_000;

/// Default log flush interval: ten minutes.
pub const DEFAULT_LOG_FLUSH_SECS: u64 = 1_800;
/// Default rotation threshold: 2 MB.
pub const DEFAULT_LOG_MAX_KB: u64 = 2_048;
/// Default number of rotated generations to keep.
pub const DEFAULT_LOG_KEEP: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// What drives the fan right now.
    pub mode: ControlMode,
    /// Which fan the policy applies to (single-fan Macs: 0).
    pub fan: u8,
    /// Temperature source for curve mode.
    pub sensor: SensorGroup,
    /// Fan curve used when `mode == Curve`.
    pub curve: Curve,
    /// RPM used when `mode == Fixed`.
    pub fixed_rpm: f64,
    /// Hard safety limit: at or above this the fan goes to max, curve or not.
    pub emergency_temp: f32,
    /// Sampling interval in milliseconds.
    pub poll_ms: u64,
    /// Return the fan to automatic control when the daemon exits.
    pub restore_auto_on_exit: bool,
    /// Seconds between log flushes. `0` writes every line immediately (the old
    /// behaviour); the default 1800 batches up to thirty minutes into one write.
    pub log_flush_secs: u64,
    /// Rotate `daemon.log` once it passes this many KB. `0` disables rotation.
    pub log_max_kb: u64,
    /// Rotated generations kept as `daemon.log.0` … `daemon.log.N`.
    pub log_keep: u32,
    /// Fold consecutive RPM updates into one summary line per flush window.
    pub log_coalesce: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: ControlMode::Auto,
            fan: 0,
            sensor: SensorGroup::Cpu,
            curve: Curve::default(),
            fixed_rpm: 1800.0,
            emergency_temp: 98.0,
            poll_ms: DEFAULT_POLL_MS,
            restore_auto_on_exit: true,
            log_flush_secs: DEFAULT_LOG_FLUSH_SECS,
            log_max_kb: DEFAULT_LOG_MAX_KB,
            log_keep: DEFAULT_LOG_KEEP,
            log_coalesce: true,
        }
    }
}

impl Config {
    /// Repair out-of-range values. Always safe to call after loading from disk.
    pub fn normalize(&mut self) {
        self.curve.normalize();
        self.fixed_rpm = self.fixed_rpm.clamp(1000.0, 8000.0);
        self.emergency_temp = self.emergency_temp.clamp(70.0, 110.0);
        self.poll_ms = self.poll_ms.clamp(500, 5_000);
        // Log knobs: never let a bad value disable rotation entirely by
        // accident (0 is an explicit opt-out and is preserved), and keep a
        // sane ceiling so a typo cannot pin 4 GB.
        self.log_flush_secs = self.log_flush_secs.min(86_400);
        self.log_max_kb = self.log_max_kb.min(1_048_576);
        self.log_keep = self.log_keep.min(16);
        if self.fan > 7 {
            self.fan = 0;
        }
    }
}

/// Where the daemon and CLI keep their state. Root-owned when installed.
#[derive(Debug, Clone)]
pub struct Paths {
    pub socket: PathBuf,
    pub config_dir: PathBuf,
    pub log_dir: PathBuf,
    pub run_dir: PathBuf,
}

impl Paths {
    /// System-wide paths used by the privileged daemon.
    ///
    /// `FANCTL_CONFIG_DIR` and `FANCTL_SOCKET` override the defaults, which is
    /// what `fand --dry-run` and the test suite use to stay inside a sandbox.
    pub fn system() -> Self {
        let mut paths = Self {
            socket: PathBuf::from("/var/run/fanctl.sock"),
            config_dir: PathBuf::from("/Library/Application Support/FanCtl"),
            log_dir: PathBuf::from("/Library/Logs/FanCtl"),
            run_dir: PathBuf::from("/var/run"),
        };
        paths.apply_env();
        paths
    }

    /// Per-user paths, used for development and `fand --dry-run`.
    pub fn user() -> Self {
        let base = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("Library/Application Support/FanCtl");
        let mut paths = Self {
            socket: std::env::temp_dir().join("fanctl.sock"),
            config_dir: base.clone(),
            log_dir: base.join("Logs"),
            run_dir: std::env::temp_dir(),
        };
        paths.apply_env();
        paths
    }

    fn apply_env(&mut self) {
        if let Some(dir) = std::env::var_os("FANCTL_CONFIG_DIR") {
            let dir = PathBuf::from(dir);
            self.log_dir = dir.join("Logs");
            self.config_dir = dir;
        }
        if let Some(path) = std::env::var_os("FANCTL_SOCKET") {
            self.socket = PathBuf::from(path);
        }
    }

    /// Explicit paths, used by tests and `fand --dry-run`.
    pub fn in_dir(config_dir: PathBuf, socket: PathBuf) -> Self {
        Self { log_dir: config_dir.join("Logs"), config_dir, socket, run_dir: std::env::temp_dir() }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.json")
    }

    pub fn log_file(&self) -> PathBuf {
        self.log_dir.join("daemon.log")
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.config_dir)?;
        fs::create_dir_all(&self.log_dir)?;
        Ok(())
    }

    pub fn load_config(&self) -> Config {
        match fs::read_to_string(self.config_file()) {
            Ok(text) => match serde_json::from_str::<Config>(&text) {
                Ok(mut cfg) => {
                    cfg.normalize();
                    cfg
                }
                Err(_) => Config::default(),
            },
            Err(_) => Config::default(),
        }
    }

    /// Atomically persist the config, creating the directories if needed.
    pub fn save_config(&self, cfg: &Config) -> std::io::Result<()> {
        self.ensure_dirs()?;
        let path = self.config_file();
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(cfg).unwrap_or_else(|_| "{}".into());
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.write_all(b"\n")?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &path)?;
        restrict_permissions(&path);
        Ok(())
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

// Minimal home-directory lookup so the library does not depend on the `dirs` crate.
mod dirs {
    use std::path::PathBuf;

    pub fn home_dir() -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let mut c = Config::default();
        c.normalize();
        assert_eq!(c.mode, ControlMode::Auto);
        assert!(c.curve.is_reasonable());
        assert!(c.poll_ms >= 500);
    }

    #[test]
    fn normalize_clamps_bad_values() {
        let mut c =
            Config { fixed_rpm: 10.0, emergency_temp: 500.0, poll_ms: 1, ..Config::default() };
        c.normalize();
        assert_eq!(c.fixed_rpm, 1000.0);
        assert_eq!(c.emergency_temp, 110.0);
        assert_eq!(c.poll_ms, 500);
    }

    #[test]
    fn config_roundtrip_json() {
        let c = Config {
            mode: ControlMode::Curve,
            sensor: SensorGroup::PCore,
            fixed_rpm: 2400.0,
            ..Config::default()
        };
        let text = serde_json::to_string(&c).unwrap();
        let back: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(c, back);
    }
}
