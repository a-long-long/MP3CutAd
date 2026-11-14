//! 保存（按广告区间裁剪写盘）流程

use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;

use crate::pipeline;

use super::super::state::{AppState, FileInfo, SharedState};

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

    // 只处理用户通过「删除所选」标记过的文件（未标记的文件不动）
    let targets: Vec<(usize, FileInfo)> = files
        .into_iter()
        .enumerate()
        .filter(|(_, f)| f.needs_cut())
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

    // 预先为每个文件分配唯一的输出路径，避免不同目录下的同名文件互相覆盖
    let mut used_names: HashSet<String> = HashSet::new();
    let output_paths: Vec<Option<PathBuf>> = if overwrite {
        Vec::new()
    } else {
        let dir = output_dir.as_ref().unwrap();
        targets
            .iter()
            .map(|(_, file)| Some(unique_output_path(dir, &file.name, &mut used_names)))
            .collect()
    };

    let mut done = 0;
    let mut saved = 0;
    let mut failures: Vec<String> = Vec::new();

    for (target_idx, (file_idx, file)) in targets.into_iter().enumerate() {
        {
            let mut s = state.lock().unwrap();
            s.status_message = format!("[{}/{}] 处理: {}", done + 1, total, file.name);
        }

        // 只删除被标记的段（索引基于当前有效广告段）
        let ad_ranges = file.marked_segments(thresholds);
        if ad_ranges.is_empty() {
            // 标记的段在当前阈值下已失效，跳过
            done += 1;
            continue;
        }

        let result = if overwrite {
            pipeline::overwrite_file(&file.path, &ad_ranges, file.length)
        } else {
            let output_path = output_paths[target_idx]
                .as_ref()
                .expect("非覆盖模式必然已分配输出路径");
            pipeline::cut_file(&file.path, output_path, &ad_ranges, file.length)
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
                    // 覆盖后磁盘文件已变短，旧的时间轴与区间不能再用于二次裁剪
                    if overwrite {
                        f.reset_detection();
                    }
                }
                if overwrite {
                    s.ads_to_delete.retain(|&(fi, _)| fi != file_idx);
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
            format!(
                "保存完成！已覆盖原文件 ({} 个文件)，请重新检测后再操作",
                saved
            )
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

/// 为输出文件生成不冲突的路径：重名时在扩展名前追加 " (1)"、" (2)"。
///
/// `used` 保存已占用的小写文件名，兼容 Windows 大小写不敏感的文件系统。
fn unique_output_path(dir: &Path, name: &str, used: &mut HashSet<String>) -> PathBuf {
    let name = if name.is_empty() { "output" } else { name };
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string());
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    let mut candidate = name.to_string();
    let mut n = 0usize;
    while !used.insert(candidate.to_lowercase()) {
        n += 1;
        candidate = format!("{} ({}){}", stem, n, ext);
    }
    dir.join(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_output_path_renames_duplicates() {
        let dir = Path::new("out");
        let mut used = HashSet::new();

        assert_eq!(unique_output_path(dir, "a.mp3", &mut used), dir.join("a.mp3"));
        assert_eq!(
            unique_output_path(dir, "a.mp3", &mut used),
            dir.join("a (1).mp3")
        );
        // 大小写不同也视为重名（Windows 文件系统不区分大小写）
        assert_eq!(
            unique_output_path(dir, "A.MP3", &mut used),
            dir.join("A (2).MP3")
        );
        assert_eq!(
            unique_output_path(dir, "a.mp3", &mut used),
            dir.join("a (3).mp3")
        );
    }
}
