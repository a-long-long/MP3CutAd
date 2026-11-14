//! 选中文件的能量包络与 Mel 频谱可视化
//!
//! 数据来自检测阶段写入磁盘的 FFT 缓存，只用于展示，不参与广告检测本身。
//! 频谱图沿用 MFCC 的 Mel 滤波器组，因此与人耳感知一致。

use egui::{Color32, ColorImage, TextureHandle, TextureOptions};
use ndarray::Array2;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::audio::cache;
use crate::core::dsp::features::{self, MEL_BANDS};

use super::super::state::{FileInfo, SharedState};

/// 能量包络高度
const ENERGY_HEIGHT: f32 = 32.0;
/// Mel 频谱高度
const MEL_HEIGHT: f32 = 84.0;

/// 可视化纹理缓存（键为文件路径 + 修改时间，宽度变化时重建）
pub struct VizCache {
    key: (PathBuf, Option<SystemTime>),
    width: usize,
    energy: TextureHandle,
    mel: TextureHandle,
}

/// 绘制选中文件的能量包络与 Mel 频谱，并叠加广告段位置
pub fn show(ui: &mut egui::Ui, state: &SharedState, file_idx: usize, file: &FileInfo) {
    let width = ui.available_width().max(64.0) as usize;
    let key = (
        file.path.clone(),
        std::fs::metadata(&file.path)
            .and_then(|m| m.modified())
            .ok(),
    );

    // 命中缓存时只克隆纹理句柄（内部为 Arc），避免重复读取 FFT
    let (cached, thresholds) = {
        let s = state.lock().unwrap();
        (
            s.viz_cache
                .as_ref()
                .filter(|c| c.key == key && c.width == width)
                .map(|c| (c.energy.clone(), c.mel.clone())),
            s.thresholds,
        )
    };

    let (energy_tex, mel_tex) = match cached {
        Some(textures) => textures,
        None => {
            let Some(fft) = cache::read_fft(&file.path) else {
                ui.weak("频谱缓存不可用，请重新执行检测后再查看");
                return;
            };

            let energy = ui.ctx().load_texture(
                format!("viz-energy-{file_idx}"),
                energy_image(&fft, width, ENERGY_HEIGHT as usize),
                TextureOptions::LINEAR,
            );
            let mel = ui.ctx().load_texture(
                format!("viz-mel-{file_idx}"),
                mel_image(&fft, width, MEL_HEIGHT as usize),
                TextureOptions::LINEAR,
            );

            state.lock().unwrap().viz_cache = Some(VizCache {
                key,
                width,
                energy: energy.clone(),
                mel: mel.clone(),
            });
            (energy, mel)
        }
    };

    ui.label(
        egui::RichText::new("能量包络")
            .small()
            .color(Color32::from_gray(150)),
    );
    draw_textured(ui, &energy_tex, file, ENERGY_HEIGHT, thresholds);

    ui.add_space(2.0);
    ui.label(
        egui::RichText::new("Mel 频谱（对数能量，低频在下）")
            .small()
            .color(Color32::from_gray(150)),
    );
    draw_textured(ui, &mel_tex, file, MEL_HEIGHT, thresholds);
}

/// 绘制纹理并在有效广告段位置叠加半透明红框
fn draw_textured(
    ui: &mut egui::Ui,
    tex: &TextureHandle,
    file: &FileInfo,
    height: f32,
    thresholds: super::super::state::Thresholds,
) {
    let width = ui.available_width();
    let (resp, painter) = ui.allocate_painter(egui::vec2(width, height), egui::Sense::hover());
    let rect = resp.rect;

    painter.image(
        tex.id(),
        rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );

    let total = file.length.max(1) as f32;
    for r in file.effective_ads(thresholds) {
        let x0 = rect.left() + rect.width() * r.begin as f32 / total;
        let x1 = rect.left() + rect.width() * r.end as f32 / total;
        let ad_rect =
            egui::Rect::from_x_y_ranges(x0..=x1, rect.top()..=rect.bottom());
        painter.rect_filled(
            ad_rect,
            0.0,
            Color32::from_rgba_unmultiplied(255, 60, 60, 40),
        );
        painter.rect_stroke(
            ad_rect,
            0.0,
            egui::Stroke::new(1.0_f32, Color32::from_rgb(255, 90, 90)),
        );
    }

    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0_f32, Color32::from_rgb(60, 60, 70)),
    );
}

/// 生成能量包络：每列取该时间段内所有频率幅度之和的峰值
fn energy_image(fft: &Array2<f64>, width: usize, height: usize) -> ColorImage {
    let frames = fft.nrows();
    if frames == 0 || width == 0 || height == 0 {
        return ColorImage::new([width.max(1), height.max(1)], Color32::BLACK);
    }

    let mut columns = vec![0.0f64; width];
    for (x, column) in columns.iter_mut().enumerate() {
        let start = x * frames / width;
        let end = ((x + 1) * frames / width).max(start + 1).min(frames);
        for f in start..end {
            let energy: f64 = fft.row(f).iter().sum();
            *column = column.max(energy);
        }
    }

    let max = columns.iter().cloned().fold(0.0, f64::max).max(1e-9);
    let mut image = ColorImage::new([width, height], Color32::BLACK);
    for (x, &value) in columns.iter().enumerate() {
        let level = (value / max).sqrt().clamp(0.0, 1.0) as f32;
        let color = heat(level);
        for y in 0..height {
            image.pixels[y * width + x] = color;
        }
    }
    image
}

/// 生成 Mel 频谱图：时间轴横向，频带纵向（低频在下）
fn mel_image(fft: &Array2<f64>, width: usize, height: usize) -> ColorImage {
    let frames = fft.nrows();
    if frames == 0 || width == 0 || height == 0 {
        return ColorImage::new([width.max(1), height.max(1)], Color32::BLACK);
    }

    let mel = features::mel_spectrogram(fft);

    // 每个像素列对应一段时间，取该时间段内各频带的峰值
    let mut grid = vec![f64::NEG_INFINITY; width * MEL_BANDS];
    for x in 0..width {
        let start = x * frames / width;
        let end = ((x + 1) * frames / width).max(start + 1).min(frames);
        for m in 0..MEL_BANDS {
            let mut peak = f64::NEG_INFINITY;
            for f in start..end {
                peak = peak.max(mel[[f, m]]);
            }
            grid[x * MEL_BANDS + m] = peak;
        }
    }

    // 稳健归一化：用 5% / 95% 分位，避免个别尖峰把整图压暗
    let mut sorted: Vec<f64> = grid.iter().copied().filter(|v| v.is_finite()).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let (low, high) = if sorted.is_empty() {
        (0.0, 1.0)
    } else {
        (sorted[sorted.len() / 20], sorted[sorted.len() * 19 / 20])
    };
    let span = (high - low).max(1e-9);

    let mut image = ColorImage::new([width, height], Color32::BLACK);
    for y in 0..height {
        // 图像顶部对应高频
        let band = ((height - 1 - y) * MEL_BANDS / height).min(MEL_BANDS - 1);
        for x in 0..width {
            let value = grid[x * MEL_BANDS + band];
            let level = ((value - low) / span).clamp(0.0, 1.0) as f32;
            image.pixels[y * width + x] = heat(level);
        }
    }
    image
}

/// 热力配色：深蓝 → 紫 → 橙红 → 浅黄
fn heat(level: f32) -> Color32 {
    const STOPS: [(f32, (u8, u8, u8)); 4] = [
        (0.0, (10, 12, 40)),
        (0.35, (60, 40, 130)),
        (0.65, (200, 80, 70)),
        (1.0, (255, 225, 130)),
    ];

    let level = level.clamp(0.0, 1.0);
    for pair in STOPS.windows(2) {
        let (t0, c0) = pair[0];
        let (t1, c1) = pair[1];
        if level <= t1 {
            let k = if t1 > t0 { (level - t0) / (t1 - t0) } else { 0.0 };
            let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * k).round() as u8;
            return Color32::from_rgb(
                mix(c0.0, c1.0),
                mix(c0.1, c1.1),
                mix(c0.2, c1.2),
            );
        }
    }

    let last = STOPS[STOPS.len() - 1].1;
    Color32::from_rgb(last.0, last.1, last.2)
}
