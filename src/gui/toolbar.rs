//! 顶部工具栏

use egui::Context;

use super::state::SharedState;
use super::tasks::classify::{pick_model, start_classification};
use super::tasks::detect::{
    add_directory, add_files, mark_selected_for_deletion, select_all_ads, start_detection,
};
use super::tasks::save;

/// 顶部菜单栏
pub fn menu_bar(ctx: &Context, state: &SharedState) {
    egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.heading("MP3CutAd-RS");
            ui.separator();

            let is_processing = state.lock().unwrap().is_processing;

            if ui.button("添加文件").clicked() && !is_processing {
                add_files(state);
            }

            if ui.button("添加目录").clicked() && !is_processing {
                add_directory(state);
            }

            ui.separator();

            let can_detect = {
                let s = state.lock().unwrap();
                !s.files.is_empty() && !s.is_processing
            };
            if ui.button("检测广告").clicked() && can_detect {
                start_detection(state);
            }

            // HPSS checkbox
            {
                let mut s = state.lock().unwrap();
                ui.checkbox(&mut s.enable_hpss, "HPSS 分离")
                    .on_hover_text("启用打击乐轨道源分离，对带背景音乐的广告段匹配更稳定");
            }

            ui.separator();

            // 事件分类区域
            let model_label = {
                let s = state.lock().unwrap();
                match &s.model_path {
                    Some(p) => p
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "模型已加载".to_string()),
                    None => "未选择模型".to_string(),
                }
            };
            if ui.button(format!("📂 模型: {}", model_label)).clicked() {
                pick_model(state);
            }

            let can_classify = {
                let s = state.lock().unwrap();
                s.model_path.is_some() && !s.files.is_empty() && !s.is_processing
            };
            if ui.button("🎯 分类事件").clicked() && can_classify {
                start_classification(state);
            }

            if ui.button("🏋 训练模型").clicked() {
                let mut s = state.lock().unwrap();
                s.show_train = !s.show_train;
            }

            ui.separator();

            let match_open = state.lock().unwrap().show_match_select;
            if ui
                .selectable_label(match_open, "按匹配度选择")
                .on_hover_text("按高/中/低置信度或自定义下限批量选择广告段")
                .clicked()
            {
                let mut s = state.lock().unwrap();
                s.show_match_select = !match_open;
            }

            if ui.button("全选广告").clicked() {
                select_all_ads(state);
            }

            if ui.button("取消全选").clicked() {
                let mut s = state.lock().unwrap();
                s.ads_to_delete.clear();
                s.status_message = "已取消选择".to_string();
            }

            let selected_count = state.lock().unwrap().ads_to_delete.len();
            if ui
                .button(format!("删除所选 ({})", selected_count))
                .on_hover_text("标记选中的广告段，保存时删除这些段并拼接前后音频")
                .clicked()
                && selected_count > 0
            {
                mark_selected_for_deletion(state);
            }

            {
                let mut s = state.lock().unwrap();
                ui.checkbox(&mut s.overwrite_original, "覆盖原文件");
            }

            let files_to_save = {
                let s = state.lock().unwrap();
                // 与 save.rs 一致：只有被「删除所选」标记过的文件才会被保存
                s.files.iter().filter(|f| f.needs_cut()).count()
            };

            let save_label = if files_to_save > 0 {
                format!("保存 ({} 个文件)", files_to_save)
            } else {
                "保存".to_string()
            };

            let can_save = files_to_save > 0 && !state.lock().unwrap().is_processing;
            if ui.button(save_label).clicked() && can_save {
                save::start_save(state);
            }

            ui.separator();

            let is_processing = state.lock().unwrap().is_processing;
            if ui.button("清空列表").clicked() && !is_processing {
                let mut s = state.lock().unwrap();
                s.files.clear();
                s.selected_file = None;
                s.selected_ad = None;
                s.ads_to_delete.clear();
                s.status_message = "列表已清空".to_string();
            }

            let settings_open = state.lock().unwrap().show_settings;
            if ui.selectable_label(settings_open, "阈值设置").clicked() {
                let mut s = state.lock().unwrap();
                s.show_settings = !settings_open;
            }
        });
    });
}
