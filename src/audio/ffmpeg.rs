//! FFmpeg 封装模块
//!
//! 提供音频解码（转 WAV）和音频裁剪功能，支持 ffmpeg 能解码的所有格式

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
///
/// 通过闭包构造参数，路径直接以 `OsStr` 传入，避免非 UTF-8 路径 `to_str().unwrap()` panic。
fn run_ffmpeg<F>(build: F) -> Result<()>
where
    F: FnOnce(&mut Command),
{
    let mut cmd = Command::new(FFMPEG_PATH);
    build(&mut cmd);

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

/// 确保输出文件的父目录存在。
///
/// ffmpeg 不会自动创建输出路径中的目录，缓存目录（如 `/tmp/mp3cutad_rs`）
/// 首次不存在时会直接导致 `No such file or directory`，因此写文件前先建目录。
fn ensure_parent_dir(output: &Path) -> Result<()> {
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("无法创建输出目录: {}", parent.display()))?;
        }
    }
    Ok(())
}

/// 将任意音频格式转换为 WAV（单声道，22050Hz），用于特征提取
///
/// # 参数
/// - `input`: 输入音频文件路径（mp3/wav/flac/m4a/aac/ogg/opus/wma 等）
/// - `output`: 输出 WAV 文件路径
pub fn audio_to_wav(input: &Path, output: &Path) -> Result<()> {
    ensure_parent_dir(output)?;
    run_ffmpeg(|cmd| {
        cmd.arg("-i")
            .arg(input)
            .args(["-ac", "1"]) // 单声道
            .args(["-ar", "22050"]) // 22050 Hz 采样率
            .args(["-f", "wav"])
            .arg("-y") // 覆盖输出文件
            .arg(output);
    })
}

/// 裁剪音频文件
///
/// # 参数
/// - `input`: 输入文件路径
/// - `output`: 输出文件路径
/// - `from_sec`: 开始时间（秒）
/// - `to_sec`: 结束时间（秒）
pub fn split_audio(input: &Path, output: &Path, from_sec: f64, to_sec: f64) -> Result<()> {
    ensure_parent_dir(output)?;
    run_ffmpeg(|cmd| {
        cmd.arg("-i")
            .arg(input)
            .args(["-c", "copy", "-ss"])
            .arg(format!("{}", from_sec))
            .arg("-to")
            .arg(format!("{}", to_sec))
            .arg("-y")
            .arg(output);
    })
}

/// 拼接多个音频文件
///
/// mp3/wav 使用流拷贝（无重编码损耗）；flac 无损重编码，
/// 其余有损格式（ogg/opus/m4a/aac/wma）重编码一次——这些容器
/// 用流拷贝拼接容易出现时长元数据错误或时间戳混乱。
///
/// # 参数
/// - `list_file`: 包含文件列表的文本文件（每行格式：file 'path'）
/// - `output`: 输出文件路径
pub fn concat_audio(list_file: &Path, output: &Path) -> Result<()> {
    let ext = output
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    let codec_args: Vec<String> = match ext.as_str() {
        "mp3" | "wav" => vec!["-c".into(), "copy".into()],
        "flac" => vec!["-c:a".into(), "flac".into()],
        "ogg" => vec!["-c:a".into(), "libvorbis".into()],
        "opus" => vec!["-c:a".into(), "libopus".into()],
        // m4a/aac/wma 等：让 ffmpeg 按容器选择默认编码器
        _ => Vec::new(),
    };

    ensure_parent_dir(output)?;
    run_ffmpeg(|cmd| {
        cmd.args(["-f", "concat", "-safe", "0"]) // safe=0 允许列表中使用绝对路径
            .arg("-i")
            .arg(list_file);
        for arg in &codec_args {
            cmd.arg(arg);
        }
        cmd.arg("-y").arg(output);
    })
}

/// 裁剪并拼接音频（按照 C# 版本的逻辑）
///
/// 输出容器格式由 `output` 的扩展名决定，保持与原文件一致。
///
/// # 参数
/// - `input`: 输入音频文件
/// - `output`: 输出文件
/// - `segments`: 要保留的时间段列表（单位：0.1秒）
pub fn cut_and_combine(input: &Path, output: &Path, segments: &[(i32, i32)]) -> Result<()> {
    // 临时文件创建在输出文件旁边（与 C# 版本一致）
    let output_dir = output.parent().unwrap_or(Path::new("."));
    let base_name = output.file_stem().unwrap_or_default().to_string_lossy();
    // 临时片段使用与输出相同的扩展名，保证 -c copy 产出相同容器
    let ext = output
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "mp3".to_string());

    let mut temp_files = Vec::new();
    let mut list_content = String::new();

    // 裁剪每个片段
    for (i, &(begin, end)) in segments.iter().enumerate() {
        let from_sec = begin as f64 / 10.0;
        let to_sec = end as f64 / 10.0;

        let temp_file = output_dir.join(format!("{}_{}.{}", base_name, i, ext));
        split_audio(input, &temp_file, from_sec, to_sec)?;

        list_content.push_str(&format!("file '{}'\n", temp_file.display()));
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
