use anyhow::{bail, Result};
use ndarray::Array2;
use std::sync::OnceLock;
use std::f64::consts::PI;

pub const SEG_SR: u32 = 22050;
pub const SEG_N_FFT: usize = 2048;
pub const SEG_HOP: usize = 1024;
pub const SEG_N_MELS: usize = 40;
pub const SEG_N_MFCC: usize = 13;
pub const SEG_CONTRAST_BANDS: usize = 6;
pub const SEG_FEATURE_DIM: usize = 128;
const FREQ_BINS: usize = SEG_N_FFT / 2 + 1;

fn hann_window() -> &'static [f64] {
    static HANN: OnceLock<Vec<f64>> = OnceLock::new();
    HANN.get_or_init(|| {
        let n = SEG_N_FFT;
        (0..n).map(|i| 0.5 * (1.0 - (2.0 * PI * i as f64 / (n - 1) as f64).cos())).collect()
    })
}

fn mel_filterbank() -> &'static Array2<f64> {
    static FB: OnceLock<Array2<f64>> = OnceLock::new();
    FB.get_or_init(|| build_mel_filterbank(FREQ_BINS, SEG_N_MELS, SEG_SR))
}

fn build_mel_filterbank(n_freq: usize, n_mels: usize, sr: u32) -> Array2<f64> {
    let fmax = sr as f64 / 2.0;
    let mel_min = hz_to_mel(0.0);
    let mel_max = hz_to_mel(fmax);
    let mel_points: Vec<f64> = (0..=n_mels + 1)
        .map(|i| mel_min + (mel_max - mel_min) * i as f64 / (n_mels + 1) as f64)
        .collect();
    let hz_points: Vec<f64> = mel_points.iter().map(|&m| mel_to_hz(m)).collect();
    let bin_points: Vec<usize> = hz_points
        .iter()
        .map(|&f| ((SEG_N_FFT + 1) as f64 * f / sr as f64) as usize)
        .collect();
    let mut fb = Array2::zeros((n_mels, n_freq));
    for m in 0..n_mels {
        let f_left = bin_points[m];
        let f_center = bin_points[m + 1];
        let f_right = bin_points[m + 2];
        for k in f_left..f_center {
            if f_center > f_left { fb[[m, k]] = (k - f_left) as f64 / (f_center - f_left) as f64; }
        }
        for k in f_center..f_right.min(n_freq) {
            if f_right > f_center { fb[[m, k]] = (f_right - k) as f64 / (f_right - f_center) as f64; }
        }
    }
    fb
}

pub(crate) fn hz_to_mel(hz: f64) -> f64 { 2595.0 * (1.0 + hz / 700.0).log10() }
pub(crate) fn mel_to_hz(mel: f64) -> f64 { 700.0 * (10.0f64.powf(mel / 2595.0) - 1.0) }

fn stft_power(samples: &[f64]) -> Array2<f64> {
    use rustfft::{num_complex::Complex64, FftPlanner};
    let window = hann_window();
    let n_fft = SEG_N_FFT; let hop = SEG_HOP;
    let n_frames = if samples.len() < n_fft { 1 } else { (samples.len() - n_fft) / hop + 1 };
    let bins = n_fft / 2 + 1;
    let mut result = Array2::<f64>::zeros((n_frames, bins));
    let mut planner = FftPlanner::<f64>::new();
    let mut fft = planner.plan_fft_forward(n_fft);
    let mut buf = vec![Complex64::default(); n_fft];
    for fi in 0..n_frames {
        let start = fi * hop;
        for k in 0..n_fft { let s = samples.get(start + k).copied().unwrap_or(0.0); buf[k] = Complex64::new(s * window[k], 0.0); }
        fft.process(&mut buf);
        for k in 0..bins { let v = buf[k]; result[[fi, k]] = v.re * v.re + v.im * v.im; }
    }
    result
}

pub fn extract_segment_features(samples: &[f64]) -> Result<[f64; SEG_FEATURE_DIM]> {
    if samples.is_empty() { bail!("空音频片段"); }
    if samples.len() < SEG_N_FFT { bail!("音频过短"); }
    let power = stft_power(samples);
    let n_frames = power.nrows(); let n_freq = power.ncols();
    let fb = mel_filterbank();
    let mut mel = Array2::<f64>::zeros((n_frames, SEG_N_MELS));
    for t in 0..n_frames { for m in 0..SEG_N_MELS { let mut s=0.0; for k in 0..n_freq { s += fb[[m,k]]*power[[t,k]]; } mel[[t,m]] = (s.max(1e-12)).ln(); } }
    let mut mfcc = Array2::<f64>::zeros((n_frames, SEG_N_MFCC));
    for t in 0..n_frames { for i in 0..SEG_N_MFCC { let mut s=0.0; for j in 0..SEG_N_MELS { s += mel[[t,j]]*(PI*i as f64*(2*j+1) as f64/(2*SEG_N_MELS) as f64).cos(); } mfcc[[t,i]] = s; } }
    let n_bands = SEG_CONTRAST_BANDS + 1;
    let freq_bands = contrast_band_edges(n_freq, SEG_SR, SEG_CONTRAST_BANDS);
    let mut contrast = Array2::<f64>::zeros((n_frames, n_bands));
    for t in 0..n_frames { for b in 0..n_bands { let fl=freq_bands[b]; let fh=freq_bands[b+1]; let mut vals:Vec<f64>=(fl..fh).map(|k|(power[[t,k]]+1e-12).log10()).collect(); if vals.is_empty(){continue;} vals.sort_by(|a,b|a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)); let idx=((vals.len() as f64*0.02) as usize).min(vals.len()-1); contrast[[t,b]] = vals[vals.len()-1]-vals[idx]; } }
    let mut zcr=vec![0.0f64;n_frames]; let mut rms=vec![0.0f64;n_frames]; let mut centroid=vec![0.0f64;n_frames]; let mut bandwidth=vec![0.0f64;n_frames];
    for t in 0..n_frames {
        let start=t*SEG_HOP; let end=(start+SEG_N_FFT).min(samples.len()); let seg=&samples[start..end];
        let mut zc=0usize; for i in 1..seg.len(){ if (seg[i]>=0.0)!=(seg[i-1]>=0.0){zc+=1;} }
        zcr[t]=zc as f64/seg.len() as f64;
        let sum_sq:f64=seg.iter().map(|s|s*s).sum(); rms[t]=(sum_sq/seg.len() as f64).sqrt();
        let mut num=0.0f64; let mut den=0.0f64;
        for k in 0..n_freq { let f=k as f64*SEG_SR as f64/SEG_N_FFT as f64; let p=power[[t,k]]; num+=f*p; den+=p; }
        if den>0.0 { centroid[t]=num/den; let mut bw=0.0f64; for k in 0..n_freq { let f=k as f64*SEG_SR as f64/SEG_N_FFT as f64; let p=power[[t,k]]; bw += (f-centroid[t]).abs()*p; } bandwidth[t]=bw/den; }
    }
    let mut out=[0.0f64;SEG_FEATURE_DIM]; let mut idx=0usize;
    for c in 0..SEG_N_MFCC { let(m,s)=mean_std_col(&mfcc,c); out[idx]=m; idx+=1; out[idx]=s; idx+=1; }
    for m in 0..SEG_N_MELS { let(a,b)=mean_std_col(&mel,m); out[idx]=a; idx+=1; out[idx]=b; idx+=1; }
    for b in 0..n_bands { let(a,c)=mean_std_col(&contrast,b); out[idx]=a; idx+=1; out[idx]=c; idx+=1; }
    let(a,b)=mean_std_slice(&zcr); out[idx]=a; idx+=1; out[idx]=b; idx+=1;
    let(a,b)=mean_std_slice(&rms); out[idx]=a; idx+=1; out[idx]=b; idx+=1;
    let(a,b)=mean_std_slice(&centroid); out[idx]=a; idx+=1; out[idx]=b; idx+=1;
    let(a,b)=mean_std_slice(&bandwidth); out[idx]=a; idx+=1; out[idx]=b; idx+=1;
    Ok(out)
}

fn contrast_band_edges(n_freq: usize, sr: u32, n_bands: usize) -> Vec<usize> {
    let fmax = sr as f64 / 2.0;
    let edges_hz: Vec<f64> = (0..=n_bands).map(|i| fmax * (i as f64 / n_bands as f64)).collect();
    let mut edges = vec![0usize];
    for &f in &edges_hz { let bin = ((SEG_N_FFT + 1) as f64 * f / sr as f64) as usize; edges.push(bin.min(n_freq)); }
    edges
}

fn mean_std_col(arr: &Array2<f64>, col: usize) -> (f64, f64) {
    let n = arr.nrows();
    let mean: f64 = (0..n).map(|t| arr[[t, col]]).sum::<f64>() / n as f64;
    let vsum: f64 = (0..n).map(|t| (arr[[t, col]] - mean).powi(2)).sum::<f64>();
    (mean, (vsum as f64 / n as f64).sqrt())
}

fn mean_std_slice(s: &[f64]) -> (f64, f64) {
    if s.is_empty() { return (0.0, 0.0); }
    let n = s.len() as f64;
    let mean = s.iter().sum::<f64>() / n;
    let vsum = s.iter().map(|x| (x - mean).powi(2)).sum::<f64>();
    (mean, (vsum / n).sqrt())
}
