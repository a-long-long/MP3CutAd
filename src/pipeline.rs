//! 检测与裁剪的流程编排
//!
//! CLI、GUI 与 FFI 共用这里的逻辑，避免各自重复实现"反转区间 → 拼接片段"。

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

use crate::audio::{cache, ffmpeg};
use crate::core::detect::reverse_range;
use crate::core::range::Range;

/// 收集目录下的 MP3 文件（按路径排序，保证处理顺序稳定）
pub fn collect_mp3_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("无法读取目录: {}", dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .map(|ext| ext.eq_ignore_ascii_case("mp3"))
                .unwrap_or(false)
        })
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
    let tmp = input.with_file_name(format!(".{}.{}.tmp.mp3", file_stem, std::process::id()));

    cut_file(input, &tmp, ad_ranges, total_len)?;

    let result =
        std::fs::copy(&tmp, input).with_context(|| format!("无法覆盖原文件: {}", input.display()));

    if result.is_ok() {
        let _ = std::fs::remove_file(&tmp);
        cache::clear(input);
    }

    result.map(|_| ())
}
