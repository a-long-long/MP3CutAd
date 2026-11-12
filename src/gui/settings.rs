//! 阈值设置窗口

use egui::Context;

use super::state::{SharedState, Thresholds};

/// 显示阈值设置窗口
pub fn show(ctx: &Context, state: &SharedState) {
    if !state.lock().unwrap().show_settings {
        return;
    }

    let mut th = state.lock().unwrap().thresholds;
    let mut changed = false;
    let mut open = true;

    egui::Window::new("阈值设置")
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            egui::Grid::new("threshold_grid")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("最小广告长度");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut th.min_length_sec)
                                .range(0..=60)
                                .suffix(" 秒"),
                        )
                        .on_hover_text("0 表示不限制，过滤后不参与显示与裁剪")
                        .changed();
                    ui.end_row();

                    ui.label("最小出现次数");
                    changed |= ui
                        .add(egui::DragValue::new(&mut th.min_count).range(2..=10))
                        .on_hover_text("广告至少要在多少个文件中重复出现")
                        .changed();
                    ui.end_row();

                    ui.label("高置信度阈值");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut th.score_high)
                                .range(th.score_low..=1.0)
                                .speed(0.01)
                                .fixed_decimals(2),
                        )
                        .on_hover_text("匹配度不低于此值显示为红色")
                        .changed();
                    ui.end_row();

                    ui.label("低置信度阈值");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut th.score_low)
                                .range(0.0..=th.score_high)
                                .speed(0.01)
                                .fixed_decimals(2),
                        )
                        .on_hover_text("低于此值显示为黄色，介于两者之间为橙色")
                        .changed();
                    ui.end_row();
                });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("恢复默认").clicked() {
                    th = Thresholds::default();
                    changed = true;
                }
                ui.label(
                    egui::RichText::new("修改后立即生效；长度与次数会影响显示和裁剪结果")
                        .small()
                        .color(egui::Color32::from_gray(150)),
                );
            });
        });

    if changed {
        let mut s = state.lock().unwrap();
        s.thresholds = th;
        // 阈值变化会改变有效广告段的数量与序号，清空旧选择与标记避免错位
        s.ads_to_delete.clear();
        for f in &mut s.files {
            f.marked_for_deletion.clear();
        }
    }
    if !open {
        state.lock().unwrap().show_settings = false;
    }
}
