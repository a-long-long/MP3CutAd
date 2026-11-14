//! 音频条、事件轨道与时间刻度

use std::collections::HashMap;

use egui::{self, Context};

use crate::core::detect::reverse_range;

use super::super::panels::results::handle_track_click;
use super::super::state::{FileInfo, SharedState};
use super::legend::confidence_color;
use super::spectrogram;

/// 音频条高度
const TRACK_HEIGHT: f32 = 28.0;
/// 事件分类轨道高度
const EVENT_TRACK_HEIGHT: f32 = 20.0;
/// 时间刻度高度
const TICK_HEIGHT: f32 = 20.0;

/// 有效广告段的平均匹配度；没有可用匹配度时返回 None
pub(crate) fn average_score(
    file: &FileInfo,
    thresholds: crate::gui::state::Thresholds,
) -> Option<f64> {
    let scored: Vec<f64> = file
        .effective_ads(thresholds)
        .filter(|r| r.score > 0.0)
        .map(|r| r.score)
        .collect();
    if scored.is_empty() {
        None
    } else {
        Some(scored.iter().sum::<f64>() / scored.len() as f64)
    }
}

pub(crate) fn draw_file_track(
    ui: &mut egui::Ui,
    ctx: &Context,
    state: &SharedState,
    file_idx: usize,
    file: &FileInfo,
    colors: &[egui::Color32],
    thresholds: crate::gui::state::Thresholds,
    label_colors: &HashMap<String, egui::Color32>,
) {
    let file_len = file.length as f32;
    let effective_ads: Vec<&crate::core::range::Range> =
        file.effective_ads(thresholds).collect();
    let event_ranges = &file.event_ranges;

    // 剩余时长按实际会被裁剪的段（已标记删除）计算，与保存结果保持一致
    let marked_owned = file.marked_segments(thresholds);
    let remaining_duration = if marked_owned.is_empty() {
        file.length as f64 / 10.0
    } else {
        let remaining_segments = reverse_range(&marked_owned, file.length);
        remaining_segments
            .iter()
            .map(|r| (r.end - r.begin) as f64 / 10.0)
            .sum()
    };
    let avg_score = average_score(file, thresholds);

    ui.horizontal(|ui| {
        // 状态图标
        let icon = if file.detected && file.classified { "⚡" }
            else if file.detected { "✓" }
            else if file.classified { "🎯" }
            else { "○" };
        ui.label(icon);
        ui.strong(&file.name);

        let orig_dur = if file.length > 0 { file.length as f64 / 10.0 } else { 0.0 };
        let mut summary = format!(
            "  原始: {:.1}s", orig_dur
        );
        if !effective_ads.is_empty() {
            summary.push_str(&format!(
                " | 剩余: {:.1}s | {} 广告段（已标记删除 {}）",
                remaining_duration,
                effective_ads.len(),
                file.marked_for_deletion.len()
            ));
        }
        if !event_ranges.is_empty() {
            summary.push_str(&format!(" | {} 事件", event_ranges.len()));
        }
        if let Some(score) = avg_score {
            summary.push_str(&format!(" | 平均匹配度: {:.0}%", score * 100.0));
        }
        ui.label(summary);
    });

    ui.add_space(2.0);

    if file_len > 0.0 {
        let bar_width = ui.available_width();
        let (bar_resp, painter) =
            ui.allocate_painter(egui::vec2(bar_width, TRACK_HEIGHT), egui::Sense::click());
        let bar_rect = bar_resp.rect;

        painter.rect_filled(bar_rect, 3.0, egui::Color32::from_rgb(30, 30, 35));
        painter.rect_stroke(
            bar_rect,
            3.0,
            egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(70, 70, 70)),
        );

        let (is_this_file_playing, playing_ad_idx, selected_ads) = {
            let s = state.lock().unwrap();
            let selected = s
                .ads_to_delete
                .iter()
                .filter(|&&(fi, _)| fi == file_idx)
                .map(|&(_, ad_idx)| ad_idx)
                .collect::<Vec<_>>();
            (
                s.playback.playing_file == Some(file_idx),
                s.playback.playing_ad,
                selected,
            )
        };

        for (ai, ad) in effective_ads.iter().enumerate() {
            let x0 = bar_rect.left() + (ad.begin as f32 / file_len) * bar_rect.width();
            let x1 = bar_rect.left() + (ad.end as f32 / file_len) * bar_rect.width();
            let ad_rect = egui::Rect::from_x_y_ranges(
                x0.max(bar_rect.left())..=x1.min(bar_rect.right()),
                bar_rect.top() + 2.0..=bar_rect.bottom() - 2.0,
            );
            let color =
                confidence_color(ad.score, colors[(ai + file_idx) % colors.len()], thresholds);

            // 正在播放的段更亮，其他段半透明
            let alpha = if is_this_file_playing && playing_ad_idx == Some(ai) {
                255u8
            } else if is_this_file_playing {
                50u8
            } else {
                180u8
            };
            painter.rect_filled(
                ad_rect,
                2.0,
                egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha),
            );

            // 已标记删除的段：压暗并画红色斜线
            let marked = file.marked_for_deletion.contains(&ai);
            if marked {
                painter.rect_filled(
                    ad_rect,
                    2.0,
                    egui::Color32::from_rgba_unmultiplied(0, 0, 0, 120),
                );
                painter.rect_stroke(
                    ad_rect,
                    2.0,
                    egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(230, 60, 60)),
                );
                painter.line_segment(
                    [ad_rect.left_bottom(), ad_rect.right_top()],
                    egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(230, 60, 60)),
                );
            }

            // 已选中的段加黄色边框
            if selected_ads.contains(&ai) {
                painter.rect_stroke(
                    ad_rect,
                    2.0,
                    egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(200, 200, 50)),
                );
            }

            // 正在播放的段加白色边框
            if is_this_file_playing && playing_ad_idx == Some(ai) {
                painter.rect_stroke(
                    ad_rect,
                    2.0,
                    egui::Stroke::new(2.0_f32, egui::Color32::WHITE),
                );
            }

            // 在段内显示时间与匹配度
            let seg_width = x1 - x0;
            if seg_width > 50.0 {
                let label = if ad.score > 0.0 {
                    format!(
                        "{:.0}-{:.0}s {:.0}%",
                        ad.begin as f64 / 10.0,
                        ad.end as f64 / 10.0,
                        ad.score * 100.0
                    )
                } else {
                    format!("{:.0}-{:.0}s", ad.begin as f64 / 10.0, ad.end as f64 / 10.0)
                };
                painter.text(
                    egui::pos2((x0 + x1) / 2.0, bar_rect.center().y),
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(10.0),
                    egui::Color32::WHITE,
                );
            }
        }

        if bar_resp.clicked() {
            handle_track_click(
                ctx,
                state,
                file_idx,
                &effective_ads,
                bar_rect,
                file_len,
                &bar_resp,
            );
        }

        // 事件分类轨道（叠加在音频条下方）
        if !event_ranges.is_empty() {
            draw_event_track(ui, file_len, event_ranges, label_colors);
        }
    }

    draw_time_ticks(ui, file_len as f64 / 10.0);

    // 仅对选中的文件展示频谱，避免同时渲染多张纹理
    let (is_selected, show_spectrogram) = {
        let s = state.lock().unwrap();
        (s.selected_file == Some(file_idx), s.show_spectrogram)
    };
    if is_selected && show_spectrogram {
        spectrogram::show(ui, state, file_idx, file);
    }

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);
}
fn draw_event_track(
    ui: &mut egui::Ui,
    file_len: f32,
    events: &[crate::core::classify::EventRange],
    label_colors: &HashMap<String, egui::Color32>,
) {
    let bar_width = ui.available_width();
    let (resp, painter) =
        ui.allocate_painter(egui::vec2(bar_width, EVENT_TRACK_HEIGHT), egui::Sense::hover());
    let event_rect = resp.rect;

    // 事件轨道背景（比主条浅）
    painter.rect_filled(event_rect, 2.0, egui::Color32::from_rgb(22, 26, 32));
    painter.rect_stroke(
        event_rect, 2.0,
        egui::Stroke::new(0.5_f32, egui::Color32::from_rgb(50, 55, 60)),
    );

    for ev in events {
        let x0 = event_rect.left() + (ev.begin as f32 / file_len) * event_rect.width();
        let x1 = event_rect.left() + (ev.end as f32 / file_len) * event_rect.width();
        let seg_rect = egui::Rect::from_x_y_ranges(
            x0.max(event_rect.left())..=x1.min(event_rect.right()),
            event_rect.top() + 2.0..=event_rect.bottom() - 2.0,
        );

        // 颜色来自全局标签映射，与图例保持一致
        let color = label_colors
            .get(&ev.label)
            .copied()
            .unwrap_or(egui::Color32::GRAY);

        // 填充（低 alpha）+ 边框（高 alpha）
        let fill = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 50);
        painter.rect_filled(seg_rect, 2.0, fill);
        painter.rect_stroke(
            seg_rect, 2.0,
            egui::Stroke::new(1.5_f32, color),
        );

        // 标签文字 + 置信度（段够宽才显示）
        let seg_width = x1 - x0;
        if seg_width > 60.0 {
            let label_text = format!(
                "{} {:.0}%",
                ev.label, ev.confidence * 100.0
            );
            painter.text(
                egui::pos2((x0 + x1) / 2.0, seg_rect.center().y),
                egui::Align2::CENTER_CENTER,
                label_text,
                egui::FontId::proportional(9.0),
                egui::Color32::WHITE,
            );
        } else if seg_width > 25.0 {
            // 窄段只显示置信度
            painter.text(
                egui::pos2((x0 + x1) / 2.0, seg_rect.center().y),
                egui::Align2::CENTER_CENTER,
                format!("{:.0}%", ev.confidence * 100.0),
                egui::FontId::proportional(8.0),
                egui::Color32::from_rgb(200, 200, 200),
            );
        }
    }
}
fn draw_time_ticks(ui: &mut egui::Ui, original_duration: f64) {
    let tick_count = if original_duration > 300.0 { 10 } else { 5 };
    let (response, painter) = ui.allocate_painter(
        egui::vec2(ui.available_width(), TICK_HEIGHT),
        egui::Sense::hover(),
    );
    let rect = response.rect;

    for i in 0..=tick_count {
        let x = rect.left() + rect.width() * i as f32 / tick_count as f32;
        let t = original_duration * i as f64 / tick_count as f64;

        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.top() + 3.0)],
            egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(100, 100, 100)),
        );
        painter.text(
            egui::pos2(x, rect.top() + 7.0),
            egui::Align2::CENTER_TOP,
            format!("{:.0}s", t),
            egui::FontId::proportional(9.0),
            egui::Color32::from_rgb(140, 140, 140),
        );
    }
}
