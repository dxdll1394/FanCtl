//! Shared data types: sensor groups, fan readings, curves, configuration.

use serde::{Deserialize, Serialize};

/// How the SMC drives a fan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FanMode {
    Auto,
    Manual,
    Unknown,
}

impl FanMode {
    pub fn from_raw(v: i32) -> Self {
        match v {
            0 => FanMode::Auto,
            1 => FanMode::Manual,
            _ => FanMode::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FanMode::Auto => "自动",
            FanMode::Manual => "手动",
            FanMode::Unknown => "未知",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct FanReading {
    pub index: u8,
    pub actual_rpm: f64,
    pub target_rpm: f64,
    pub min_rpm: f64,
    pub max_rpm: f64,
    pub mode: FanMode,
}

/// A named set of SMC keys that plays the role of one virtual temperature.
///
/// Apple Silicon has ~170 temperature keys but only a handful are useful for a
/// fan curve. Grouping keeps the UI readable and matches how iStat/TG Pro
/// present things.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorGroup {
    /// CPU die hotspot (`TCMz`/`Tp3X`/`Tp0f`…), the fastest-reacting peak.
    Cpu,
    /// P-core cluster sensors (`Tp0x`, `Tp1x`, `Tp3x`).
    PCore,
    /// E-core cluster sensors (`Te0x`).
    ECore,
    /// GPU cluster sensors (`Tg0x`).
    Gpu,
    /// Combo/SoC-level (`TVD0` etc.).
    Soc,
    /// Storage / NAND (`TH0x`, `Ts0x`).
    Storage,
    /// Ambient / enclosure (`Ta0p`, `TPDx`, `TPDX`).
    Ambient,
}

impl SensorGroup {
    pub const ALL: [SensorGroup; 7] = [
        SensorGroup::Cpu,
        SensorGroup::PCore,
        SensorGroup::ECore,
        SensorGroup::Gpu,
        SensorGroup::Soc,
        SensorGroup::Storage,
        SensorGroup::Ambient,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SensorGroup::Cpu => "CPU 热点",
            SensorGroup::PCore => "性能核",
            SensorGroup::ECore => "能效核",
            SensorGroup::Gpu => "GPU",
            SensorGroup::Soc => "SoC",
            SensorGroup::Storage => "存储",
            SensorGroup::Ambient => "环境",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            SensorGroup::Cpu => "封装内最高温，响应最快，最接近 Apple 自己的调速依据",
            SensorGroup::PCore => "P 核集群温度，重负载时升温明显",
            SensorGroup::ECore => "E 核集群温度，轻负载时更灵敏",
            SensorGroup::Gpu => "GPU 集群温度，渲染/视频编码时升高",
            SensorGroup::Soc => "SoC 供电/封装复合温度",
            SensorGroup::Storage => "SSD / NAND 温度",
            SensorGroup::Ambient => "机箱进风与内存等环境传感器",
        }
    }

    /// Pick the SMC keys whose readings belong to this group.
    ///
    /// Returns an empty list for groups this machine does not expose.
    pub fn keys(self, available: &[String]) -> Vec<String> {
        let has = |k: &str| available.iter().any(|a| a == k);
        let mut out = Vec::new();
        let mut push = |k: &str| {
            if has(k) && !out.iter().any(|x| x == k) {
                out.push(k.to_string());
            }
        };
        match self {
            SensorGroup::Cpu => {
                // Priority order: die hotspot first, then the per-cluster peaks.
                for k in ["TCMz", "TCMb", "Tp1S", "Tp3T", "Tp3X", "Tp0f", "Tp1C", "Tp0E"] {
                    push(k);
                }
            }
            SensorGroup::PCore => {
                for k in [
                    "Tp01", "Tp05", "Tp09", "Tp0D", "Tp0V", "Tp0Z", "Tp0b", "Tp0f", "Tp0e", "Tp1B",
                    "Tp1F", "Tp1R", "Tp1G", "Tp1C", "Tp3O", "Tp3S", "Tp3W", "Tp3P", "Tp3X", "Tp3T",
                    "Tp0Y", "Tp0W", "Tp0c",
                ] {
                    push(k);
                }
            }
            SensorGroup::ECore => {
                for k in
                    ["Te05", "Te09", "Te0A", "Te0H", "Te0S", "Te0V", "Te0I", "Te0T", "Te06", "Te0X"]
                {
                    push(k);
                }
            }
            SensorGroup::Gpu => {
                for k in [
                    "Tg0D", "Tg0H", "Tg0L", "Tg0P", "Tg0V", "Tg0d", "Tg0k", "Tg0n", "Tg0j", "Tg0e",
                    "Tg0Y", "Tg0U", "Tg0C", "Tg0G", "Tg0K", "Tg0O", "Tg0X", "Tg0m",
                ] {
                    push(k);
                }
            }
            SensorGroup::Soc => {
                for k in ["TVD0", "TVV0", "TVS0", "TVS1", "TVA0", "TPD0", "TPD5", "TPD7", "TPDX"] {
                    push(k);
                }
            }
            SensorGroup::Storage => {
                for k in ["TH0a", "TH0b", "TH0x", "TH0p", "Ts0E", "Ts0i", "Ts0R", "Ts0U"] {
                    push(k);
                }
            }
            SensorGroup::Ambient => {
                for k in
                    ["Ta0p", "TPD1", "TPD2", "TPD3", "TPD4", "TPD6", "TPD8", "TPD9", "TPDA", "TPDB"]
                {
                    push(k);
                }
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ControlMode {
    /// Let macOS / SMC decide.
    #[default]
    Auto,
    /// One fixed RPM.
    Fixed,
    /// RPM follows the selected temperature sensor through a curve.
    Curve,
}

impl ControlMode {
    pub fn label(self) -> &'static str {
        match self {
            ControlMode::Auto => "系统自动",
            ControlMode::Fixed => "固定转速",
            ControlMode::Curve => "温度曲线",
        }
    }
}

/// A point on the fan curve. `temp` °C -> `rpm`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct CurvePoint {
    pub temp: f32,
    pub rpm: f32,
}

/// A piecewise-linear fan curve plus the safety envelope applied on top of it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Curve {
    /// Ascending, 2–16 points.
    pub points: Vec<CurvePoint>,
    /// At or above this temperature the fan is forced to max.
    #[serde(default = "default_max_temp")]
    pub max_temp: f32,
    /// Below this temperature the curve holds its first point.
    #[serde(default)]
    pub spin_up_rpm: f32,
    /// Only act on temperature rises that persist this long.
    #[serde(default = "default_smoothing")]
    pub smoothing_secs: f32,
    /// Ignore RPM changes smaller than this to avoid hunting.
    #[serde(default = "default_deadband")]
    pub rpm_deadband: f32,
    /// Maximum RPM change per second (rate limiting).
    #[serde(default = "default_max_rate")]
    pub max_rpm_per_sec: f32,
}

fn default_max_temp() -> f32 {
    100.0
}
fn default_smoothing() -> f32 {
    3.0
}
fn default_deadband() -> f32 {
    60.0
}
fn default_max_rate() -> f32 {
    600.0
}

impl Default for Curve {
    fn default() -> Self {
        Self {
            points: vec![
                CurvePoint { temp: 45.0, rpm: 1000.0 },
                CurvePoint { temp: 60.0, rpm: 1400.0 },
                CurvePoint { temp: 70.0, rpm: 2200.0 },
                CurvePoint { temp: 80.0, rpm: 3200.0 },
                CurvePoint { temp: 90.0, rpm: 4300.0 },
                CurvePoint { temp: 100.0, rpm: 4900.0 },
            ],
            max_temp: 100.0,
            spin_up_rpm: 0.0,
            smoothing_secs: 3.0,
            rpm_deadband: 60.0,
            max_rpm_per_sec: 600.0,
        }
    }
}

impl Curve {
    /// Evaluate the curve at `temp` °C, clamped to the supplied hardware range.
    pub fn evaluate(&self, temp: f32, min_rpm: f32, max_rpm: f32) -> f32 {
        if !temp.is_finite() {
            return max_rpm;
        }
        if temp >= self.max_temp {
            return max_rpm;
        }
        let pts = &self.points;
        if pts.is_empty() {
            return min_rpm;
        }
        let mut rpm = if temp <= pts[0].temp {
            pts[0].rpm
        } else if temp >= pts[pts.len() - 1].temp {
            pts[pts.len() - 1].rpm
        } else {
            let mut value = pts[pts.len() - 1].rpm;
            for w in pts.windows(2) {
                let (a, b) = (w[0], w[1]);
                if temp >= a.temp && temp <= b.temp {
                    let span = (b.temp - a.temp).max(0.01);
                    let t = (temp - a.temp) / span;
                    value = a.rpm + t * (b.rpm - a.rpm);
                    break;
                }
            }
            value
        };
        if rpm < self.spin_up_rpm {
            rpm = self.spin_up_rpm;
        }
        rpm.clamp(min_rpm, max_rpm)
    }

    /// Run the curve through `validate` in place, repairing common mistakes.
    pub fn normalize(&mut self) {
        self.max_temp = self.max_temp.clamp(50.0, 115.0);
        self.smoothing_secs = self.smoothing_secs.clamp(0.0, 30.0);
        self.rpm_deadband = self.rpm_deadband.clamp(0.0, 500.0);
        self.max_rpm_per_sec = self.max_rpm_per_sec.clamp(50.0, 4900.0);
        self.points.retain(|p| p.temp.is_finite() && p.rpm.is_finite());
        self.points.sort_by(|a, b| a.temp.total_cmp(&b.temp));
        self.points.dedup_by(|a, b| (a.temp - b.temp).abs() < 0.01);
        for p in &mut self.points {
            p.temp = p.temp.clamp(0.0, 120.0);
            p.rpm = p.rpm.max(0.0);
        }
        if self.points.len() < 2 {
            let fallback = Curve::default().points;
            self.points = fallback;
        }
        if self.points.len() > 16 {
            self.points.truncate(16);
        }
    }

    pub fn is_reasonable(&self) -> bool {
        !self.points.is_empty()
            && self.points.windows(2).all(|w| w[0].temp < w[1].temp)
            && self.max_temp > self.points[0].temp
    }
}

/// Per-fan control plan computed by the daemon.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct FanTarget {
    pub index: u8,
    pub rpm: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1.0
    }

    #[test]
    fn curve_interpolates_and_clamps() {
        let c = Curve::default();
        assert!(approx(c.evaluate(40.0, 1000.0, 4900.0), 1000.0));
        assert!(approx(c.evaluate(70.0, 1000.0, 4900.0), 2200.0));
        assert!(approx(c.evaluate(75.0, 1000.0, 4900.0), 2700.0));
        assert!(approx(c.evaluate(100.0, 1000.0, 4900.0), 4900.0));
        assert!(approx(c.evaluate(120.0, 1000.0, 4900.0), 4900.0));
        // below the hardware floor
        assert!(approx(c.evaluate(0.0, 1500.0, 4900.0), 1500.0));
    }

    #[test]
    fn curve_normalize_sorts_and_dedups() {
        let mut c = Curve {
            points: vec![
                CurvePoint { temp: 80.0, rpm: 3000.0 },
                CurvePoint { temp: 50.0, rpm: 1200.0 },
                CurvePoint { temp: 80.0, rpm: 3100.0 },
            ],
            ..Curve::default()
        };
        c.normalize();
        assert_eq!(c.points.len(), 2);
        assert_eq!(c.points[0].temp, 50.0);
        assert_eq!(c.points[1].temp, 80.0);
    }

    #[test]
    fn empty_curve_falls_back_to_default() {
        let mut c = Curve { points: vec![], ..Curve::default() };
        c.normalize();
        assert!(c.points.len() >= 2);
    }

    #[test]
    fn sensor_keys_resolve_from_available() {
        let avail: Vec<String> = ["TCMz", "Tp01", "Te05", "Tg0D", "TVD0", "TH0a", "Ta0p"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(SensorGroup::Cpu.keys(&avail), vec!["TCMz"]);
        assert_eq!(SensorGroup::PCore.keys(&avail), vec!["Tp01"]);
        assert_eq!(SensorGroup::Storage.keys(&avail), vec!["TH0a"]);
        assert_eq!(SensorGroup::Gpu.keys(&avail), vec!["Tg0D"]);
    }

    #[test]
    fn missing_sensor_group_is_empty() {
        assert!(SensorGroup::Storage.keys(&[]).is_empty());
    }
}
