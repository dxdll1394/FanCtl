//! Settings tab: sensor source, safety limits, startup behaviour, diagnostics.

use eframe::egui::{self, RichText};

use crate::theme::{self, MUTED, WARN};
use crate::App;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    sensor_settings(app, ui);
    ui.add_space(10.0);
    safety_settings(app, ui);
    ui.add_space(10.0);
    app_settings(app, ui);
    ui.add_space(10.0);
    diagnostics(app, ui);
}

fn sensor_settings(app: &mut App, ui: &mut egui::Ui) {
    theme::card_show(ui, |ui| {
        ui.label(RichText::new("温度源").strong());
        ui.label(
            RichText::new(
                "曲线用哪一组传感器作为输入。Apple 的调速以封装热点为主，一般保持默认即可。",
            )
            .small()
            .color(MUTED),
        );
        ui.add_space(6.0);
        let available: Vec<fancore::SensorGroup> = app
            .snapshot
            .as_ref()
            .map(|s| s.sensors.iter().filter(|x| x.temp.is_some()).map(|x| x.group).collect())
            .unwrap_or_else(|| fancore::SensorGroup::ALL.to_vec());

        for group in fancore::SensorGroup::ALL {
            if !available.contains(&group) {
                continue;
            }
            let selected = app.draft.sensor == group;
            ui.horizontal(|ui| {
                if ui.radio(selected, RichText::new(group.label()).size(13.5)).clicked()
                    && !selected
                {
                    app.draft.sensor = group;
                    app.save_draft();
                }
                if let Some(sensor) =
                    app.snapshot.as_ref().and_then(|s| s.sensors.iter().find(|s| s.group == group))
                {
                    if let Some(t) = sensor.temp {
                        ui.label(
                            RichText::new(format!("{t:.1}°C")).color(theme::temp_colour(t)).small(),
                        );
                    }
                    ui.label(RichText::new(group.description()).small().color(MUTED));
                }
            });
        }
    });
}

fn safety_settings(app: &mut App, ui: &mut egui::Ui) {
    theme::card_show(ui, |ui| {
        ui.label(RichText::new("安全").strong());
        ui.add_space(6.0);
        egui::Grid::new("safety").num_columns(2).spacing(egui::vec2(18.0, 10.0)).show(ui, |ui| {
            ui.label("紧急全速温度");
            ui.add(
                egui::Slider::new(&mut app.draft.emergency_temp, 70.0..=110.0)
                    .suffix(" °C")
                    .text(""),
            )
            .on_hover_text("达到这个温度立即全速，不管曲线怎么设置");
            ui.end_row();

            ui.label("退出时交还系统");
            ui.checkbox(&mut app.draft.restore_auto_on_exit, "守护进程停止或退出时恢复自动");
            ui.end_row();

            ui.label("采样间隔");
            ui.add(egui::Slider::new(&mut app.draft.poll_ms, 500..=5000).suffix(" ms").text(""))
                .on_hover_text("越小响应越快，CPU 占用越高（默认 1000 ms）");
            ui.end_row();
        });
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "保护逻辑：温度传感器连续 5 秒读不到时自动交还 macOS；\
                 写入失败会记录到日志并保持风扇在安全转速。",
            )
            .small()
            .color(MUTED),
        );
    });
}

fn app_settings(app: &mut App, ui: &mut egui::Ui) {
    theme::card_show(ui, |ui| {
        ui.label(RichText::new("界面").strong());
        ui.add_space(6.0);
        let mut menu_only = app.menu_bar_only;
        if ui.checkbox(&mut menu_only, "关闭窗口后留在菜单栏（否则直接退出界面）").changed()
        {
            app.menu_bar_only = menu_only;
            app.save_ui_prefs();
        }
        ui.label(
            RichText::new("无论选哪个，后台守护进程都会继续按曲线调速；这里只决定界面窗口的去留。")
                .small()
                .color(MUTED),
        );
        let mut text = app.menu_bar_text;
        if ui.checkbox(&mut text, "菜单栏显示温度与转速").changed() {
            app.menu_bar_text = text;
            app.save_ui_prefs();
        }
        ui.add_space(6.0);
        if ui.button("立即交还系统自动控制").clicked() {
            app.backend.send(crate::backend::Action::Auto);
        }
        ui.label(
            RichText::new("守护进程配置：/Library/Application Support/FanCtl/config.json")
                .small()
                .color(MUTED),
        );
        ui.label(
            RichText::new("界面偏好：~/Library/Application Support/FanCtl/ui.json")
                .small()
                .color(MUTED),
        );
    });
}

fn diagnostics(app: &mut App, ui: &mut egui::Ui) {
    theme::card_show(ui, |ui| {
        ui.label(RichText::new("诊断").strong());
        ui.add_space(4.0);
        let status = match &app.snapshot {
            Some(s) => format!(
                "✔ 已连接 · PID {} · 协议 v{} · {}",
                s.pid,
                s.version,
                if s.dry_run { "只读预览" } else { "可写" }
            ),
            None => "✘ 未连接后台服务".to_string(),
        };
        let colour = if app.snapshot.is_some() { theme::OK } else { theme::DANGER };
        ui.label(RichText::new(status).color(colour));
        ui.label(
            RichText::new(format!("socket：{}", app.backend.socket_path().display()))
                .small()
                .color(MUTED),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("打开日志").clicked() {
                app.open_path = Some("/Library/Logs/FanCtl/daemon.log".to_string());
            }
            if ui.button("打开配置目录").clicked() {
                app.open_path = Some("/Library/Application Support/FanCtl".to_string());
            }
            if ui.button("复制自检命令").clicked() {
                app.copy_to_clipboard("fanctl doctor");
            }
            if ui.button("重新载入配置").clicked() {
                app.backend.send(crate::backend::Action::Reload);
            }
        });
        if let Some(err) = app.error_text() {
            ui.add_space(6.0);
            ui.label(RichText::new(err).color(WARN));
        }
    });
}
