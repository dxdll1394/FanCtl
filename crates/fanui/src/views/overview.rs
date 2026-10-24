//! Overview tab: live status, sensor table and the temperature/RPM chart.

use eframe::egui::{self, Color32, RichText};
use egui_plot::{Legend, Line, Plot, PlotPoints};

use crate::theme::{self, ACCENT, MUTED, RPM, TEMP};
use crate::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartRange {
    Min1,
    Min5,
    Min15,
    Hour1,
}

impl ChartRange {
    pub const ALL: [ChartRange; 4] =
        [ChartRange::Min1, ChartRange::Min5, ChartRange::Min15, ChartRange::Hour1];

    pub fn label(self) -> &'static str {
        match self {
            ChartRange::Min1 => "1 分钟",
            ChartRange::Min5 => "5 分钟",
            ChartRange::Min15 => "15 分钟",
            ChartRange::Hour1 => "1 小时",
        }
    }

    pub fn seconds(self) -> f64 {
        match self {
            ChartRange::Min1 => 60.0,
            ChartRange::Min5 => 300.0,
            ChartRange::Min15 => 900.0,
            ChartRange::Hour1 => 3600.0,
        }
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let Some(snapshot) = app.snapshot.clone() else {
        offline_panel(app, ui);
        return;
    };

    ui.add_space(2.0);
    top_stats(app, ui, &snapshot);
    ui.add_space(12.0);
    ui.columns(2, |cols| {
        sensor_card(&mut cols[0], &snapshot);
        policy_card(app, &mut cols[1], &snapshot);
    });
    ui.add_space(12.0);
    chart_card(app, ui);

    if !snapshot.warnings.is_empty() {
        ui.add_space(10.0);
        for warning in &snapshot.warnings {
            ui.horizontal(|ui| {
                ui.label(RichText::new("⚠").color(theme::WARN));
                ui.label(RichText::new(warning).color(theme::WARN));
            });
        }
    }
}

fn offline_panel(app: &mut App, ui: &mut egui::Ui) {
    theme::card_show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(30.0);
            ui.label(RichText::new("后台服务未运行").size(20.0).strong());
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "风扇控制需要 root 权限的守护进程。安装一次即可，它会随系统启动。",
                )
                .color(MUTED),
            );
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if ui
                    .button(RichText::new("安装并启动后台服务").strong())
                    .clicked()
                {
                    app.run_install_dialog = true;
                }
                if ui.button("重试连接").clicked() {
                    app.backend.send(crate::backend::Action::Refresh);
                }
                if ui.button("打开安装说明").clicked() {
                    app.open_path = Some(app.install_script().display().to_string());
                }
            });
            if let Some(err) = app.error_text().or_else(|| {
                app.backend
                    .last_error
                    .lock()
                    .ok()
                    .and_then(|e| e.clone())
            }) {
                ui.add_space(10.0);
                ui.label(RichText::new(err).color(theme::DANGER));
            }
            ui.add_space(10.0);
            ui.label(
                RichText::new(
                    "命令行替代方案： sudo /usr/local/libexec/fanctl/fand  （或 cargo run -p fand -- --dry-run 只观察）",
                )
                .small()
                .color(MUTED),
            );
            ui.add_space(30.0);
        });
    });

    if app.run_install_dialog {
        install_dialog(app, ui);
    }
}

fn install_dialog(app: &mut App, ui: &mut egui::Ui) {
    let mut open = app.run_install_dialog;
    egui::Window::new("安装后台服务")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ui.ctx(), |ui| {
            ui.label("macOS 会要求输入管理员密码，然后：");
            ui.add_space(4.0);
            for line in [
                "1. 安装 /usr/local/libexec/fanctl/fand",
                "2. 注册 launchd 服务 local.fanctl.daemon",
                "3. 服务启动后本窗口会自动连接",
            ] {
                ui.label(RichText::new(line).small().color(MUTED));
            }
            ui.add_space(8.0);
            ui.label(RichText::new("命令：sudo ./install.sh --daemon").monospace().small());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("打开终端并复制命令").clicked() {
                    app.copy_to_clipboard("sudo ./install.sh --daemon");
                }
                if ui.button("关闭").clicked() {
                    app.run_install_dialog = false;
                }
            });
        });
    if !open {
        app.run_install_dialog = false;
    }
}

fn top_stats(_app: &mut App, ui: &mut egui::Ui, snap: &fancore::protocol::Snapshot) {
    theme::card_show(ui, |ui| {
        ui.horizontal(|ui| {
            let temp = snap.control_temp;
            theme::stat(
                ui,
                &temp.map(|t| format!("{t:.1}")).unwrap_or_else(|| "--".into()),
                "°C",
                &format!("{}（曲线输入）", snap.config.sensor.label()),
                temp.map(theme::temp_colour).unwrap_or(MUTED),
            );
            ui.add_space(22.0);
            let actual = snap.fans.first().map(|f| f.actual_rpm).unwrap_or(0.0);
            theme::stat(ui, &format!("{actual:.0}"), "rpm", "实际转速", RPM);
            ui.add_space(22.0);
            let target = snap.target_rpm.or(snap.fans.first().map(|f| f.target_rpm));
            theme::stat(
                ui,
                &target.map(|t| format!("{t:.0}")).unwrap_or_else(|| "--".into()),
                "rpm",
                "目标转速",
                ACCENT,
            );
            ui.add_space(22.0);
            let fan = snap.fans.first();
            let (lo, hi) = fan.map(|f| (f.min_rpm, f.max_rpm)).unwrap_or((1000.0, 4900.0));
            theme::stat(ui, &format!("{lo:.0}–{hi:.0}"), "rpm", "硬件范围", MUTED);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let colour = match snap.mode {
                    fancore::types::ControlMode::Auto => MUTED,
                    fancore::types::ControlMode::Fixed => ACCENT,
                    fancore::types::ControlMode::Curve => theme::OK,
                };
                theme::badge(ui, snap.mode.label(), colour);
                if snap.dry_run {
                    theme::badge(ui, "只读预览", theme::WARN);
                }
                ui.vertical(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(&snap.status).color(MUTED).small());
                        ui.label(RichText::new("状态").color(MUTED).small());
                    });
                    ui.label(
                        RichText::new(format!(
                            "PID {} · 已运行 {}",
                            snap.pid,
                            uptime(snap.uptime_secs)
                        ))
                        .color(MUTED)
                        .small(),
                    );
                });
            });
        });

        ui.add_space(6.0);
        let fan_fraction = snap
            .fans
            .first()
            .map(|f| {
                if f.max_rpm > f.min_rpm {
                    ((f.actual_rpm - f.min_rpm) / (f.max_rpm - f.min_rpm)) as f32
                } else {
                    0.0
                }
            })
            .unwrap_or(0.0);
        theme::meter(ui, fan_fraction, RPM, 6.0);
    });
}

fn uptime(secs: u64) -> String {
    if secs < 60 {
        format!("{secs} 秒")
    } else if secs < 3600 {
        format!("{} 分钟", secs / 60)
    } else if secs < 86_400 {
        format!("{} 小时 {} 分", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{} 天", secs / 86_400)
    }
}

fn sensor_card(ui: &mut egui::Ui, snap: &fancore::protocol::Snapshot) {
    theme::card_show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        ui.horizontal(|ui| {
            ui.label(RichText::new("温度传感器").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!(
                        "共 {} 个键",
                        snap.sensors.iter().map(|s| s.keys.len()).sum::<usize>()
                    ))
                    .small()
                    .color(MUTED),
                );
            });
        });
        ui.add_space(6.0);
        for sensor in &snap.sensors {
            let selected = sensor.group == snap.config.sensor;
            let (rect, response) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), 17.0), egui::Sense::hover());
            if selected {
                ui.painter().rect_filled(rect, 4.0, ACCENT.gamma_multiply(0.12));
            }
            let temp = sensor.temp;
            ui.put(
                egui::Rect::from_min_size(rect.min + egui::vec2(6.0, 2.0), egui::vec2(110.0, 18.0)),
                egui::Label::new(RichText::new(&sensor.name).size(13.0).color(if selected {
                    Color32::WHITE
                } else {
                    Color32::LIGHT_GRAY
                })),
            );
            let bar_width = 110.0;
            let bar_rect = egui::Rect::from_min_size(
                rect.min + egui::vec2(rect.width() - bar_width - 92.0, 6.0),
                egui::vec2(bar_width, 10.0),
            );
            let colour = temp.map(theme::temp_colour).unwrap_or(MUTED);
            ui.painter().rect_filled(bar_rect, 5.0, Color32::from_rgb(0x2C, 0x31, 0x39));
            if let Some(t) = temp {
                let mut fill = bar_rect;
                fill.set_width(bar_rect.width() * ((t / 110.0).clamp(0.0, 1.0)));
                ui.painter().rect_filled(fill, 5.0, colour);
            }
            ui.painter().text(
                egui::pos2(rect.max.x - 6.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                temp.map(|t| format!("{t:.1}°C")).unwrap_or_else(|| "—".into()),
                egui::FontId::proportional(12.5),
                colour,
            );
            response.on_hover_text(format!(
                "{}\n{}\n传感器：{}",
                sensor.name,
                sensor.group.description(),
                theme::keys_preview(&sensor.keys, 12)
            ));
        }
        ui.add_space(4.0);
        ui.label(
            RichText::new("高亮行是当前曲线使用的温度源，可在「设置」里切换").small().color(MUTED),
        );
    });
}

fn policy_card(app: &mut App, ui: &mut egui::Ui, snap: &fancore::protocol::Snapshot) {
    theme::card_show(ui, |ui| {
        ui.label(RichText::new("控制方式").strong());
        ui.add_space(4.0);
        let mut mode = snap.config.mode;
        for option in [
            fancore::types::ControlMode::Auto,
            fancore::types::ControlMode::Fixed,
            fancore::types::ControlMode::Curve,
        ] {
            let selected = mode == option;
            let text = match option {
                fancore::types::ControlMode::Auto => "系统自动 — 由 macOS 决定转速",
                fancore::types::ControlMode::Fixed => "固定转速 — 始终跑同一个转速",
                fancore::types::ControlMode::Curve => "温度曲线 — 按温度线性调速",
            };
            if ui.add(egui::Button::selectable(selected, RichText::new(text).size(13.5))).clicked()
                && !selected
            {
                mode = option;
                app.backend.send(crate::backend::Action::SetMode(option));
            }
        }
        ui.add_space(6.0);
        match snap.config.mode {
            fancore::types::ControlMode::Fixed => {
                ui.horizontal(|ui| {
                    ui.label("目标转速");
                    let mut rpm = snap.config.fixed_rpm as f32;
                    let (lo, hi) = snap
                        .fans
                        .first()
                        .map(|f| (f.min_rpm as f32, f.max_rpm as f32))
                        .unwrap_or((1000.0, 4900.0));
                    if ui.add(egui::Slider::new(&mut rpm, lo..=hi).suffix(" rpm")).changed() {
                        app.draft.fixed_rpm = rpm as f64;
                    }
                    if ui.button("应用").clicked() {
                        app.backend.send(crate::backend::Action::SetFixed {
                            rpm: app.draft.fixed_rpm,
                            fan: None,
                        });
                    }
                });
            }
            fancore::types::ControlMode::Curve => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("曲线").color(MUTED));
                    if let Some(t) = snap.control_temp {
                        let rpm = snap.config.curve.evaluate(
                            t,
                            snap.fans.first().map(|f| f.min_rpm as f32).unwrap_or(1000.0),
                            snap.fans.first().map(|f| f.max_rpm as f32).unwrap_or(4900.0),
                        );
                        ui.label(
                            RichText::new(format!("{t:.1}°C → {rpm:.0} rpm"))
                                .color(theme::OK)
                                .strong(),
                        );
                    } else {
                        ui.label(RichText::new("等待温度").color(MUTED));
                    }
                });
                ui.label(
                    RichText::new(format!(
                        "平滑 {:.0}s · 回差 {:.0} rpm · 限速 {:.0} rpm/s · {:.0}°C 以上全速",
                        snap.config.curve.smoothing_secs,
                        snap.config.curve.rpm_deadband,
                        snap.config.curve.max_rpm_per_sec,
                        snap.config.curve.max_temp
                    ))
                    .small()
                    .color(MUTED),
                );
            }
            fancore::types::ControlMode::Auto => {
                ui.label(
                    RichText::new("macOS 会在 1000–4900 rpm 之间自行调节，fanctl 不干预。")
                        .small()
                        .color(MUTED),
                );
            }
        }
        ui.add_space(6.0);
        if ui.button("编辑曲线…").clicked() {
            app.tab = crate::Tab::Curve;
        }
    });
}

fn chart_card(app: &mut App, ui: &mut egui::Ui) {
    theme::card_show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("温度 / 转速").strong());
            ui.add_space(8.0);
            for range in ChartRange::ALL {
                if ui.selectable_label(app.chart_range == range, range.label()).clicked() {
                    app.chart_range = range;
                    app.chart_auto = true;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.checkbox(&mut app.chart_auto, "跟随最新");
                ui.label(
                    RichText::new(format!("{} 个采样", app.history_len())).small().color(MUTED),
                );
            });
        });

        let samples = app.history_window(app.chart_range.seconds());
        let now = samples.last().map(|s| s.t).unwrap_or(0.0);
        let span = app.chart_range.seconds();
        // X axis is seconds relative to "now" (negative = past) so the tick
        // labels stay readable regardless of the wall clock.
        let temp_points: Vec<[f64; 2]> =
            samples.iter().filter_map(|s| s.control_temp.map(|t| [s.t - now, t as f64])).collect();
        let rpm_points: Vec<[f64; 2]> =
            samples.iter().filter_map(|s| s.fan_rpm.map(|r| [s.t - now, r as f64])).collect();
        let target_points: Vec<[f64; 2]> =
            samples.iter().filter_map(|s| s.target_rpm.map(|r| [s.t - now, r as f64])).collect();

        let (lo, hi) = app
            .snapshot
            .as_ref()
            .and_then(|s| s.fans.first())
            .map(|f| (f.min_rpm, f.max_rpm))
            .unwrap_or((1000.0, 4900.0));

        let tick = nice_step(span);
        // The chart gets whatever is left after the two cards above it, but at
        // least enough to read; the tab scrolls if the window is very short.
        let plot_height = (ui.available_height() - 40.0).clamp(110.0, 300.0);
        let plot = Plot::new("temps")
            .height(plot_height)
            .legend(Legend::default())
            .allow_drag(true)
            .allow_zoom(true)
            .allow_scroll(false)
            .include_y(lo.max(0.0))
            .include_y(hi)
            .include_y(20.0)
            .include_y(100.0)
            .default_x_bounds(-span, 0.0)
            .x_grid_spacer(move |input| {
                egui_plot::uniform_grid_spacer(move |_| [tick, tick * 2.0, tick * 5.0])(input)
            })
            .x_axis_formatter(|mark, _| {
                let secs = -mark.value;
                if secs.abs() < 0.5 {
                    "现在".to_string()
                } else if secs < 60.0 {
                    format!("-{:.0}s", secs)
                } else {
                    format!("-{:.0}:{:02}", (secs / 60.0).floor(), (secs % 60.0).round())
                }
            })
            .y_axis_label("°C  /  rpm")
            .label_formatter(|name, value| {
                if name.is_empty() {
                    String::new()
                } else if name == "温度" {
                    format!("{name}: {:.1} °C", value.y)
                } else {
                    format!("{name}: {:.0} rpm", value.y)
                }
            });

        let response = plot.show(ui, |plot_ui| {
            if !rpm_points.is_empty() {
                plot_ui.line(
                    Line::new("实际转速", PlotPoints::from(rpm_points)).color(RPM).width(1.8),
                );
            }
            if !target_points.is_empty() {
                plot_ui.line(
                    Line::new("目标转速", PlotPoints::from(target_points))
                        .color(ACCENT.gamma_multiply(0.7))
                        .width(1.2)
                        .style(egui_plot::LineStyle::dashed_dense()),
                );
            }
            if !temp_points.is_empty() {
                plot_ui
                    .line(Line::new("温度", PlotPoints::from(temp_points)).color(TEMP).width(1.8));
            }
        });

        if response.response.double_clicked() {
            app.chart_auto = true;
        }
    });
}

/// Pick a readable time-axis step for a window of `span` seconds.
fn nice_step(span: f64) -> f64 {
    let candidates = [5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0];
    for c in candidates {
        if span / c <= 8.0 {
            return c;
        }
    }
    1800.0
}

#[cfg(test)]
mod tests {
    use super::nice_step;

    #[test]
    fn tick_steps_stay_readable() {
        assert!(nice_step(60.0) <= 10.0);
        for span in [60.0, 300.0, 900.0, 3600.0] {
            let ticks = span / nice_step(span);
            assert!((4.0..=8.0).contains(&ticks), "span {span} -> {ticks} ticks");
        }
    }
}
