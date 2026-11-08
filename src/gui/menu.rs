//! 顶部工具栏与底部状态栏

use std::collections::HashMap;
use std::thread;

use egui::Context;

use crate::core::detect::detect_ad_with_progress;

use super::save;
use super::state::{FileInfo, SharedState};

/// 顶部菜单栏
pub fn menu_bar(ctx: &Context, state: &SharedState) {
    egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.heading("MP3CutAd-RS");
            ui.separator();

            if ui.button("添加文件").clicked() {
                add_files(state);
            }

            if ui.button("添加目录").clicked() {
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

            ui.separator();

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
                .button(format!("删除已选 ({})", selected_count))
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
                s.files
                    .iter()
                    .filter(|f| f.detected && f.has_effective_ads())
                    .count()
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

            if ui.button("清空列表").clicked() {
                let mut s = state.lock().unwrap();
                s.files.clear();
                s.selected_file = None;
                s.selected_ad = None;
                s.ads_to_delete.clear();
                s.status_message = "列表已清空".to_string();
            }
        });
    });
}

/// 底部状态栏
pub fn status_bar(ctx: &Context, state: &SharedState) {
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
                ui.label(format!("文件数: {}", s.files.len()));
                ui.separator();
                ui.label(format!("最小长度: {}秒", s.min_length));
                ui.label(format!("最小次数: {}", s.min_count));
            });
        });
    });
}

fn add_files(state: &SharedState) {
    let state = state.clone();
    thread::spawn(move || {
        let paths = rfd::FileDialog::new()
            .add_filter("MP3 文件", &["mp3"])
            .set_title("选择 MP3 文件")
            .pick_files();

        if let Some(paths) = paths {
            let mut s = state.lock().unwrap();
            for path in paths {
                if !s.files.iter().any(|f| f.path == path) {
                    s.files.push(FileInfo::from_path(path));
                }
            }
            s.status_message = format!("已添加 {} 个文件", s.files.len());
        }
    });
}

fn add_directory(state: &SharedState) {
    let state = state.clone();
    thread::spawn(move || {
        let Some(dir) = rfd::FileDialog::new().set_title("选择目录").pick_folder() else {
            return;
        };

        let mut s = state.lock().unwrap();
        let mut count = 0;

        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let file_path = entry.path();
                let is_mp3 = file_path
                    .extension()
                    .map(|e| e.eq_ignore_ascii_case("mp3"))
                    .unwrap_or(false);

                if is_mp3 && !s.files.iter().any(|f| f.path == file_path) {
                    s.files.push(FileInfo::from_path(file_path));
                    count += 1;
                }
            }
        }

        s.status_message = format!("从目录添加了 {} 个文件", count);
    });
}

fn start_detection(state: &SharedState) {
    let state = state.clone();
    {
        let mut s = state.lock().unwrap();
        s.is_processing = true;
        s.status_message = "正在检测广告...".to_string();
        s.progress = 0.0;
    }

    thread::spawn(move || {
        let files: Vec<std::path::PathBuf> = {
            let s = state.lock().unwrap();
            s.files.iter().map(|f| f.path.clone()).collect()
        };

        // 进度回调：实时更新进度条与状态文本（可能来自并行线程）
        let progress_state = state.clone();
        let on_progress = move |progress: f32, message: &str| {
            if let Ok(mut s) = progress_state.lock() {
                s.progress = progress;
                s.status_message = message.to_string();
            }
        };

        match detect_ad_with_progress(&files, on_progress) {
            Ok(result) => {
                let mut s = state.lock().unwrap();
                for (i, (ranges, length)) in
                    result.ranges.into_iter().zip(result.lengths).enumerate()
                {
                    if let Some(file) = s.files.get_mut(i) {
                        file.ad_ranges = ranges;
                        file.length = length;
                        file.detected = true;
                        file.needs_save = true;
                    }
                }
                s.is_processing = false;
                s.progress = 1.0;
                s.status_message = "检测完成！".to_string();
            }
            Err(e) => {
                let mut s = state.lock().unwrap();
                s.is_processing = false;
                s.status_message = format!("检测失败: {}", e);
            }
        }
    });
}

fn select_all_ads(state: &SharedState) {
    let mut s = state.lock().unwrap();
    let mut selected = Vec::new();

    for (file_idx, file) in s.files.iter().enumerate() {
        if file.detected {
            for ad_idx in 0..file.effective_ad_count() {
                selected.push((file_idx, ad_idx));
            }
        }
    }

    s.ads_to_delete = selected;
    s.status_message = format!("已全选 {} 个广告段", s.ads_to_delete.len());
}

fn mark_selected_for_deletion(state: &SharedState) {
    let mut s = state.lock().unwrap();

    let mut by_file: HashMap<usize, Vec<usize>> = HashMap::new();
    for &(file_idx, ad_idx) in &s.ads_to_delete {
        by_file.entry(file_idx).or_default().push(ad_idx);
    }

    for (file_idx, ad_indices) in by_file {
        if let Some(file) = s.files.get_mut(file_idx) {
            for idx in ad_indices {
                if !file.marked_for_deletion.contains(&idx) {
                    file.marked_for_deletion.push(idx);
                }
            }
            file.needs_save = true;
        }
    }

    s.ads_to_delete.clear();

    let remaining: usize = s
        .files
        .iter()
        .filter(|f| f.detected)
        .map(|f| {
            f.effective_ad_count()
                .saturating_sub(f.marked_for_deletion.len())
        })
        .sum();

    s.status_message = format!(
        "已标记删除，剩余 {} 个广告段（点击保存写入文件）",
        remaining
    );
}
