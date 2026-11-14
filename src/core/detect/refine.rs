//! 时序一致性过滤与边界精修
//!
//! Viterbi 过滤剔除孤立候选，多特征相似度用于确定区间起止。

use crate::core::dsp::features::{self, AudioFeatures};
use crate::core::range::{Link, Range};
use crate::core::viterbi::{self, Candidate};

use super::HASH_WINDOW_SIZE;

/// 精细调整时的相似度阈值
const FINE_TUNE_THRESHOLD: f64 = 0.65;
/// 精细调整的步长
const FINE_TUNE_STEP: usize = 1;
/// 精细调整使用的窗口大小
const FINE_TUNE_WINDOW: usize = 4;
/// 位置偏移搜索范围
const POSITION_SEARCH_RANGE: usize = 5;
/// 边界收紧阈值：高于此值才保留
const BOUNDARY_TIGHTEN_THRESHOLD: f64 = 0.75;
/// 块级置信度：每块的帧数（10 帧 = 1 秒）
const CONFIDENCE_BLOCK: usize = 10;
/// 边界精修窗口（帧）
const BOUNDARY_REFINE_WINDOW: usize = 2;
/// 边界精修阈值：低于此值则继续向内收缩
const BOUNDARY_REFINE_THRESHOLD: f64 = 0.80;
/// 边界精修时单侧最多收缩的帧数
const BOUNDARY_REFINE_MAX_SHIFT: usize = 5;
/// 多特征融合权重：[FFT, MFCC, Chroma]
const FEATURE_WEIGHTS: [f64; 3] = [0.50, 0.30, 0.20];

/// Viterbi 时序一致性过滤
pub(super) fn viterbi_filter_candidates(candidates: &[(usize, usize, f64)]) -> Vec<(usize, usize)> {
    if candidates.len() < 2 {
        return candidates.iter().map(|&(a, b, _)| (a, b)).collect();
    }

    let viterbi_candidates: Vec<Candidate> = candidates
        .iter()
        .map(|&(p1, p2, score)| Candidate {
            pos1: p1,
            pos2: p2,
            score,
        })
        .collect();

    let config = viterbi::ViterbiConfig {
        max_drift: 3,
        max_lookback: 200,
        min_chain_score: 2.5,
        min_chain_length: 2,
    };

    let filtered = viterbi::viterbi_filter(&viterbi_candidates, &config);

    if filtered.is_empty() {
        candidates.iter().map(|&(a, b, _)| (a, b)).collect()
    } else {
        filtered.into_iter().map(|c| (c.pos1, c.pos2)).collect()
    }
}

/// 多特征融合精细调整
pub(super) fn fine_tune_multi(
    feat1: &AudioFeatures,
    feat2: &AudioFeatures,
    ranges1: &mut Vec<Range>,
    ranges2: &mut Vec<Range>,
    candidates: &[(usize, usize)],
    file_idx1: usize,
    file_idx2: usize,
) -> Vec<Link> {
    let len1 = feat1.fft.nrows();
    let len2 = feat2.fft.nrows();
    let mut links = Vec::new();

    // 音频过短时哈希窗口无法覆盖，直接跳过，避免 len - HASH_WINDOW_SIZE 下溢
    if len1 < HASH_WINDOW_SIZE || len2 < HASH_WINDOW_SIZE {
        return links;
    }

    for &(p1_init, p2_init) in candidates {
        let mut p1 = p1_init;
        let p2 = p2_init;

        if in_range(ranges1, p1) && in_range(ranges2, p2) {
            continue;
        }

        // 候选位置必须能容纳完整的哈希窗口
        if p1 + HASH_WINDOW_SIZE > len1 || p2 + HASH_WINDOW_SIZE > len2 {
            continue;
        }

        let search_start = p1.saturating_sub(POSITION_SEARCH_RANGE);
        let search_end = (p1 + POSITION_SEARCH_RANGE).min(len1.saturating_sub(HASH_WINDOW_SIZE));

        let mut best_sim = 0.0f64;
        let mut best_p1 = p1;

        for i in search_start..search_end {
            let sim = features::multi_feature_sim(
                feat1,
                feat2,
                i,
                p2,
                HASH_WINDOW_SIZE,
                &FEATURE_WEIGHTS,
            );
            if sim > best_sim {
                best_sim = sim;
                best_p1 = i;
            }
        }
        p1 = best_p1;

        // 向后扩展
        let mut next = 0;
        while p1 + next + FINE_TUNE_WINDOW < len1 && p2 + next + FINE_TUNE_WINDOW < len2 {
            let sim = features::multi_feature_sim(
                feat1,
                feat2,
                p1 + next,
                p2 + next,
                FINE_TUNE_WINDOW,
                &FEATURE_WEIGHTS,
            );
            if sim < FINE_TUNE_THRESHOLD {
                break;
            }
            next += FINE_TUNE_STEP;
        }

        // 向前扩展
        let mut prev = 0i64;
        while p1 as i64 + prev >= 0 && p2 as i64 + prev >= 0 {
            let sim = features::multi_feature_sim(
                feat1,
                feat2,
                (p1 as i64 + prev) as usize,
                (p2 as i64 + prev) as usize,
                FINE_TUNE_WINDOW,
                &FEATURE_WEIGHTS,
            );
            if sim < FINE_TUNE_THRESHOLD {
                prev += FINE_TUNE_WINDOW as i64 - 1;
                break;
            }
            prev -= FINE_TUNE_STEP as i64;
        }

        // 边界精炼：从后端逐帧向内收缩，直到找到高相似度帧
        while next > FINE_TUNE_WINDOW {
            let check_pos = next - 1;
            if p1 + check_pos + FINE_TUNE_WINDOW >= len1
                || p2 + check_pos + FINE_TUNE_WINDOW >= len2
            {
                next = check_pos;
                continue;
            }
            let sim = features::multi_feature_sim(
                feat1,
                feat2,
                p1 + check_pos,
                p2 + check_pos,
                FINE_TUNE_WINDOW,
                &FEATURE_WEIGHTS,
            );
            if sim >= BOUNDARY_TIGHTEN_THRESHOLD {
                break;
            }
            next = check_pos;
        }

        // 边界精炼：从前端逐帧向内收缩
        while prev < -1 {
            let check_pos = prev + 1;
            if p1 as i64 + check_pos < 0 || p2 as i64 + check_pos < 0 {
                prev = check_pos;
                continue;
            }
            let sim = features::multi_feature_sim(
                feat1,
                feat2,
                (p1 as i64 + check_pos) as usize,
                (p2 as i64 + check_pos) as usize,
                FINE_TUNE_WINDOW,
                &FEATURE_WEIGHTS,
            );
            if sim >= BOUNDARY_TIGHTEN_THRESHOLD {
                break;
            }
            prev = check_pos;
        }

        // 记录匹配度，供界面展示与结果文件保存
        let start = (p1 as i64 + prev).max(0) as usize;
        let end = (p1 + next).min(len1);
        let (start, end) = refine_boundaries(feat1, feat2, p1, p2, start, end);
        if end <= start {
            continue;
        }

        // 第二个文件中与 start 对齐的位置（prev 可能为负，需防止下溢）
        let q2 = (p2 as i64 + start as i64 - p1 as i64).max(0) as usize;
        let span = end - start;
        if q2 >= len2 || q2 + span > len2 {
            continue;
        }
        let score = block_confidence(feat1, feat2, start, q2, span);

        let mut add1 = Range::new_simple(start as i32, end as i32);
        add1.score = score;
        let mut add2 = Range::new_simple(q2 as i32, (q2 + span) as i32);
        add2.score = score;

        let r1 = create_range(ranges1, len1, add1);
        let r2 = create_range(ranges2, len2, add2);

        if let (Some(r1), Some(r2)) = (r1, r2) {
            links.push(Link::new(file_idx1, file_idx2, r1, r2));
        }
    }

    links
}

/// 逐帧精修区间边界：两侧各自向内收缩，最多 `BOUNDARY_REFINE_MAX_SHIFT` 帧
fn refine_boundaries(
    feat1: &AudioFeatures,
    feat2: &AudioFeatures,
    p1: usize,
    p2: usize,
    start: usize,
    end: usize,
) -> (usize, usize) {
    let len1 = feat1.fft.nrows();
    let len2 = feat2.fft.nrows();
    let (mut s, mut e) = (start, end);

    for _ in 0..BOUNDARY_REFINE_MAX_SHIFT {
        if e.saturating_sub(s) <= 2 {
            break;
        }
        if p1 + s + BOUNDARY_REFINE_WINDOW > len1 || p2 + s + BOUNDARY_REFINE_WINDOW > len2 {
            break;
        }
        let sim = features::multi_feature_sim(
            feat1,
            feat2,
            p1 + s,
            p2 + s,
            BOUNDARY_REFINE_WINDOW,
            &FEATURE_WEIGHTS,
        );
        if sim >= BOUNDARY_REFINE_THRESHOLD {
            break;
        }
        s += 1;
    }

    for _ in 0..BOUNDARY_REFINE_MAX_SHIFT {
        if e.saturating_sub(s) <= 2 {
            break;
        }
        let pos = e - 1;
        if p1 + pos + BOUNDARY_REFINE_WINDOW > len1
            || p2 + pos + BOUNDARY_REFINE_WINDOW > len2
        {
            e -= 1;
            continue;
        }
        let sim = features::multi_feature_sim(
            feat1,
            feat2,
            p1 + pos,
            p2 + pos,
            BOUNDARY_REFINE_WINDOW,
            &FEATURE_WEIGHTS,
        );
        if sim >= BOUNDARY_REFINE_THRESHOLD {
            break;
        }
        e -= 1;
    }

    (s, e)
}

/// 块级置信度：按 1 秒分块统计，未达阈值的块按 0 计
///
/// 相比"区间内最高的一帧相似度"，该值反映整段有多大比例、有多像，
/// 避免局部强匹配掩盖中间断裂。
fn block_confidence(
    feat1: &AudioFeatures,
    feat2: &AudioFeatures,
    start: usize,
    q2: usize,
    len: usize,
) -> f64 {
    if len == 0 {
        return 0.0;
    }

    let len1 = feat1.fft.nrows();
    let len2 = feat2.fft.nrows();
    let mut matched = 0.0;
    let mut blocks = 0usize;
    let mut offset = 0usize;

    while offset < len {
        let window = FINE_TUNE_WINDOW.min(len - offset).max(1);
        if start + offset + window <= len1 && q2 + offset + window <= len2 {
            let sim = features::multi_feature_sim(
                feat1,
                feat2,
                start + offset,
                q2 + offset,
                window,
                &FEATURE_WEIGHTS,
            );
            if sim >= FINE_TUNE_THRESHOLD {
                matched += sim;
            }
        }
        blocks += 1;
        offset += CONFIDENCE_BLOCK;
    }

    if blocks == 0 {
        0.0
    } else {
        matched / blocks as f64
    }
}

/// 检查点是否在任何区间内
fn in_range(ranges: &[Range], p: usize) -> bool {
    let p = p as i32;
    ranges.iter().any(|r| r.begin <= p && p < r.end)
}

/// 创建或扩展区间
pub(super) fn create_range(ranges: &mut Vec<Range>, len: usize, add: Range) -> Option<Range> {
    let mut add = add;

    // 边界吸附
    if add.begin <= 50 {
        add.begin = 0;
    }
    if add.end >= len as i32 - 50 {
        add.end = len as i32;
    }

    // 最小长度检查（至少 2 秒 = 20 帧）
    if add.begin >= add.end - 20 {
        return None;
    }

    let mut cross = false;
    for r in ranges.iter_mut() {
        // 检查是否相交
        if r.begin > add.end || r.end < add.begin {
            continue;
        }
        cross = true;
        // 合并区间
        r.begin = r.begin.min(add.begin);
        r.end = r.end.max(add.end);
        r.count += add.count;
        r.score = r.score.max(add.score);
    }

    if !cross {
        ranges.push(add);
    }

    Some(add)
}
