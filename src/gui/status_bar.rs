//! 底部状态栏

use egui::Context;

use super::state::SharedState;

/// 底部状态栏
pub fn status_bar(ctx: &Context, state: &SharedState, sys: super::sysmon::Snapshot) {
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            let s = state.lock().unwrap();

            ui.label(&s.status_message);

            if s.is_processing {
                ui.add_space(10.0);
                ui.add(
                    egui::ProgressBar::new(s.progress)
                        .show_percentage()
                        .animate(true),
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // 从右向左添加：GPU、CPU 位于最右侧
                match (sys.gpu, sys.cpu) {
                    (Some(gpu), Some(cpu)) => {
                        ui.label(format!("GPU {:.0}%", gpu));
                        ui.separator();
                        ui.label(format!("CPU {:.0}%", cpu));
                    }
                    (Some(gpu), None) => {
                        ui.label(format!("GPU {:.0}%", gpu));
                    }
                    (None, Some(cpu)) => {
                        ui.label(format!("CPU {:.0}%", cpu));
                    }
                    (None, None) => {}
                }
                ui.separator();
                ui.label(format!("文件数: {}", s.files.len()));
                ui.separator();
                let min_len = if s.thresholds.min_length_sec > 0 {
                    format!("最小长度: {}秒", s.thresholds.min_length_sec)
                } else {
                    "最小长度: 不限".to_string()
                };
                ui.label(min_len);
                ui.label(format!("最小次数: {}", s.thresholds.min_count));
            });
        });
    });
}
