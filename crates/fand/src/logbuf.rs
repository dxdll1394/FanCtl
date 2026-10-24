//! Buffered, self-rotating log sink.
//!
//! The old sink did `writeln!` + `flush()` per line, so a curve-mode machine
//! wrote ~3200 one-line `write(2)` calls a day and the file grew forever. This
//! sink instead:
//!
//! * keeps lines in memory and flushes at most once per `flush_secs`
//!   (default 1800 s, i.e. thirty minutes),
//! * coalesces a run of consecutive RPM updates into a single summary line,
//!   which cuts the payload by ~two orders of magnitude,
//! * rotates in place (copy to `.0`, then truncate) once the file passes
//!   `max_kb`, keeping `keep` generations.
//!
//! Rotation is *copy-truncate* rather than rename because launchd holds its own
//! append fd for stdout/stderr on the same inode: a rename would leave the
//! daemon writing into an archive file forever. Truncating in place keeps one
//! inode, so launchd's fd stays valid.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Flush regardless of the interval once the buffer passes this size.
const BUFFER_CAP: usize = 64 * 1024;

/// How the daemon wants the sink to behave; rebuilt from `Config` each write so
/// `fanctl config set` takes effect without a restart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogSettings {
    /// Seconds between flushes. `0` means "write every line" (legacy behaviour).
    pub flush_secs: u64,
    /// Rotate once the file passes this many kilobytes. `0` disables rotation.
    pub max_kb: u64,
    /// Rotated generations to keep (`.0` … `.{keep-1}`).
    pub keep: u32,
    /// Fold consecutive RPM updates into one summary line per window.
    pub coalesce: bool,
}

impl Default for LogSettings {
    fn default() -> Self {
        Self { flush_secs: 1_800, max_kb: 2048, keep: 3, coalesce: true }
    }
}

impl LogSettings {
    fn flush_interval(&self) -> Option<Duration> {
        if self.flush_secs == 0 {
            None
        } else {
            Some(Duration::from_secs(self.flush_secs))
        }
    }

    fn max_bytes(&self) -> u64 {
        self.max_kb.saturating_mul(1024)
    }
}

/// A run of consecutive RPM changes for one fan and one reason.
#[derive(Debug, Clone)]
struct RpmRun {
    fan: u8,
    reason: String,
    first: f64,
    last: f64,
    min: f64,
    max: f64,
    count: u32,
}

impl RpmRun {
    fn new(fan: u8, rpm: f64, reason: &str) -> Self {
        Self {
            fan,
            reason: reason.to_string(),
            first: rpm,
            last: rpm,
            min: rpm,
            max: rpm,
            count: 1,
        }
    }

    fn push(&mut self, rpm: f64) {
        self.last = rpm;
        self.min = self.min.min(rpm);
        self.max = self.max.max(rpm);
        self.count += 1;
    }

    fn render(&self) -> String {
        let range = if self.count == 1 {
            format!("fan {} → {:.0} rpm ({})", self.fan, self.first, self.reason)
        } else if (self.max - self.min).abs() < 0.5 {
            format!(
                "fan {} → {:.0} rpm ({}, {} 次调整)",
                self.fan, self.first, self.reason, self.count
            )
        } else {
            format!(
                "fan {} → {:.0}~{:.0} rpm ({}, {} 次调整)",
                self.fan, self.min, self.max, self.reason, self.count
            )
        };
        range
    }
}

#[derive(Debug, Clone)]
enum Entry {
    Line(String),
    Rpm(RpmRun),
}

impl Entry {
    fn render(&self) -> String {
        match self {
            Entry::Line(s) => s.clone(),
            Entry::Rpm(run) => run.render(),
        }
    }

    fn bytes(&self) -> usize {
        self.render().len() + 32 // timestamp + newline, generous
    }
}

/// Buffered writer around `<log_dir>/daemon.log`.
pub struct LogSink {
    path: PathBuf,
    file: Option<File>,
    entries: Vec<Entry>,
    /// Bytes already in the current file, including anything launchd wrote.
    written: u64,
    last_flush: Instant,
    /// Set when the buffer could not be flushed (disk full and the like), so we
    /// do not retry-spin on every tick.
    failed: bool,
}

impl LogSink {
    /// Open (and create) the log file, appending.
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            path: path.to_path_buf(),
            file: Some(file),
            entries: Vec::new(),
            written,
            last_flush: Instant::now(),
            failed: false,
        })
    }

    /// A sink that drops everything; used when the real file cannot be opened.
    pub fn null() -> Self {
        Self {
            path: PathBuf::new(),
            file: None,
            entries: Vec::new(),
            written: 0,
            last_flush: Instant::now(),
            failed: true,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Queue a plain line. `urgent` lines are flushed immediately because they
    /// record state changes a post-mortem cannot reconstruct (mode switches,
    /// write failures, start/stop).
    pub fn log(&mut self, msg: &str, urgent: bool, settings: &LogSettings) {
        self.entries.push(Entry::Line(msg.to_string()));
        if urgent {
            self.flush(settings);
        } else {
            self.maybe_flush(settings);
        }
    }

    /// Queue an RPM update. With coalescing on, a contiguous run for the same
    /// fan and reason folds into one line; with it off the line is kept as is.
    pub fn log_rpm(&mut self, fan: u8, rpm: f64, reason: &str, settings: &LogSettings) {
        if settings.coalesce {
            match self.entries.last_mut() {
                Some(Entry::Rpm(run)) if run.fan == fan && run.reason == reason => {
                    run.push(rpm);
                    self.maybe_flush(settings);
                    return;
                }
                _ => {}
            }
            self.entries.push(Entry::Rpm(RpmRun::new(fan, rpm, reason)));
        } else {
            self.entries.push(Entry::Line(format!("fan {fan} → {rpm:.0} rpm ({reason})")));
        }
        self.maybe_flush(settings);
    }

    fn pending_bytes(&self) -> usize {
        self.entries.iter().map(Entry::bytes).sum()
    }

    /// Flush when the interval elapsed or the buffer got big.
    pub fn maybe_flush(&mut self, settings: &LogSettings) {
        if self.entries.is_empty() {
            return;
        }
        let due = match settings.flush_interval() {
            None => true,
            Some(every) => self.last_flush.elapsed() >= every,
        };
        if due || self.pending_bytes() >= BUFFER_CAP {
            self.flush(settings);
        }
    }

    /// Write the buffer out now. Safe to call at any time (shutdown, SIGHUP).
    pub fn flush(&mut self, settings: &LogSettings) {
        if self.entries.is_empty() {
            return;
        }
        if self.file.is_none() {
            self.entries.clear();
            return;
        }
        // Back off for a minute after a failed write (disk full, read-only
        // volume) instead of retrying on every urgent line.
        if self.failed && self.last_flush.elapsed() < Duration::from_secs(60) {
            return;
        }
        let body = self.render();
        if self.write_all(&body, settings).is_err() {
            // Keep the lines for the next attempt.
            self.failed = true;
            self.last_flush = Instant::now();
            return;
        }
        self.failed = false;
        self.last_flush = Instant::now();
    }

    fn render(&self) -> String {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let mut out = String::with_capacity(self.pending_bytes());
        for entry in &self.entries {
            out.push_str(&stamp);
            out.push(' ');
            out.push_str(&entry.render());
            out.push('\n');
        }
        out
    }

    fn write_all(&mut self, body: &str, settings: &LogSettings) -> std::io::Result<()> {
        if self.needs_rotate(body.len() as u64, settings) {
            self.rotate(settings)?;
        }
        if let Some(file) = self.file.as_mut() {
            file.write_all(body.as_bytes())?;
            file.flush()?;
        }
        self.written += body.len() as u64;
        self.entries.clear();
        Ok(())
    }

    fn needs_rotate(&self, incoming: u64, settings: &LogSettings) -> bool {
        settings.max_bytes() > 0 && self.written + incoming > settings.max_bytes()
    }

    /// Copy the live file to `.0`, shift older generations up, truncate in
    /// place. Failures fall back to plain appending (an unbounded file beats a
    /// daemon that stops logging).
    fn rotate(&mut self, settings: &LogSettings) -> std::io::Result<()> {
        if let Some(file) = self.file.as_mut() {
            file.flush()?;
        }

        if settings.keep == 0 {
            if let Some(file) = self.file.as_mut() {
                file.set_len(0)?;
            }
            self.written = 0;
            return Ok(());
        }

        // Generations are `.0` (newest) through `.{keep-1}` (oldest): drop the
        // oldest, shift every other one up, then copy the live file to `.0`.
        let _ = fs::remove_file(archive_path(&self.path, settings.keep - 1));
        for i in (0..settings.keep.saturating_sub(1)).rev() {
            let from = archive_path(&self.path, i);
            if from.exists() {
                let _ = fs::rename(&from, archive_path(&self.path, i + 1));
            }
        }

        let mut current = Vec::new();
        if let Ok(mut src) = File::open(&self.path) {
            let _ = src.read_to_end(&mut current);
        }
        if !current.is_empty() {
            let target = archive_path(&self.path, 0);
            if let Ok(mut dst) = File::create(&target) {
                let _ = dst.write_all(&current);
                let _ = dst.flush();
            }
        }

        if let Some(file) = self.file.as_mut() {
            file.set_len(0)?;
        }
        self.written = 0;
        Ok(())
    }

    /// Bytes currently waiting in memory (for tests and diagnostics).
    pub fn pending(&self) -> usize {
        self.entries.len()
    }

    /// Force the next flush to happen even if the interval has not elapsed.
    pub fn mark_due(&mut self) {
        self.last_flush = Instant::now() - Duration::from_secs(365 * 24 * 3600);
    }
}

fn archive_path(path: &Path, index: u32) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(format!(".{index}"));
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fanctl-log-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn immediate() -> LogSettings {
        LogSettings { flush_secs: 0, max_kb: 0, keep: 0, coalesce: false }
    }

    #[test]
    fn buffers_until_the_interval_elapses() {
        let dir = temp_dir("buffer");
        let path = dir.join("daemon.log");
        let mut sink = LogSink::open(&path).unwrap();
        let settings = LogSettings { flush_secs: 600, max_kb: 0, keep: 0, coalesce: true };

        sink.log_rpm(0, 1200.0, "温度曲线", &settings);
        sink.log_rpm(0, 1400.0, "温度曲线", &settings);
        assert_eq!(fs::read_to_string(&path).unwrap(), "", "还没到时间，不应落盘");
        assert_eq!(sink.pending(), 1, "连续两条应合并成一条");

        sink.flush(&settings);
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains("1200~1400 rpm"), "实际：{text}");
        assert!(text.contains("2 次调整"));
    }

    #[test]
    fn urgent_lines_flush_immediately() {
        let dir = temp_dir("urgent");
        let path = dir.join("daemon.log");
        let mut sink = LogSink::open(&path).unwrap();
        let settings = LogSettings { flush_secs: 600, max_kb: 0, keep: 0, coalesce: true };

        sink.log_rpm(0, 1200.0, "温度曲线", &settings);
        sink.log("mode → Curve", true, &settings);
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2, "紧急事件应立刻落盘并带出前面的缓冲");
        assert!(text.contains("mode → Curve"));
    }

    #[test]
    fn coalescing_respects_order_and_reason_changes() {
        let dir = temp_dir("order");
        let path = dir.join("daemon.log");
        let mut sink = LogSink::open(&path).unwrap();
        let settings = LogSettings { flush_secs: 600, max_kb: 0, keep: 0, coalesce: true };

        sink.log_rpm(0, 1000.0, "温度曲线", &settings);
        sink.log_rpm(0, 1500.0, "温度曲线", &settings);
        sink.log("mode → Fixed", true, &settings);
        sink.log_rpm(0, 2000.0, "固定转速", &settings);
        sink.flush(&settings);

        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("1000~1500 rpm"), "实际：{}", lines[0]);
        assert!(lines[1].contains("mode → Fixed"));
        assert!(lines[2].contains("2000 rpm"), "实际：{}", lines[2]);
    }

    #[test]
    fn coalescing_can_be_turned_off() {
        let dir = temp_dir("nocoalesce");
        let path = dir.join("daemon.log");
        let mut sink = LogSink::open(&path).unwrap();
        let settings = LogSettings { flush_secs: 600, max_kb: 0, keep: 0, coalesce: false };

        for rpm in [1000.0, 1200.0, 1300.0] {
            sink.log_rpm(0, rpm, "温度曲线", &settings);
        }
        sink.flush(&settings);
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 3);
    }

    #[test]
    fn rotates_and_keeps_requested_generations() {
        let dir = temp_dir("rotate");
        let path = dir.join("daemon.log");
        let mut sink = LogSink::open(&path).unwrap();
        // 1 KB max, keep 2 generations.
        let settings = LogSettings { flush_secs: 0, max_kb: 1, keep: 2, coalesce: false };

        for i in 0..80 {
            sink.log(&format!("event {i} padding padding padding"), true, &settings);
        }
        assert!(path.exists(), "当前日志仍应存在");
        assert!(archive_path(&path, 0).exists(), "应产生 .0 归档");
        assert!(archive_path(&path, 1).exists(), "应产生 .1 归档");
        assert!(!archive_path(&path, 2).exists(), "不应超过 keep 份");
        let live = fs::metadata(&path).unwrap().len();
        assert!(live <= 1024, "当前文件应已回落到上限内，实际 {live}");
    }

    #[test]
    fn rotation_disabled_when_size_is_zero() {
        let dir = temp_dir("norotate");
        let path = dir.join("daemon.log");
        let mut sink = LogSink::open(&path).unwrap();
        let settings = LogSettings { flush_secs: 0, max_kb: 0, keep: 3, coalesce: false };
        for i in 0..200 {
            sink.log(&format!("line {i}"), true, &settings);
        }
        assert!(!archive_path(&path, 0).exists(), "禁用轮转时不应产生归档");
        assert!(fs::metadata(&path).unwrap().len() > 1024);
    }

    #[test]
    fn interval_of_zero_writes_each_line() {
        let dir = temp_dir("every");
        let path = dir.join("daemon.log");
        let mut sink = LogSink::open(&path).unwrap();
        let settings = immediate();
        sink.log("a", false, &settings);
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 1);
        sink.log("b", false, &settings);
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 2);
    }
}
