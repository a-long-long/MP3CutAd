//! CLI 命令实现

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::audio::ffmpeg;
use crate::core::classify::{classify_file_with_progress, ClassifyOptions};
use crate::core::detect::{detect_ad_with_progress, DetectOptions};
use crate::core::classify::model::load_model;
use crate::core::range::Range;
use crate::training::{TrainConfig, TrainSample};
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

        /// 启用 HPSS 打击乐轨道源分离（对带背景音乐的广告段更稳定）
        #[arg(long)]
        hpss: bool,
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

    /// 训练事件分类模型
    Train {
        #[arg(short, long)]
        data_list: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long, default_value = "100")]
        trees: usize,
        #[arg(long, default_value = "32")]
        max_depth: usize,
        #[arg(long, default_value = "0.2")]
        test_split: f64,
        #[arg(long, default_value = "42")]
        seed: u64,
    },

    /// 用事件分类模型对音频文件打标签
    Classify {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        #[arg(short, long)]
        model: PathBuf,
        #[arg(short, long)]
        window: Option<f64>,
        #[arg(long)]
        hop: Option<f64>,
        #[arg(short, long)]
        confidence: Option<f64>,
    },
}

/// CLI 入口
pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Detect {
            files,
            output,
            hpss,
        } => cmd_detect(&files, output.as_ref(), hpss),
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
        Commands::Train {
            data_list,
            output,
            trees,
            max_depth,
            test_split,
            seed,
        } => cmd_train(&data_list, &output, trees, max_depth, test_split, seed),
        Commands::Classify {
            files,
            model,
            window,
            hop,
            confidence,
        } => cmd_classify(&files, &model, window, hop, confidence),
    }
}

fn cmd_detect(files: &[PathBuf], output: Option<&PathBuf>, hpss: bool) -> Result<()> {
    println!("开始检测广告...");
    println!("输入文件数量: {}", files.len());
    if hpss {
        println!("HPSS 分离: 开启");
    }

    for f in files {
        if !f.exists() {
            bail!("文件不存在: {}", f.display());
        }
    }

    let opts = if hpss {
        DetectOptions::default().with_hpss()
    } else {
        DetectOptions::default()
    };
    // 复用同一个进度条，避免每次回调新建导致大量瞬时条
    let pb = progress_bar(1000);
    let result = detect_ad_with_progress(files, opts, |p, msg| {
        pb.set_position((p * 1000.0) as u64);
        pb.set_message(msg.to_string());
    })?;
    pb.finish_and_clear();

    println!("\n检测结果:");
    println!("============");
    if let Some(gpu) = &result.gpu {
        println!("GPU 加速: {}", gpu);
    }

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

    let (result_paths, result) = result_io::load_with_paths(result_file)?;
    let audio_files = pipeline::collect_audio_files(input_dir)?;

    // 结果文件记录的是检测时的文件路径；按路径匹配，避免目录扫描顺序不同导致切错文件
    let mut by_path: HashMap<PathBuf, (Vec<Range>, usize)> = HashMap::new();
    for ((path, ranges), length) in result_paths
        .iter()
        .zip(result.ranges.into_iter())
        .zip(result.lengths.into_iter())
    {
        by_path.insert(normalize_path(path), (ranges, length));
    }

    let mut items: Vec<(PathBuf, Vec<Range>, usize)> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for audio_path in &audio_files {
        match by_path.get(&normalize_path(audio_path)) {
            Some((ranges, length)) => items.push((audio_path.clone(), ranges.clone(), *length)),
            None => missing.push(audio_path.display().to_string()),
        }
    }

    if items.is_empty() {
        bail!("没有音频文件与结果文件中的路径匹配（请检查 --input-dir 是否与检测时一致）");
    }
    if !missing.is_empty() {
        println!("警告：以下文件不在结果文件中，已跳过：");
        for m in &missing {
            println!("  {}", m);
        }
    }

    std::fs::create_dir_all(output_dir)?;
    cut_files(&items, output_dir)?;

    println!("\n输出目录: {}", output_dir.display());
    Ok(())
}

/// 统一路径形式，便于结果文件中的路径与目录扫描结果匹配
fn normalize_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn cmd_auto(input_dir: &Path, output_dir: &Path) -> Result<()> {
    println!("自动模式: 检测并裁剪广告");

    let audio_files = pipeline::collect_audio_files(input_dir)?;

    if audio_files.is_empty() {
        println!("输入目录中没有音频文件");
        return Ok(());
    }

    println!("找到 {} 个音频文件", audio_files.len());

    let result = detect_ad_with_progress(&audio_files, DetectOptions::default(), |_, _| {})?;

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

    let items: Vec<(PathBuf, Vec<Range>, usize)> = audio_files
        .into_iter()
        .zip(result.ranges)
        .zip(result.lengths)
        .map(|((path, ranges), length)| (path, ranges, length))
        .collect();
    std::fs::create_dir_all(output_dir)?;
    cut_files(&items, output_dir)?;

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

fn cmd_train(
    data_list: &Path,
    output: &Path,
    trees: usize,
    max_depth: usize,
    test_split: f64,
    seed: u64,
) -> Result<()> {
    let list_dir = data_list.parent().unwrap_or(Path::new("."));
    let content = std::fs::read_to_string(data_list)?;
    let mut samples: Vec<TrainSample> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((path_str, label)) = line.rsplit_once(char::is_whitespace) else {
            bail!("无法解析数据行: {}", line);
        };
        let path = PathBuf::from(path_str);
        let path = if path.is_absolute() {
            path
        } else {
            list_dir.join(path)
        };
        if label.contains('到') {
            let parts: Vec<&str> = label.split('到').collect();
            for (i, part) in parts.iter().enumerate() {
                samples.push(TrainSample {
                    path: path.clone(),
                    label: part.trim().to_string(),
                    split: Some((i, parts.len())),
                });
            }
        } else {
            samples.push(TrainSample::whole(path, label));
        }
    }

    let mut labels: Vec<String> = samples.iter().map(|s| s.label.clone()).collect();
    labels.sort();
    labels.dedup();

    println!("训练随机森林事件分类模型...");
    println!("样本: {}, 标签: {:?}", samples.len(), labels);

    let config = TrainConfig {
        n_trees: trees,
        max_depth,
        test_split,
        seed,
        ..Default::default()
    };
    let report = crate::training::train_model(&samples, &config, output, |_, _| {})?;

    println!("窗口扩展后样本: {}", report.samples);
    println!("\n测试集评估:");
    println!("{:<10} {:>8} {:>8} {:>8}", "标签", "精确率", "召回率", "F1");
    for m in &report.metrics {
        println!(
            "{:<10} {:>8.2} {:>8.2} {:>8.2}",
            m.label, m.precision, m.recall, m.f1
        );
    }

    println!("\n模型已保存到: {}", output.display());
    Ok(())
}

fn cmd_classify(
    files: &[PathBuf],
    model_path: &Path,
    window: Option<f64>,
    hop: Option<f64>,
    confidence: Option<f64>,
) -> Result<()> {
    let mut model = load_model(model_path)?;
    if let Some(w) = window {
        model.window_sec = w;
    }
    if let Some(h) = hop {
        model.hop_sec = h;
    }
    let opts = ClassifyOptions {
        confidence_threshold: confidence.unwrap_or(0.55),
        ..Default::default()
    };
    println!("使用模型: {}", model_path.display());
    for f in files {
        if !f.exists() {
            bail!("文件不存在: {}", f.display());
        }
        let name = f.file_name().unwrap().to_string_lossy();
        println!("\n文件: {}", name);
        let (events, n_frames) = classify_file_with_progress(f, &model, &opts, |_, _| {})?;
        println!("  总帧数: {} ({}秒)", n_frames, n_frames as f64 / 10.0);
        if events.is_empty() {
            println!("  未检测到事件");
        } else {
            for e in &events {
                println!(
                    "  {:.1}s - {:.1}s  {}  ({:.1}%)",
                    e.begin as f64 / 10.0,
                    e.end as f64 / 10.0,
                    e.label,
                    e.confidence * 100.0
                );
            }
        }
    }
    Ok(())
}

/// 按检测结果批量裁剪并输出到指定目录（输出格式与源文件一致）
///
/// 每一项为 `(源文件, 广告区间, 总帧数)`，区间与文件一一对应，避免按下标误配。
fn cut_files(items: &[(PathBuf, Vec<Range>, usize)], output_dir: &Path) -> Result<()> {
    let pb = progress_bar(items.len() as u64);
    let mut failures: Vec<String> = Vec::new();

    for (audio_path, ranges, length) in items {
        let Some(file_name) = audio_path.file_name() else {
            failures.push(format!("{}: 无法获取文件名", audio_path.display()));
            pb.inc(1);
            continue;
        };
        let output_path = output_dir.join(file_name);
        if let Err(e) = pipeline::cut_file(audio_path, &output_path, ranges, *length) {
            failures.push(format!("{}: {}", audio_path.display(), e));
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
