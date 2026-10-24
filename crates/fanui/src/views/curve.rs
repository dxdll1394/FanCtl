//! Interactive fan-curve editor.
//!
//! Drag points vertically (and horizontally between neighbours), add/remove
//! points, and preview the resulting RPM at the current temperature.

use eframe::egui::{self, RichText};
use egui_plot::{Legend, Line, Plot, PlotPoints, Points};

use crate::theme::{self, ACCENT, MUTED, RPM};
use crate::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drag {
    None,
    Point(usize),
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    if app.snapshot.is_none() {
        theme::card_show(ui, |ui| {
            ui.label(RichText::new("后台服务未运行，曲线暂时无法下发").color(theme::WARN));
            ui.label(
                RichText::new("你仍可以在这里编辑，配置会在服务启动后保存。").small().color(MUTED),
            );
        });
        ui.add_space(10.0);
    }

    let (min_rpm, max_rpm) = app
        .snapshot
        .as_ref()
        .and_then(|s| s.fans.first())
        .map(|f| (f.min_rpm as f32, f.max_rpm as f32))
        .unwrap_or((1000.0, 4900.0));

    theme::card_show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("风扇曲线").size(16.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("恢复默认").clicked() {
                    app.draft.curve = fancore::Curve::default();
                }
                if ui.button(RichText::new("保存并应用").strong()).clicked() {
                    app.save_draft();
                }
                if app.draft_dirty() {
                    theme::badge(ui, "有未保存修改", theme::WARN);
                }
            });
        });
        ui.add_space(4.0);
        ui.label(
            RichText::new("拖动圆点调整；双击空白处新增点，右键点删除。X 轴温度、Y 轴转速。")
                .small()
                .color(MUTED),
        );
    });

    ui.add_space(8.0);

    theme::card_show(ui, |ui| {
        let temp_now = app.current_temp();
        if let Some(t) = temp_now {
            let want = app.draft.curve.evaluate(t, min_rpm, max_rpm);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("当前 {t:.1}°C")).color(theme::temp_colour(t)));
                ui.label(RichText::new("→").color(MUTED));
                ui.label(RichText::new(format!("{want:.0} rpm")).color(RPM).strong());
                if let Some(actual) = app.snapshot.as_ref().and_then(|s| s.fans.first()) {
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(format!("（实际 {:.0} rpm）", actual.actual_rpm))
                            .small()
                            .color(MUTED),
                    );
                }
            });
        }
        ui.add_space(4.0);

        let plot = Plot::new("curve_editor")
            .height(320.0)
            .legend(Legend::default())
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .include_x(30.0)
            .include_x(105.0)
            .include_y(min_rpm * 0.9)
            .include_y(max_rpm * 1.05)
            .default_x_bounds(30.0, 105.0)
            .default_y_bounds((min_rpm * 0.9) as f64, (max_rpm * 1.05) as f64)
            .x_axis_label("温度 °C")
            .y_axis_label("转速 rpm")
            .label_formatter(|name, value| {
                if name.is_empty() {
                    format!("{:.0}°C / {:.0} rpm", value.x, value.y)
                } else {
                    name.to_string()
                }
            });

        let mut drag: Drag = match app.drag {
            Some(crate::CurveDrag::Point(i)) => Drag::Point(i),
            None => Drag::None,
        };

        let response = plot.show(ui, |plot_ui| {
            let curve: Vec<[f64; 2]> =
                app.draft.curve.points.iter().map(|p| [p.temp as f64, p.rpm as f64]).collect();
            plot_ui.line(Line::new("曲线", PlotPoints::from(curve)).color(ACCENT).width(2.4));
            let pts: Vec<[f64; 2]> =
                app.draft.curve.points.iter().map(|p| [p.temp as f64, p.rpm as f64]).collect();
            plot_ui.points(
                Points::new("控制点", PlotPoints::from(pts)).color(RPM).radius(6.0).filled(true),
            );
            if let Some(t) = temp_now {
                plot_ui.vline(
                    egui_plot::VLine::new("当前温度", t as f64)
                        .color(theme::temp_colour(t))
                        .width(1.5),
                );
            }
            plot_ui.hline(
                egui_plot::HLine::new("紧急全速", app.draft.curve.max_temp as f64)
                    .color(theme::DANGER)
                    .width(1.0),
            );

            let pointer = plot_ui.pointer_coordinate();
            if plot_ui.response().drag_started() {
                if let Some(p) = pointer {
                    if let Some(idx) = nearest_point(&app.draft.curve, p.x as f32, p.y as f32) {
                        drag = Drag::Point(idx);
                    }
                }
            }
            if plot_ui.response().dragged() {
                if let (Drag::Point(idx), Some(p)) = (drag, pointer) {
                    move_point(&mut app.draft.curve, idx, p.x as f32, p.y as f32, min_rpm, max_rpm);
                }
            }
            if plot_ui.response().drag_stopped() && matches!(drag, Drag::Point(_)) {
                app.draft.curve.normalize();
                drag = Drag::None;
            }
            if plot_ui.response().double_clicked() {
                if let Some(p) = pointer {
                    add_point(&mut app.draft.curve, p.x as f32, p.y as f32, min_rpm, max_rpm);
                }
            }
            if plot_ui.response().secondary_clicked() {
                if let Some(p) = pointer {
                    if let Some(idx) = nearest_point(&app.draft.curve, p.x as f32, p.y as f32) {
                        if app.draft.curve.points.len() > 2 {
                            app.draft.curve.points.remove(idx);
                        }
                    }
                }
            }
        });
        let _ = response;

        app.drag = match drag {
            Drag::None => None,
            Drag::Point(i) => Some(crate::CurveDrag::Point(i)),
        };
    });

    ui.add_space(8.0);
    theme::card_show(ui, |ui| {
        ui.label(RichText::new("控制点").strong());
        ui.add_space(4.0);
        egui::Grid::new("points_grid")
            .num_columns(4)
            .spacing(egui::vec2(12.0, 6.0))
            .striped(true)
            .show(ui, |ui| {
                ui.label(RichText::new("#").small().color(MUTED));
                ui.label(RichText::new("温度 °C").small().color(MUTED));
                ui.label(RichText::new("转速 rpm").small().color(MUTED));
                ui.label("");
                ui.end_row();
                let mut remove: Option<usize> = None;
                let point_count = app.draft.curve.points.len();
                for (i, point) in app.draft.curve.points.iter_mut().enumerate() {
                    ui.label(RichText::new(format!("{}", i + 1)).color(MUTED));
                    ui.add(
                        egui::DragValue::new(&mut point.temp)
                            .speed(0.5)
                            .range(0.0..=110.0)
                            .suffix(" °C"),
                    );
                    ui.add(
                        egui::DragValue::new(&mut point.rpm)
                            .speed(20.0)
                            .range(min_rpm..=max_rpm)
                            .suffix(" rpm"),
                    );
                    if ui.add_enabled(point_count > 2, egui::Button::new("删除").small()).clicked()
                    {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
                if let Some(i) = remove {
                    app.draft.curve.points.remove(i);
                }
            });
        app.draft.curve.normalize();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("+ 添加点").clicked() {
                let last = app
                    .draft
                    .curve
                    .points
                    .last()
                    .copied()
                    .unwrap_or(fancore::CurvePoint { temp: 80.0, rpm: max_rpm });
                let temp = (last.temp + 5.0).min(105.0);
                app.draft
                    .curve
                    .points
                    .push(fancore::CurvePoint { temp, rpm: last.rpm.max(min_rpm) });
                app.draft.curve.normalize();
            }
            ui.label(
                RichText::new(format!(
                    "{} 个点 · 曲线最多 16 个点，至少 2 个",
                    app.draft.curve.points.len()
                ))
                .small()
                .color(MUTED),
            );
        });
    });

    ui.add_space(8.0);
    theme::card_show(ui, |ui| {
        ui.label(RichText::new("行为调节").strong());
        ui.add_space(6.0);
        egui::Grid::new("curve_behaviour").num_columns(2).spacing(egui::vec2(16.0, 10.0)).show(
            ui,
            |ui| {
                ui.label("温度平滑");
                ui.add(
                    egui::Slider::new(&mut app.draft.curve.smoothing_secs, 0.0..=20.0)
                        .suffix(" s")
                        .text(""),
                )
                .on_hover_text("越大越稳，但升温响应越慢");
                ui.end_row();
                ui.label("转速回差");
                ui.add(
                    egui::Slider::new(&mut app.draft.curve.rpm_deadband, 0.0..=400.0)
                        .suffix(" rpm")
                        .text(""),
                )
                .on_hover_text("小于此变化量不调整，避免风扇反复变速");
                ui.end_row();
                ui.label("最快变化");
                ui.add(
                    egui::Slider::new(&mut app.draft.curve.max_rpm_per_sec, 100.0..=3000.0)
                        .suffix(" rpm/s")
                        .text(""),
                )
                .on_hover_text(
                    "调速速率上限；空闲时最多攒 1.5 秒的额度，所以一次最多跳 1.5 倍，不会突然拉满",
                );
                ui.end_row();
                ui.label("全速温度");
                ui.add(
                    egui::Slider::new(&mut app.draft.curve.max_temp, 70.0..=110.0)
                        .suffix(" °C")
                        .text(""),
                )
                .on_hover_text("达到此温度直接拉满，忽略曲线");
                ui.end_row();
            },
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("载入当前配置").clicked() {
                app.reload_draft();
            }
            if ui.button("恢复默认曲线").clicked() {
                app.draft.curve = fancore::Curve::default();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(RichText::new("保存并应用").strong()).clicked() {
                    app.save_draft();
                }
            });
        });
    });
}

fn nearest_point(curve: &fancore::Curve, x: f32, y: f32) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for (i, p) in curve.points.iter().enumerate() {
        let dx = (p.temp - x) / 3.0;
        let dy = (p.rpm - y) / 250.0;
        let d = dx * dx + dy * dy;
        if d < 1.0 && best.map(|(bd, _)| d < bd).unwrap_or(true) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| i)
}

fn move_point(
    curve: &mut fancore::Curve,
    idx: usize,
    temp: f32,
    rpm: f32,
    min_rpm: f32,
    max_rpm: f32,
) {
    let n = curve.points.len();
    if idx >= n {
        return;
    }
    let lo = if idx == 0 { 0.0 } else { curve.points[idx - 1].temp + 1.0 };
    let hi = if idx + 1 == n { 120.0 } else { curve.points[idx + 1].temp - 1.0 };
    let point = &mut curve.points[idx];
    point.temp = temp.clamp(lo.max(0.0), hi);
    point.rpm = rpm.clamp(min_rpm, max_rpm);
}

fn add_point(curve: &mut fancore::Curve, temp: f32, rpm: f32, min_rpm: f32, max_rpm: f32) {
    curve.points.push(fancore::CurvePoint {
        temp: temp.clamp(0.0, 110.0),
        rpm: rpm.clamp(min_rpm, max_rpm),
    });
    curve.normalize();
}
