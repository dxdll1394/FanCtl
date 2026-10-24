//! fanctl shared library: SMC access, fan curve, protocol and config.

pub mod client;
pub mod config;
pub mod controller;
pub mod protocol;
pub mod rotation;
pub mod smc;
pub mod types;

pub use config::{Config, Paths};
pub use controller::Controller;
pub use types::{ControlMode, Curve, CurvePoint, FanMode, FanReading, SensorGroup};

/// Version string shown in the UI/CLI.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Keys that belong to one fan curve sensor group, resolved once at startup.
#[derive(Debug, Clone)]
pub struct SensorSet {
    pub group: SensorGroup,
    pub keys: Vec<String>,
}

impl SensorSet {
    pub fn resolve(group: SensorGroup, available: &[String]) -> Self {
        Self { group, keys: group.keys(available) }
    }
}

/// Average of the readings from a sensor group.
///
/// The die has many sensors spread over the cores; averaging the group gives a
/// stable signal. `None` when none of the keys could be read.
pub fn group_temp(group_keys: &[String], readings: &[(String, f64)]) -> Option<f32> {
    let mut sum = 0.0;
    let mut n = 0;
    for (key, value) in readings {
        if group_keys.iter().any(|k| k == key) {
            sum += *value as f32;
            n += 1;
        }
    }
    (n > 0).then_some(sum / n as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_temp_averages_only_members() {
        let keys = vec!["Tp01".to_string(), "Tp05".to_string()];
        let readings = vec![
            ("Tp01".to_string(), 40.0),
            ("Tp05".to_string(), 60.0),
            ("Tg0D".to_string(), 90.0),
        ];
        assert_eq!(group_temp(&keys, &readings), Some(50.0));
    }

    #[test]
    fn group_temp_none_when_absent() {
        let keys = vec!["Tp01".to_string()];
        let readings = vec![("Tg0D".to_string(), 90.0)];
        assert_eq!(group_temp(&keys, &readings), None);
    }
}
