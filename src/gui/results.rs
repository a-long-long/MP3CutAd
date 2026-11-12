//! 中央结果视图：音频条、广告段、事件分类与时间刻度

use std::collections::HashMap;

use egui::{self, Context};

use crate::core::detect::reverse_range;

use super::playback;
use super::spectrogram;
use super::state::{FileInfo, SharedState};

/// 音频条高度
const TRACK_HEIGHT: f32 = 28.0;
/// 事件分类轨道高度
const EVENT_TRACK_HEIGHT: f32 = 20.0;
/// 时间刻度高度
const TICK_HEIGHT: f32 = 20.0;

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
            let all_labels: Vec<String> = {
                let mut labels: Vec<String> = all_files
                    .iter()
                    .flat_map(|(_, f)| f.event_ranges.iter().map(|e| e.label.clone()))
                    .collect();
                labels.sort(); labels.dedup(); labels
            };
            if !all_labels.is_empty() {
                ui.separator();
                event_legend(ui, &all_labels);
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
                draw_file_track(ui, ctx, state, *file_idx, file, &colors, thresholds);
            }
        });
    });
}

/// 绘制单个文件的摘要、音频条、事件轨道与时间刻度
fn draw_file_track(
    ui: &mut egui::Ui,
    ctx: &Context,
    state: &SharedState,
    file_idx: usize,
    file: &FileInfo,
    colors: &[egui::Color32],
    thresholds: crate::gui::state::Thresholds,
) {
    let file_len = file.length as f32;
    let effective_ads: Vec<&crate::core::range::Range> =
        file.effective_ads(thresholds).collect();
    let event_ranges = &file.event_ranges;

    // 剩余时长按实际会被裁剪的段（已标记删除）计算，与保存结果保持一致
    let marked_owned: Vec<crate::core::range::Range> = file
        .marked_for_deletion
        .iter()
        .filter_map(|&idx| effective_ads.get(idx).copied().copied())
        .collect();
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
            draw_event_track(ui, file_len, event_ranges, bar_rect);
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

/// 在主音频条下方绘制事件分类轨道（边框样式 + 标签 + 置信度）
fn draw_event_track(
    ui: &mut egui::Ui,
    file_len: f32,
    events: &[crate::core::classify::EventRange],
    above_bar_rect: egui::Rect,
) {
    let bar_width = ui.available_width();
    let (resp, painter) =
        ui.allocate_painter(egui::vec2(bar_width, EVENT_TRACK_HEIGHT), egui::Sense::hover());
    let event_rect = resp.rect;

    // 事件轨道背景（比主条浅）
    painter.rect_filled(event_rect, 2.0, egui::Color32::from_rgb(22, 26, 32));
    painter.rect_stroke(
        event_rect, 2.0,
        egui::Stroke::new(0.5, egui::Color32::from_rgb(50, 55, 60)),
    );

    // 为每个 label 分配一个颜色
    let mut label_color_map: HashMap<String, egui::Color32> = HashMap::new();
    let event_palette = event_palette();
    let mut color_idx = 0usize;

    for ev in events {
        let x0 = event_rect.left() + (ev.begin as f32 / file_len) * event_rect.width();
        let x1 = event_rect.left() + (ev.end as f32 / file_len) * event_rect.width();
        let seg_rect = egui::Rect::from_x_y_ranges(
            x0.max(event_rect.left())..=x1.min(event_rect.right()),
            event_rect.top() + 2.0..=event_rect.bottom() - 2.0,
        );

        let color = *label_color_map.entry(ev.label.clone()).or_insert_with(|| {
            let c = event_palette[color_idx % event_palette.len()];
            color_idx += 1;
            c
        });

        // 填充（低 alpha）+ 边框（高 alpha）
        let fill = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 50);
        painter.rect_filled(seg_rect, 2.0, fill);
        painter.rect_stroke(
            seg_rect, 2.0,
            egui::Stroke::new(1.5, color),
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

/// 处理音频条点击：选中广告段（Ctrl）或切换播放
fn handle_track_click(
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

/// 切换某个广告段的选择状态
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

/// 时间刻度（与音频条一致，标注原始时长；广告段按原始时间位置绘制）
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

/// 广告段配色方案（匹配度未知时使用，例如旧版结果文件）
fn ad_colors() -> Vec<egui::Color32> {
    vec![
        egui::Color32::from_rgb(200, 80, 80),
        egui::Color32::from_rgb(80, 160, 200),
        egui::Color32::from_rgb(200, 160, 60),
        egui::Color32::from_rgb(120, 200, 120),
        egui::Color32::from_rgb(180, 100, 200),
        egui::Color32::from_rgb(200, 120, 60),
    ]
}

/// 有效广告段的平均匹配度；没有可用匹配度时返回 None
fn average_score(file: &FileInfo, thresholds: crate::gui::state::Thresholds) -> Option<f64> {
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

/// 按匹配度着色：高=红，中=橙，低=黄；未知（0）沿用原配色
fn confidence_color(
    score: f64,
    fallback: egui::Color32,
    thresholds: crate::gui::state::Thresholds,
) -> egui::Color32 {
    if score <= 0.0 {
        fallback
    } else if score >= thresholds.score_high {
        egui::Color32::from_rgb(200, 80, 80)
    } else if score >= thresholds.score_low {
        egui::Color32::from_rgb(226, 146, 60)
    } else {
        egui::Color32::from_rgb(198, 188, 66)
    }
}

/// 匹配度配色图例（随阈值设置变化）
fn confidence_legend(ui: &mut egui::Ui, thresholds: crate::gui::state::Thresholds) {
    let high = thresholds.score_high;
    let low = thresholds.score_low;
    let items = [
        (
            format!("≥{:.0}%", high * 100.0),
            confidence_color(high, egui::Color32::WHITE, thresholds),
        ),
        (
            format!(
                "{:.0}-{:.0}%",
                low * 100.0,
                high * 100.0
            ),
            confidence_color((low + high) / 2.0, egui::Color32::WHITE, thresholds),
        ),
        (
            format!("<{:.0}%", low * 100.0),
            confidence_color(low * 0.5, egui::Color32::WHITE, thresholds),
        ),
    ];
    for (text, color) in items {
        ui.label(
            egui::RichText::new(format!("● {text}"))
                .small()
                .color(color),
        );
    }
    ui.label(
        egui::RichText::new("匹配度")
            .small()
            .color(egui::Color32::from_gray(150)),
    );
}

/// 事件分类调色板（比广告段更冷色调，区分开）
fn event_palette() -> Vec<egui::Color32> {
    vec![
        egui::Color32::from_rgb(100, 200, 255),  // 浅蓝
        egui::Color32::from_rgb(150, 255, 150),  // 浅绿
        egui::Color32::from_rgb(255, 180, 100),  // 橙黄
        egui::Color32::from_rgb(200, 150, 255),  // 淡紫
        egui::Color32::from_rgb(100, 255, 220),  // 青
        egui::Color32::from_rgb(255, 140, 180),  // 粉红
        egui::Color32::from_rgb(255, 230, 100),  // 亮黄
        egui::Color32::from_rgb(140, 200, 140),  // 苔绿
    ]
}

/// 事件标签图例（所有出现过的 label + 对应颜色）
fn event_legend(ui: &mut egui::Ui, labels: &[String]) {
    let palette = event_palette();
    ui.label(
        egui::RichText::new("🎯 事件")
            .small()
            .color(egui::Color32::from_gray(180)),
    );
    for (i, label) in labels.iter().enumerate() {
        let color = palette[i % palette.len()];
        ui.label(
            egui::RichText::new(format!("▢ {}", label))
                .small()
                .color(color),
        );
    }
}
