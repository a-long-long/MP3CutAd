//! FFmpeg 封装模块
//!
//! 提供 MP3/WAV 转换和音频裁剪功能

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Windows 下隐藏控制台窗口的标志
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// FFmpeg 可执行文件路径
const FFMPEG_PATH: &str = "ffmpeg";

/// 执行 FFmpeg 命令
fn run_ffmpeg(args: &[&str]) -> Result<()> {
    let mut cmd = Command::new(FFMPEG_PATH);
    cmd.args(args);

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let output = cmd
        .output()
        .context("无法启动 ffmpeg，请确保已安装并添加到 PATH")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ffmpeg 执行失败: {}", stderr);
    }

    Ok(())
}

/// 将 MP3 转换为 WAV（单声道，22050Hz）
///
/// # 参数
/// - `input`: 输入 MP3 文件路径
/// - `output`: 输出 WAV 文件路径
pub fn mp3_to_wav(input: &Path, output: &Path) -> Result<()> {
    run_ffmpeg(&[
        "-i",
        input.to_str().unwrap(),
        "-ac",
        "1", // 单声道
        "-ar",
        "22050", // 22050 Hz 采样率
        "-f",
        "wav",
        "-y", // 覆盖输出文件
        output.to_str().unwrap(),
    ])
}

/// 裁剪音频文件
///
/// # 参数
/// - `input`: 输入文件路径
/// - `output`: 输出文件路径
/// - `from_sec`: 开始时间（秒）
/// - `to_sec`: 结束时间（秒）
pub fn split_audio(input: &Path, output: &Path, from_sec: f64, to_sec: f64) -> Result<()> {
    run_ffmpeg(&[
        "-i",
        input.to_str().unwrap(),
        "-c",
        "copy",
        "-ss",
        &format!("{}", from_sec),
        "-to",
        &format!("{}", to_sec),
        "-y",
        output.to_str().unwrap(),
    ])
}

/// 拼接多个音频文件
///
/// # 参数
/// - `list_file`: 包含文件列表的文本文件（每行格式：file 'path'）
/// - `output`: 输出文件路径
pub fn concat_audio(list_file: &Path, output: &Path) -> Result<()> {
    run_ffmpeg(&[
        "-f",
        "concat",
        "-safe",
        "0", // 允许列表中使用绝对路径
        "-i",
        list_file.to_str().unwrap(),
        "-c",
        "copy",
        "-y",
        output.to_str().unwrap(),
    ])
}

/// 裁剪并拼接音频（按照 C# 版本的逻辑）
///
/// # 参数
/// - `mp3`: 输入 MP3 文件
/// - `output`: 输出文件
/// - `segments`: 要保留的时间段列表（单位：0.1秒）
pub fn cut_and_combine(mp3: &Path, output: &Path, segments: &[(i32, i32)]) -> Result<()> {
    // 临时文件创建在输出文件旁边（与 C# 版本一致）
    let output_dir = output.parent().unwrap_or(Path::new("."));
    let base_name = output.file_stem().unwrap_or_default().to_string_lossy();

    let mut temp_files = Vec::new();
    let mut list_content = String::new();

    // 裁剪每个片段（与 C# 版本：wav + i + ".mp3"）
    for (i, &(begin, end)) in segments.iter().enumerate() {
        let from_sec = begin as f64 / 10.0;
        let to_sec = end as f64 / 10.0;

        let temp_file = output_dir.join(format!("{}_{}.mp3", base_name, i));
        split_audio(mp3, &temp_file, from_sec, to_sec)?;

        list_content.push_str(&format!("file '{}'\n", temp_file.to_str().unwrap()));
        temp_files.push(temp_file);
    }

    // 写入列表文件（与 C# 版本：wav + ".list"）
    let list_file = output_dir.join(format!("{}.list", base_name));
    std::fs::write(&list_file, &list_content)?;
    temp_files.push(list_file.clone());

    // 拼接
    let result = concat_audio(&list_file, output);

    // 清理临时文件（C# 版本不清理，但我们应该清理）
    for f in &temp_files {
        let _ = std::fs::remove_file(f);
    }

    result
}

/// 检查 FFmpeg 是否可用
pub fn check_ffmpeg() -> bool {
    Command::new(FFMPEG_PATH)
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ffmpeg_available() {
        // 这个测试需要系统安装了 FFmpeg
        if check_ffmpeg() {
            println!("FFmpeg 已安装");
        } else {
            println!("FFmpeg 未安装，跳过测试");
        }
    }
}
