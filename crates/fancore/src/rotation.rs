//! Ring buffer of recent samples for the UI's live chart.
//!
//! One hour at 1 Hz is 3600 samples; memory is trivial and it makes the chart
//! usable without any database.

use std::collections::VecDeque;

use crate::protocol::Sample;

/// Largest window we retain (seconds).
pub const RETENTION_SECS: f64 = 3600.0;

#[derive(Debug, Default)]
pub struct History {
    samples: VecDeque<Sample>,
}

impl History {
    pub fn new() -> Self {
        // 3600 samples at 1 Hz is the retention window; the old 4096 over-
        // provisioned by ~14% and, more importantly, allocated it all up front
        // (~128 KB) even when the daemon had just started and held nothing.
        Self { samples: VecDeque::with_capacity(RETENTION_SECS as usize) }
    }

    pub fn push(&mut self, sample: Sample) {
        if let Some(last) = self.samples.back() {
            if sample.t <= last.t {
                // Clock went backwards; drop everything older than the new sample.
                self.samples.retain(|s| s.t < sample.t);
            }
        }
        self.samples.push_back(sample);
        let cutoff = self.samples.back().map(|s| s.t - RETENTION_SECS).unwrap_or(0.0);
        while self.samples.front().map(|s| s.t < cutoff).unwrap_or(false) {
            self.samples.pop_front();
        }
    }

    /// Samples covering the last `seconds`, oldest first.
    pub fn tail(&self, seconds: u64) -> Vec<Sample> {
        match self.samples.back() {
            None => Vec::new(),
            Some(last) => {
                let cutoff = last.t - seconds as f64;
                self.samples.iter().filter(|s| s.t >= cutoff).cloned().collect()
            }
        }
    }

    pub fn latest(&self) -> Option<&Sample> {
        self.samples.back()
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ControlMode;

    fn sample(t: f64) -> Sample {
        Sample {
            t,
            control_temp: Some(50.0),
            fan_rpm: Some(1200.0),
            target_rpm: Some(1200.0),
            mode: ControlMode::Curve,
        }
    }

    #[test]
    fn trims_to_retention() {
        let mut h = History::new();
        for i in 0..(RETENTION_SECS as usize + 100) {
            h.push(sample(i as f64));
        }
        assert!((h.len() as f64) <= RETENTION_SECS + 2.0);
        let tail = h.tail(60);
        assert_eq!(tail.len(), 61);
    }

    #[test]
    fn survives_clock_going_backwards() {
        let mut h = History::new();
        h.push(sample(1000.0));
        h.push(sample(500.0));
        assert_eq!(h.len(), 1);
        assert_eq!(h.latest().unwrap().t, 500.0);
    }

    #[test]
    fn empty_tail() {
        assert!(History::new().tail(60).is_empty());
    }
}
