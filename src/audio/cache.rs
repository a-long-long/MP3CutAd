//! 检测中间结果缓存
//!
//! 缓存 WAV、FFT 频谱图与 LSH 哈希，避免重复解码与计算。
//!
//! 缓存键由输入文件的完整规范路径计算（而非文件名），
//! 因此不同目录下的同名文件互不干扰；同时用修改时间判断缓存是否过期。

use anyhow::{bail, Context, Result};
use ndarray::Array2;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 缓存格式版本，算法或参数变化时递增，使旧缓存自动失效
const CACHE_VERSION: u32 = 2;
/// 缓存目录名
const CACHE_DIR_NAME: &str = "mp3cutad_rs";

/// 缓存目录
pub fn cache_dir() -> PathBuf {
    std::env::temp_dir().join(CACHE_DIR_NAME)
}

/// 输入文件对应的 WAV 缓存路径
pub fn wav_path(input: &Path) -> PathBuf {
    cache_dir().join(format!("v{}-{}.wav", CACHE_VERSION, file_key(input)))
}

/// 输入文件对应的 FFT 缓存路径
pub fn fft_path(input: &Path) -> PathBuf {
    cache_dir().join(format!("v{}-{}.fft", CACHE_VERSION, file_key(input)))
}

/// 输入文件对应的哈希缓存路径
pub fn hash_path(input: &Path) -> PathBuf {
    cache_dir().join(format!("v{}-{}.hash", CACHE_VERSION, file_key(input)))
}

/// 由完整路径生成稳定的缓存键（FNV-1a）
fn file_key(input: &Path) -> String {
    let canonical = std::fs::canonicalize(input).unwrap_or_else(|_| input.to_path_buf());
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in canonical.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:016x}", hash)
}

/// 缓存是否仍然有效（存在且不早于源文件修改时间）
pub fn is_fresh(input: &Path, cache_file: &Path) -> bool {
    let source_time = std::fs::metadata(input).and_then(|m| m.modified()).ok();
    let cache_time = std::fs::metadata(cache_file)
        .and_then(|m| m.modified())
        .ok();

    match (source_time, cache_time) {
        (Some(src), Some(cached)) => is_not_older(cached, src),
        _ => false,
    }
}

fn is_not_older(cached: SystemTime, source: SystemTime) -> bool {
    cached >= source
}

/// 读取缓存的 FFT 频谱图；缓存缺失、过期或损坏时返回 `None`
pub fn read_fft(input: &Path) -> Option<Array2<f64>> {
    let path = fft_path(input);
    if !is_fresh(input, &path) {
        return None;
    }
    load_fft(&path).ok()
}

/// 写入 FFT 频谱图缓存
pub fn save_fft(input: &Path, data: &Array2<f64>) -> Result<()> {
    let mut buf = Vec::with_capacity(8 + data.len() * 8);
    buf.extend_from_slice(&(data.nrows() as u32).to_le_bytes());
    buf.extend_from_slice(&(data.ncols() as u32).to_le_bytes());
    for &val in data.iter() {
        buf.extend_from_slice(&val.to_le_bytes());
    }
    write_atomic(&fft_path(input), &buf)
}

/// 读取缓存的哈希序列；缓存缺失、过期或长度不符时返回 `None`
pub fn read_hash(input: &Path) -> Option<Vec<i32>> {
    let path = hash_path(input);
    if !is_fresh(input, &path) {
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    let hashes: Vec<i32> = content.lines().filter_map(|l| l.parse().ok()).collect();
    if hashes.is_empty() {
        None
    } else {
        Some(hashes)
    }
}

/// 写入哈希序列缓存
pub fn save_hash(input: &Path, data: &[i32]) -> Result<()> {
    let content: String = data
        .iter()
        .map(|h| h.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    write_atomic(&hash_path(input), content.as_bytes())
}

/// 清除某个输入文件的全部缓存
pub fn clear(input: &Path) {
    let _ = std::fs::remove_file(fft_path(input));
    let _ = std::fs::remove_file(hash_path(input));
    let _ = std::fs::remove_file(wav_path(input));
}

fn load_fft(path: &Path) -> Result<Array2<f64>> {
    let buf = std::fs::read(path)?;
    if buf.len() < 8 {
        bail!("FFT 缓存文件损坏（长度不足）: {}", path.display());
    }

    let nrows = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
    let ncols = u32::from_le_bytes(buf[4..8].try_into().unwrap()) as usize;
    let cells = nrows.checked_mul(ncols).context("FFT 缓存头信息溢出")?;

    if buf.len() != 8 + cells * 8 {
        bail!("FFT 缓存文件损坏（数据长度不匹配）: {}", path.display());
    }

    let values: Vec<f64> = buf[8..]
        .chunks_exact(8)
        .map(|chunk| f64::from_le_bytes(chunk.try_into().unwrap()))
        .collect();

    Ok(Array2::from_shape_vec((nrows, ncols), values)?)
}

/// 先写临时文件再重命名，避免中断时留下半截缓存
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!("{}.{}.tmp", file_name, std::process::id()));

    std::fs::write(&tmp, bytes)?;
    let _ = std::fs::remove_file(path);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_key_differs_for_same_name_in_different_dirs() {
        let a = Path::new("C:/music/disc1/track.mp3");
        let b = Path::new("C:/music/disc2/track.mp3");
        assert_ne!(file_key(a), file_key(b));
    }

    #[test]
    fn test_file_key_stable_for_same_path() {
        let a = Path::new("C:/music/track.mp3");
        assert_eq!(file_key(a), file_key(a));
    }
}
