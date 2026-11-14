//! 事件分类任务

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use crate::core::classify::model::load_model;
use crate::core::classify::{ClassifyOptions, EventRange};

use super::super::state::SharedState;

/// 选择事件分类模型文件
pub(crate) fn pick_model(state: &SharedState) {
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
pub(crate) fn start_classification(state: &SharedState) {
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
        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut all_events: Vec<(Vec<EventRange>, usize)> = Vec::with_capacity(n_files);

            for (i, path) in files.iter().enumerate() {
                let progress_state = state.clone();
                let on_progress = move |p: f32, msg: &str| {
                    let total_p = (i as f32 + p) / n_files.max(1) as f32;
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
            all_events
        }));

        match result {
            Ok(all_events) => {
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
            }
            Err(_) => {
                let mut s = state.lock().unwrap();
                s.is_processing = false;
                s.status_message = "分类线程发生 panic（已恢复）".to_string();
            }
        }
    });
}
