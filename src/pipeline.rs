//! 检测与裁剪的流程编排
//!
//! CLI、GUI 与 FFI 共用这里的逻辑，避免各自重复实现"反转区间 → 拼接片段"。

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

use crate::audio::{cache, ffmpeg};
use crate::core::detect::reverse_range;
use crate::core::range::Range;

/// 支持的音频扩展名（ffmpeg 可解码的常见格式）
pub const SUPPORTED_EXTENSIONS: &[&str] =
    &["mp3", "wav", "flac", "m4a", "aac", "ogg", "opus", "wma"];

/// 判断路径是否为支持的音频文件
pub fn is_supported_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            SUPPORTED_EXTENSIONS
                .iter()
                .any(|e| e.eq_ignore_ascii_case(ext))
        })
        .unwrap_or(false)
}

/// 收集目录下的音频文件（按路径排序，保证处理顺序稳定）
pub fn collect_audio_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("无法读取目录: {}", dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| is_supported_audio(path))
        .collect();

    files.sort();
    Ok(files)
}

/// 由广告区间得到需要保留的片段（单位：0.1 秒）
pub fn keep_segments(ad_ranges: &[Range], total_len: usize) -> Vec<(i32, i32)> {
    reverse_range(ad_ranges, total_len)
        .iter()
        .map(|r| (r.begin, r.end))
        .collect()
}

/// 按广告区间裁剪单个文件，输出到指定位置
pub fn cut_file(input: &Path, output: &Path, ad_ranges: &[Range], total_len: usize) -> Result<()> {
    let segments = keep_segments(ad_ranges, total_len);

    if segments.is_empty() {
        bail!(
            "{} 的全部内容都被判定为广告，没有可保留的片段",
            input.display()
        );
    }

    ffmpeg::cut_and_combine(input, output, &segments)
}

/// 裁剪后覆盖写回原文件，并清理该文件的检测缓存
pub fn overwrite_file(input: &Path, ad_ranges: &[Range], total_len: usize) -> Result<()> {
    let file_stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = input
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "mp3".to_string());
    let tmp = input.with_file_name(format!(".{}.{}.tmp.{}", file_stem, std::process::id(), ext));

    cut_file(input, &tmp, ad_ranges, total_len)?;

    // 用同目录 rename 原子替换原文件，避免 fs::copy 中途失败把原文件写坏。
    // tmp 与 input 同目录，因此 rename 不会跨文件系统。
    match std::fs::rename(&tmp, input) {
        Ok(_) => {
            cache::clear(input);
            Ok(())
        }
        Err(e) => {
            // 失败时清理临时文件，原文件保持不动
            let _ = std::fs::remove_file(&tmp);
            Err(e).with_context(|| format!("无法覆盖原文件: {}", input.display()))
        }
    }
}
