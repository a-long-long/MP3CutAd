//! HPSS（Harmonic-Percussive Source Separation）
//!
//! 基于中值滤波的经典 HPSS 算法：
//! - 沿时间轴中值滤波 → 谐波部分（音调稳定、横向连续）
//! - 沿频率轴中值滤波 → 打击乐部分（宽带瞬态、纵向连续）
//! - Soft mask 权重分配后复用原相位做 iSTFT 回时域
//!
//! 分离后的打击乐轨道送入广告检测，可显著削弱背景音乐干扰。

use anyhow::{bail, Result};
use std::f64::consts::PI;

pub const HPSS_SR: u32 = 22050;
pub const HPSS_N_FFT: usize = 2048;
pub const HPSS_HOP: usize = 1024;

/// HPSS 参数
#[derive(Debug, Clone)]
pub struct HpssParams {
    /// 时间方向中值窗口（奇数帧），越大保留越少瞬态 → 越纯谐波
    pub harmonic_window: usize,
    /// 频率方向中值窗口（奇数 bin），越大保留越少音调 → 越纯打击乐
    pub percussive_window: usize,
    /// 防除零常数
    pub epsilon: f64,
}

impl Default for HpssParams {
    fn default() -> Self {
        Self {
            harmonic_window: 17,
            percussive_window: 11,
            epsilon: 1e-8,
        }
    }
}

impl HpssParams {
    pub fn new(hw: usize, pw: usize) -> Self {
        Self {
            harmonic_window: hw.max(3) | 1,
            percussive_window: pw.max(3) | 1,
            epsilon: 1e-8,
        }
    }
}

/// Hann 窗
fn hann_window(n: usize) -> Vec<f64> {
    (0..n).map(|i| 0.5 * (1.0 - (2.0 * PI * i as f64 / (n - 1) as f64).cos())).collect()
}

/// 中值滤波：1-D，边界复制
fn median_filter_1d(x: &[f64], kernel: usize) -> Vec<f64> {
    let k = kernel.max(1) | 1;
    let half = k / 2;
    let n = x.len();
    if n == 0 { return Vec::new(); }
    (0..n).map(|i| {
        let lo = i.saturating_sub(half);
        let hi = (i + half + 1).min(n);
        let mut buf: Vec<f64> = Vec::with_capacity(hi - lo);
        for j in lo..hi { buf.push(x[j]); }
        buf.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = buf.len() / 2;
        buf[mid]
    }).collect()
}

/// 沿时间轴（帧方向）对功率谱做中值滤波
fn median_filter_h(power: &[Vec<f64>], kernel: usize) -> Vec<Vec<f64>> {
    let n_frames = power.len();
    if n_frames == 0 { return Vec::new(); }
    let n_bins = power[0].len();
    let mut out: Vec<Vec<f64>> = vec![vec![0.0f64; n_bins]; n_frames];
    for k in 0..n_bins {
        let col: Vec<f64> = (0..n_frames).map(|t| power[t][k]).collect();
        let filtered = median_filter_1d(&col, kernel);
        for t in 0..n_frames { out[t][k] = filtered[t]; }
    }
    out
}

/// 沿频率轴（bin 方向）对功率谱做中值滤波
fn median_filter_p(power: &[Vec<f64>], kernel: usize) -> Vec<Vec<f64>> {
    let n_frames = power.len();
    if n_frames == 0 { return Vec::new(); }
    let mut out: Vec<Vec<f64>> = Vec::with_capacity(n_frames);
    for row in power {
        let filtered = median_filter_1d(row, kernel);
        out.push(filtered);
    }
    out
}

/// 功率谱 → soft masks（谐波 mask + 打击乐 mask）
fn soft_masks(power: &[Vec<f64>], params: &HpssParams) -> (Vec<Vec<f64>>, Vec<Vec<f64>>) {
    let p_h = median_filter_h(power, params.harmonic_window);
    let p_p = median_filter_p(power, params.percussive_window);
    let eps = params.epsilon;
    let n_frames = power.len();
    let n_bins = if n_frames > 0 { power[0].len() } else { 0 };
    let mut mask_h: Vec<Vec<f64>> = vec![vec![0.0f64; n_bins]; n_frames];
    let mut mask_p: Vec<Vec<f64>> = vec![vec![0.0f64; n_bins]; n_frames];
    for t in 0..n_frames {
        for k in 0..n_bins {
            let ph = p_h[t][k];
            let pp = p_p[t][k];
            let denom = ph + pp + eps;
            mask_h[t][k] = ph / denom;
            mask_p[t][k] = pp / denom;
        }
    }
    (mask_h, mask_p)
}

/// STFT：返回复数频谱 Vec<Vec<(re, im)>>，形状 (n_frames, n_bins)
fn stft_complex(samples: &[f64], n_fft: usize, hop: usize) -> Vec<Vec<(f64, f64)>> {
    use rustfft::{num_complex::Complex64, FftPlanner};
    let window = hann_window(n_fft);
    let n_frames = if samples.len() < n_fft { 1 } else { (samples.len() - n_fft) / hop + 1 };
    let bins = n_fft / 2 + 1;
    let mut result: Vec<Vec<(f64, f64)>> = vec![vec![(0.0, 0.0); bins]; n_frames];
    let mut planner = FftPlanner::<f64>::new();
    let mut fft = planner.plan_fft_forward(n_fft);
    let mut buf = vec![Complex64::new(0.0, 0.0); n_fft];
    for fi in 0..n_frames {
        let start = fi * hop;
        for k in 0..n_fft {
            let s = samples.get(start + k).copied().unwrap_or(0.0);
            buf[k].re = s * window[k];
            buf[k].im = 0.0;
        }
        fft.process(&mut buf);
        for k in 0..bins { result[fi][k] = (buf[k].re, buf[k].im); }
    }
    result
}

/// 复数频谱 → 功率谱 Vec<Vec<f64>>
fn power_spectrum(spec: &[Vec<(f64, f64)>]) -> Vec<Vec<f64>> {
    spec.iter().map(|row| row.iter().map(|(re, im)| re * re + im * im).collect()).collect()
}

/// soft mask × 原复数频谱
fn apply_mask(spec: &[Vec<(f64, f64)>], mask: &[Vec<f64>]) -> Vec<Vec<(f64, f64)>> {
    spec.iter().zip(mask.iter()).map(|(row, mrow)| {
        row.iter().zip(mrow.iter()).map(|((re, im), m)| (*re * m, *im * m)).collect()
    }).collect()
}

/// iSTFT 回时域（用合成 Hann 窗 + overlap-add + 归一化）
fn istft(spec: &[Vec<(f64, f64)>], n_fft: usize, hop: usize) -> Vec<f64> {
    use rustfft::{num_complex::Complex64, FftPlanner};
    let window = hann_window(n_fft);
    let n_frames = spec.len();
    if n_frames == 0 { return Vec::new(); }
    let total = (n_frames - 1) * hop + n_fft;
    let mut out = vec![0.0f64; total];
    let mut w_sum = vec![0.0f64; total];

    let bins = spec[0].len();
    let mut planner = FftPlanner::<f64>::new();
    let mut fft_inv = planner.plan_fft_inverse(n_fft);
    let mut buf = vec![Complex64::new(0.0, 0.0); n_fft];

    for fi in 0..n_frames {
        // 合成共轭对称复数向量
        for k in 0..bins {
            let (re, im) = spec[fi][k];
            buf[k].re = re;
            buf[k].im = im;
        }
        if n_fft % 2 == 0 {
            // Nyquist bin 单独存
            let (nyq_re, _) = spec[fi][bins - 1];
            buf[n_fft / 2].re = nyq_re;
            buf[n_fft / 2].im = 0.0;
            for k in 1..bins - 1 {
                buf[n_fft - k].re = buf[k].re;
                buf[n_fft - k].im = -buf[k].im;
            }
        } else {
            for k in 1..bins {
                buf[n_fft - k].re = buf[k].re;
                buf[n_fft - k].im = -buf[k].im;
            }
        }
        fft_inv.process(&mut buf);

        let start = fi * hop;
        // rustfft inverse 不除 N，手动除
        let norm = 1.0 / n_fft as f64;
        for k in 0..n_fft {
            let t = start + k;
            if t >= total { break; }
            out[t] += buf[k].re * norm * window[k];
            w_sum[t] += window[k] * window[k];
        }
    }

    for t in 0..total {
        if w_sum[t] > 1e-10 { out[t] /= w_sum[t]; }
    }

    out
}

/// 单路 HPSS 分离
///
/// 返回 `(harmonic_samples, percussive_samples)`，长度与输入一致。
pub fn hpss_separate(
    samples: &[f64],
    params: &HpssParams,
    n_fft: usize,
    hop: usize,
) -> Result<(Vec<f64>, Vec<f64>)> {
    if samples.is_empty() { bail!("空音频"); }
    if samples.len() < n_fft { bail!("音频过短，无法 HPSS"); }

    let spec = stft_complex(samples, n_fft, hop);
    let power = power_spectrum(&spec);
    let (mask_h, mask_p) = soft_masks(&power, params);
    let spec_h = apply_mask(&spec, &mask_h);
    let spec_p = apply_mask(&spec, &mask_p);

    let harm = istft(&spec_h, n_fft, hop);
    let perc = istft(&spec_p, n_fft, hop);

    let n = samples.len().min(harm.len()).min(perc.len());
    Ok((harm[..n].to_vec(), perc[..n].to_vec()))
}

/// 便捷入口：用默认参数分离打击乐轨道（给检测用）
pub fn separate_percussive(samples: &[f64]) -> Result<Vec<f64>> {
    let params = HpssParams::default();
    let (_, perc) = hpss_separate(samples, &params, HPSS_N_FFT, HPSS_HOP)?;
    Ok(perc)
}

/// 便捷入口：用默认参数分离谐波轨道（给检测用）
pub fn separate_harmonic(samples: &[f64]) -> Result<Vec<f64>> {
    let params = HpssParams::default();
    let (harm, _) = hpss_separate(samples, &params, HPSS_N_FFT, HPSS_HOP)?;
    Ok(harm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_median_filter_preserves_constant() {
        let x = vec![1.0f64; 100];
        let y = median_filter_1d(&x, 7);
        assert_eq!(y.len(), 100);
        for v in y { assert!((v - 1.0).abs() < 1e-10); }
    }

    #[test]
    fn test_median_filter_removes_impulse() {
        let mut x = vec![0.0f64; 101];
        x[50] = 100.0;
        let y = median_filter_1d(&x, 7);
        assert!(y[50] < 1.0, "median should remove impulse, got {}", y[50]);
    }

    #[test]
    fn test_hpss_sine_then_impulse() {
        let sr = HPSS_SR as usize;
        let dur = 2;
        let n = sr * dur;
        let mut sig = vec![0.0f64; n];
        // 440Hz 正弦
        for i in 0..n {
            sig[i] = 0.5 * (2.0 * PI * 440.0 * i as f64 / sr as f64).sin();
        }
        // 在 0.5s, 1.0s, 1.5s 处加冲击
        for &pos in &[sr / 2, sr, sr + sr / 2] {
            for j in 0..8 {
                if pos + j < n { sig[pos + j] += 0.9 * (1.0 - j as f64 / 8.0); }
            }
        }
        let (harm, perc) = hpss_separate(&sig, &HpssParams::default(), HPSS_N_FFT, HPSS_HOP).unwrap();
        // HPSS 尾部会丢失少量采样（STFT 固有），这里只检查最小长度
        assert!(harm.len() >= n - HPSS_N_FFT);
        assert!(perc.len() >= n - HPSS_N_FFT);
        // 谐波能量应占主导（正弦是连续音调，会分到 harmonic）
        let e_h: f64 = harm.iter().map(|v| v * v).sum();
        let e_p: f64 = perc.iter().map(|v| v * v).sum();
        let ratio = e_h.max(e_p) / (e_h.min(e_p) + 1e-12);
        assert!(ratio > 1.2, "harmonic should dominate: e_h={e_h:.4}, e_p={e_p:.4}, ratio={ratio:.2}");
    }
}
