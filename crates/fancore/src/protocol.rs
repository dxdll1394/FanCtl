//! Wire protocol between the UI/CLI and the privileged daemon.
//!
//! Newline-delimited JSON over a unix socket: one request object, one response
//! object. Deliberately boring so `socat`/`nc` can talk to it while debugging.

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::types::{ControlMode, FanReading, FanTarget};

/// Protocol version; bump when a breaking change lands.
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// Liveness and version probe.
    Ping,
    /// Everything the UI needs for one frame.
    Snapshot,
    /// Replace the whole configuration (UI edits then applies once).
    SetConfig { config: Config },
    /// Switch mode only.
    SetMode { mode: ControlMode },
    /// Set a fixed RPM (implies `mode = fixed` unless `apply` is false).
    ///
    /// `fan` is optional so older clients keep working; when present it
    /// selects which fan the fixed policy drives.
    SetFixed {
        rpm: f64,
        apply: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fan: Option<u8>,
    },
    /// Send the fan back to SMC automatic control.
    SetAuto,
    /// Live preview: force an RPM now without touching the saved config.
    Manual { fan: u8, rpm: f64 },
    /// Tail of the daemon's history buffer.
    History { seconds: u64 },
    /// Ask the daemon to stop.
    Shutdown,
    /// Reload the config file from disk.
    Reload,
    /// Write buffered log lines to disk now. Lines otherwise go out at most
    /// once every `log_flush_secs` (default ten minutes).
    FlushLog,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub pid: u32,
    pub uptime_secs: u64,
    pub config: Config,
    pub fans: Vec<FanReading>,
    /// Temperature groups in display order: (group, current °C, peak °C).
    pub sensors: Vec<SensorReading>,
    /// Temperature value the curve is currently evaluating.
    pub control_temp: Option<f32>,
    /// What the controller asked for on the last tick.
    pub target_rpm: Option<f64>,
    /// Human-readable status line, e.g. "曲线控制中 · 46.2°C → 1180 rpm".
    pub status: String,
    /// Non-fatal problems worth surfacing in the UI.
    pub warnings: Vec<String>,
    /// True when running with write access to the SMC.
    pub privileged: bool,
    /// True when the daemon is in a dry-run (read-only) session.
    pub dry_run: bool,
    /// Current effective control mode.
    pub mode: ControlMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorReading {
    pub group: crate::types::SensorGroup,
    pub name: String,
    pub temp: Option<f32>,
    pub peak: Option<f32>,
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    /// Unix seconds (fractional) for plotting.
    pub t: f64,
    pub control_temp: Option<f32>,
    pub fan_rpm: Option<f32>,
    pub target_rpm: Option<f32>,
    pub mode: ControlMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Snapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<Vec<Sample>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Response {
    pub fn ok() -> Self {
        Self { ok: true, error: None, snapshot: None, history: None, message: None }
    }

    pub fn ok_msg(msg: impl Into<String>) -> Self {
        Self { ok: true, error: None, snapshot: None, history: None, message: Some(msg.into()) }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self { ok: false, error: Some(msg.into()), snapshot: None, history: None, message: None }
    }

    pub fn with_snapshot(mut self, s: Snapshot) -> Self {
        self.snapshot = Some(s);
        self
    }

    pub fn with_history(mut self, h: Vec<Sample>) -> Self {
        self.history = Some(h);
        self
    }
}

/// What the controller decided on one tick; separated for testability.
#[derive(Debug, Clone, PartialEq)]
pub struct TickPlan {
    pub targets: Vec<FanTarget>,
    /// `true` when the fan should be handed back to the SMC.
    pub release: bool,
    pub reason: TickReason,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TickReason {
    Auto,
    Fixed,
    Curve,
    /// Temperature at/above `emergency_temp`.
    Emergency,
    /// Control temperature became unavailable.
    SensorLost,
    /// Safety rail tripped (e.g. bad curve), handing back to the SMC.
    Fault,
}

impl TickReason {
    pub fn label(&self) -> &'static str {
        match self {
            TickReason::Auto => "系统自动",
            TickReason::Fixed => "固定转速",
            TickReason::Curve => "温度曲线",
            TickReason::Emergency => "温度过高，全速",
            TickReason::SensorLost => "读不到温度，已交还系统",
            TickReason::Fault => "保护性交还系统",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip() {
        let r = Request::SetFixed { rpm: 2200.0, apply: true, fan: None };
        let text = serde_json::to_string(&r).unwrap();
        assert!(text.contains("\"cmd\":\"set_fixed\""));
        let back: Request = serde_json::from_str(&text).unwrap();
        assert!(matches!(back, Request::SetFixed { rpm, .. } if (rpm - 2200.0).abs() < 0.01));

        // Explicit fan index survives the round trip.
        let r = Request::SetFixed { rpm: 1800.0, apply: true, fan: Some(1) };
        let back: Request = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert!(matches!(back, Request::SetFixed { fan: Some(1), .. }));
    }

    #[test]
    fn response_omits_empty_fields() {
        let text = serde_json::to_string(&Response::ok_msg("hi")).unwrap();
        assert!(!text.contains("snapshot"));
        assert!(!text.contains("error"));
    }
}
