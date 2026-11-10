//! 广告检测核心算法
//!
//! 集成多项优化：
//! - 自适应阈值（基于统计动态调整）
//! - 多探针 LSH（提高召回率）
//! - 多特征融合（FFT + MFCC + Chroma）
//! - Viterbi 时序一致性过滤

use anyhow::Result;
use ndarray::Array2;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::audio::{cache, ffmpeg};
use crate::core::features::{self, AudioFeatures};
use crate::core::fft::{self, FFT_LEN};
use crate::core::lsh;
use crate::core::range::{Link, Range};
use crate::core::viterbi::{self, Candidate};

/// 哈希窗口大小（帧数）
const HASH_WINDOW_SIZE: usize = 20;
/// 自适应阈值的最低底线
const MIN_THRESHOLD: f64 = 0.45;
/// 自适应阈值的默认上限
const MAX_THRESHOLD: f64 = 0.80;
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

/// 检测结果
#[derive(Debug)]
pub struct DetectionResult {
    /// 每个文件的广告区间
    pub ranges: Vec<Vec<Range>>,
    /// 每个文件的总帧数
    pub lengths: Vec<usize>,
}

/// 检测一系列 MP3 文件中的广告
///
/// # 参数
/// - `mp3_files`: MP3 文件路径列表
///
/// # 返回
/// 每个文件的广告区间和总长度
pub fn detect_ad(mp3_files: &[PathBuf]) -> Result<DetectionResult> {
    detect_ad_with_progress(mp3_files, |_, _| {})
}

/// 带进度回调的广告检测
///
/// `on_progress(progress, message)`：
/// - `progress` 为 0.0 ~ 1.0 的总体进度
/// - `message` 为当前阶段的说明文本
///
/// 回调可能从多个并行线程调用，因此要求 `Sync`。
pub fn detect_ad_with_progress<F>(mp3_files: &[PathBuf], on_progress: F) -> Result<DetectionResult>
where
    F: Fn(f32, &str) + Sync,
{
    let num_files = mp3_files.len();
    if num_files == 0 {
        return Ok(DetectionResult {
            ranges: Vec::new(),
            lengths: Vec::new(),
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

    // 1. 处理每个文件：MP3 -> WAV -> FFT -> Features -> Hash（中间结果按文件缓存）
    for (idx, mp3_path) in mp3_files.iter().enumerate() {
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

        let feature_data = features::extract_features(&fft_data);

        let expected_hash_len = fft_data.nrows().saturating_sub(HASH_WINDOW_SIZE);
        let hash_data = match cache::read_hash(mp3_path) {
            Some(h) if h.len() == expected_hash_len => h,
            _ => {
                let h = lsh::hash_all(&fft_data, HASH_WINDOW_SIZE);
                cache::save_hash(mp3_path, &h)?;
                h
            }
        };

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

    pairs.par_iter().for_each(|&(i, j)| {
        let ranges_i = Arc::new(Mutex::new(Vec::new()));
        let ranges_j = Arc::new(Mutex::new(Vec::new()));

        // 2.1 自适应阈值 + 多探针 LSH
        let raw_candidates = check_same_adaptive(&ffts[i], &ffts[j], &hashes[i], &hashes[j]);

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

    Ok(DetectionResult { ranges, lengths })
}

/// 自适应阈值 + 多探针 LSH 候选匹配
fn check_same_adaptive(
    fft1: &Array2<f64>,
    fft2: &Array2<f64>,
    hash1: &[i32],
    hash2: &[i32],
) -> Vec<(usize, usize, f64)> {
    let mut pos: HashMap<i32, Vec<usize>> = HashMap::new();
    for (i, &h) in hash2.iter().enumerate() {
        pos.entry(h).or_default().push(i);
    }

    let mut all_scores: Vec<f64> = Vec::new();
    let mut best_per_frame: Vec<(usize, usize, f64)> = Vec::new();

    for (i, &h) in hash1.iter().enumerate().step_by(HASH_WINDOW_SIZE) {
        let mut best_sim = 0.0f64;
        let mut best_pos = 0usize;

        // 多探针 LSH：检查原始哈希 + 邻近哈希
        let probes = lsh::multi_probe_hashes(h);
        for probe_h in probes {
            if let Some(positions) = pos.get(&probe_h) {
                for &p in positions {
                    let sim = lsh::cosine_sim(fft1, fft2, i, p, HASH_WINDOW_SIZE);
                    if sim > best_sim {
                        best_sim = sim;
                        best_pos = p;
                    }
                }
            }
        }

        if best_sim >= MIN_THRESHOLD {
            all_scores.push(best_sim);
            best_per_frame.push((i, best_pos, best_sim));
        }
    }

    if all_scores.is_empty() {
        return Vec::new();
    }

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

/// Viterbi 时序一致性过滤
fn viterbi_filter_candidates(candidates: &[(usize, usize, f64)]) -> Vec<(usize, usize)> {
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
fn fine_tune_multi(
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

    for &(p1_init, p2_init) in candidates {
        let mut p1 = p1_init;
        let p2 = p2_init;

        if in_range(ranges1, p1) && in_range(ranges2, p2) {
            continue;
        }

        let search_start = p1.saturating_sub(POSITION_SEARCH_RANGE);
        let search_end = (p1 + POSITION_SEARCH_RANGE).min(len1 - HASH_WINDOW_SIZE);

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

        // 第二个文件中与 start 对齐的位置
        let q2 = (p2 as i64 + start as i64 - p1 as i64) as usize;
        let score = block_confidence(feat1, feat2, start, q2, end - start);

        let mut add1 = Range::new_simple(start as i32, end as i32);
        add1.score = score;
        let mut add2 = Range::new_simple(q2 as i32, (q2 + (end - start)) as i32);
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
fn create_range(ranges: &mut Vec<Range>, len: usize, add: Range) -> Option<Range> {
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

/// 合并两组区间
fn combine_to_ranges(existing: &[Range], new_ranges: &[Range]) -> Vec<Range> {
    let mut result = existing.to_vec();
    for r in new_ranges {
        let mut r = *r;
        r.count = 1;
        // 使用 i32::MAX 而不是 usize::MAX，避免溢出
        create_range(&mut result, i32::MAX as usize, r);
    }
    compress_range(&result)
}

/// 压缩重叠区间
fn compress_range(ranges: &[Range]) -> Vec<Range> {
    let mut result: Vec<Range> = Vec::new();

    for &r in ranges {
        let mut merged = false;
        for existing in &mut result {
            // 检查是否相交或相邻
            if existing.begin <= r.end && r.begin <= existing.end {
                existing.begin = existing.begin.min(r.begin);
                existing.end = existing.end.max(r.end);
                existing.count += r.count;
                existing.score = existing.score.max(r.score);
                merged = true;
                break;
            }
        }
        if !merged {
            result.push(r);
        }
    }

    result
}

/// 计算区间类型（关联不同文件中的相同广告）
fn calc_range_types(ranges: &mut [Vec<Range>], links: &[Link]) {
    // 构建邻接表
    let mut adj: Vec<Vec<(usize, usize)>> = vec![Vec::new(); ranges.len()];

    for link in links {
        // 查找包含 link.r1 的区间
        if let Some(i1) = ranges[link.f1]
            .iter()
            .position(|r| r.contains_range(&link.r1))
        {
            if let Some(i2) = ranges[link.f2]
                .iter()
                .position(|r| r.contains_range(&link.r2))
            {
                adj[link.f1].push((link.f2, i2));
                adj[link.f2].push((link.f1, i1));
            }
        }
    }

    // 使用 BFS 标记连通分量
    let mut type_id = 0;
    let mut counts: Vec<i32> = Vec::new();

    for i in 0..ranges.len() {
        for j in 0..ranges[i].len() {
            if ranges[i][j].type_id == -1 {
                let count = bfs_set_type(ranges, &adj, i, j, type_id);
                counts.push(count);
                type_id += 1;
            }
        }
    }

    // 用连通分量的大小更新 count，使其表示该广告的出现次数。
    // 否则仅两个文件互相匹配时 count 仍为 1，会被后续 count > 1 的过滤误删。
    for file_ranges in ranges.iter_mut() {
        for r in file_ranges.iter_mut() {
            if r.type_id >= 0 {
                r.count = counts[r.type_id as usize];
            }
        }
    }

    // 只保留出现多次的区间
    for i in 0..ranges.len() {
        ranges[i].retain(|r| counts[r.type_id as usize] > 1);
    }
}

/// BFS 标记连通分量
fn bfs_set_type(
    ranges: &mut [Vec<Range>],
    adj: &[Vec<(usize, usize)>],
    start_i: usize,
    start_j: usize,
    type_id: i32,
) -> i32 {
    let mut queue = vec![(start_i, start_j)];
    let mut count = 0;

    while let Some((i, j)) = queue.pop() {
        if ranges[i][j].type_id == type_id {
            continue;
        }
        ranges[i][j].type_id = type_id;
        count += 1;

        for &(ni, nj) in &adj[i] {
            if ranges[ni][nj].type_id == -1 {
                queue.push((ni, nj));
            }
        }
    }

    count
}

/// 反转区间（获取非广告部分）
pub fn reverse_range(ranges: &[Range], total_len: usize) -> Vec<Range> {
    let mut result = Vec::new();
    let mut sorted: Vec<_> = ranges.iter().filter(|r| r.count > 1).collect();
    sorted.sort_by_key(|r| r.begin);

    if sorted.is_empty() {
        result.push(Range::new_simple(0, total_len as i32));
        return result;
    }

    if sorted[0].begin > 0 {
        result.push(Range::new_simple(0, sorted[0].begin));
    }

    for i in 1..sorted.len() {
        result.push(Range::new_simple(sorted[i - 1].end, sorted[i].begin));
    }

    if sorted.last().unwrap().end < total_len as i32 {
        result.push(Range::new_simple(
            sorted.last().unwrap().end,
            total_len as i32,
        ));
    }

    result
}
