//! 训练样本管理与事件分类模型训练窗口
//!
//! 用户可以把检测出的广告段导出为独立音频文件、补充其他标签的样本，
//! 然后在界面内训练随机森林并直接加载生成的 `.mcrf` 模型。

use std::path::PathBuf;
use std::thread;

use egui::Context;

use crate::audio::ffmpeg;
use crate::training::{format_report, train_model, TrainConfig, TrainSample};

use super::super::state::SharedState;

/// 音频文件对话框过滤器
const AUDIO_EXTS: &[&str] = &["mp3", "wav", "flac", "m4a", "aac", "ogg", "opus", "wma"];

/// 显示训练窗口
pub fn show(ctx: &Context, state: &SharedState) {
    if !state.lock().unwrap().show_train {
        return;
    }

    let mut open = true;
    egui::Window::new("训练分类模型")
        .open(&mut open)
        .default_width(640.0)
        .resizable(true)
        .show(ctx, |ui| train_ui(ui, state));

    if !open {
        state.lock().unwrap().show_train = false;
    }
}

fn train_ui(ui: &mut egui::Ui, state: &SharedState) {
    let (mut label, samples, mut trees, mut depth, mut split, mut export_idx, report, model_path) = {
        let s = state.lock().unwrap();
        (
            s.train_label.clone(),
            s.train_samples.clone(),
            s.train_trees,
            s.train_max_depth,
            s.train_test_split,
            s.train_export_index,
            s.train_report.clone(),
            s.model_path.clone(),
        )
    };

    ui.label(
        egui::RichText::new(
            "用已检测的广告段导出片段作为正样本，再添加节目、音乐等负样本；\
             至少需要两种标签。训练按 2 秒窗口、1 秒步长扩展样本。",
        )
        .small()
        .color(egui::Color32::from_gray(150)),
    );
    ui.add_space(4.0);

    // 目标文件与可选广告段
    let ads: Vec<(i32, i32, f64)> = {
        let s = state.lock().unwrap();
        match s.selected_file.and_then(|i| s.files.get(i)) {
            Some(f) => f
                .effective_ads(s.thresholds)
                .map(|r| (r.begin, r.end, r.score))
                .collect(),
            None => Vec::new(),
        }
    };
    let target_name = {
        let s = state.lock().unwrap();
        s.selected_file
            .and_then(|i| s.files.get(i))
            .map(|f| f.name.clone())
    };

    egui::Grid::new("train_export_grid")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("标签");
            ui.text_edit_singleline(&mut label);
            ui.end_row();

            ui.label("目标文件");
            match &target_name {
                Some(name) => {
                    ui.label(name);
                }
                None => {
                    ui.label(
                        egui::RichText::new("先在左侧选择一个文件")
                            .color(egui::Color32::from_gray(150)),
                    );
                }
            }
            ui.end_row();

            ui.label("导出区间");
            if ads.is_empty() {
                ui.label(
                    egui::RichText::new("该文件没有满足阈值的广告段")
                        .color(egui::Color32::from_gray(150)),
                );
            } else {
                if export_idx >= ads.len() {
                    export_idx = 0;
                }
                egui::ComboBox::from_id_source("train_export_range")
                    .selected_text(format!(
                        "{:.1}–{:.1} 秒（匹配度 {:.2}）",
                        ads[export_idx].0 as f64 / 10.0,
                        ads[export_idx].1 as f64 / 10.0,
                        ads[export_idx].2
                    ))
                    .show_ui(ui, |ui| {
                        for (i, (b, e, score)) in ads.iter().enumerate() {
                            ui.selectable_value(
                                &mut export_idx,
                                i,
                                format!(
                                    "{:.1}–{:.1} 秒（匹配度 {:.2}）",
                                    *b as f64 / 10.0,
                                    *e as f64 / 10.0,
                                    score
                                ),
                            );
                        }
                    });
            }
            ui.end_row();
        });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui
            .button("导出选中广告段...")
            .on_hover_text("把选中的广告段另存为独立音频，并加入训练样本")
            .clicked()
        {
            if let Some((b, e, _)) = ads.get(export_idx).copied() {
                export_selected_ad(state, b, e, label.clone());
            }
        }

        if ui
            .button("添加文件样本...")
            .on_hover_text("选择音频文件，整段作为一条训练样本")
            .clicked()
        {
            add_file_samples(state, label.clone());
        }

        if ui.button("清空样本").clicked() {
            let mut s = state.lock().unwrap();
            s.train_samples.clear();
            s.train_report = None;
            s.status_message = "已清空训练样本".to_string();
        }
    });

    ui.add_space(6.0);
    ui.separator();
    ui.label(format!("训练样本（{}）", samples.len()));

    let mut remove: Option<usize> = None;
    egui::ScrollArea::vertical()
        .max_height(200.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if samples.is_empty() {
                ui.label(
                    egui::RichText::new("还没有样本。先用上面的按钮添加。")
                        .color(egui::Color32::from_gray(150)),
                );
            }
            for (i, sample) in samples.iter().enumerate() {
                ui.horizontal(|ui| {
                    let name = sample
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| sample.path.display().to_string());
                    ui.label(format!("[{}] {}", sample.label, name));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("移除").clicked() {
                            remove = Some(i);
                        }
                    });
                });
            }
        });

    if let Some(i) = remove {
        state.lock().unwrap().train_samples.remove(i);
    }

    ui.separator();
    egui::Grid::new("train_param_grid")
        .num_columns(4)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("树数量");
            ui.add(egui::DragValue::new(&mut trees).range(10..=500));
            ui.label("最大深度");
            ui.add(egui::DragValue::new(&mut depth).range(4..=64));
            ui.end_row();

            ui.label("测试集比例");
            ui.add(
                egui::DragValue::new(&mut split)
                    .range(0.0..=0.5)
                    .speed(0.01)
                    .fixed_decimals(2),
            );
            ui.label("");
            ui.label("");
            ui.end_row();
        });

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let can_train = samples.len() >= 2 && !state.lock().unwrap().is_processing;
        if ui
            .add_enabled(can_train, egui::Button::new("训练并保存模型..."))
            .on_hover_text("训练完成后自动加载模型，可直接用于事件分类")
            .clicked()
        {
            start_training(state, samples.clone(), trees, depth, split);
        }

        if let Some(path) = &model_path {
            ui.label(
                egui::RichText::new(format!("当前模型: {}", path.display()))
                    .small()
                    .color(egui::Color32::from_gray(150)),
            );
        }
    });

    if let Some(text) = report {
        ui.add_space(6.0);
        ui.group(|ui| {
            ui.label(egui::RichText::new("最近一次训练结果").strong());
            ui.label(egui::RichText::new(text).monospace().small());
        });
    }

    // 参数写回
    {
        let mut s = state.lock().unwrap();
        s.train_label = label;
        s.train_trees = trees;
        s.train_max_depth = depth;
        s.train_test_split = split;
        s.train_export_index = export_idx;
    }
}

/// 把选中的广告段导出为独立文件并加入样本
fn export_selected_ad(state: &SharedState, begin: i32, end: i32, label: String) {
    let target = {
        let s = state.lock().unwrap();
        s.selected_file
            .and_then(|i| s.files.get(i))
            .map(|f| (f.path.clone(), f.name.clone()))
    };
    let Some((path, name)) = target else {
        state.lock().unwrap().status_message = "请先选择一个已检测的文件".to_string();
        return;
    };

    {
        let mut s = state.lock().unwrap();
        s.status_message = "选择广告片段的保存位置...".to_string();
    }
    let state = state.clone();
    thread::spawn(move || {
        let stem = PathBuf::from(&name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "segment".to_string());
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_else(|| "mp3".to_string());
        let default_name = format!(
            "{}_{:.1}-{:.1}.{}",
            stem,
            begin as f64 / 10.0,
            end as f64 / 10.0,
            ext
        );

        let Some(mut out) = rfd::FileDialog::new()
            .set_title("导出广告片段")
            .add_filter("音频文件", AUDIO_EXTS)
            .set_file_name(&default_name)
            .save_file()
        else {
            state.lock().unwrap().status_message = "已取消导出".to_string();
            return;
        };
        if out.extension().is_none() {
            out.set_extension(&ext);
        }

        let from = begin as f64 / 10.0;
        let to = end as f64 / 10.0;
        match ffmpeg::split_audio(&path, &out, from, to) {
            Ok(()) => {
                let mut s = state.lock().unwrap();
                let is_dup = s
                    .train_samples
                    .iter()
                    .any(|x| x.path == out && x.label == label);
                if !is_dup {
                    s.train_samples.push(TrainSample::whole(out.clone(), label));
                }
                s.status_message = format!("已导出片段并加入样本: {}", out.display());
            }
            Err(e) => {
                state.lock().unwrap().status_message = format!("导出片段失败: {}", e);
            }
        }
    });
}

/// 通过文件对话框批量添加整段音频样本
fn add_file_samples(state: &SharedState, label: String) {
    let state = state.clone();
    thread::spawn(move || {
        let Some(paths) = rfd::FileDialog::new()
            .set_title("选择训练样本")
            .add_filter("音频文件", AUDIO_EXTS)
            .pick_files()
        else {
            return;
        };

        let mut s = state.lock().unwrap();
        let mut added = 0;
        for path in paths {
            let is_dup = s
                .train_samples
                .iter()
                .any(|x| x.path == path && x.label == label);
            if !is_dup {
                s.train_samples
                    .push(TrainSample::whole(path, label.clone()));
                added += 1;
            }
        }
        s.status_message = format!("已添加 {} 条训练样本", added);
    });
}

/// 选择保存位置并后台训练模型
fn start_training(
    state: &SharedState,
    samples: Vec<TrainSample>,
    trees: usize,
    depth: usize,
    split: f64,
) {
    {
        let mut s = state.lock().unwrap();
        if s.is_processing {
            return;
        }
        s.is_processing = true;
        s.progress = 0.0;
        s.status_message = "选择模型保存位置...".to_string();
    }

    let state = state.clone();
    thread::spawn(move || {
        let Some(mut path) = rfd::FileDialog::new()
            .set_title("保存分类模型")
            .add_filter("分类模型", &["mcrf"])
            .set_file_name("model.mcrf")
            .save_file()
        else {
            let mut s = state.lock().unwrap();
            s.is_processing = false;
            s.status_message = "已取消训练".to_string();
            return;
        };
        if path.extension().is_none() {
            path.set_extension("mcrf");
        }

        let config = TrainConfig {
            n_trees: trees,
            max_depth: depth,
            test_split: split,
            ..Default::default()
        };

        let progress_state = state.clone();
        let on_progress = move |progress: f32, message: &str| {
            if let Ok(mut s) = progress_state.lock() {
                s.progress = progress;
                s.status_message = message.to_string();
            }
        };

        match train_model(&samples, &config, &path, on_progress) {
            Ok(report) => {
                let mut s = state.lock().unwrap();
                s.progress = 1.0;
                s.is_processing = false;
                s.train_report = Some(format!(
                    "{}\n模型: {}",
                    format_report(&report),
                    path.display()
                ));
                s.model_path = Some(path.clone());
                s.status_message =
                    format!("训练完成: {}（{} 个窗口）", path.display(), report.samples);
            }
            Err(e) => {
                let mut s = state.lock().unwrap();
                s.is_processing = false;
                s.status_message = format!("训练失败: {}", e);
            }
        }
    });
}
