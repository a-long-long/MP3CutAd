//! FFT 处理模块
//!
//! 负责将 WAV 文件转换为频谱图

use anyhow::Result;
use hound;
use ndarray::{Array1, Array2};
use num_complex::Complex;
use rustfft::FftPlanner;
use std::path::Path;

/// FFT 窗口大小（必须 <= STEP）
pub const FFT_LEN: usize = 128;
/// 采样步长（22050Hz 采样率下，2205 约等于 0.1 秒）
pub const STEP: usize = 2205;
/// 采样率
pub const SAMPLE_RATE: u32 = 22050;
/// 频谱图的频率 bin 数
pub const FREQ_BINS: usize = FFT_LEN / 2;

/// 读取 WAV 文件并返回频谱图
///
/// # 参数
/// - `path`: WAV 文件路径
///
/// # 返回
/// - `Array2<f64>`: 频谱图，形状为 (时间帧数, 频率bin数)
pub fn process_wav(path: &Path) -> Result<Array2<f64>> {
    let samples = read_wav_samples(path)?;
    samples_to_spectrogram(&samples)
}

/// 读取 WAV 文件的采样值
fn read_wav_samples(path: &Path) -> Result<Vec<f64>> {
    let mut reader = hound::WavReader::open(path)?;

    let spec = reader.spec();
    let samples: Vec<f64> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max_val = (1i64 << (spec.bits_per_sample - 1)) as f64;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f64 / max_val))
                .collect::<Result<Vec<_>, _>>()?
        }
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|s| s.map(|v| v as f64))
            .collect::<Result<Vec<_>, _>>()?,
    };

    // 如果是立体声，取单声道
    if spec.channels == 2 {
        Ok(samples
            .chunks(2)
            .map(|chunk| (chunk[0] + chunk[1]) / 2.0)
            .collect())
    } else {
        Ok(samples)
    }
}

/// 将采样值转换为频谱图
fn samples_to_spectrogram(samples: &[f64]) -> Result<Array2<f64>> {
    if samples.len() < FFT_LEN {
        anyhow::bail!("音频太短，无法进行分析");
    }

    let width = (samples.len() - FFT_LEN) / STEP;
    let mut spectrogram = Array2::zeros((width, FREQ_BINS));

    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(FFT_LEN);

    for (frame_idx, start) in (0..samples.len() - FFT_LEN).step_by(STEP).enumerate() {
        if frame_idx >= width {
            break;
        }

        // 收集窗口内的采样值，进行平均降采样
        let mut window = Array1::<f64>::zeros(FFT_LEN);
        let mut counts = Array1::<usize>::zeros(FFT_LEN);

        let total = (STEP * 2).min(samples.len() - start);
        for j in 0..total {
            let k = j * FFT_LEN / total;
            if k < FFT_LEN {
                window[k] += samples[start + j];
                counts[k] += 1;
            }
        }

        for j in 0..FFT_LEN {
            if counts[j] > 0 {
                window[j] /= counts[j] as f64;
            }
        }

        // 执行 FFT
        let mut fft_data: Vec<Complex<f64>> =
            window.iter().map(|&x| Complex::new(x, 0.0)).collect();

        fft.process(&mut fft_data);

        // 取幅度谱的前半部分
        for j in 0..FREQ_BINS {
            spectrogram[[frame_idx, j]] = fft_data[j].norm();
        }
    }

    Ok(spectrogram)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spectrogram_shape() {
        // 创建一个简单的测试信号
        let samples: Vec<f64> = (0..44100)
            .map(|i| (i as f64 * 440.0 * 2.0 * std::f64::consts::PI / 44100.0).sin())
            .collect();
        let result = samples_to_spectrogram(&samples).unwrap();

        // 检查输出形状
        assert_eq!(result.ncols(), FREQ_BINS);
        assert!(result.nrows() > 0);
    }
}
