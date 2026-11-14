//! 事件分类模型训练流程
//!
//! 供 CLI 与 GUI 共用：解码样本 → 滑窗提特征 → 标准化 → 训练随机森林
//! → 测试集评估 → 保存 `.mcrf` 模型。

use anyhow::{bail, Result};
use ndarray::Array2;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::audio::{cache, ffmpeg};
use crate::core::classify::forest::{Forest, ForestParams};
use crate::core::classify::model::{save_model, ModelFile, MODEL_VERSION};
use crate::core::dsp::fft::read_wav_samples;
use crate::core::dsp::segfeat::{extract_segment_features, SEG_SR};

/// 默认窗口时长（秒）
pub const DEFAULT_WINDOW_SEC: f64 = 2.0;
/// 默认窗口步长（秒）
pub const DEFAULT_HOP_SEC: f64 = 1.0;

/// 一条训练样本
#[derive(Debug, Clone)]
pub struct TrainSample {
    /// 音频文件路径
    pub path: PathBuf,
    /// 标签名
    pub label: String,
    /// 把文件按时长均分后取第 `index` 段（共 `total` 段）；`None` 表示整段文件
    pub split: Option<(usize, usize)>,
}

impl TrainSample {
    /// 整段文件作为一条样本
    pub fn whole(path: PathBuf, label: impl Into<String>) -> Self {
        Self {
            path,
            label: label.into(),
            split: None,
        }
    }
}

/// 训练参数
#[derive(Debug, Clone)]
pub struct TrainConfig {
    /// 随机森林树数量
    pub n_trees: usize,
    /// 单棵树最大深度
    pub max_depth: usize,
    /// 测试集比例（0.0–0.5）
    pub test_split: f64,
    /// 随机种子
    pub seed: u64,
    /// 窗口时长（秒）
    pub window_sec: f64,
    /// 窗口步长（秒）
    pub hop_sec: f64,
}

impl Default for TrainConfig {
    fn default() -> Self {
        Self {
            n_trees: 100,
            max_depth: 32,
            test_split: 0.2,
            seed: 42,
            window_sec: DEFAULT_WINDOW_SEC,
            hop_sec: DEFAULT_HOP_SEC,
        }
    }
}

/// 单个标签的测试集指标
#[derive(Debug, Clone)]
pub struct LabelMetrics {
    pub label: String,
    pub precision: f64,
    pub recall: f64,
    pub f1: f64,
}

/// 训练结果摘要
#[derive(Debug, Clone, Default)]
pub struct TrainReport {
    /// 窗口扩展后的总样本数
    pub samples: usize,
    /// 训练集样本数
    pub train_samples: usize,
    /// 测试集样本数
    pub test_samples: usize,
    /// 标签集合（按名称排序）
    pub labels: Vec<String>,
    /// 各标签测试集指标
    pub metrics: Vec<LabelMetrics>,
}

/// 训练并保存模型
///
/// `on_progress(progress, message)` 在特征提取阶段被并行调用，
/// 因此要求 `Sync`。
pub fn train_model<F>(
    samples: &[TrainSample],
    config: &TrainConfig,
    output: &Path,
    on_progress: F,
) -> Result<TrainReport>
where
    F: Fn(f32, &str) + Sync,
{
    if samples.is_empty() {
        bail!("没有训练样本");
    }
    if config.window_sec <= 0.0 || config.hop_sec <= 0.0 {
        bail!("窗口与步长必须大于 0");
    }

    let mut labels: Vec<String> = samples.iter().map(|s| s.label.clone()).collect();
    labels.sort();
    labels.dedup();
    if labels.len() < 2 {
        bail!(
            "至少需要两种标签才能训练（当前只有: {}）",
            labels.join(", ")
        );
    }
    let label_to_idx: HashMap<String, usize> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| (l.clone(), i))
        .collect();

    let ws = (config.window_sec * SEG_SR as f64).round().max(1.0) as usize;
    let hs = (config.hop_sec * SEG_SR as f64).round().max(1.0) as usize;
    if hs > ws {
        bail!("窗口步长不能大于窗口时长");
    }

    // 1. 解码 + 滑窗提特征（并行）
    let total = samples.len();
    let done = AtomicUsize::new(0);
    let extracted: Vec<Result<Vec<(Vec<f64>, usize)>>> = samples
        .par_iter()
        .map(|sample| {
            let raw = decode_samples(&sample.path)?;
            let slice = sample_slice(&raw, sample.split);
            let mut rows = Vec::new();
            if slice.len() >= ws {
                let n_windows = (slice.len() - ws) / hs + 1;
                for j in 0..n_windows {
                    let feat = extract_segment_features(&slice[j * hs..j * hs + ws])?;
                    rows.push((feat.to_vec(), label_to_idx[&sample.label]));
                }
            }
            let step = done.fetch_add(1, Ordering::Relaxed) + 1;
            let name = sample
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            on_progress(
                step as f32 / total as f32,
                &format!("提取特征 {}/{}: {}", step, total, name),
            );
            Ok(rows)
        })
        .collect();

    let mut features: Vec<(Vec<f64>, usize)> = Vec::new();
    for item in extracted {
        features.extend(item?);
    }
    if features.is_empty() {
        bail!("没有可用的训练样本，请确认音频长度大于窗口时长");
    }

    let n_samples = features.len();
    let dim = features[0].0.len();
    let mut x = Array2::zeros((n_samples, dim));
    let mut y = vec![0usize; n_samples];
    for (i, (feat, label_idx)) in features.iter().enumerate() {
        for j in 0..dim {
            x[[i, j]] = feat[j];
        }
        y[i] = *label_idx;
    }

    // 2. 标准化
    let mut mean = vec![0.0f64; dim];
    let mut std = vec![0.0f64; dim];
    for j in 0..dim {
        let sum: f64 = (0..n_samples).map(|i| x[[i, j]]).sum();
        mean[j] = sum / n_samples as f64;
    }
    for j in 0..dim {
        let var: f64 = (0..n_samples)
            .map(|i| (x[[i, j]] - mean[j]).powi(2))
            .sum::<f64>()
            / n_samples as f64;
        std[j] = var.sqrt().max(1e-12);
    }
    for i in 0..n_samples {
        for j in 0..dim {
            x[[i, j]] = (x[[i, j]] - mean[j]) / std[j];
        }
    }

    // 3. 划分训练集 / 测试集
    let mut rng = rand::rngs::StdRng::seed_from_u64(config.seed);
    let mut indices: Vec<usize> = (0..n_samples).collect();
    indices.shuffle(&mut rng);
    let n_test = ((n_samples as f64 * config.test_split.clamp(0.0, 0.5)) as usize)
        .min(n_samples.saturating_sub(1));
    let test_set: std::collections::HashSet<usize> = indices.iter().take(n_test).copied().collect();
    let train_idx: Vec<usize> = indices.iter().skip(n_test).copied().collect();
    let train_x = x.select(ndarray::Axis(0), &train_idx);
    let train_y: Vec<usize> = train_idx.iter().map(|&i| y[i]).collect();

    // 4. 训练随机森林
    on_progress(1.0, "训练随机森林...");
    let params = ForestParams {
        n_trees: config.n_trees.max(1),
        max_depth: config.max_depth.clamp(1, crate::core::classify::forest::MAX_DEPTH_CAP),
        ..Default::default()
    };
    let forest = Forest::train(&train_x, &train_y, &params, config.seed, Some(labels.len()));

    // 5. 测试集评估
    let mut metrics = Vec::with_capacity(labels.len());
    for (label_idx, label) in labels.iter().enumerate() {
        let mut tp = 0usize;
        let mut fp = 0usize;
        let mut fn_ = 0usize;
        for &i in &indices {
            if !test_set.contains(&i) {
                continue;
            }
            let feat: Vec<f64> = (0..dim).map(|j| x[[i, j]]).collect();
            let (pred, _) = forest.predict(&feat);
            let actual = y[i];
            if pred == label_idx && actual == label_idx {
                tp += 1;
            } else if pred == label_idx && actual != label_idx {
                fp += 1;
            } else if pred != label_idx && actual == label_idx {
                fn_ += 1;
            }
        }
        let precision = if tp + fp > 0 {
            tp as f64 / (tp + fp) as f64
        } else {
            0.0
        };
        let recall = if tp + fn_ > 0 {
            tp as f64 / (tp + fn_) as f64
        } else {
            0.0
        };
        let f1 = if precision + recall > 0.0 {
            2.0 * precision * recall / (precision + recall)
        } else {
            0.0
        };
        metrics.push(LabelMetrics {
            label: label.clone(),
            precision,
            recall,
            f1,
        });
    }

    let model = ModelFile {
        version: MODEL_VERSION,
        feature_dim: dim,
        labels: labels.clone(),
        scaler_mean: mean,
        scaler_std: std,
        window_sec: config.window_sec,
        hop_sec: config.hop_sec,
        forest,
    };
    save_model(&model, output)?;

    Ok(TrainReport {
        samples: n_samples,
        train_samples: train_idx.len(),
        test_samples: n_test,
        labels,
        metrics,
    })
}

/// 把训练结果整理成多行文本（供界面或终端展示）
pub fn format_report(report: &TrainReport) -> String {
    let mut text = format!(
        "样本: {} 窗口（训练 {} / 测试 {}）\n标签: {}",
        report.samples,
        report.train_samples,
        report.test_samples,
        report.labels.join(", ")
    );
    for m in &report.metrics {
        text.push_str(&format!(
            "\n{}  精确率 {:.2}  召回率 {:.2}  F1 {:.2}",
            m.label, m.precision, m.recall, m.f1
        ));
    }
    text
}

/// 解码任意格式为 22050Hz 单声道采样
fn decode_samples(path: &Path) -> Result<Vec<f64>> {
    let is_wav = path
        .extension()
        .map(|e| e.eq_ignore_ascii_case("wav"))
        .unwrap_or(false);
    if is_wav {
        return read_wav_samples(path);
    }
    let wav_path = cache::wav_path(path);
    if !cache::is_fresh(path, &wav_path) {
        ffmpeg::audio_to_wav(path, &wav_path)?;
    }
    read_wav_samples(&wav_path)
}

/// 按样本的多段标注截取对应部分
fn sample_slice<'a>(raw: &'a [f64], split: Option<(usize, usize)>) -> &'a [f64] {
    let Some((index, total)) = split else {
        return raw;
    };
    let total = total.max(1);
    let seg_len = raw.len() / total;
    if seg_len == 0 {
        return raw;
    }
    let start = index.min(total - 1) * seg_len;
    let end = if index + 1 == total {
        raw.len()
    } else {
        (index + 1) * seg_len
    }
    .min(raw.len());
    &raw[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_slice_splits_evenly() {
        let raw: Vec<f64> = (0..100).map(|i| i as f64).collect();
        let first = sample_slice(&raw, Some((0, 2)));
        let second = sample_slice(&raw, Some((1, 2)));
        assert_eq!(first.len(), 50);
        assert_eq!(second.len(), 50);
        assert_eq!(first[0], 0.0);
        assert_eq!(second[0], 50.0);
    }

    #[test]
    fn test_sample_slice_whole_file() {
        let raw: Vec<f64> = vec![1.0; 10];
        assert_eq!(sample_slice(&raw, None).len(), 10);
    }

    #[test]
    fn test_train_requires_two_labels() {
        let samples = vec![TrainSample::whole(PathBuf::from("a.mp3"), "广告")];
        let err = train_model(
            &samples,
            &TrainConfig::default(),
            Path::new("model.mcrf"),
            |_, _| {},
        )
        .unwrap_err();
        assert!(err.to_string().contains("两种标签"));
    }
}
