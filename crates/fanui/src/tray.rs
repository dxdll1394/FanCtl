//! Menu-bar extra: status item with live temperature and a quick menu.

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

pub struct Tray {
    tray: TrayIcon,
    show_item: MenuItem,
    quit_item: MenuItem,
    auto_item: CheckMenuItem,
    fixed_item: CheckMenuItem,
    curve_item: CheckMenuItem,
    separator: PredefinedMenuItem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    Show,
    Quit,
    Mode(fancore::types::ControlMode),
}

/// Build the icon: a small fan glyph drawn into RGBA pixels.
fn icon_rgba(size: u32) -> (Vec<u8>, u32, u32) {
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let c = size as f32 / 2.0;
    let r = size as f32 * 0.48;
    // Template images on macOS use alpha only; colour is ignored.
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - c;
            let dy = y as f32 + 0.5 - c;
            let dist = (dx * dx + dy * dy).sqrt();
            let angle = dy.atan2(dx);
            let mut alpha = 0.0;
            if dist <= r {
                // Four blades: radius modulated by the angle.
                let blade = (4.0 * angle).cos().abs().powf(0.6);
                let inner = dist > size as f32 * 0.08;
                let outer = dist < r * (0.55 + 0.45 * blade);
                if (inner && outer) || dist <= size as f32 * 0.13 {
                    alpha = 1.0;
                }
            }
            let i = ((y * size + x) * 4) as usize;
            rgba[i] = 0;
            rgba[i + 1] = 0;
            rgba[i + 2] = 0;
            rgba[i + 3] = (alpha * 255.0) as u8;
        }
    }
    (rgba, size, size)
}

impl Tray {
    pub fn new(app_name: &str) -> Option<Self> {
        let menu = Menu::new();
        let show_item = MenuItem::new("显示主窗口", true, None);
        let auto_item = CheckMenuItem::new("系统自动", true, true, None);
        let fixed_item = CheckMenuItem::new("固定转速", true, false, None);
        let curve_item = CheckMenuItem::new("温度曲线", true, false, None);
        let separator = PredefinedMenuItem::separator();
        let quit_item = MenuItem::new(format!("退出 {app_name}"), true, None);
        menu.append_items(&[
            &show_item,
            &PredefinedMenuItem::separator(),
            &auto_item,
            &fixed_item,
            &curve_item,
            &separator,
            &quit_item,
        ])
        .ok()?;

        let (rgba, w, h) = icon_rgba(18);
        let icon = tray_icon::Icon::from_rgba(rgba, w, h).ok()?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(app_name)
            .with_icon_templated(icon)
            .with_title("")
            .with_autosave_name("local.fanctl.tray")
            .build()
            .ok()?;

        Some(Self { tray, show_item, quit_item, auto_item, fixed_item, curve_item, separator })
    }

    /// Drain pending menu clicks.
    pub fn poll(&self) -> Vec<TrayCommand> {
        let mut out = Vec::new();
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let id = event.id();
            if id == self.show_item.id() {
                out.push(TrayCommand::Show);
            } else if id == self.quit_item.id() {
                out.push(TrayCommand::Quit);
            } else if id == self.auto_item.id() {
                out.push(TrayCommand::Mode(fancore::types::ControlMode::Auto));
            } else if id == self.fixed_item.id() {
                out.push(TrayCommand::Mode(fancore::types::ControlMode::Fixed));
            } else if id == self.curve_item.id() {
                out.push(TrayCommand::Mode(fancore::types::ControlMode::Curve));
            }
        }
        out
    }

    pub fn update(&self, mode: Option<fancore::types::ControlMode>, title: &str, tooltip: &str) {
        if let Some(mode) = mode {
            self.auto_item.set_checked(mode == fancore::types::ControlMode::Auto);
            self.fixed_item.set_checked(mode == fancore::types::ControlMode::Fixed);
            self.curve_item.set_checked(mode == fancore::types::ControlMode::Curve);
        }
        // `set_title` is infallible; the tooltip can fail while AppKit rebuilds
        // the menu bar, which is cosmetic.
        self.tray.set_title(Some(title));
        if let Err(e) = self.tray.set_tooltip(Some(tooltip)) {
            eprintln!("tray tooltip: {e}");
        }
    }

    pub fn set_visible(&self, visible: bool) {
        if let Err(e) = self.tray.set_visible(visible) {
            eprintln!("tray visibility: {e}");
        }
    }

    /// Clicking the status item without a menu action should show the window.
    pub fn show_window_on_left_click(&self) {
        self.tray.set_show_menu_on_left_click(false);
    }

    #[allow(dead_code)]
    pub fn separator_id(&self) -> &tray_icon::menu::MenuId {
        self.separator.id()
    }
}

/// Format the menu-bar title, e.g. `62.4° 2400`.
pub fn title_text(temp: Option<f32>, rpm: Option<f64>, enabled: bool) -> String {
    if !enabled {
        return String::new();
    }
    match (temp, rpm) {
        (Some(t), Some(r)) => format!("{t:.0}° {r:.0}"),
        (Some(t), None) => format!("{t:.0}°"),
        _ => String::new(),
    }
}
