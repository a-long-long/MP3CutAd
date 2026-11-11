
use anyhow::{Context, Result};
use std::path::Path;
use crate::audio::{cache, ffmpeg};
use crate::core::fft::read_wav_samples;
use crate::core::model::ModelFile;
use crate::core::segfeat::{extract_segment_features, SEG_SR};

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
    let mut sm = Vec::with_capacity(fp.len());
    for i in 0..fp.len() {
        let lo = i.saturating_sub(k / 2);
        let hi = (i + k / 2 + 1).min(fp.len());
        let mut cnt = std::collections::HashMap::new();
        for j in lo..hi { if let Some((cls, _)) = &fp[j] { *cnt.entry(*cls).or_insert(0usize) += 1; } }
        let total = hi - lo;
        let mut bc = None; let mut bn = 0;
        for (cls, n) in &cnt { if *n > bn { bn = *n; bc = Some(*cls); } }
        if let Some(cls) = bc {
            if bn * 2 > total {
                sm.push(Some((cls, fp[i].map(|(_, c)| c).unwrap_or(0.0))));
            } else { sm.push(None); }
        } else { sm.push(None); }
    }
    let to01 = |wi: usize| -> i32 {
        let c = wi as f64 * hs as f64 + ws as f64 / 2.0;
        (c / (SEG_SR as f64 / 10.0)) as i32
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
                            merged.push((c, to01(sw), to01(ew) + model.hop_sec as i32 * 10, avg));
                        }
                        cur = Some((*cls, i, i, vec![*conf]));
                    }
                }
            }
            None => {
                if let Some((c, sw, ew, confs)) = cur.take() {
                    let avg = confs.iter().sum::<f64>() / confs.len() as f64;
                    merged.push((c, to01(sw), to01(ew) + model.hop_sec as i32 * 10, avg));
                }
            }
        }
    }
    if let Some((c, sw, ew, confs)) = cur.take() {
        let avg = confs.iter().sum::<f64>() / confs.len() as f64;
        merged.push((c, to01(sw), to01(ew) + model.hop_sec as i32 * 10, avg));
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
