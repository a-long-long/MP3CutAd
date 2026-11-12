//! 按匹配度选择广告段
//!
//! 用户可以按高/中/低置信度或自定义下限筛选广告段，批量加入选择，
//! 随后在工具栏中删除选中的段（从检测结果中移除，保存时不再裁剪）。

use egui::Context;

use super::state::{AppState, MatchMode, SharedState};

/// 显示按匹配度选择窗口
pub fn show(ctx: &Context, state: &SharedState) {
    if !state.lock().unwrap().show_match_select {
        return;
    }

    let mut open = true;
    egui::Window::new("按匹配度选择")
        .open(&mut open)
        .default_width(380.0)
        .resizable(false)
        .show(ctx, |ui| select_ui(ui, state));

    if !open {
        state.lock().unwrap().show_match_select = false;
    }
}

fn select_ui(ui: &mut egui::Ui, state: &SharedState) {
    let (mut mode, mut min_score, mut all_files, th, has_selected_file) = {
        let s = state.lock().unwrap();
        (
            s.match_mode,
            s.match_min_score,
            s.match_all_files,
            s.thresholds,
            s.selected_file.is_some(),
        )
    };

    ui.label(
        egui::RichText::new(
            "按匹配度筛选广告段。选中的段可用工具栏「删除所选」标记，\
             点击保存后才会从音频中删除并拼接前后内容；未知匹配度（旧版结果）不参与筛选。",
        )
        .small()
        .color(egui::Color32::from_gray(150)),
    );
    ui.add_space(6.0);

    ui.label("匹配度范围");
    ui.radio_value(
        &mut mode,
        MatchMode::High,
        format!("高置信度（≥ {:.0}%）", th.score_high * 100.0),
    );
    ui.radio_value(
        &mut mode,
        MatchMode::Medium,
        format!(
            "中等置信度（{:.0}% – {:.0}%）",
            th.score_low * 100.0,
            th.score_high * 100.0
        ),
    );
    ui.radio_value(
        &mut mode,
        MatchMode::Low,
        format!("低置信度（< {:.0}%）", th.score_low * 100.0),
    );
    ui.radio_value(&mut mode, MatchMode::Custom, "自定义下限");
    if mode == MatchMode::Custom {
        ui.horizontal(|ui| {
            ui.add(
                egui::Slider::new(&mut min_score, 0.0..=1.0)
                    .fixed_decimals(2)
                    .text("匹配度"),
            );
        });
    }

    ui.add_space(6.0);
    ui.label("作用范围");
    ui.radio_value(&mut all_files, true, "全部文件");
    ui.add_enabled_ui(has_selected_file, |ui| {
        ui.radio_value(&mut all_files, false, "仅当前选中文件");
    });

    let matched = {
        let s = state.lock().unwrap();
        matching_pairs(&s, mode, min_score, all_files).len()
    };

    ui.add_space(6.0);
    ui.label(format!("匹配到 {} 个广告段", matched));

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(matched > 0, egui::Button::new("选择匹配段"))
            .clicked()
        {
            select_matching(state, mode, min_score, all_files);
        }

        if ui
            .add_enabled(matched > 0, egui::Button::new("取消选择匹配段"))
            .clicked()
        {
            deselect_matching(state, mode, min_score, all_files);
        }
    });

    // 写回筛选条件
    {
        let mut s = state.lock().unwrap();
        s.match_mode = mode;
        s.match_min_score = min_score;
        s.match_all_files = all_files;
    }
}

/// 当前筛选条件下的 (文件索引, 有效广告段索引)
fn matching_pairs(
    s: &AppState,
    mode: MatchMode,
    min_score: f64,
    all_files: bool,
) -> Vec<(usize, usize)> {
    let th = s.thresholds;
    let mut pairs = Vec::new();
    for (file_idx, file) in s.files.iter().enumerate() {
        if !all_files && s.selected_file != Some(file_idx) {
            continue;
        }
        for (ad_idx, r) in file.effective_ads(th).enumerate() {
            if mode.matches(r.score, min_score, th) {
                pairs.push((file_idx, ad_idx));
            }
        }
    }
    pairs
}

fn select_matching(state: &SharedState, mode: MatchMode, min_score: f64, all_files: bool) {
    let mut s = state.lock().unwrap();
    let pairs = matching_pairs(&s, mode, min_score, all_files);
    let mut added = 0;
    for pair in pairs {
        if !s.ads_to_delete.contains(&pair) {
            s.ads_to_delete.push(pair);
            added += 1;
        }
    }
    let total = s.ads_to_delete.len();
    s.status_message = format!("已选择 {} 个匹配段（累计 {} 个）", added, total);
}

fn deselect_matching(state: &SharedState, mode: MatchMode, min_score: f64, all_files: bool) {
    let mut s = state.lock().unwrap();
    let pairs = matching_pairs(&s, mode, min_score, all_files);
    let before = s.ads_to_delete.len();
    s.ads_to_delete.retain(|p| !pairs.contains(p));
    let removed = before - s.ads_to_delete.len();
    s.status_message = format!("已取消选择 {} 个匹配段", removed);
}
