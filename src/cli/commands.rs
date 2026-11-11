//! CLI 命令实现

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::audio::ffmpeg;
use crate::core::classify::{classify_file_with_progress, ClassifyOptions};
use crate::core::detect::{detect_ad_with_progress, DetectOptions, DetectionResult};
use crate::core::forest::{Forest, ForestParams};
use crate::core::model::{load_model, save_model, ModelFile};
use crate::core::segfeat::extract_segment_features;
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
        Commands::Detect { files, output, hpss } => cmd_detect(&files, output.as_ref(), hpss),
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
        Commands::Train { data_list, output, trees, max_depth, test_split, seed } => {
            cmd_train(&data_list, &output, trees, max_depth, test_split, seed)
        }
        Commands::Classify { files, model, window, hop, confidence } => {
            cmd_classify(&files, &model, window, hop, confidence)
        }
    }
}

fn cmd_detect(files: &[PathBuf], output: Option<&PathBuf>, hpss: bool) -> Result<()> {
    println!("开始检测广告...");
    println!("输入文件数量: {}", files.len());
    if hpss { println!("HPSS 分离: 开启"); }

    for f in files {
        if !f.exists() {
            bail!("文件不存在: {}", f.display());
        }
    }

    let opts = if hpss { DetectOptions::default().with_hpss() } else { DetectOptions::default() };
    let result = detect_ad_with_progress(files, opts, |p, msg| {
        let bar = ProgressBar::new((p * 100.0) as u64);
        bar.set_style(ProgressStyle::default_bar());
        bar.println(msg);
    })?;

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

    let result = detect_ad_with_progress(&audio_files, DetectOptions::default(), |_, _| {})?;

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

fn cmd_train(data_list: &Path, output: &Path, trees: usize, max_depth: usize, test_split: f64, seed: u64) -> Result<()> {
    use crate::audio::cache;
    use ndarray::Array2;
    use rand::seq::SliceRandom;
    use rand::SeedableRng;
    use rayon::prelude::*;

    let list_dir = data_list.parent().unwrap_or(Path::new("."));
    let content = std::fs::read_to_string(data_list)?;
    let mut entries: Vec<(PathBuf, String)> = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        let Some((path_str, label)) = line.rsplit_once(char::is_whitespace) else { bail!("无法解析数据行: {}", line); };
        let path = PathBuf::from(path_str);
        let path = if path.is_absolute() { path } else { list_dir.join(path) };
        entries.push((path, label.to_string()));
    }
    let mut samples: Vec<(PathBuf, String, usize, usize)> = Vec::new();
    for (path, label) in entries {
        if label.contains('到') {
            let parts: Vec<&str> = label.split('到').collect();
            let total = parts.len();
            for (i, p) in parts.iter().enumerate() { samples.push((path.clone(), p.trim().to_string(), i, total)); }
        } else {
            samples.push((path, label, 0, 1));
        }
    }
    let mut labels: Vec<String> = samples.iter().map(|(_, l, _, _)| l.clone()).collect();
    labels.sort(); labels.dedup();
    let label_to_idx: HashMap<String, usize> = labels.iter().enumerate().map(|(i, l)| (l.clone(), i)).collect();
    println!("训练随机森林事件分类模型...");
    println!("样本: {}, 标签: {:?}", samples.len(), labels);

    let decoded: Vec<(Vec<f64>, String)> = samples.par_iter().map(|(path, label, seg_idx, total_segs)| {
        let is_wav = path.extension().map(|e| e.eq_ignore_ascii_case("wav")).unwrap_or(false);
        let raw = if is_wav { crate::core::fft::read_wav_samples(path)? } else {
            let wp = cache::wav_path(path);
            if !cache::is_fresh(path, &wp) { ffmpeg::audio_to_wav(path, &wp)?; }
            crate::core::fft::read_wav_samples(&wp)?
        };
        let sl = raw.len() / (*total_segs).max(1);
        let st = seg_idx * sl;
        let en = if seg_idx + 1 == *total_segs { raw.len() } else { (seg_idx + 1) * sl };
        Ok((raw[st..en].to_vec(), label.clone()))
    }).collect::<Result<Vec<_>>>()?;
    let ws = 44100; let hs = 22050;
    let mut expanded: Vec<(&[f64], usize)> = Vec::new();
    for (raw, label) in &decoded {
        let lbl = label_to_idx[label];
        if raw.len() < ws { continue; }
        let n_windows = (raw.len() - ws) / hs + 1;
        for j in 0..n_windows {
            expanded.push((&raw[j * hs..j * hs + ws], lbl));
        }
    }
    println!("窗口扩展后样本: {}", expanded.len());
    let features: Vec<(Vec<f64>, usize)> = expanded.par_iter().map(|(seg, lbl)| {
        let feat = extract_segment_features(seg)?; Ok((feat.to_vec(), *lbl))
    }).collect::<Result<Vec<_>>>()?;

    if features.is_empty() { bail!("没有可用的训练数据"); }
    let n_samples = features.len();
    let dim = features[0].0.len();
    let mut x = Array2::zeros((n_samples, dim));
    let mut y = vec![0usize; n_samples];
    for (i, (feat, lbl)) in features.iter().enumerate() {
        for j in 0..dim { x[[i, j]] = feat[j]; }
        y[i] = *lbl;
    }
    let mut mean = vec![0.0f64; dim];
    let mut std = vec![0.0f64; dim];
    for j in 0..dim {
        let s: f64 = (0..n_samples).map(|i| x[[i, j]]).sum();
        mean[j] = s / n_samples as f64;
    }
    for j in 0..dim {
        let s: f64 = (0..n_samples).map(|i| (x[[i, j]] - mean[j]).powi(2)).sum();
        std[j] = (s / n_samples as f64).sqrt().max(1e-12);
    }
    for i in 0..n_samples { for j in 0..dim { x[[i, j]] = (x[[i, j]] - mean[j]) / std[j]; } }

    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let mut indices: Vec<usize> = (0..n_samples).collect();
    indices.shuffle(&mut rng);
    let n_test = (n_samples as f64 * test_split) as usize;
    let test_set: std::collections::HashSet<usize> = indices.iter().take(n_test).copied().collect();
    let train_idx: Vec<usize> = indices.iter().skip(n_test).copied().collect();
    let train_x = x.select(ndarray::Axis(0), &train_idx);
    let train_y: Vec<usize> = train_idx.iter().map(|&i| y[i]).collect();

    let params = ForestParams { n_trees: trees, max_depth, ..Default::default() };
    let forest = Forest::train(&train_x, &train_y, &params, seed);

    println!("\n测试集评估:");
    println!("{:<10} {:>8} {:>8} {:>8}", "标签", "精确率", "召回率", "F1");
    for (lbl_idx, label) in labels.iter().enumerate() {
        let mut tp = 0usize; let mut fp = 0usize; let mut fn_ = 0usize;
        for &i in &indices {
            if !test_set.contains(&i) { continue; }
            let feat: Vec<f64> = (0..dim).map(|j| x[[i, j]]).collect();
            let (pred, _) = forest.predict(&feat);
            let actual = y[i];
            if pred == lbl_idx && actual == lbl_idx { tp += 1; }
            else if pred == lbl_idx && actual != lbl_idx { fp += 1; }
            else if pred != lbl_idx && actual == lbl_idx { fn_ += 1; }
        }
        let prec = if tp + fp > 0 { tp as f64 / (tp + fp) as f64 } else { 0.0 };
        let rec = if tp + fn_ > 0 { tp as f64 / (tp + fn_) as f64 } else { 0.0 };
        let f1 = if prec + rec > 0.0 { 2.0 * prec * rec / (prec + rec) } else { 0.0 };
        println!("{:<10} {:>8.2} {:>8.2} {:>8.2}", label, prec, rec, f1);
    }

    let model = ModelFile {
        version: crate::core::model::MODEL_VERSION,
        feature_dim: dim, labels, scaler_mean: mean, scaler_std: std,
        window_sec: 2.0, hop_sec: 1.0, forest,
    };
    save_model(&model, output)?;
    println!("\n模型已保存到: {}", output.display());
    Ok(())
}

fn cmd_classify(files: &[PathBuf], model_path: &Path, window: Option<f64>, hop: Option<f64>, confidence: Option<f64>) -> Result<()> {
    let mut model = load_model(model_path)?;
    if let Some(w) = window { model.window_sec = w; }
    if let Some(h) = hop { model.hop_sec = h; }
    let opts = ClassifyOptions { confidence_threshold: confidence.unwrap_or(0.55), ..Default::default() };
    println!("使用模型: {}", model_path.display());
    for f in files {
        if !f.exists() { bail!("文件不存在: {}", f.display()); }
        let name = f.file_name().unwrap().to_string_lossy();
        println!("\n文件: {}", name);
        let (events, n_frames) = classify_file_with_progress(f, &model, &opts, |_, _| {})?;
        println!("  总帧数: {} ({}秒)", n_frames, n_frames as f64 / 10.0);
        if events.is_empty() {
            println!("  未检测到事件");
        } else {
            for e in &events {
                println!("  {:.1}s - {:.1}s  {}  ({:.1}%)", e.begin as f64 / 10.0, e.end as f64 / 10.0, e.label, e.confidence * 100.0);
            }
        }
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
