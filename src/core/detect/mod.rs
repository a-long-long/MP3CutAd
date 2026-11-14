//! 广告检测核心算法
//!
//! 集成多项优化：
//! - 自适应阈值（基于统计动态调整）
//! - 多探针 LSH（提高召回率）
//! - 多特征融合（FFT + MFCC + Chroma）
//! - Viterbi 时序一致性过滤
//!
//! 子模块：
//! - [`scan`]：自适应阈值与 LSH 候选扫描（含 GPU 批量打分）
//! - [`refine`]：Viterbi 过滤、多特征精细调整与边界精修
//! - [`merge`]：区间合并、类型计算与补集

pub mod merge;
pub mod refine;
pub mod scan;

pub use merge::reverse_range;

use anyhow::Result;
use ndarray::Array2;
use rayon::prelude::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::audio::{cache, ffmpeg};
use crate::core::dsp::features::{self, AudioFeatures};
use crate::core::dsp::fft::{self, FFT_LEN};
use crate::core::dsp::separation;
use crate::core::matching::lsh;
use crate::core::range::{Link, Range};

use merge::{calc_range_types, combine_to_ranges, compress_range};
use refine::{fine_tune_multi, viterbi_filter_candidates};
use scan::check_same_adaptive;

/// 检测选项
#[derive(Debug, Clone, Default)]
pub struct DetectOptions {
    /// 是否启用 HPSS 源分离（分离打击乐轨道后再做检测）
    /// 对带背景音乐的广告段，打击乐轨道匹配更稳定。
    pub enable_hpss: bool,
}

impl DetectOptions {
    pub fn new() -> Self { Self::default() }
    pub fn with_hpss(mut self) -> Self { self.enable_hpss = true; self }
}

/// 哈希窗口大小（帧数）
pub(super) const HASH_WINDOW_SIZE: usize = 20;

/// 检测结果
#[derive(Debug)]
pub struct DetectionResult {
    /// 每个文件的广告区间
    pub ranges: Vec<Vec<Range>>,
    /// 每个文件的总帧数
    pub lengths: Vec<usize>,
    /// 实际使用的 GPU 后端名称（未使用 GPU 时为 None）
    pub gpu: Option<String>,
}

/// 检测一系列 MP3 文件中的广告
///
/// # 参数
/// - `mp3_files`: MP3 文件路径列表
///
/// # 返回
/// 每个文件的广告区间和总长度
/// 检测一系列 MP3 文件中的广告（默认选项）
pub fn detect_ad(mp3_files: &[PathBuf]) -> Result<DetectionResult> {
    detect_ad_with_progress(mp3_files, DetectOptions::default(), |_, _| {})
}

/// 带选项和进度回调的广告检测
pub fn detect_ad_with_progress<F>(
    mp3_files: &[PathBuf],
    opts: DetectOptions,
    on_progress: F,
) -> Result<DetectionResult>
where
    F: Fn(f32, &str) + Sync,
{
    let num_files = mp3_files.len();
    if num_files == 0 {
        return Ok(DetectionResult {
            ranges: Vec::new(),
            lengths: Vec::new(),
            gpu: None,
        });
    }

    lsh::init(HASH_WINDOW_SIZE, FFT_LEN / 2);

    let mut ffts: Vec<Array2<f64>> = Vec::with_capacity(num_files);
    let mut feats: Vec<AudioFeatures> = Vec::with_capacity(num_files);
    let mut hashes: Vec<Vec<i32>> = Vec::with_capacity(num_files);

    // 进度步数：每个文件的预处理 + 每对文件的比较
    let pairs: Vec<(usize, usize)> = (0..num_files)
        .flat_map(|i| (0..i).map(move |j| (j, i)))
        .collect();
    let total_steps = (num_files + pairs.len()).max(1);
    let done = AtomicUsize::new(0);
    // 预处理失败的文件（不中止整批，仅跳过并记录）
    let mut failed: Vec<String> = Vec::new();

    // 1. 处理每个文件：MP3 → WAV → (HPSS 可选) → FFT → Features → Hash
    for (idx, mp3_path) in mp3_files.iter().enumerate() {
        let file_name = mp3_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        // 预处理单个文件；任一步失败只跳过该文件，不中止整批
        let processed: Result<(Array2<f64>, Vec<i32>)> = (|| {
            Ok(if opts.enable_hpss {
                // HPSS 路径：绕过 FFT/Hash 缓存，每次重新计算
                let wav_path = cache::wav_path(mp3_path);
                if !cache::is_fresh(mp3_path, &wav_path) {
                    ffmpeg::audio_to_wav(mp3_path, &wav_path)?;
                }

                on_progress(
                    idx as f32 / num_files as f32,
                    &format!("HPSS 分离中 {}/{}: {}", idx + 1, num_files, file_name),
                );

                // 读原始采样 → HPSS 打击乐 → 写临时 WAV
                let raw = fft::read_wav_samples(&wav_path)?;
                let perc = separation::separate_percussive(&raw)?;
                // 文件名加 pid + 时间戳 + 随机，避免多进程/并发运行冲突，崩溃后残留也不会互相覆盖
                let tmp_wav = std::env::temp_dir().join(format!(
                    "mp3cutad_hpss_{}_{}_{}_{}.wav",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis())
                        .unwrap_or(0),
                    idx,
                    rand::random::<u32>()
                ));
                write_f64_wav(&tmp_wav, &perc)?;

                let fft = fft::process_wav(&tmp_wav)?;
                let hash = lsh::hash_all(&fft, HASH_WINDOW_SIZE);
                let _ = std::fs::remove_file(&wav_path);
                let _ = std::fs::remove_file(&tmp_wav);
                (fft, hash)
            } else {
                // 常规路径：使用缓存
                let fft_data = match cache::read_fft(mp3_path) {
                    Some(data) => data,
                    None => {
                        let wav_path = cache::wav_path(mp3_path);
                        if !cache::is_fresh(mp3_path, &wav_path) {
                            ffmpeg::audio_to_wav(mp3_path, &wav_path)?;
                        }
                        let data = fft::process_wav(&wav_path)?;
                        cache::save_fft(mp3_path, &data)?;
                        let _ = std::fs::remove_file(&wav_path);
                        data
                    }
                };
                let expected_hash_len = fft_data.nrows().saturating_sub(HASH_WINDOW_SIZE);
                let hash_data = match cache::read_hash(mp3_path) {
                    Some(h) if h.len() == expected_hash_len => h,
                    _ => {
                        let h = lsh::hash_all(&fft_data, HASH_WINDOW_SIZE);
                        cache::save_hash(mp3_path, &h)?;
                        h
                    }
                };
                (fft_data, hash_data)
            })
        })();

        let (fft_data, hash_data) = match processed {
            Ok(v) => v,
            Err(e) => {
                failed.push(format!("{}: {}", file_name, e));
                // 占位空数据，保证 ffts/feats/hashes 与文件索引对齐
                (Array2::zeros((0, fft::FREQ_BINS)), Vec::new())
            }
        };

        let feature_data = features::extract_features(&fft_data);

        ffts.push(fft_data);
        feats.push(feature_data);
        hashes.push(hash_data);

        let step = done.fetch_add(1, Ordering::Relaxed) + 1;
        let file_name = mp3_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        on_progress(
            step as f32 / total_steps as f32,
            &format!("预处理音频 {}/{}: {}", idx + 1, num_files, file_name),
        );
    }

    // 2. 两两比较，查找相似片段
    let links: Arc<Mutex<Vec<Link>>> = Arc::new(Mutex::new(Vec::new()));
    let ranges: Arc<Mutex<Vec<Vec<Range>>>> = Arc::new(Mutex::new(vec![Vec::new(); num_files]));
    let gpu_used = std::sync::atomic::AtomicBool::new(false);

    pairs.par_iter().for_each(|&(i, j)| {
        let ranges_i = Arc::new(Mutex::new(Vec::new()));
        let ranges_j = Arc::new(Mutex::new(Vec::new()));

        // 2.1 自适应阈值 + 多探针 LSH（可选的 GPU 批量相似度）
        let raw_candidates =
            check_same_adaptive(&ffts[i], &ffts[j], &hashes[i], &hashes[j], &gpu_used);

        // 2.2 Viterbi 时序一致性过滤
        let filtered_candidates = viterbi_filter_candidates(&raw_candidates);

        // 2.3 多特征融合精细调整
        let pair_links = fine_tune_multi(
            &feats[i],
            &feats[j],
            &mut ranges_i.lock().unwrap(),
            &mut ranges_j.lock().unwrap(),
            &filtered_candidates,
            i,
            j,
        );

        {
            let mut links_guard = links.lock().unwrap();
            for link in pair_links {
                links_guard.push(link);
            }
        }

        let ri = ranges_i.lock().unwrap().clone();
        let rj = ranges_j.lock().unwrap().clone();

        let mut ranges_guard = ranges.lock().unwrap();
        ranges_guard[i] = combine_to_ranges(&ranges_guard[i], &ri);
        ranges_guard[j] = combine_to_ranges(&ranges_guard[j], &rj);
        drop(ranges_guard);

        let step = done.fetch_add(1, Ordering::Relaxed) + 1;
        on_progress(
            step as f32 / total_steps as f32,
            &format!("比对音频片段 {}/{}", step, total_steps),
        );
    });

    // 3. 计算区间类型
    let links = Arc::try_unwrap(links).unwrap().into_inner().unwrap();
    let mut ranges = Arc::try_unwrap(ranges).unwrap().into_inner().unwrap();
    calc_range_types(&mut ranges, &links);

    // 4. 压缩最终结果
    for r in &mut ranges {
        *r = compress_range(r);
    }

    let lengths: Vec<usize> = ffts.iter().map(|f| f.nrows()).collect();

    if !failed.is_empty() {
        eprintln!("警告：{} 个文件预处理失败，已跳过（不影响其余文件）：", failed.len());
        for f in &failed {
            eprintln!("  - {}", f);
        }
    }

    Ok(DetectionResult {
        ranges,
        lengths,
        gpu: if gpu_used.load(Ordering::Relaxed) {
            crate::core::matching::gpu::backend_name()
        } else {
            None
        },
    })
}

/// 将 f64 采样（已归一化）写为单声道 int16 WAV（22050Hz）
fn write_f64_wav(path: &std::path::Path, samples: &[f64]) -> Result<()> {
    use hound::{SampleFormat, WavSpec, WavWriter};
    let spec = WavSpec {
        channels: 1,
        sample_rate: 22050,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut w = WavWriter::create(path, spec)?;
    for &s in samples {
        let clipped = s.clamp(-1.0, 1.0);
        let v = (clipped * 32767.0) as i16;
        w.write_sample(v)?;
    }
    w.finalize()?;
    Ok(())
}
