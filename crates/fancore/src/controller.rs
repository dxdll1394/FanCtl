//! Pure control logic: turn measurements + config into a fan target.
//!
//! Kept free of I/O so it can be unit-tested exhaustively. The daemon feeds it
//! samples and executes the resulting [`TickPlan`].

use crate::config::Config;
use crate::protocol::{TickPlan, TickReason};
use crate::types::{ControlMode, FanTarget};

/// Consecutive failed temperature reads before we give the fans back to macOS.
const MAX_SENSOR_MISSES: u32 = 5;

#[derive(Debug, Clone)]
pub struct Controller {
    /// Exponentially smoothed control temperature.
    smoothed: Option<f32>,
    /// RPM we last commanded (per fan index 0..8).
    last_rpm: [f64; 8],
    /// Consecutive ticks without a usable temperature.
    sensor_misses: u32,
    /// RPM change allowance, refilled at `max_rpm_per_sec` and capped so that a
    /// long idle period cannot turn into one huge jump.
    step_budget: f32,
}

/// How many seconds' worth of RPM change may be saved up while idle.
const STEP_BUDGET_SECS: f32 = 1.5;

impl Default for Controller {
    fn default() -> Self {
        Self { smoothed: None, last_rpm: [f64::NAN; 8], sensor_misses: 0, step_budget: 0.0 }
    }
}

impl Controller {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget history, e.g. after switching modes.
    pub fn reset(&mut self) {
        self.smoothed = None;
        self.last_rpm = [f64::NAN; 8];
        self.sensor_misses = 0;
        self.step_budget = 0.0;
    }

    /// Feed one measurement and get the plan for this tick.
    ///
    /// * `temp` – control temperature (already grouped), `None` if unreadable.
    /// * `fan` – current hardware reading (min/max/actual).
    /// * `dt` – seconds since the previous tick.
    pub fn tick(
        &mut self,
        cfg: &Config,
        temp: Option<f32>,
        fan_min: f64,
        fan_max: f64,
        dt: f32,
    ) -> TickPlan {
        // Refill the rate-limit budget; everything below spends from it.
        let dt = dt.max(0.0);
        let rate = cfg.curve.max_rpm_per_sec;
        self.step_budget = (self.step_budget + rate * dt).min(rate * STEP_BUDGET_SECS);

        match cfg.mode {
            ControlMode::Auto => {
                self.sensor_misses = 0;
                TickPlan { targets: vec![], release: true, reason: TickReason::Auto }
            }
            ControlMode::Fixed => {
                self.sensor_misses = 0;
                let rpm = cfg.fixed_rpm.clamp(fan_min, fan_max);
                // Record it so switching back to the curve starts from here.
                self.commit(cfg.fan, rpm, fan_min, fan_max);
                TickPlan {
                    targets: vec![FanTarget { index: cfg.fan, rpm }],
                    release: false,
                    reason: TickReason::Fixed,
                }
            }
            ControlMode::Curve => self.curve_tick(cfg, temp, fan_min, fan_max, dt),
        }
    }

    fn curve_tick(
        &mut self,
        cfg: &Config,
        temp: Option<f32>,
        fan_min: f64,
        fan_max: f64,
        dt: f32,
    ) -> TickPlan {
        let Some(raw) = temp.filter(|t| t.is_finite() && *t > -20.0 && *t < 150.0) else {
            self.sensor_misses += 1;
            if self.sensor_misses >= MAX_SENSOR_MISSES {
                // Never run blind: hand control back to the SMC.
                self.smoothed = None;
                return TickPlan { targets: vec![], release: true, reason: TickReason::SensorLost };
            }
            // Hold the previous target while the sensor blips.
            return TickPlan {
                targets: self.hold(cfg, fan_min, fan_max),
                release: false,
                reason: TickReason::Curve,
            };
        };
        self.sensor_misses = 0;

        // Exponential smoothing; tau from the config, floored so a single
        // sample never whipsaws the fan.
        let tau = cfg.curve.smoothing_secs.max(0.5);
        let alpha = (dt.max(0.0) / tau).clamp(0.02, 1.0);
        let smoothed = match self.smoothed {
            Some(prev) => prev + alpha * (raw - prev),
            None => raw,
        };
        self.smoothed = Some(smoothed);

        if !cfg.curve.is_reasonable() {
            return TickPlan { targets: vec![], release: true, reason: TickReason::Fault };
        }

        // Emergency ceiling ignores the curve entirely.
        if smoothed >= cfg.emergency_temp || raw >= cfg.emergency_temp {
            let rpm = fan_max;
            self.commit(cfg.fan, rpm, fan_min, fan_max);
            return TickPlan {
                targets: vec![FanTarget { index: cfg.fan, rpm }],
                release: false,
                reason: TickReason::Emergency,
            };
        }

        let wanted = cfg.curve.evaluate(smoothed, fan_min as f32, fan_max as f32) as f64;
        let previous = self.last_rpm[cfg.fan as usize];

        if previous.is_nan() {
            // First tick after a mode switch: go straight to the curve value.
            self.commit(cfg.fan, wanted, fan_min, fan_max);
            return TickPlan {
                targets: vec![FanTarget { index: cfg.fan, rpm: wanted }],
                release: false,
                reason: TickReason::Curve,
            };
        }

        // Deadband: ignore small changes so the fan does not hunt.
        if (wanted - previous).abs() < cfg.curve.rpm_deadband as f64 {
            return TickPlan {
                targets: vec![FanTarget { index: cfg.fan, rpm: previous }],
                release: false,
                reason: TickReason::Curve,
            };
        }

        // Rate limit, spent from the refilled budget.
        let allowance = self.step_budget.max(120.0) as f64;
        let delta = (wanted - previous).clamp(-allowance, allowance);
        let next = (previous + delta).clamp(fan_min, fan_max);

        if (next - previous).abs() >= 1.0 {
            self.commit(cfg.fan, next, fan_min, fan_max);
        }
        TickPlan {
            targets: vec![FanTarget { index: cfg.fan, rpm: next }],
            release: false,
            reason: TickReason::Curve,
        }
    }

    fn hold(&self, cfg: &Config, fan_min: f64, fan_max: f64) -> Vec<FanTarget> {
        let previous = self.last_rpm[cfg.fan as usize];
        if previous.is_nan() {
            vec![]
        } else {
            vec![FanTarget { index: cfg.fan, rpm: previous.clamp(fan_min, fan_max) }]
        }
    }

    fn commit(&mut self, fan: u8, rpm: f64, fan_min: f64, fan_max: f64) {
        let rpm = rpm.clamp(fan_min, fan_max);
        let previous = self.last_rpm[fan as usize];
        if !previous.is_nan() {
            self.step_budget = (self.step_budget - (rpm - previous).abs() as f32).max(0.0);
        }
        self.last_rpm[fan as usize] = rpm;
    }

    pub fn smoothed_temp(&self) -> Option<f32> {
        self.smoothed
    }

    pub fn last_target(&self, fan: u8) -> Option<f64> {
        let v = self.last_rpm[fan as usize];
        (!v.is_nan()).then_some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve_cfg() -> Config {
        Config {
            mode: ControlMode::Curve,
            curve: crate::Curve {
                smoothing_secs: 1.0,
                rpm_deadband: 20.0,
                max_rpm_per_sec: 5000.0,
                ..crate::Curve::default()
            },
            emergency_temp: 98.0,
            ..Config::default()
        }
    }

    #[test]
    fn auto_mode_releases() {
        let mut c = Controller::new();
        let plan = c.tick(&Config::default(), Some(50.0), 1000.0, 4900.0, 1.0);
        assert!(plan.release);
        assert_eq!(plan.reason, TickReason::Auto);
    }

    #[test]
    fn fixed_mode_clamps() {
        let cfg = Config { mode: ControlMode::Fixed, fixed_rpm: 9000.0, ..Config::default() };
        let mut c = Controller::new();
        let plan = c.tick(&cfg, Some(50.0), 1000.0, 4900.0, 1.0);
        assert!(!plan.release);
        assert_eq!(plan.targets[0].rpm, 4900.0);
    }

    #[test]
    fn curve_tracks_temperature() {
        let cfg = curve_cfg();
        let mut c = Controller::new();
        let cool = c.tick(&cfg, Some(45.0), 1000.0, 4900.0, 1.0);
        assert!(cool.targets[0].rpm <= 1100.0);
        // Let the smoothing settle on a hot reading.
        let mut hot = cool;
        for _ in 0..12 {
            hot = c.tick(&cfg, Some(85.0), 1000.0, 4900.0, 1.0);
        }
        assert!(hot.targets[0].rpm > 3000.0, "got {}", hot.targets[0].rpm);
    }

    #[test]
    fn curve_respects_rate_limit() {
        let mut cfg = curve_cfg();
        cfg.curve.max_rpm_per_sec = 200.0;
        cfg.curve.smoothing_secs = 0.5;
        let mut c = Controller::new();
        c.tick(&cfg, Some(45.0), 1000.0, 4900.0, 1.0);
        let plan = c.tick(&cfg, Some(95.0), 1000.0, 4900.0, 1.0);
        // One second of allowance at 200 rpm/s (floored at 120) plus smoothing
        // means we must not jump thousands of rpm.
        assert!(plan.targets[0].rpm < 1600.0, "got {}", plan.targets[0].rpm);
    }

    #[test]
    fn emergency_goes_max() {
        let cfg = curve_cfg();
        let mut c = Controller::new();
        let plan = c.tick(&cfg, Some(99.0), 1000.0, 4900.0, 1.0);
        assert_eq!(plan.reason, TickReason::Emergency);
        assert_eq!(plan.targets[0].rpm, 4900.0);
    }

    #[test]
    fn sensor_loss_falls_back_to_auto() {
        let cfg = curve_cfg();
        let mut c = Controller::new();
        c.tick(&cfg, Some(50.0), 1000.0, 4900.0, 1.0);
        let mut last = None;
        for _ in 0..MAX_SENSOR_MISSES {
            last = Some(c.tick(&cfg, None, 1000.0, 4900.0, 1.0));
        }
        let plan = last.unwrap();
        assert!(plan.release);
        assert_eq!(plan.reason, TickReason::SensorLost);
    }

    #[test]
    fn bad_curve_releases() {
        let mut cfg = curve_cfg();
        cfg.curve.points.clear();
        let mut c = Controller::new();
        let plan = c.tick(&cfg, Some(60.0), 1000.0, 4900.0, 1.0);
        // Empty curve -> not reasonable -> fault.
        assert!(plan.release);
        assert_eq!(plan.reason, TickReason::Fault);
    }

    #[test]
    fn deadband_suppresses_tiny_changes() {
        let mut cfg = curve_cfg();
        cfg.curve.rpm_deadband = 300.0;
        cfg.curve.smoothing_secs = 0.5;
        let mut c = Controller::new();
        let first = c.tick(&cfg, Some(70.0), 1000.0, 4900.0, 1.0);
        let second = c.tick(&cfg, Some(71.0), 1000.0, 4900.0, 1.0);
        assert_eq!(first.targets[0].rpm, second.targets[0].rpm);
    }
}

#[cfg(test)]
mod trajectory_tests {
    use super::*;

    fn cfg() -> Config {
        Config {
            mode: ControlMode::Curve,
            curve: crate::Curve {
                points: vec![
                    crate::CurvePoint { temp: 45.0, rpm: 1000.0 },
                    crate::CurvePoint { temp: 65.0, rpm: 1600.0 },
                    crate::CurvePoint { temp: 85.0, rpm: 3200.0 },
                    crate::CurvePoint { temp: 95.0, rpm: 4200.0 },
                ],
                smoothing_secs: 2.0,
                rpm_deadband: 40.0,
                max_rpm_per_sec: 400.0,
                max_temp: 100.0,
                spin_up_rpm: 0.0,
            },
            emergency_temp: 98.0,
            ..Config::default()
        }
    }

    /// Simulate a real workload: idle, ramp up to 90 °C, hold, then cool down.
    #[test]
    fn full_load_trajectory_is_monotonic_and_rate_limited() {
        let cfg = cfg();
        let mut c = Controller::new();
        let dt = 1.0;
        let mut log = Vec::new();

        // 10 s idle at 45 °C.
        for _ in 0..10 {
            log.push(c.tick(&cfg, Some(45.0), 1000.0, 4900.0, dt));
        }
        let idle_rpm = log.last().unwrap().targets[0].rpm;
        assert!((idle_rpm - 1000.0).abs() < 1.0, "idle rpm {idle_rpm}");

        // 25 s at 90 °C: must climb, never overshoot the curve value, and never
        // exceed the rate limit between consecutive ticks.
        let mut prev = idle_rpm;
        let mut peak = idle_rpm;
        for _ in 0..25 {
            let plan = c.tick(&cfg, Some(90.0), 1000.0, 4900.0, dt);
            let rpm = plan.targets[0].rpm;
            assert!(rpm >= prev - 1.0, "rpm went backwards while heating: {prev} -> {rpm}");
            // Budget is capped at `max_rpm_per_sec * STEP_BUDGET_SECS`, so one
            // tick can never spend more than that.
            let max_step = (cfg.curve.max_rpm_per_sec * STEP_BUDGET_SECS) as f64 + 1.0;
            assert!(rpm - prev <= max_step, "rate limit ignored: {prev} -> {rpm} (cap {max_step})");
            assert!(rpm <= 4900.0);
            peak = peak.max(rpm);
            prev = rpm;
        }
        // The curve at 90 °C is between the 85 and 95 points (~3700), and the
        // controller should have caught up after 25 s of smoothing.
        assert!(peak > 3000.0, "fan did not spin up: {peak}");
        assert!(peak <= 3700.0 + 40.0, "fan overshot the curve: {peak}");

        // Cool back to 50 °C: must fall, and never jump down instantly.
        let mut prev = peak;
        for _ in 0..30 {
            let plan = c.tick(&cfg, Some(50.0), 1000.0, 4900.0, dt);
            let rpm = plan.targets[0].rpm;
            assert!(rpm <= prev + 1.0, "rpm rose while cooling: {prev} -> {rpm}");
            let max_step = (cfg.curve.max_rpm_per_sec * STEP_BUDGET_SECS) as f64 + 1.0;
            assert!(prev - rpm <= max_step, "rate limit ignored while cooling");
            prev = rpm;
        }
        assert!(prev < 1200.0, "fan did not spin down: {prev}");
    }

    #[test]
    fn every_tick_stays_inside_hardware_bounds() {
        let mut cfg = cfg();
        cfg.curve.points = vec![
            crate::CurvePoint { temp: 40.0, rpm: 500.0 },
            crate::CurvePoint { temp: 90.0, rpm: 9000.0 },
        ];
        let mut c = Controller::new();
        for temp in [10.0, 40.0, 60.0, 90.0, 120.0, -5.0] {
            for _ in 0..5 {
                let plan = c.tick(&cfg, Some(temp), 1200.0, 4800.0, 1.0);
                let rpm = plan.targets[0].rpm;
                assert!((1200.0..=4800.0).contains(&rpm), "temp {temp} -> {rpm}");
            }
        }
    }

    #[test]
    fn switching_fan_index_starts_from_scratch() {
        let mut cfg = cfg();
        cfg.mode = ControlMode::Fixed;
        cfg.fixed_rpm = 2000.0;
        let mut c = Controller::new();
        c.tick(&cfg, None, 1000.0, 4900.0, 1.0);
        assert_eq!(c.last_target(0), Some(2000.0));
        assert_eq!(c.last_target(1), None);
        cfg.fan = 1;
        let plan = c.tick(&cfg, None, 1000.0, 4900.0, 1.0);
        assert_eq!(plan.targets[0].index, 1);
        assert_eq!(plan.targets[0].rpm, 2000.0);
    }
}
