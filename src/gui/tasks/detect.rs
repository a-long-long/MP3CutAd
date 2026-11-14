//! 文件添加与广告检测任务

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::thread;

use crate::core::detect::{detect_ad_with_progress, DetectOptions};

use super::super::state::{FileInfo, SharedState};

pub(crate) fn add_files(state: &SharedState) {
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

pub(crate) fn add_directory(state: &SharedState) {
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

pub(crate) fn start_detection(state: &SharedState) {
    let state = state.clone();
    {
        let mut s = state.lock().unwrap();
        s.is_processing = true;
        s.status_message = "正在检测广告...".to_string();
        s.progress = 0.0;
    }

    thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| {
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

            detect_ad_with_progress(&files, opts, on_progress)
        }));

        match result {
            Ok(Ok(result)) => {
                let mut s = state.lock().unwrap();
                let mut failed = 0usize;
                for (i, (ranges, length)) in
                    result.ranges.into_iter().zip(result.lengths).enumerate()
                {
                    if let Some(file) = s.files.get_mut(i) {
                        if length == 0 {
                            // 预处理失败（FFT 为空）。GUI 没有控制台，stderr 警告不可见，
                            // 这里显式清空状态而不是伪装成"已检测"。
                            file.ad_ranges.clear();
                            file.event_ranges.clear();
                            file.length = 0;
                            file.detected = false;
                            file.needs_save = false;
                            failed += 1;
                        } else {
                            file.ad_ranges = ranges;
                            file.length = length;
                            file.detected = true;
                            file.needs_save = true;
                        }
                    }
                }
                s.is_processing = false;
                s.progress = 1.0;
                let mut message = match &result.gpu {
                    Some(gpu) => format!("检测完成！（GPU 加速: {}）", gpu),
                    None => "检测完成！".to_string(),
                };
                if failed > 0 {
                    message.push_str(&format!(
                        " {} 个文件预处理失败已跳过（请检查文件是否损坏或格式不支持）",
                        failed
                    ));
                }
                s.status_message = message;
            }
            Ok(Err(e)) => {
                let mut s = state.lock().unwrap();
                s.is_processing = false;
                s.status_message = format!("检测失败: {}", e);
            }
            Err(_) => {
                let mut s = state.lock().unwrap();
                s.is_processing = false;
                s.status_message = "检测线程发生 panic（已恢复）".to_string();
            }
        }
    });
}
pub(crate) fn select_all_ads(state: &SharedState) {
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
/// 把选中的广告段标记为删除（保存时才真正写入文件）
pub(crate) fn mark_selected_for_deletion(state: &SharedState) {
    let mut s = state.lock().unwrap();
    let th = s.thresholds;

    let selection = std::mem::take(&mut s.ads_to_delete);
    let added = super::super::state::mark_ads_for_deletion(&mut s.files, th, &selection);

    let marked: usize = s
        .files
        .iter()
        .filter(|f| f.detected)
        .map(|f| f.marked_for_deletion.len())
        .sum();

    let remaining: usize = s
        .files
        .iter()
        .filter(|f| f.detected)
        .map(|f| f.effective_ad_count(th).saturating_sub(f.marked_for_deletion.len()))
        .sum();

    s.status_message = format!(
        "已标记删除 {} 个广告段（累计 {} 个，剩余 {} 个未标记），点击保存将删除并拼接前后音频",
        added, marked, remaining
    );
}
