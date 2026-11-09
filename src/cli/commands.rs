//! CLI 命令实现

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::{Path, PathBuf};

use crate::audio::ffmpeg;
use crate::core::detect::{detect_ad, DetectionResult};
use crate::{pipeline, result_io};

#[derive(Parser)]
#[command(name = "mp3cutad-rs")]
#[command(about = "MP3 广告检测与裁剪工具 (Rust 版本)", long_about = None)]
#[command(version = "0.1.0")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 检测音频文件中的广告
    Detect {
        /// 音频文件列表（mp3/wav/flac/m4a/aac/ogg/opus/wma）
        #[arg(required = true)]
        files: Vec<PathBuf>,

        /// 输出检测结果到文件
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// 裁剪音频文件（根据检测结果）
    Cut {
        /// 输入目录（包含音频文件）
        #[arg(short, long)]
        input_dir: PathBuf,

        /// 输出目录
        #[arg(short, long)]
        output_dir: PathBuf,

        /// 检测结果文件（由 detect 命令生成）
        #[arg(short, long)]
        result_file: PathBuf,
    },

    /// 一键检测并裁剪
    Auto {
        /// 输入目录
        #[arg(short, long)]
        input_dir: PathBuf,

        /// 输出目录
        #[arg(short, long)]
        output_dir: PathBuf,
    },

    /// 检查 FFmpeg 是否可用
    Check,
}

/// CLI 入口
pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Detect { files, output } => cmd_detect(&files, output.as_ref()),
        Commands::Cut {
            input_dir,
            output_dir,
            result_file,
        } => cmd_cut(&input_dir, &output_dir, &result_file),
        Commands::Auto {
            input_dir,
            output_dir,
        } => cmd_auto(&input_dir, &output_dir),
        Commands::Check => cmd_check(),
    }
}

fn cmd_detect(files: &[PathBuf], output: Option<&PathBuf>) -> Result<()> {
    println!("开始检测广告...");
    println!("输入文件数量: {}", files.len());

    for f in files {
        if !f.exists() {
            bail!("文件不存在: {}", f.display());
        }
    }

    let result = detect_ad(files)?;

    println!("\n检测结果:");
    println!("============");

    for (i, (ranges, len)) in result.ranges.iter().zip(result.lengths.iter()).enumerate() {
        let file_name = files[i].file_name().unwrap().to_string_lossy();
        println!("\n文件: {}", file_name);
        println!("  总帧数: {}", len);
        println!("  检测到 {} 个广告区间:", ranges.len());

        for r in ranges {
            let begin_sec = r.begin as f64 / 10.0;
            let end_sec = r.end as f64 / 10.0;
            let duration = end_sec - begin_sec;
            println!(
                "    {:.1}s - {:.1}s (时长 {:.1}s, 出现 {} 次)",
                begin_sec, end_sec, duration, r.count
            );
        }
    }

    if let Some(output_path) = output {
        result_io::save(&result, files, output_path)?;
        println!("\n结果已保存到: {}", output_path.display());
    }

    Ok(())
}

fn cmd_cut(input_dir: &Path, output_dir: &Path, result_file: &Path) -> Result<()> {
    println!("开始裁剪广告...");

    let result = result_io::load(result_file)?;
    let audio_files = pipeline::collect_audio_files(input_dir)?;

    if audio_files.len() != result.ranges.len() {
        bail!(
            "文件数量不匹配: 期望 {}, 实际 {}",
            result.ranges.len(),
            audio_files.len()
        );
    }

    std::fs::create_dir_all(output_dir)?;
    cut_files(&audio_files, &result, output_dir)?;

    println!("\n输出目录: {}", output_dir.display());
    Ok(())
}

fn cmd_auto(input_dir: &Path, output_dir: &Path) -> Result<()> {
    println!("自动模式: 检测并裁剪广告");

    let audio_files = pipeline::collect_audio_files(input_dir)?;

    if audio_files.is_empty() {
        println!("输入目录中没有音频文件");
        return Ok(());
    }

    println!("找到 {} 个音频文件", audio_files.len());

    let result = detect_ad(&audio_files)?;

    std::fs::create_dir_all(output_dir)?;
    cut_files(&audio_files, &result, output_dir)?;

    println!("\n处理统计:");
    for (i, ranges) in result.ranges.iter().enumerate() {
        let file_name = audio_files[i].file_name().unwrap().to_string_lossy();
        let ad_duration: i32 = ranges
            .iter()
            .filter(|r| r.count > 1)
            .map(|r| r.end - r.begin)
            .sum();
        let total = result.lengths[i] as i32;
        let ratio = ad_duration as f64 / total as f64 * 100.0;
        println!(
            "  {}: 检测到 {} 个广告，总时长 {:.1}s ({:.1}%)",
            file_name,
            ranges.len(),
            ad_duration as f64 / 10.0,
            ratio
        );
    }

    println!("\n输出目录: {}", output_dir.display());
    Ok(())
}

fn cmd_check() -> Result<()> {
    println!("检查系统环境...");

    if ffmpeg::check_ffmpeg() {
        println!("  FFmpeg: 已安装");
    } else {
        println!("  FFmpeg: 未安装");
        println!("    请安装 FFmpeg 并确保其在 PATH 中");
    }

    Ok(())
}

/// 按检测结果批量裁剪并输出到指定目录（输出格式与源文件一致）
fn cut_files(audio_files: &[PathBuf], result: &DetectionResult, output_dir: &Path) -> Result<()> {
    let pb = progress_bar(audio_files.len() as u64);
    let mut failures: Vec<String> = Vec::new();

    for (i, audio_path) in audio_files.iter().enumerate() {
        let output_path = output_dir.join(audio_path.file_name().unwrap());
        if let Err(e) = pipeline::cut_file(
            audio_path,
            &output_path,
            &result.ranges[i],
            result.lengths[i],
        ) {
            failures.push(format!(
                "{}: {}",
                audio_path.file_name().unwrap().to_string_lossy(),
                e
            ));
        }
        pb.inc(1);
    }

    pb.finish_with_message("裁剪完成");

    if !failures.is_empty() {
        println!("\n以下文件未生成输出:");
        for failure in &failures {
            println!("  {}", failure);
        }
    }

    Ok(())
}

fn progress_bar(total: u64) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})",
            )
            .unwrap()
            .progress_chars("#>-"),
    );
    pb
}
