//! Look and feel: CJK font loading, colours, and small reusable widgets.

use std::sync::Arc;

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, Stroke, TextStyle};

/// Apple-ish accent used for the "control" colour.
pub const ACCENT: Color32 = Color32::from_rgb(0x2E, 0x8B, 0xF7);
pub const TEMP: Color32 = Color32::from_rgb(0xFF, 0x8A, 0x3D);
pub const RPM: Color32 = Color32::from_rgb(0x35, 0xC7, 0x8F);
pub const OK: Color32 = Color32::from_rgb(0x3F, 0xC9, 0x6A);
pub const WARN: Color32 = Color32::from_rgb(0xF2, 0xB1, 0x35);
pub const DANGER: Color32 = Color32::from_rgb(0xEF, 0x53, 0x50);
pub const MUTED: Color32 = Color32::from_rgb(0x8A, 0x8F, 0x98);

/// Candidate CJK fonts, macOS ships at least one of these on every release.
const CJK_FONTS: &[(&str, u32)] = &[
    ("/System/Library/Fonts/PingFang.ttc", 0),
    ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
    ("/System/Library/Fonts/STHeiti Medium.ttc", 1),
    ("/System/Library/Fonts/STHeiti Light.ttc", 1),
    ("/System/Library/Fonts/Supplemental/Songti.ttc", 0),
    ("/System/Library/Fonts/Supplemental/Arial Unicode.ttf", 0),
];

/// Install a system CJK font as the primary proportional font.
///
/// egui's bundled Ubuntu font has no CJK glyphs, so without this every Chinese
/// label renders as tofu boxes.
pub fn install_fonts(ctx: &egui::Context) -> Option<String> {
    let mut fonts = FontDefinitions::default();
    let mut loaded = None;
    for (path, index) in CJK_FONTS {
        let Ok(bytes) = std::fs::read(path) else { continue };
        let data = FontData {
            font: std::borrow::Cow::Owned(bytes),
            index: *index,
            tweak: Default::default(),
        };
        fonts.font_data.insert("cjk".to_owned(), Arc::new(data));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            if let Some(list) = fonts.families.get_mut(&family) {
                list.insert(0, "cjk".to_owned());
            }
        }
        loaded = Some((*path).to_string());
        break;
    }
    ctx.set_fonts(fonts);
    loaded
}

/// Apply the dark theme, spacing and text styles.
pub fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(0x16, 0x18, 0x1C);
    visuals.window_fill = Color32::from_rgb(0x1C, 0x1F, 0x24);
    visuals.extreme_bg_color = Color32::from_rgb(0x0F, 0x11, 0x14);
    visuals.faint_bg_color = Color32::from_rgb(0x22, 0x26, 0x2C);
    visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(0x22, 0x26, 0x2C);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(0x2A, 0x2F, 0x36);
    visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x24, 0x28, 0x2E);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(0x33, 0x39, 0x42);
    visuals.widgets.active.bg_fill = Color32::from_rgb(0x3A, 0x41, 0x4B);
    visuals.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    visuals.window_stroke = Stroke::new(1.0, Color32::from_rgb(0x30, 0x35, 0x3C));
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.slider_width = 180.0;
    style.text_styles = [
        (TextStyle::Heading, egui::FontId::proportional(20.0)),
        (TextStyle::Body, egui::FontId::proportional(14.0)),
        (TextStyle::Button, egui::FontId::proportional(14.0)),
        (TextStyle::Small, egui::FontId::proportional(11.5)),
        (TextStyle::Monospace, egui::FontId::monospace(13.0)),
    ]
    .into();
    ctx.set_style(style);
}

/// A card-like frame used for each block of content.
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::from_rgb(0x20, 0x24, 0x2A))
        .stroke(Stroke::new(1.0, Color32::from_rgb(0x2C, 0x31, 0x39)))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::same(14))
}

/// Draw a full-width card. egui frames shrink to their content, so force the
/// available width to keep the layout aligned.
pub fn card_show<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let width = ui.available_width();
    card()
        .show(ui, |ui| {
            ui.set_min_width((width - 30.0).max(120.0));
            add(ui)
        })
        .inner
}

/// Big number + caption, used at the top of the overview.
pub fn stat(ui: &mut egui::Ui, value: &str, unit: &str, caption: &str, colour: Color32) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(value).size(26.0).strong().color(colour));
            ui.label(egui::RichText::new(unit).size(12.0).color(MUTED));
        });
        ui.label(egui::RichText::new(caption).size(11.0).color(MUTED));
    });
}

/// Horizontal usage bar with a coloured fill.
pub fn meter(ui: &mut egui::Ui, fraction: f32, colour: Color32, height: f32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter();
    let bg = Color32::from_rgb(0x2C, 0x31, 0x39);
    painter.rect_filled(rect, height * 0.5, bg);
    let f = fraction.clamp(0.0, 1.0);
    if f > 0.0 {
        let mut fill = rect;
        fill.set_width(rect.width() * f);
        fill.set_height(height);
        painter.rect_filled(fill, height * 0.5, colour);
    }
}

/// Pill-shaped badge, e.g. the current mode.
pub fn badge(ui: &mut egui::Ui, text: &str, colour: Color32) {
    let galley =
        ui.painter().layout_no_wrap(text.to_owned(), egui::FontId::proportional(12.0), colour);
    let pad = egui::vec2(8.0, 3.0);
    let size = galley.size() + pad * 2.0;
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, size.y * 0.5, colour.gamma_multiply(0.16));
    ui.painter().galley(rect.min + pad, galley, colour);
}

/// Temperature colour ramp: cool grey-blue -> amber -> red.
pub fn temp_colour(temp: f32) -> Color32 {
    match temp {
        t if t < 50.0 => Color32::from_rgb(0x5A, 0xC8, 0xE8),
        t if t < 65.0 => OK,
        t if t < 78.0 => WARN,
        t if t < 90.0 => Color32::from_rgb(0xF5, 0x7C, 0x36),
        _ => DANGER,
    }
}

/// Shorten long key lists for display.
pub fn keys_preview(keys: &[String], max: usize) -> String {
    if keys.is_empty() {
        "无".to_string()
    } else if keys.len() <= max {
        keys.join(" ")
    } else {
        format!("{} … 共 {} 个", keys[..max].join(" "), keys.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every CJK character that appears in the UI source must be renderable by
    /// the installed font, otherwise macOS users see tofu boxes.
    #[test]
    fn cjk_glyphs_are_available() {
        let ctx = egui::Context::default();
        let loaded = install_fonts(&ctx);
        assert!(loaded.is_some(), "no CJK font found on this system");
        // Fonts are only compiled into atlases during a pass, so run one.
        let _ = ctx.run(egui::RawInput::default(), |_| {});

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut chars = std::collections::BTreeSet::new();
        collect_cjk(&root, &mut chars);
        assert!(chars.len() > 200, "expected plenty of CJK text, got {}", chars.len());

        let missing: Vec<char> = chars
            .into_iter()
            .filter(|c| !ctx.fonts(|f| f.has_glyph(&egui::FontId::proportional(14.0), *c)))
            .collect();
        assert!(missing.is_empty(), "font cannot render: {missing:?}");
    }

    fn collect_cjk(dir: &std::path::Path, out: &mut std::collections::BTreeSet<char>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_cjk(&path, out);
            } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    for c in text.chars() {
                        if ('\u{3000}'..='\u{9FFF}').contains(&c) {
                            out.insert(c);
                        }
                    }
                }
            }
        }
    }
}
