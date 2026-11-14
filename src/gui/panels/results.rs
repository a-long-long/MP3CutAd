//! 中央结果视图
//!
//! 负责整体布局与文件摘要；音频条、事件轨道与图例见 [`crate::gui::widgets`]。

use std::collections::HashMap;

use egui::{self, Context};

use super::super::playback;
use super::super::state::{FileInfo, SharedState};
use super::super::widgets::legend::{ad_colors, confidence_legend, event_legend, event_palette};
use super::super::widgets::track::{average_score, draw_file_track};

/// 检测结果面板
pub fn show(ctx: &Context, state: &SharedState) {
    egui::CentralPanel::default().show(ctx, |ui| {
        let mut all_files: Vec<(usize, FileInfo)> = {
            let s = state.lock().unwrap();
            s.files
                .iter()
                .enumerate()
                .filter(|(_, f)| f.detected || f.classified || !f.event_ranges.is_empty())
                .map(|(i, f)| (i, f.clone()))
                .collect()
        };

        let (mut sort_by_score, thresholds) = {
            let s = state.lock().unwrap();
            (s.sort_by_score, s.thresholds)
        };
        if sort_by_score {
            // 匹配度高的排在前面；未知匹配度（旧结果）排最后
            all_files.sort_by(|a, b| {
                let sa = average_score(&a.1, thresholds).unwrap_or(-1.0);
                let sb = average_score(&b.1, thresholds).unwrap_or(-1.0);
                sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        if all_files.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.heading("检测/分类完成后在此显示结果");
            });
            return;
        }

        let ads_total: usize = all_files
            .iter()
            .map(|(_, f)| f.effective_ad_count(thresholds))
            .sum();
        let events_total: usize = all_files
            .iter()
            .map(|(_, f)| f.event_ranges.len())
            .sum();

        // 事件标签颜色由全局排序后的标签顺序决定，保证轨道与图例颜色一致
        let all_labels: Vec<String> = {
            let mut labels: Vec<String> = all_files
                .iter()
                .flat_map(|(_, f)| f.event_ranges.iter().map(|e| e.label.clone()))
                .collect();
            labels.sort();
            labels.dedup();
            labels
        };
        let palette = event_palette();
        let label_colors: HashMap<String, egui::Color32> = all_labels
            .iter()
            .enumerate()
            .map(|(i, label)| (label.clone(), palette[i % palette.len()]))
            .collect();

        ui.horizontal(|ui| {
            let label = match (ads_total > 0, events_total > 0) {
                (true, true) => format!(
                    "检测结果 ({} 文件, {} 广告段, {} 事件)",
                    all_files.len(), ads_total, events_total
                ),
                (true, false) => format!(
                    "检测结果 ({} 文件, {} 广告段)",
                    all_files.len(), ads_total
                ),
                (false, true) => format!(
                    "分类结果 ({} 文件, {} 事件)",
                    all_files.len(), events_total
                ),
                _ => format!("结果 ({} 文件)", all_files.len()),
            };
            ui.heading(label);

            let mut show_spectrogram = state.lock().unwrap().show_spectrogram;
            if ui
                .checkbox(&mut show_spectrogram, "频谱")
                .on_hover_text("选中文件后显示能量包络与 Mel 频谱")
                .changed()
            {
                state.lock().unwrap().show_spectrogram = show_spectrogram;
            }

            if ui
                .checkbox(&mut sort_by_score, "按匹配度排序")
                .on_hover_text("匹配度高的文件排在前面，未知匹配度排最后")
                .changed()
            {
                state.lock().unwrap().sort_by_score = sort_by_score;
            }

            ui.separator();
            confidence_legend(ui, thresholds);

            // 事件标签图例（只在有事件分类结果时显示）
            if !all_labels.is_empty() {
                ui.separator();
                event_legend(ui, &all_labels, &label_colors);
            }
        });
        ui.separator();

        // 滚动条改为非悬浮并收窄：悬浮滚动条会覆盖在音频条右端，
        // 导致尾部广告段被遮挡、难以点击。非悬浮模式会为滚动条保留独立空间。
        ui.spacing_mut().scroll.floating = false;
        ui.spacing_mut().scroll.bar_width = 8.0;
        ui.spacing_mut().scroll.bar_inner_margin = 2.0;
        ui.spacing_mut().scroll.bar_outer_margin = 2.0;

        egui::ScrollArea::vertical().show(ui, |ui| {
            let colors = ad_colors();
            for (file_idx, file) in &all_files {
                draw_file_track(
                    ui,
                    ctx,
                    state,
                    *file_idx,
                    file,
                    &colors,
                    thresholds,
                    &label_colors,
                );
            }
        });
    });
}

/// 处理音频条点击：选中广告段（Ctrl）或切换播放
pub(crate) fn handle_track_click(
    ctx: &Context,
    state: &SharedState,
    file_idx: usize,
    effective_ads: &[&crate::core::range::Range],
    bar_rect: egui::Rect,
    file_len: f32,
    response: &egui::Response,
) {
    let Some(click_pos) = response.interact_pointer_pos() else {
        return;
    };

    let clicked_ad = effective_ads.iter().enumerate().find_map(|(ai, ad)| {
        let x0 = bar_rect.left() + (ad.begin as f32 / file_len) * bar_rect.width();
        let x1 = bar_rect.left() + (ad.end as f32 / file_len) * bar_rect.width();
        (click_pos.x >= x0 && click_pos.x <= x1).then_some(ai)
    });

    // 点击任意位置都同步选中文件，便于查看该文件的频谱
    state.lock().unwrap().selected_file = Some(file_idx);

    match clicked_ad {
        Some(ai) if ctx.input(|i| i.modifiers.ctrl) => toggle_selection(state, file_idx, ai),
        Some(ai) => playback::toggle_ad_playback(state, file_idx, ai),
        None => {}
    }
}

fn toggle_selection(state: &SharedState, file_idx: usize, ad_idx: usize) {
    let mut s = state.lock().unwrap();
    let position = s
        .ads_to_delete
        .iter()
        .position(|&(fi, idx)| fi == file_idx && idx == ad_idx);

    if let Some(pos) = position {
        s.ads_to_delete.remove(pos);
        s.status_message = format!("已取消选择广告段 {}", ad_idx + 1);
    } else {
        s.ads_to_delete.push((file_idx, ad_idx));
        let count = s.ads_to_delete.len();
        s.status_message = format!("已选择广告段 {} (共 {} 个)", ad_idx + 1, count);
    }
}

