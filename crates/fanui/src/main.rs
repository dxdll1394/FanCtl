//! fanui – the window and menu-bar UI for fanctl.

mod backend;
mod theme;
mod tray;
mod views;

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use fancore::protocol::{Sample, Snapshot};
use fancore::types::ControlMode;

use backend::{Action, Backend};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Curve,
    Settings,
    About,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveDrag {
    Point(usize),
}

/// Editable copy of the daemon config, applied with "保存并应用".
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub curve: fancore::Curve,
    pub sensor: fancore::SensorGroup,
    pub fixed_rpm: f64,
    pub emergency_temp: f32,
    pub poll_ms: u64,
    pub restore_auto_on_exit: bool,
    pub mode: ControlMode,
}

impl From<&fancore::Config> for Draft {
    fn from(c: &fancore::Config) -> Self {
        Self {
            curve: c.curve.clone(),
            sensor: c.sensor,
            fixed_rpm: c.fixed_rpm,
            emergency_temp: c.emergency_temp,
            poll_ms: c.poll_ms,
            restore_auto_on_exit: c.restore_auto_on_exit,
            mode: c.mode,
        }
    }
}

impl Draft {
    fn to_config(&self, base: &fancore::Config) -> fancore::Config {
        let mut c = base.clone();
        c.curve = self.curve.clone();
        c.sensor = self.sensor;
        c.fixed_rpm = self.fixed_rpm;
        c.emergency_temp = self.emergency_temp;
        c.poll_ms = self.poll_ms;
        c.restore_auto_on_exit = self.restore_auto_on_exit;
        c.mode = self.mode;
        c.normalize();
        c
    }
}

pub struct App {
    pub backend: Backend,
    pub snapshot: Option<Snapshot>,
    pub draft: Draft,
    saved: Draft,
    pub tab: Tab,
    pub chart_range: views::overview::ChartRange,
    pub chart_auto: bool,
    pub drag: Option<CurveDrag>,
    pub menu_bar_only: bool,
    pub menu_bar_text: bool,
    pub run_install_dialog: bool,
    pub open_path: Option<String>,
    pub toasts: Vec<(String, Instant, bool)>,
    pub tray: Option<tray::Tray>,
    last_tray_update: Instant,
    last_revision: u64,
    quitting: bool,
    frames: u32,
    /// Whether the window is currently on screen (false = menu bar only).
    window_visible: bool,
    /// Hide the window once the first frames are up.
    hide_on_start: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply_theme(&cc.egui_ctx);
        let _font = theme::install_fonts(&cc.egui_ctx);
        let backend = Backend::start(backend::default_socket());
        {
            let ctx = cc.egui_ctx.clone();
            backend.set_wake(move || ctx.request_repaint());
        }
        let prefs = UiPrefs::load();
        let mut app = Self {
            backend,
            snapshot: None,
            draft: Draft::from(&fancore::Config::default()),
            saved: Draft::from(&fancore::Config::default()),
            tab: Tab::Overview,
            chart_range: views::overview::ChartRange::Min5,
            chart_auto: true,
            drag: None,
            menu_bar_only: prefs.menu_bar_only,
            menu_bar_text: prefs.menu_bar_text,
            run_install_dialog: false,
            open_path: None,
            toasts: Vec::new(),
            tray: tray::Tray::new("FanCtl"),
            last_tray_update: Instant::now() - Duration::from_secs(10),
            last_revision: 0,
            quitting: false,
            frames: 0,
            window_visible: true,
            hide_on_start: false,
        };
        if let Some(tray) = &app.tray {
            tray.show_window_on_left_click();
        }
        // `FANCtl_START_HIDDEN=1` starts straight in menu-bar mode (used by the
        // optional login item so the app does not pop a window at boot).
        if std::env::var_os("FANCtl_START_HIDDEN").is_some() && app.menu_bar_only {
            app.window_visible = false;
            app.hide_on_start = true;
            app.backend.visible.store(false, std::sync::atomic::Ordering::Relaxed);
        }
        app
    }

    pub fn save_draft(&mut self) {
        let base = self.snapshot.as_ref().map(|s| s.config.clone()).unwrap_or_default();
        let config = self.draft.to_config(&base);
        self.saved = Draft::from(&config);
        self.draft = Draft::from(&config);
        self.backend.send(Action::SetConfig(config));
    }

    pub fn reload_draft(&mut self) {
        if let Some(snap) = &self.snapshot {
            self.draft = Draft::from(&snap.config);
            self.saved = Draft::from(&snap.config);
        }
    }

    pub fn draft_dirty(&self) -> bool {
        self.draft != self.saved
    }

    pub fn current_temp(&self) -> Option<f32> {
        self.snapshot.as_ref().and_then(|s| s.control_temp)
    }

    pub fn history_len(&self) -> usize {
        self.backend.history.lock().map(|h| h.len()).unwrap_or(0)
    }

    pub fn history_window(&self, seconds: f64) -> Vec<Sample> {
        let Ok(history) = self.backend.history.lock() else {
            return Vec::new();
        };
        let Some(last) = history.last() else {
            return Vec::new();
        };
        let cutoff = last.t - seconds;
        history.iter().filter(|s| s.t >= cutoff).cloned().collect()
    }

    pub fn error_text(&self) -> Option<String> {
        self.toasts.iter().rev().find(|(_, _, err)| *err).map(|(msg, _, _)| msg.clone())
    }

    pub fn copy_to_clipboard(&mut self, text: &str) {
        match arboard::Clipboard::new() {
            Ok(mut clipboard) => {
                let _ = clipboard.set_text(text.to_owned());
                self.toast(format!("已复制：{text}"), false);
            }
            Err(e) => self.toast(format!("复制失败：{e}"), true),
        }
    }

    fn toast(&mut self, msg: impl Into<String>, is_error: bool) {
        self.toasts.push((msg.into(), Instant::now(), is_error));
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }

    pub fn install_script(&self) -> std::path::PathBuf {
        let exe = std::env::current_exe().unwrap_or_default();
        for candidate in [
            exe.join("../../../../install.sh"),
            exe.join("../../../install.sh"),
            std::path::PathBuf::from("install.sh"),
        ] {
            if candidate.exists() {
                return candidate;
            }
        }
        std::path::PathBuf::from("install.sh")
    }

    pub fn save_ui_prefs(&self) {
        UiPrefs { menu_bar_only: self.menu_bar_only, menu_bar_text: self.menu_bar_text }.save();
    }

    fn apply_snapshot(&mut self, snap: Snapshot) {
        let changed = self.snapshot.as_ref().map(|prev| prev.config != snap.config).unwrap_or(true);
        if changed && !self.draft_dirty() {
            self.draft = Draft::from(&snap.config);
            self.saved = Draft::from(&snap.config);
        }
        self.snapshot = Some(snap);
    }

    fn handle_replies(&mut self) {
        for reply in self.backend.poll_replies() {
            match reply.result {
                Ok(resp) => {
                    if let Some(snap) = resp.snapshot {
                        let changed = self
                            .snapshot
                            .as_ref()
                            .map(|prev| prev.config != snap.config)
                            .unwrap_or(true);
                        if changed {
                            self.saved = Draft::from(&snap.config);
                            if !self.draft_dirty() {
                                self.draft = Draft::from(&snap.config);
                            }
                        }
                        self.snapshot = Some(snap);
                    }
                    if let Some(msg) = resp.message {
                        if !matches!(reply.action, Action::Refresh) {
                            self.toast(msg, false);
                        }
                    }
                }
                Err(e) => self.toast(e, true),
            }
        }
    }

    fn update_tray(&mut self, ctx: &egui::Context) {
        if self.last_tray_update.elapsed() < Duration::from_millis(900) {
            return;
        }
        self.last_tray_update = Instant::now();
        let Some(tray) = &self.tray else { return };
        let (temp, rpm) = match &self.snapshot {
            Some(s) => (s.control_temp, s.fans.first().map(|f| f.actual_rpm)),
            None => (None, None),
        };
        let title =
            if self.menu_bar_text { tray::title_text(temp, rpm, true) } else { String::new() };
        let tooltip = match &self.snapshot {
            Some(s) => format!(
                "FanCtl — {} · {} · {:.0} rpm",
                s.mode.label(),
                s.status,
                s.fans.first().map(|f| f.actual_rpm).unwrap_or(0.0)
            ),
            None => "FanCtl — 后台服务未运行".to_string(),
        };
        tray.update(self.snapshot.as_ref().map(|s| s.mode), &title, &tooltip);
        for cmd in tray.poll() {
            match cmd {
                tray::TrayCommand::Show => self.show_window(ctx),
                tray::TrayCommand::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                tray::TrayCommand::Mode(mode) => {
                    self.backend.send(Action::SetMode(mode));
                }
            }
        }
    }

    fn handle_window_visibility(&mut self, ctx: &egui::Context) {
        // Default behaviour: closing the window quits the UI process. The root
        // daemon keeps running and keeps applying the curve, so fan control is
        // unaffected — only the window goes away. Users who want the menu bar
        // to stay can opt in from Settings.
        let can_hide = self.menu_bar_only && self.tray.is_some();
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            if can_hide {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                self.set_window_visible(false);
            } else {
                // Let eframe close; the daemon is untouched.
                self.backend.visible.store(false, std::sync::atomic::Ordering::Relaxed);
            }
        }
        // Minimising also means nobody is looking at the chart.
        if let Some(minimized) = ctx.input(|i| i.viewport().minimized) {
            let visible = !minimized;
            if visible != self.window_visible {
                self.set_window_visible(visible);
            }
        }
    }

    fn set_window_visible(&mut self, visible: bool) {
        if self.window_visible == visible {
            return;
        }
        self.window_visible = visible;
        self.backend.visible.store(visible, std::sync::atomic::Ordering::Relaxed);
        // Force the next poll to happen immediately when coming back.
        self.backend.send(Action::Refresh);
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        self.set_window_visible(true);
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Visible: animate the chart twice a second. Hidden: the background
        // poller wakes us when there is new data (see `Backend::set_wake`), so
        // the timer only exists as a fallback.
        if self.window_visible {
            ctx.request_repaint_after(Duration::from_millis(500));
        } else {
            ctx.request_repaint_after(Duration::from_secs(30));
        }

        // The app runs as a menu-bar accessory, so AppKit does not focus the
        // window on launch; do it explicitly once the first frames are drawn.
        self.frames = self.frames.saturating_add(1);
        if self.frames == 5 {
            if self.hide_on_start {
                self.hide_on_start = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else if self.window_visible {
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }

        let revision = self.backend.revision.lock().map(|r| *r).unwrap_or(0);
        if revision != self.last_revision {
            self.last_revision = revision;
            let latest = self.backend.latest.lock().ok().and_then(|s| s.clone());
            if let Some(snap) = latest {
                self.apply_snapshot(snap);
            }
        }
        self.handle_replies();
        self.update_tray(ctx);
        self.handle_window_visibility(ctx);

        if let Some(path) = self.open_path.take() {
            if let Err(e) = open::that(&path) {
                self.toast(format!("打开 {path} 失败：{e}"), true);
            }
        }

        if self.snapshot.is_none() {
            if let Some(err) = self.backend.last_error.lock().ok().and_then(|e| e.clone()) {
                if !self.toasts.iter().any(|(m, _, _)| *m == err) {
                    self.toast(err, true);
                }
            }
        }

        top_bar(self, ctx);

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(0x16, 0x18, 0x1C))
                    .inner_margin(egui::Margin::symmetric(16, 12)),
            )
            .show(ctx, |ui| {
                // The overview is laid out to fit the window (the chart fills
                // whatever is left); the other tabs scroll.
                match self.tab {
                    Tab::Overview => views::overview::show(self, ui),
                    Tab::Curve => {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| views::curve::show(self, ui));
                    }
                    Tab::Settings => {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| views::settings::show(self, ui));
                    }
                    Tab::About => {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| about(self, ui));
                    }
                }
            });

        toasts(self, ctx);
    }
}

fn top_bar(app: &mut App, ctx: &egui::Context) {
    egui::TopBottomPanel::top("tabs")
        .frame(
            egui::Frame::new()
                .fill(Color32::from_rgb(0x1B, 0x1E, 0x23))
                .inner_margin(egui::Margin::symmetric(16, 8)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("FanCtl").size(17.0).strong().color(Color32::WHITE));
                ui.add_space(16.0);
                for (tab, label) in [
                    (Tab::Overview, "总览"),
                    (Tab::Curve, "曲线"),
                    (Tab::Settings, "设置"),
                    (Tab::About, "关于"),
                ] {
                    let selected = app.tab == tab;
                    if ui
                        .add(egui::Button::selectable(selected, RichText::new(label).size(14.0)))
                        .clicked()
                    {
                        app.tab = tab;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let online = app.snapshot.is_some();
                    let colour = if online { theme::OK } else { theme::DANGER };
                    ui.label(
                        RichText::new(if online { "● 已连接" } else { "● 未连接" })
                            .color(colour)
                            .small(),
                    );
                    if app.draft_dirty() && ui.button(RichText::new("保存修改").strong()).clicked()
                    {
                        app.save_draft();
                    }
                });
            });
        });
}

fn about(app: &mut App, ui: &mut egui::Ui) {
    theme::card_show(ui, |ui| {
        ui.label(RichText::new("FanCtl").size(22.0).strong());
        ui.label(RichText::new(format!("版本 {}", fancore::VERSION)).color(theme::MUTED));
        ui.add_space(8.0);
        ui.label("给 Apple Silicon Mac 用的风扇控制工具：直接读写 AppleSMC，不依赖第三方 App。");
        ui.add_space(8.0);
        ui.label(RichText::new("工作原理").strong());
        ui.label(
            RichText::new(
                "界面 → unix socket → root 守护进程（launchd）→ IOKit/AppleSMC。\
                 读温度不需要权限，写风扇只在守护进程里进行。",
            )
            .small()
            .color(theme::MUTED),
        );
        ui.add_space(8.0);
        ui.label(RichText::new("安全边界").strong());
        for line in [
            "• 转速始终限制在硬件最小/最大值之间",
            "• 温度连续读不到时自动交还 macOS",
            "• 紧急温度到达即全速，无条件",
            "• 守护进程退出时恢复自动控制",
            "• 每次写入记录到 /Library/Logs/FanCtl/daemon.log",
        ] {
            ui.label(RichText::new(line).small());
        }
        ui.add_space(8.0);
        ui.label(RichText::new("常用命令").strong());
        for line in [
            "fanctl status",
            "fanctl set 2200",
            "fanctl curve --sensor cpu",
            "fanctl auto",
            "sudo fanctl daemon install",
        ] {
            ui.label(RichText::new(line).monospace().small());
        }
        ui.add_space(8.0);
        ui.label(
            RichText::new("本机风扇范围与传感器分组由 AppleSMC 实时探测，换机器无需改代码。")
                .small()
                .color(theme::MUTED),
        );
        let _ = app;
    });
}

fn toasts(app: &mut App, ctx: &egui::Context) {
    app.toasts.retain(|(_, at, _)| at.elapsed() < Duration::from_secs(4));
    if app.toasts.is_empty() {
        return;
    }
    egui::Area::new(egui::Id::new("toasts"))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
        .interactable(false)
        .show(ctx, |ui| {
            for (msg, _, is_error) in &app.toasts {
                let colour = if *is_error { theme::DANGER } else { theme::OK };
                egui::Frame::new()
                    .fill(Color32::from_rgb(0x24, 0x28, 0x2E))
                    .stroke(egui::Stroke::new(1.0, colour.gamma_multiply(0.6)))
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.label(RichText::new(msg).color(colour).size(13.0));
                    });
                ui.add_space(6.0);
            }
        });
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
struct UiPrefs {
    #[serde(default = "default_true")]
    menu_bar_only: bool,
    #[serde(default = "default_true")]
    menu_bar_text: bool,
}

fn default_true() -> bool {
    true
}

impl Default for UiPrefs {
    fn default() -> Self {
        // Closing the window quits the UI by default; the daemon keeps running.
        Self { menu_bar_only: false, menu_bar_text: true }
    }
}

impl UiPrefs {
    fn path() -> Option<std::path::PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(std::path::PathBuf::from(home).join("Library/Application Support/FanCtl/ui.json"))
    }

    fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        if let Some(path) = Self::path() {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(text) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(path, text);
            }
        }
    }
}

fn main() -> eframe::Result<()> {
    // Screenshot mode is only for the visual regression helper.
    if let Ok(path) = std::env::var("FANCtl_SHOT") {
        return shot_mode(&path);
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("FanCtl")
            .with_inner_size([1000.0, 820.0])
            .with_min_inner_size([860.0, 640.0])
            .with_app_id("local.fanctl.ui"),
        ..Default::default()
    };
    eframe::run_native("FanCtl", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

/// Renders one frame of each tab into PNGs for visual review.
fn shot_mode(out_dir: &str) -> eframe::Result<()> {
    let dir = std::path::PathBuf::from(out_dir);
    let _ = std::fs::create_dir_all(&dir);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("FanCtl")
            .with_inner_size([1000.0, 820.0]),
        ..Default::default()
    };
    eframe::run_native("FanCtl", options, Box::new(move |cc| Ok(Box::new(ShotApp::new(cc, dir)))))
}

struct ShotApp {
    app: App,
    started: Instant,
    dir: std::path::PathBuf,
    stage: usize,
    pending: bool,
    frames: u32,
}

impl ShotApp {
    fn new(cc: &eframe::CreationContext<'_>, dir: std::path::PathBuf) -> Self {
        Self {
            app: App::new(cc),
            started: Instant::now(),
            dir,
            stage: 0,
            pending: false,
            frames: 0,
        }
    }

    fn tab_for(stage: usize) -> (Tab, &'static str) {
        match stage {
            0 => (Tab::Overview, "overview"),
            1 => (Tab::Curve, "curve"),
            2 => (Tab::Settings, "settings"),
            _ => (Tab::Overview, "done"),
        }
    }
}

impl eframe::App for ShotApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.frames += 1;
        ctx.request_repaint_after(Duration::from_millis(100));
        if self.started.elapsed() < Duration::from_secs(5) {
            return;
        }
        if self.stage >= 3 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let (tab, name) = Self::tab_for(self.stage);
        self.app.tab = tab;
        self.app.update(ctx, frame);

        if !self.pending {
            self.pending = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            return;
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let w = image.width() as u32;
            let h = image.height() as u32;
            let mut rgba = Vec::with_capacity((w * h * 4) as usize);
            for px in image.pixels.iter() {
                rgba.extend_from_slice(&px.to_array());
            }
            if let Some(buf) = image::RgbaImage::from_raw(w, h, rgba) {
                let _ = buf.save(self.dir.join(format!("{name}.png")));
            }
            self.stage += 1;
            self.pending = false;
        }
    }
}
