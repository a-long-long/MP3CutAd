//! 左侧文件列表面板

use egui::Context;

use super::state::SharedState;

/// 文件列表
pub fn show(ctx: &Context, state: &SharedState) {
    egui::SidePanel::left("file_panel")
        .default_width(250.0)
        .show(ctx, |ui| {
            ui.heading("文件列表");
            ui.separator();

            let (files, selected) = {
                let s = state.lock().unwrap();
                (s.files.clone(), s.selected_file)
            };

            egui::ScrollArea::vertical().show(ui, |ui| {
                for (idx, file) in files.iter().enumerate() {
                    let is_selected = selected == Some(idx);
                    let bg_color = if is_selected {
                        egui::Color32::from_rgb(60, 100, 160)
                    } else {
                        egui::Color32::from_rgb(40, 40, 40)
                    };

                    let response = egui::Frame::default()
                        .fill(bg_color)
                        .inner_margin(4.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let icon = if file.detected { "✓" } else { "○" };
                                let icon_color = if file.detected {
                                    egui::Color32::from_rgb(100, 200, 100)
                                } else {
                                    egui::Color32::from_rgb(150, 150, 150)
                                };
                                ui.colored_label(icon_color, icon);
                                ui.label(&file.name);
                            });

                            if file.detected {
                                ui.indent("ads", |ui| {
                                    if file.marked_for_deletion.is_empty() {
                                        ui.label(format!("广告: {} 个", file.ad_ranges.len()));
                                    } else {
                                        ui.label(format!(
                                            "广告: {} 个（已标记删除 {}）",
                                            file.ad_ranges.len(),
                                            file.marked_for_deletion.len()
                                        ));
                                    }
                                });
                            }
                        })
                        .response;

                    if response.clicked() {
                        let mut s = state.lock().unwrap();
                        s.selected_file = Some(idx);
                        s.selected_ad = None;
                    }
                }
            });
        });
}
