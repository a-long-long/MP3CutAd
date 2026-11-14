//! 自适应阈值与 LSH 候选扫描
//!
//! 先用多探针 LSH 收集候选位置，再批量计算相似度（GPU 优先），
//! 最后用统计阈值过滤。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use ndarray::Array2;

use crate::core::matching::lsh;

use super::HASH_WINDOW_SIZE;

/// 自适应阈值的最低底线
const MIN_THRESHOLD: f64 = 0.45;
/// 自适应阈值的默认上限
const MAX_THRESHOLD: f64 = 0.80;
/// 单对文件交给 GPU 的最大候选数，超出后回退 CPU 以控制显存与内存
const MAX_GPU_PAIRS: usize = 4_000_000;

/// 自适应阈值 + 多探针 LSH 候选匹配
pub(super) fn check_same_adaptive(
    fft1: &Array2<f64>,
    fft2: &Array2<f64>,
    hash1: &[i32],
    hash2: &[i32],
    gpu_used: &AtomicBool,
) -> Vec<(usize, usize, f64)> {
    let mut pos: HashMap<i32, Vec<usize>> = HashMap::new();
    for (i, &h) in hash2.iter().enumerate() {
        pos.entry(h).or_default().push(i);
    }

    // 先收集每个采样帧的全部候选取位置，再统一计算相似度（便于 GPU 批量处理）
    let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
    for (i, &h) in hash1.iter().enumerate().step_by(HASH_WINDOW_SIZE) {
        let mut positions = Vec::new();
        // 多探针 LSH：检查原始哈希 + 邻近哈希
        for probe_h in lsh::multi_probe_hashes(h) {
            if let Some(found) = pos.get(&probe_h) {
                positions.extend_from_slice(found);
            }
        }
        if !positions.is_empty() {
            groups.push((i, positions));
        }
    }

    let mut best_per_frame = best_matches(fft1, fft2, &groups, gpu_used);

    let all_scores: Vec<f64> = best_per_frame
        .iter()
        .map(|&(_, _, sim)| sim)
        .filter(|&sim| sim >= MIN_THRESHOLD)
        .collect();

    if all_scores.is_empty() {
        return Vec::new();
    }

    best_per_frame.retain(|&(_, _, sim)| sim >= MIN_THRESHOLD);

    // 自适应阈值：mean + 1.0 * std，限制在 [MIN_THRESHOLD, MAX_THRESHOLD]
    let n = all_scores.len() as f64;
    let mean = all_scores.iter().sum::<f64>() / n;
    let variance = all_scores.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / n;
    let std_dev = variance.sqrt();
    let adaptive_threshold = (mean + 1.0 * std_dev).clamp(MIN_THRESHOLD, MAX_THRESHOLD);

    best_per_frame
        .into_iter()
        .filter(|&(_, _, sim)| sim >= adaptive_threshold)
        .collect()
}

/// 计算每个帧组的最高相似度及对应位置
///
/// 优先尝试 GPU 批量计算，不可用时回退到 CPU（AVX2/标量）实现。
fn best_matches(
    fft1: &Array2<f64>,
    fft2: &Array2<f64>,
    groups: &[(usize, Vec<usize>)],
    gpu_used: &AtomicBool,
) -> Vec<(usize, usize, f64)> {
    let total: usize = groups.iter().map(|(_, ps)| ps.len()).sum();
    if total == 0 || total > MAX_GPU_PAIRS {
        return cpu_best_matches(fft1, fft2, groups);
    }

    let mut pairs: Vec<(usize, usize)> = Vec::with_capacity(total);
    for &(i, ref positions) in groups {
        for &p in positions {
            pairs.push((i, p));
        }
    }

    if let Some(scores) = crate::core::matching::gpu::cosine_pairs(fft1, fft2, &pairs, HASH_WINDOW_SIZE) {
        gpu_used.store(true, Ordering::Relaxed);
        let mut out = Vec::with_capacity(groups.len());
        let mut k = 0usize;
        for &(i, ref positions) in groups {
            let start = k;
            let mut max_sim = 0.0f64;
            for _ in 0..positions.len() {
                let sim = scores[k];
                k += 1;
                if sim > max_sim {
                    max_sim = sim;
                }
            }
            // 重复素材会大量出现几乎相同的分数，GPU 的 f32 舍入会打乱次序；
            // 取第一个与最高分相差 1e-4 以内的位置，尽量还原 CPU 的选取结果。
            let mut best_pos = positions[0];
            for (off, &p) in positions.iter().enumerate() {
                if scores[start + off] >= max_sim - 1e-4 {
                    best_pos = p;
                    break;
                }
            }
            // 命中项用 f64 复算，保证后续阈值与链路使用与纯 CPU 一致的数值。
            let exact = lsh::cosine_sim(fft1, fft2, i, best_pos, HASH_WINDOW_SIZE);
            out.push((i, best_pos, exact));
        }
        return out;
    }

    cpu_best_matches(fft1, fft2, groups)
}

/// CPU 版本：逐组计算并选取最高相似度
fn cpu_best_matches(
    fft1: &Array2<f64>,
    fft2: &Array2<f64>,
    groups: &[(usize, Vec<usize>)],
) -> Vec<(usize, usize, f64)> {
    groups
        .iter()
        .map(|&(i, ref positions)| {
            let mut best_sim = 0.0f64;
            let mut best_pos = 0usize;
            for &p in positions {
                let sim = lsh::cosine_sim(fft1, fft2, i, p, HASH_WINDOW_SIZE);
                if sim > best_sim {
                    best_sim = sim;
                    best_pos = p;
                }
            }
            (i, best_pos, best_sim)
        })
        .collect()
}
