//! 训练样本导出与模型训练后台任务

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::thread;

use crate::audio::ffmpeg;
use crate::training::{format_report, train_model, TrainConfig, TrainSample};

use super::super::state::SharedState;

/// 音频文件对话框过滤器
const AUDIO_EXTS: &[&str] = &["mp3", "wav", "flac", "m4a", "aac", "ogg", "opus", "wma"];

/// 把选中的广告段导出为独立文件并加入样本
pub(crate) fn export_selected_ad(state: &SharedState, begin: i32, end: i32, label: String) {
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
pub(crate) fn add_file_samples(state: &SharedState, label: String) {
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
pub(crate) fn start_training(
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
        // drop 守卫：无论正常返回、取消还是 panic，都复位处理中状态，
        // 避免线程 panic 后所有按钮永久禁用。
        struct ProcessingGuard<'a> {
            state: &'a SharedState,
        }
        impl Drop for ProcessingGuard<'_> {
            fn drop(&mut self) {
                if let Ok(mut s) = self.state.lock() {
                    s.is_processing = false;
                }
            }
        }
        let _guard = ProcessingGuard { state: &state };

        let result = catch_unwind(AssertUnwindSafe(|| {
            run_training(&state, &samples, trees, depth, split);
        }));
        if result.is_err() {
            if let Ok(mut s) = state.lock() {
                s.progress = 0.0;
                s.status_message = "训练线程发生 panic（已恢复）".to_string();
            }
        }
    });
}

fn run_training(
    state: &SharedState,
    samples: &[TrainSample],
    trees: usize,
    depth: usize,
    split: f64,
) {
    let Some(mut path) = rfd::FileDialog::new()
        .set_title("保存分类模型")
        .add_filter("分类模型", &["mcrf"])
        .set_file_name("model.mcrf")
        .save_file()
    else {
        state.lock().unwrap().status_message = "已取消训练".to_string();
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

    match train_model(samples, &config, &path, on_progress) {
        Ok(report) => {
            let mut s = state.lock().unwrap();
            s.progress = 1.0;
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
            s.status_message = format!("训练失败: {}", e);
        }
    }
}
