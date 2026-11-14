//! 事件分类：滑窗特征 → 随机森林 → 事件区间
//!
//! 模型文件（`.mcrf`）的读写见 [`model`]，随机森林实现见 [`forest`]。

pub mod forest;
pub mod model;

use anyhow::{bail, Context, Result};
use std::path::Path;
use crate::audio::{cache, ffmpeg};
use crate::core::classify::model::ModelFile;
use crate::core::dsp::fft::read_wav_samples;
use crate::core::dsp::segfeat::{extract_segment_features, SEG_N_FFT, SEG_SR};

#[derive(Debug, Clone)]
pub struct EventRange {
    pub begin: i32,
    pub end: i32,
    pub label: String,
    pub confidence: f64,
}

#[derive(Debug, Clone)]
pub struct ClassifyOptions {
    pub confidence_threshold: f64,
    pub smooth_kernel: usize,
    pub min_event_len: i32,
}

impl Default for ClassifyOptions {
    fn default() -> Self { Self { confidence_threshold: 0.55, smooth_kernel: 3, min_event_len: 10 } }
}

pub fn classify_samples(samples: &[f64], model: &ModelFile, opts: &ClassifyOptions) -> Result<Vec<EventRange>> {
    let ws = (model.window_sec * SEG_SR as f64) as usize;
    let hs = (model.hop_sec * SEG_SR as f64) as usize;
    if ws == 0 {
        bail!("window_sec * 采样率取整为 0（模型 window_sec={}）", model.window_sec);
    }
    // 窗口短于一次 FFT 时特征提取必然失败；提前报错，避免全部帧被静默丢弃
    if ws < SEG_N_FFT {
        bail!(
            "分析窗口过小: {} 个采样点（{:.3}s），至少需要 {} 个（SEG_N_FFT）",
            ws,
            model.window_sec,
            SEG_N_FFT
        );
    }
    if hs == 0 {
        bail!("hop_sec * 采样率取整为 0，无法滑动（模型 hop_sec={}）", model.hop_sec);
    }
    if samples.len() < ws { return Ok(Vec::new()); }
    let nw = (samples.len() - ws) / hs + 1;
    let mut fp: Vec<Option<(usize, f64)>> = Vec::with_capacity(nw);
    for i in 0..nw {
        let seg = &samples[i * hs..i * hs + ws];
        match extract_segment_features(seg) {
            Ok(f) => {
                let s = model.scale(&f);
                let (cls, conf) = model.forest.predict(&s);
                fp.push(if conf >= opts.confidence_threshold { Some((cls, conf)) } else { None });
            }
            Err(_) => fp.push(None),
        }
    }
    let k = opts.smooth_kernel.max(1);
    let half = k / 2;
    let mut sm: Vec<Option<(usize, f64)>> = Vec::with_capacity(fp.len());
    for i in 0..fp.len() {
        let lo = i.saturating_sub(half);
        let hi = (i + half + 1).min(fp.len());
        let mut cnt = std::collections::HashMap::new();
        for j in lo..hi { if let Some((cls, _)) = &fp[j] { *cnt.entry(*cls).or_insert(0usize) += 1; } }
        let total = hi - lo;
        let mut bc = None; let mut bn = 0;
        for (cls, n) in &cnt { if *n > bn { bn = *n; bc = Some(*cls); } }
        match bc {
            Some(cls) if bn * 2 > total => {
                // 置信度取邻域内同标签帧的均值，避免用 0 拉低事件整体置信度
                let mut sum = 0.0;
                let mut count = 0usize;
                for j in lo..hi {
                    if let Some((c, conf)) = fp[j] {
                        if c == cls {
                            sum += conf;
                            count += 1;
                        }
                    }
                }
                sm.push(Some((cls, if count > 0 { sum / count as f64 } else { 0.0 })));
            }
            _ => sm.push(None),
        }
    }

    // 合并同标签之间的短间隙：单帧抖动不应把一个事件切成两段
    let max_gap = k.max(1);
    let mut i = 0;
    while i < sm.len() {
        if sm[i].is_some() {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i;
        while end < sm.len() && sm[end].is_none() {
            end += 1;
        }
        if start > 0 && end < sm.len() && end - start <= max_gap {
            if let (Some((lc, lconf)), Some((rc, rconf))) = (sm[start - 1], sm[end]) {
                if lc == rc {
                    let conf = (lconf + rconf) / 2.0;
                    for slot in &mut sm[start..end] {
                        *slot = Some((lc, conf));
                    }
                }
            }
        }
        i = end;
    }

    let total_01 = (samples.len() as f64 / (SEG_SR as f64 / 10.0)) as i32;
    let hop_01 = (model.hop_sec * 10.0).round() as i32;
    let to01 = |wi: usize| -> i32 {
        let c = wi as f64 * hs as f64 + ws as f64 / 2.0;
        (c / (SEG_SR as f64 / 10.0)) as i32
    };
    // 事件端点按总时长截断，避免尾部窗口外的时间越界
    let push_event = |merged: &mut Vec<(usize, i32, i32, f64)>, cls: usize, sw: usize, ew: usize, conf: f64| {
        let b = to01(sw).clamp(0, total_01);
        let e = (to01(ew) + hop_01).clamp(0, total_01);
        if e > b {
            merged.push((cls, b, e, conf));
        }
    };

    let mut merged: Vec<(usize, i32, i32, f64)> = Vec::new();
    let mut cur: Option<(usize, usize, usize, Vec<f64>)> = None;
    for (i, p) in sm.iter().enumerate() {
        match p {
            Some((cls, conf)) => {
                match &mut cur {
                    Some((c, _, ei, confs)) if *c == *cls => { *ei = i; confs.push(*conf); }
                    _ => {
                        if let Some((c, sw, ew, confs)) = cur.take() {
                            let avg = confs.iter().sum::<f64>() / confs.len() as f64;
                            push_event(&mut merged, c, sw, ew, avg);
                        }
                        cur = Some((*cls, i, i, vec![*conf]));
                    }
                }
            }
            None => {
                if let Some((c, sw, ew, confs)) = cur.take() {
                    let avg = confs.iter().sum::<f64>() / confs.len() as f64;
                    push_event(&mut merged, c, sw, ew, avg);
                }
            }
        }
    }
    if let Some((c, sw, ew, confs)) = cur.take() {
        let avg = confs.iter().sum::<f64>() / confs.len() as f64;
        push_event(&mut merged, c, sw, ew, avg);
    }
    Ok(merged.into_iter().filter(|(_, b, e, _)| e - b >= opts.min_event_len)
        .map(|(cls, b, e, conf)| EventRange { begin: b, end: e, label: model.labels.get(cls).cloned().unwrap_or_else(|| format!("类{}", cls)), confidence: conf })
        .collect())
}

pub fn classify_file_with_progress<F>(input: &Path, model: &ModelFile, opts: &ClassifyOptions, on_progress: F) -> Result<(Vec<EventRange>, usize)>
where F: Fn(f32, &str) + Sync {
    // 已是 WAV 则直通（省掉 ffmpeg 转码；某些精简版 ffmpeg 不认 WAV）
    let is_wav = input.extension().map(|e| e.eq_ignore_ascii_case("wav")).unwrap_or(false);
    let samples = if is_wav {
        read_wav_samples(input).with_context(|| format!("读取 WAV 失败: {}", input.display()))?
    } else {
        let wav_path = cache::wav_path(input);
        if !cache::is_fresh(input, &wav_path) { ffmpeg::audio_to_wav(input, &wav_path).with_context(|| format!("转码失败: {}", input.display()))?; }
        read_wav_samples(&wav_path).with_context(|| format!("读取 WAV 失败: {}", wav_path.display()))?
    };
    on_progress(0.2, "WAV 已加载，开始分类...");
    let events = classify_samples(&samples, model, opts)?;
    let nf = (samples.len() as f64 / (SEG_SR as f64 / 10.0)) as usize;
    on_progress(1.0, "分类完成");
    Ok((events, nf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::classify::forest::{Forest, Node};
    use crate::core::dsp::segfeat::SEG_FEATURE_DIM;

    fn dummy_model(window_sec: f64) -> ModelFile {
        ModelFile {
            version: 1,
            feature_dim: SEG_FEATURE_DIM,
            labels: vec!["广告".to_string()],
            scaler_mean: vec![0.0; SEG_FEATURE_DIM],
            scaler_std: vec![1.0; SEG_FEATURE_DIM],
            window_sec,
            hop_sec: 1.0,
            forest: Forest {
                trees: vec![vec![Node {
                    feature: 0,
                    threshold: 0.0,
                    left: 0,
                    right: 0,
                    proba: vec![1.0],
                }]],
                n_classes: 1,
                feature_dim: SEG_FEATURE_DIM,
            },
        }
    }

    #[test]
    fn classify_rejects_window_shorter_than_fft() {
        // 0.05s * 22050 ≈ 1102 < SEG_N_FFT(2048)，应直接报错而不是静默返回空事件
        let model = dummy_model(0.05);
        let err = classify_samples(&[], &model, &ClassifyOptions::default()).unwrap_err();
        assert!(
            err.to_string().contains("分析窗口过小"),
            "unexpected error: {err}"
        );
    }
}
