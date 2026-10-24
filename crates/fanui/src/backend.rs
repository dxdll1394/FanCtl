//! Background data plumbing: poll the daemon, keep a local history buffer, and
//! deliver UI actions without blocking the render thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Callback installed by the UI so the poller can request a repaint.
type WakeFn = Box<dyn Fn() + Send + Sync>;

use fancore::client::{Client, ClientError};
use fancore::config::Paths;
use fancore::protocol::{Request, Response, Sample, Snapshot};

/// Actions the UI can ask the backend to perform.
#[derive(Debug, Clone)]
pub enum Action {
    SetConfig(fancore::Config),
    SetMode(fancore::types::ControlMode),
    SetFixed { rpm: f64, fan: Option<u8> },
    Auto,
    Manual { fan: u8, rpm: f64 },
    Reload,
    Refresh,
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub action: Action,
    pub result: Result<Response, String>,
}

/// Shared handle used by both the polling thread and the UI.
#[derive(Clone)]
pub struct Backend {
    socket: Arc<Mutex<std::path::PathBuf>>,
    actions: Sender<Action>,
    replies: Arc<Mutex<Receiver<Reply>>>,
    pub latest: Arc<Mutex<Option<Snapshot>>>,
    pub history: Arc<Mutex<Vec<Sample>>>,
    pub connected: Arc<Mutex<bool>>,
    pub last_error: Arc<Mutex<Option<String>>>,
    /// Bumped whenever a fresh snapshot arrives, so the UI knows to repaint.
    pub revision: Arc<Mutex<u64>>,
    /// `false` while the window is hidden to the menu bar: poll slowly then.
    pub visible: Arc<AtomicBool>,
    /// Wake-up hook into egui. When the window is hidden egui has no reason to
    /// paint, so the poller has to ask for a frame explicitly; otherwise the
    /// menu bar title would go stale.
    wake: Arc<Mutex<Option<WakeFn>>>,
}

impl Backend {
    pub fn start(socket: std::path::PathBuf) -> Self {
        let (action_tx, action_rx) = channel::<Action>();
        let (reply_tx, reply_rx) = channel::<Reply>();
        let backend = Self {
            socket: Arc::new(Mutex::new(socket)),
            actions: action_tx,
            replies: Arc::new(Mutex::new(reply_rx)),
            latest: Arc::new(Mutex::new(None)),
            history: Arc::new(Mutex::new(Vec::new())),
            connected: Arc::new(Mutex::new(false)),
            last_error: Arc::new(Mutex::new(None)),
            revision: Arc::new(Mutex::new(0)),
            visible: Arc::new(AtomicBool::new(true)),
            wake: Arc::new(Mutex::new(None)),
        };
        let worker = backend.clone();
        std::thread::Builder::new()
            .name("fanui-backend".into())
            .spawn(move || worker.run(action_rx, reply_tx))
            .expect("spawn backend thread");
        backend
    }

    pub fn socket_path(&self) -> std::path::PathBuf {
        self.socket.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Install the callback used to request a repaint from the poller thread.
    pub fn set_wake(&self, wake: impl Fn() + Send + Sync + 'static) {
        if let Ok(mut slot) = self.wake.lock() {
            *slot = Some(Box::new(wake));
        }
    }

    fn wake_ui(&self) {
        if let Ok(slot) = self.wake.lock() {
            if let Some(wake) = slot.as_ref() {
                wake();
            }
        }
    }

    pub fn send(&self, action: Action) {
        let _ = self.actions.send(action);
    }

    pub fn poll_replies(&self) -> Vec<Reply> {
        let mut out = Vec::new();
        if let Ok(rx) = self.replies.lock() {
            while let Ok(reply) = rx.try_recv() {
                out.push(reply);
            }
        }
        out
    }

    fn run(&self, actions: Receiver<Action>, replies: Sender<Reply>) {
        let mut last_poll = Instant::now() - Duration::from_secs(10);
        let mut last_history = Instant::now() - Duration::from_secs(10);
        loop {
            // Handle every pending action first.
            while let Ok(action) = actions.try_recv() {
                let client = Client::new(self.socket_path(), Duration::from_secs(3));
                let request = match &action {
                    Action::SetConfig(c) => Some(Request::SetConfig { config: c.clone() }),
                    Action::SetMode(m) => Some(Request::SetMode { mode: *m }),
                    Action::SetFixed { rpm, fan } => {
                        Some(Request::SetFixed { rpm: *rpm, apply: true, fan: *fan })
                    }
                    Action::Auto => Some(Request::SetAuto),
                    Action::Manual { fan, rpm } => Some(Request::Manual { fan: *fan, rpm: *rpm }),
                    Action::Reload => Some(Request::Reload),
                    Action::Refresh => None,
                };
                let result = match request {
                    Some(req) => client.call(&req).map_err(|e| e.to_string()),
                    None => Ok(Response::ok()),
                };
                let _ = replies.send(Reply { action, result });
                last_poll = Instant::now() - Duration::from_secs(10);
            }

            // Poll twice a second while the window is up; when hidden there is
            // nothing to animate, so 3 s keeps the menu bar fresh while letting
            // the process go nearly idle.
            let snapshot_interval = if self.visible.load(Ordering::Relaxed) {
                Duration::from_millis(900)
            } else {
                Duration::from_secs(3)
            };
            if last_poll.elapsed() >= snapshot_interval {
                last_poll = Instant::now();
                let client = Client::new(self.socket_path(), Duration::from_secs(3));
                match client.snapshot() {
                    Ok(snap) => {
                        if let Ok(mut slot) = self.latest.lock() {
                            *slot = Some(snap);
                        }
                        if let Ok(mut c) = self.connected.lock() {
                            *c = true;
                        }
                        if let Ok(mut e) = self.last_error.lock() {
                            *e = None;
                        }
                        if let Ok(mut r) = self.revision.lock() {
                            *r += 1;
                        }
                        if !self.visible.load(Ordering::Relaxed) {
                            self.wake_ui();
                        }
                    }
                    Err(e) => {
                        if let Ok(mut c) = self.connected.lock() {
                            *c = false;
                        }
                        if let Ok(mut slot) = self.latest.lock() {
                            *slot = None;
                        }
                        if let Ok(mut err) = self.last_error.lock() {
                            *err = Some(match &e {
                                ClientError::NotRunning(p) => {
                                    format!("后台服务未运行（{}）", p.display())
                                }
                                other => other.to_string(),
                            });
                        }
                    }
                }
            }

            let history_interval = if self.visible.load(Ordering::Relaxed) {
                Duration::from_secs(2)
            } else {
                Duration::from_secs(15)
            };
            if last_history.elapsed() >= history_interval {
                last_history = Instant::now();
                let client = Client::new(self.socket_path(), Duration::from_secs(3));
                if let Ok(resp) = client.call(&Request::History { seconds: 1800 }) {
                    if let Some(samples) = resp.history {
                        if let Ok(mut h) = self.history.lock() {
                            *h = samples;
                        }
                    }
                }
            }

            let idle = self.visible.load(Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(if idle { 120 } else { 500 }));
        }
    }
}

/// Resolve the socket the daemon listens on, mirroring the CLI's rules.
pub fn default_socket() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("FANCTL_SOCKET") {
        return std::path::PathBuf::from(path);
    }
    Paths::system().socket
}
