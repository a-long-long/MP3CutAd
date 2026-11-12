//! 保存（按广告区间裁剪写盘）流程

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::thread;

use crate::core::range::Range;
use crate::pipeline;

use super::state::{AppState, FileInfo, SharedState};

/// 启动保存：在后台线程中裁剪所有含有效广告段的文件
pub fn start_save(state: &SharedState) {
    let state = state.clone();
    thread::spawn(move || {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            save_all(&state);
        }));
    });
}

fn save_all(state: &SharedState) {
    let (overwrite, files, thresholds) = {
        let s = state.lock().unwrap();
        (s.overwrite_original, s.files.clone(), s.thresholds)
    };

    let output_dir = if overwrite {
        None
    } else {
        rfd::FileDialog::new()
            .set_title("选择输出目录")
            .pick_folder()
    };

    if !overwrite && output_dir.is_none() {
        return;
    }

    // 只处理用户通过「删除已选」标记过的文件（未标记的文件不动）
    let targets: Vec<(usize, FileInfo)> = files
        .into_iter()
        .enumerate()
        .filter(|(_, f)| f.detected && !f.marked_for_deletion.is_empty())
        .collect();

    if targets.is_empty() {
        set_status(state, "没有需要保存的文件".to_string());
        return;
    }

    let total = targets.len();
    {
        let mut s = state.lock().unwrap();
        s.is_processing = true;
        s.status_message = format!("正在保存... 共 {} 个文件", total);
    }

    // drop guard：无论 save_all 正常返回还是 panic，都保证 is_processing 复位
    struct ProcessGuard<'a> {
        state: &'a SharedState,
    }
    impl<'a> Drop for ProcessGuard<'a> {
        fn drop(&mut self) {
            if let Ok(mut s) = self.state.lock() {
                s.is_processing = false;
            }
        }
    }
    let _guard = ProcessGuard { state };

    let mut done = 0;
    let mut saved = 0;
    let mut failures: Vec<String> = Vec::new();

    for (file_idx, file) in targets {
        {
            let mut s = state.lock().unwrap();
            s.status_message = format!("[{}/{}] 处理: {}", done + 1, total, file.name);
        }

        // 只删除被标记的段（索引基于当前有效广告段）
        let effective: Vec<Range> = file.effective_ads(thresholds).copied().collect();
        let ad_ranges: Vec<Range> = file
            .marked_for_deletion
            .iter()
            .filter_map(|&idx| effective.get(idx).copied())
            .collect();
        if ad_ranges.is_empty() {
            // 标记的段在当前阈值下已失效，跳过
            done += 1;
            continue;
        }

        let result = if overwrite {
            pipeline::overwrite_file(&file.path, &ad_ranges, file.length)
        } else {
            let output_path = output_dir.as_ref().unwrap().join(&file.name);
            pipeline::cut_file(&file.path, &output_path, &ad_ranges, file.length)
        };

        done += 1;

        match result {
            Ok(_) => {
                saved += 1;
                let mut s = state.lock().unwrap();
                s.progress = done as f32 / total as f32;
                s.status_message = format!("保存进度: {}/{}", done, total);
                if let Some(f) = s.files.get_mut(file_idx) {
                    f.needs_save = false;
                    f.marked_for_deletion.clear();
                }
            }
            Err(e) => {
                failures.push(format!("{}: {}", file.name, e));
                let mut s = state.lock().unwrap();
                s.progress = done as f32 / total as f32;
                s.status_message = format!("处理 {} 失败: {}", file.name, e);
            }
        }
    }

    let mut s = state.lock().unwrap();
    s.progress = 1.0;
    s.status_message = if failures.is_empty() {
        if overwrite {
            format!("保存完成！已覆盖原文件 ({} 个文件)", saved)
        } else {
            format!(
                "保存完成！输出目录: {} ({} 个文件)",
                output_dir.unwrap().display(),
                saved
            )
        }
    } else {
        format!(
            "保存结束：成功 {} 个，失败 {} 个（{}）",
            saved,
            failures.len(),
            failures.join("; ")
        )
    };
}

fn set_status(state: &Mutex<AppState>, message: String) {
    state.lock().unwrap().status_message = message;
}
