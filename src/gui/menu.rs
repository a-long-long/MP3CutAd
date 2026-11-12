//! 顶部工具栏与底部状态栏

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use egui::Context;

use crate::core::classify::{ClassifyOptions, EventRange};
use crate::core::detect::{detect_ad_with_progress, DetectOptions};
use crate::core::model::load_model;

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
                .on_hover_text("从检测结果中移除选中的广告段，保存时将保留这些片段")
                .clicked()
                && selected_count > 0
            {
                delete_selected_ads(state);
            }

            {
                let mut s = state.lock().unwrap();
                ui.checkbox(&mut s.overwrite_original, "覆盖原文件");
            }

            let files_to_save = {
                let s = state.lock().unwrap();
                let th = s.thresholds;
                s.files
                    .iter()
                    .filter(|f| f.detected && f.has_effective_ads(th))
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

            let settings_open = state.lock().unwrap().show_settings;
            if ui.selectable_label(settings_open, "阈值设置").clicked() {
                let mut s = state.lock().unwrap();
                s.show_settings = !settings_open;
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

fn add_files(state: &SharedState) {
    let state = state.clone();
    thread::spawn(move || {
        let paths = rfd::FileDialog::new()
            .add_filter(
                "音频文件",
                &["mp3", "wav", "flac", "m4a", "aac", "ogg", "opus", "wma"],
            )
            .set_title("选择音频文件")
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
                let is_audio = crate::pipeline::is_supported_audio(&file_path);

                if is_audio && !s.files.iter().any(|f| f.path == file_path) {
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
        let (files, enable_hpss): (Vec<std::path::PathBuf>, bool) = {
            let s = state.lock().unwrap();
            (
                s.files.iter().map(|f| f.path.clone()).collect(),
                s.enable_hpss,
            )
        };

        let opts = if enable_hpss {
            DetectOptions::default().with_hpss()
        } else {
            DetectOptions::default()
        };

        // 进度回调：实时更新进度条与状态文本（可能来自并行线程）
        let progress_state = state.clone();
        let on_progress = move |progress: f32, message: &str| {
            if let Ok(mut s) = progress_state.lock() {
                s.progress = progress;
                s.status_message = message.to_string();
            }
        };

        match detect_ad_with_progress(&files, opts, on_progress) {
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
    let th = s.thresholds;

    for (file_idx, file) in s.files.iter().enumerate() {
        if file.detected {
            for ad_idx in 0..file.effective_ad_count(th) {
                selected.push((file_idx, ad_idx));
            }
        }
    }

    s.ads_to_delete = selected;
    s.status_message = format!("已全选 {} 个广告段", s.ads_to_delete.len());
}

/// 从检测结果中删除选中的广告段（保存时不再裁剪这些片段）
fn delete_selected_ads(state: &SharedState) {
    let mut s = state.lock().unwrap();
    let th = s.thresholds;
    let selected = s.ads_to_delete.clone();
    if selected.is_empty() {
        return;
    }

    // 选中项记录的是“有效广告段”的序号，需要映射回 ad_ranges 的原始下标
    let mut by_file: HashMap<usize, Vec<usize>> = HashMap::new();
    for (file_idx, ad_idx) in selected {
        by_file.entry(file_idx).or_default().push(ad_idx);
    }

    let mut removed = 0usize;
    for (file_idx, ad_indices) in by_file {
        let Some(file) = s.files.get_mut(file_idx) else {
            continue;
        };

        let mut effective_index = 0usize;
        let mut original_indices = Vec::new();
        for (orig, r) in file.ad_ranges.iter().enumerate() {
            if !th.accepts(r) {
                continue;
            }
            if ad_indices.contains(&effective_index) {
                original_indices.push(orig);
            }
            effective_index += 1;
        }

        // 从后往前删，避免下标偏移
        for idx in original_indices.iter().rev() {
            file.ad_ranges.remove(*idx);
            removed += 1;
        }
        file.needs_save = true;
    }

    // 删除后有效段序号全部变化，清空选择并停止播放
    s.ads_to_delete.clear();
    *s.playback.stop_flag.lock().unwrap() = true;
    s.playback.playing_file = None;
    s.playback.playing_ad = None;

    let remaining: usize = s
        .files
        .iter()
        .filter(|f| f.detected)
        .map(|f| f.effective_ad_count(th))
        .sum();

    s.status_message = format!(
        "已从结果中删除 {} 个广告段，剩余 {} 个（保存时保留被删除的片段）",
        removed, remaining
    );
}

/// 选择事件分类模型文件
fn pick_model(state: &SharedState) {
    let state = state.clone();
    thread::spawn(move || {
        let path: Option<PathBuf> = rfd::FileDialog::new()
            .add_filter("分类模型", &["mcrf"])
            .set_title("选择事件分类模型 (.mcrf)")
            .pick_file();

        if let Some(p) = path {
            // 先验证能加载
            match load_model(&p) {
                Ok(_) => {
                    let mut s = state.lock().unwrap();
                    s.model_path = Some(p.clone());
                    s.status_message = format!("已加载模型: {}", p.display());
                }
                Err(e) => {
                    let mut s = state.lock().unwrap();
                    s.status_message = format!("模型加载失败: {}", e);
                }
            }
        }
    });
}

/// 对列表中所有文件运行事件分类
fn start_classification(state: &SharedState) {
    // 先拿到必要参数，避免锁跨线程
    let (model_path, files): (Option<PathBuf>, Vec<PathBuf>) = {
        let s = state.lock().unwrap();
        (
            s.model_path.clone(),
            s.files.iter().map(|f| f.path.clone()).collect(),
        )
    };
    let Some(mp) = model_path else { return };

    // 先同步加载模型（小文件，几毫秒）
    let model = match load_model(&mp) {
        Ok(m) => m,
        Err(e) => {
            state.lock().unwrap().status_message = format!("模型加载失败: {}", e);
            return;
        }
    };

    let opts = ClassifyOptions {
        confidence_threshold: state.lock().unwrap().classify_confidence,
        ..Default::default()
    };

    // 初始化状态
    {
        let mut s = state.lock().unwrap();
        s.is_processing = true;
        s.progress = 0.0;
        s.status_message = "正在加载模型...".to_string();
        // 先标记所有文件为未分类
        for f in s.files.iter_mut() {
            f.classified = false;
            f.event_ranges.clear();
        }
    }

    let state = state.clone();
    let n_files = files.len();
    let model = Arc::new(model);
    thread::spawn(move || {
        let mut all_events: Vec<(Vec<EventRange>, usize)> = Vec::with_capacity(n_files);

        for (i, path) in files.iter().enumerate() {
            let progress_state = state.clone();
            let on_progress = move |p: f32, msg: &str| {
                let total_p = (i as f32 + p / n_files as f32) / n_files.max(1) as f32;
                if let Ok(mut s) = progress_state.lock() {
                    s.progress = total_p;
                    s.status_message = format!("[{}/{}] {}", i + 1, n_files, msg);
                }
            };

            let events = match crate::core::classify::classify_file_with_progress(
                path,
                &model,
                &opts,
                on_progress,
            ) {
                Ok((ev, _nf)) => ev,
                Err(e) => {
                    if let Ok(mut s) = state.lock() {
                        s.status_message = format!(
                            "分类失败 {}: {}",
                            path.file_name()
                                .map(|n| n.to_string_lossy())
                                .unwrap_or_default(),
                            e
                        );
                    }
                    Vec::new()
                }
            };
            all_events.push((events, i));
        }

        // 写回结果
        let mut s = state.lock().unwrap();
        let mut total_events = 0;
        for (events, idx) in all_events {
            if let Some(f) = s.files.get_mut(idx) {
                f.event_ranges = events.clone();
                f.classified = true;
                total_events += events.len();
            }
        }
        s.is_processing = false;
        s.progress = 1.0;
        s.status_message = format!("分类完成！共检测到 {} 个事件", total_events);
    });
}
