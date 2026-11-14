//! 多特征提取模块
//!
//! 从 FFT 频谱图提取 MFCC 和 Chroma（色度）特征，
//! 与原始频谱特征融合以提高广告识别准确率。
//!
//! 频率参数直接复用 [`crate::core::dsp::fft`] 的常量，避免与实际频谱图不一致。

use ndarray::Array2;
use std::f64::consts::PI;

use crate::core::dsp::fft::{FFT_LEN, FREQ_BINS, SAMPLE_RATE};

/// FFT 窗口大小（与频谱图生成一致）
const N_FFT: usize = FFT_LEN;
/// MFCC 系数数量
const N_MFCC: usize = 13;
/// Mel 滤波器数量
const N_MELS: usize = 26;
/// Mel 频带数量（供界面可视化使用）
pub const MEL_BANDS: usize = N_MELS;
/// 色度特征 bin 数量（12 个音级）
const N_CHROMA: usize = 12;

/// 多特征集合
pub struct AudioFeatures {
    /// 原始 FFT 频谱图 (n_frames, FREQ_BINS)
    pub fft: Array2<f64>,
    /// MFCC 特征 (n_frames, N_MFCC)
    pub mfcc: Array2<f64>,
    /// Chroma 色度特征 (n_frames, N_CHROMA)
    pub chroma: Array2<f64>,
}

/// 从 FFT 频谱图提取全部特征
pub fn extract_features(fft: &Array2<f64>) -> AudioFeatures {
    let mfcc = compute_mfcc_features(fft);
    let chroma = compute_chroma_features(fft);
    AudioFeatures {
        fft: fft.clone(),
        mfcc,
        chroma,
    }
}

/// 赫兹转 Mel 刻度
#[inline]
fn hz_to_mel(f: f64) -> f64 {
    2595.0 * (1.0 + f / 700.0).log10()
}

/// Mel 刻度转赫兹
#[inline]
fn mel_to_hz(m: f64) -> f64 {
    700.0 * (10.0_f64.powf(m / 2595.0) - 1.0)
}

/// 构建 Mel 三角滤波器组
///
/// 返回 (N_MELS, FREQ_BINS) 的矩阵
fn mel_filterbank() -> Array2<f64> {
    let n_freq = FREQ_BINS;
    let sample_rate = SAMPLE_RATE as f64;

    let low_mel = hz_to_mel(0.0);
    let high_mel = hz_to_mel(sample_rate / 2.0);

    // 在 Mel 刻度上均匀分布 N_MELS+2 个点
    let mut mel_points = Vec::with_capacity(N_MELS + 2);
    for i in 0..N_MELS + 2 {
        mel_points.push(low_mel + (high_mel - low_mel) * i as f64 / (N_MELS + 1) as f64);
    }

    // 转回赫兹并映射到 FFT bin
    let bin_points: Vec<usize> = mel_points
        .iter()
        .map(|&m| {
            let hz = mel_to_hz(m);
            ((N_FFT as f64) * hz / sample_rate).floor() as usize
        })
        .collect();

    let mut filterbank = Array2::zeros((N_MELS, n_freq));
    for m in 0..N_MELS {
        let start = bin_points[m];
        let center = bin_points[m + 1];
        let end = bin_points[m + 2];

        for k in start..center.min(n_freq) {
            if center > start {
                filterbank[[m, k]] = (k - start) as f64 / (center - start) as f64;
            }
        }
        for k in center..end.min(n_freq) {
            if end > center {
                filterbank[[m, k]] = (end - k) as f64 / (end - center) as f64;
            }
        }
    }

    filterbank
}

/// 计算 MFCC 特征
///
/// 对每帧 FFT 数据：功率谱 → Mel 滤波 → 对数 → DCT-II → 前 N_MFCC 个系数
fn compute_mfcc_features(fft: &Array2<f64>) -> Array2<f64> {
    let n_frames = fft.nrows();
    let filterbank = mel_filterbank();
    let mut mfcc = Array2::zeros((n_frames, N_MFCC));

    for frame in 0..n_frames {
        let spectrum = fft.row(frame);

        // 功率谱经 Mel 滤波
        let mut mel_energies = vec![0.0f64; N_MELS];
        for m in 0..N_MELS {
            for k in 0..spectrum.len().min(filterbank.ncols()) {
                mel_energies[m] += spectrum[k].powi(2) * filterbank[[m, k]];
            }
            mel_energies[m] = (mel_energies[m] + 1e-10).ln();
        }

        // DCT-II 提取 cepstral 系数
        for i in 0..N_MFCC {
            let mut sum = 0.0;
            for j in 0..N_MELS {
                sum += mel_energies[j] * (PI * i as f64 * (j as f64 + 0.5) / N_MELS as f64).cos();
            }
            mfcc[[frame, i]] = sum;
        }
    }

    mfcc
}

/// 计算对数 Mel 频谱
///
/// 与 MFCC 使用相同的滤波器组，返回 `(帧数, MEL_BANDS)` 的对数能量，
/// 仅用于界面可视化，不参与广告匹配。
pub fn mel_spectrogram(fft: &Array2<f64>) -> Array2<f64> {
    let n_frames = fft.nrows();
    let filterbank = mel_filterbank();
    let mut mel = Array2::zeros((n_frames, N_MELS));

    for frame in 0..n_frames {
        let spectrum = fft.row(frame);
        for m in 0..N_MELS {
            let mut energy = 0.0f64;
            for k in 0..spectrum.len().min(filterbank.ncols()) {
                energy += spectrum[k].powi(2) * filterbank[[m, k]];
            }
            mel[[frame, m]] = (energy + 1e-10).ln();
        }
    }

    mel
}

/// 计算 Chroma 色度特征
///
/// 将每个 FFT bin 映射到 12 个音级（C, C#, D, ..., B），
/// 累加能量后归一化。
fn compute_chroma_features(fft: &Array2<f64>) -> Array2<f64> {
    let n_frames = fft.nrows();
    let n_freq = FREQ_BINS;
    let sample_rate = SAMPLE_RATE as f64;
    let mut chroma = Array2::zeros((n_frames, N_CHROMA));

    // 预计算每个 bin 对应的音级
    let bin_to_class: Vec<Option<usize>> = (0..n_freq)
        .map(|k| {
            let freq = k as f64 * sample_rate / N_FFT as f64;
            if freq < 30.0 || freq > 5000.0 {
                return None;
            }
            let midi = 12.0 * (freq / 440.0).log2() + 69.0;
            let pc = ((midi.round() as i32) % 12 + 12) % 12;
            Some(pc as usize)
        })
        .collect();

    for frame in 0..n_frames {
        let spectrum = fft.row(frame);
        let mut c = [0.0f64; N_CHROMA];

        for (k, class_opt) in bin_to_class.iter().enumerate() {
            if let Some(class) = class_opt {
                if k < spectrum.len() {
                    c[*class] += spectrum[k].powi(2);
                }
            }
        }

        // L2 归一化
        let norm: f64 = c.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm > 1e-10 {
            for (i, val) in c.iter().enumerate() {
                chroma[[frame, i]] = val / norm;
            }
        }
    }

    chroma
}

/// 多特征加权余弦相似度
///
/// 分别计算 FFT、MFCC、Chroma 三个特征的余弦相似度，
/// 然后按权重加权求和。
///
/// # 参数
/// - `feat1`, `feat2`: 两个文件的特征集
/// - `offset1`, `offset2`: 各自起始帧
/// - `size`: 比较窗口帧数
/// - `weights`: [fft_weight, mfcc_weight, chroma_weight]，建议 [0.5, 0.3, 0.2]
pub fn multi_feature_sim(
    feat1: &AudioFeatures,
    feat2: &AudioFeatures,
    offset1: usize,
    offset2: usize,
    size: usize,
    weights: &[f64; 3],
) -> f64 {
    let fft_sim = sim_matrix(&feat1.fft, &feat2.fft, offset1, offset2, size);
    let mfcc_sim = sim_matrix(&feat1.mfcc, &feat2.mfcc, offset1, offset2, size);
    let chroma_sim = sim_matrix(&feat1.chroma, &feat2.chroma, offset1, offset2, size);

    weights[0] * fft_sim + weights[1] * mfcc_sim + weights[2] * chroma_sim
}

/// 两个矩阵指定窗口的余弦相似度
fn sim_matrix(
    a: &Array2<f64>,
    b: &Array2<f64>,
    offset_a: usize,
    offset_b: usize,
    size: usize,
) -> f64 {
    if offset_a >= a.nrows() || offset_b >= b.nrows() {
        return 0.0;
    }

    let width = a.ncols().min(b.ncols());
    let actual_size = size.min(a.nrows() - offset_a).min(b.nrows() - offset_b);

    if actual_size == 0 || width == 0 {
        return 0.0;
    }

    let a_slice = a.as_slice().unwrap();
    let b_slice = b.as_slice().unwrap();
    let a_w = a.ncols();
    let b_w = b.ncols();

    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;

    for frame in 0..actual_size {
        let a_off = (offset_a + frame) * a_w;
        let b_off = (offset_b + frame) * b_w;
        for k in 0..width {
            let va = a_slice[a_off + k];
            let vb = b_slice[b_off + k];
            dot += va * vb;
            na += va * va;
            nb += vb * vb;
        }
    }

    let denom = (na * nb).sqrt();
    if denom < 1e-8 {
        0.0
    } else {
        dot / denom
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::Array2;

    #[test]
    fn test_mel_filterbank_shape() {
        let fb = mel_filterbank();
        assert_eq!(fb.nrows(), N_MELS);
        assert_eq!(fb.ncols(), FREQ_BINS);
    }

    #[test]
    fn test_mfcc_shape() {
        let fft = Array2::zeros((100, FREQ_BINS));
        let mfcc = compute_mfcc_features(&fft);
        assert_eq!(mfcc.nrows(), 100);
        assert_eq!(mfcc.ncols(), N_MFCC);
    }

    #[test]
    fn test_mel_spectrogram_shape() {
        let fft = Array2::zeros((100, FREQ_BINS));
        let mel = mel_spectrogram(&fft);
        assert_eq!(mel.nrows(), 100);
        assert_eq!(mel.ncols(), MEL_BANDS);
    }

    #[test]
    fn test_chroma_shape() {
        let fft = Array2::zeros((100, FREQ_BINS));
        let chroma = compute_chroma_features(&fft);
        assert_eq!(chroma.nrows(), 100);
        assert_eq!(chroma.ncols(), N_CHROMA);
    }

    #[test]
    fn test_multi_feature_identical() {
        let mut fft = Array2::zeros((50, FREQ_BINS));
        for i in 0..50 {
            for j in 0..FREQ_BINS {
                fft[[i, j]] = ((i * 100 + j) as f64).sin();
            }
        }
        let feat = extract_features(&fft);
        let sim = multi_feature_sim(&feat, &feat, 0, 0, 20, &[0.5, 0.3, 0.2]);
        assert!((sim - 1.0).abs() < 0.01, "identical sim = {}", sim);
    }
}
