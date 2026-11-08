//! 中央结果视图：音频条、广告段与时间刻度

use egui::{self, Context};

use crate::core::detect::reverse_range;

use super::playback;
use super::state::{FileInfo, SharedState};

/// 音频条高度
const TRACK_HEIGHT: f32 = 28.0;
/// 时间刻度高度
const TICK_HEIGHT: f32 = 20.0;

/// 检测结果面板
pub fn show(ctx: &Context, state: &SharedState) {
    egui::CentralPanel::default().show(ctx, |ui| {
        let detected: Vec<(usize, FileInfo)> = {
            let s = state.lock().unwrap();
            s.files
                .iter()
                .enumerate()
                .filter(|(_, f)| f.detected)
                .map(|(i, f)| (i, f.clone()))
                .collect()
        };

        if detected.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.heading("检测完成后在此显示结果");
            });
            return;
        }

        let ads_total: usize = detected.iter().map(|(_, f)| f.effective_ad_count()).sum();
        ui.heading(format!(
            "检测结果 ({} 个文件, {} 个广告段)",
            detected.len(),
            ads_total
        ));
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            let colors = ad_colors();
            for (file_idx, file) in &detected {
                draw_file_track(ui, ctx, state, *file_idx, file, &colors);
            }
        });
    });
}

/// 绘制单个文件的摘要、音频条与时间刻度
fn draw_file_track(
    ui: &mut egui::Ui,
    ctx: &Context,
    state: &SharedState,
    file_idx: usize,
    file: &FileInfo,
    colors: &[egui::Color32],
) {
    let file_len = file.length as f32;
    let remaining_segments = reverse_range(&file.ad_ranges, file.length);
    let remaining_duration: f64 = remaining_segments
        .iter()
        .map(|r| (r.end - r.begin) as f64 / 10.0)
        .sum();
    let effective_ads: Vec<&crate::core::range::Range> = file.effective_ads().collect();

    ui.horizontal(|ui| {
        ui.colored_label(egui::Color32::from_rgb(100, 200, 100), "✓");
        ui.strong(&file.name);
        ui.label(format!(
            "  原始: {:.1}s | 剩余: {:.1}s | 删除 {} 个广告段",
            file.length as f64 / 10.0,
            remaining_duration,
            effective_ads.len()
        ));
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
            let color = colors[(ai + file_idx) % colors.len()];

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

            // 在段内显示时间
            let seg_width = x1 - x0;
            if seg_width > 40.0 {
                let label = format!("{:.0}-{:.0}s", ad.begin as f64 / 10.0, ad.end as f64 / 10.0);
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
    }

    draw_time_ticks(ui, file_len as f64 / 10.0);

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);
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

    match clicked_ad {
        Some(ai) if ctx.input(|i| i.modifiers.ctrl) => toggle_selection(state, file_idx, ai),
        Some(ai) => playback::toggle_ad_playback(state, file_idx, ai),
        None => {
            state.lock().unwrap().selected_file = Some(file_idx);
        }
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

/// 广告段配色方案
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
