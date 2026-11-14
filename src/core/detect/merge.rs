//! 区间合并、类型计算与补集
//!
//! 负责把两两比对得到的区间压缩为不重叠结果，并通过连通分量计算出现次数。

use crate::core::range::{Link, Range};

use super::refine::create_range;

/// 合并两组区间
pub(super) fn combine_to_ranges(existing: &[Range], new_ranges: &[Range]) -> Vec<Range> {
    let mut result = existing.to_vec();
    for r in new_ranges {
        let mut r = *r;
        r.count = 1;
        // 使用 i32::MAX 而不是 usize::MAX，避免溢出
        create_range(&mut result, i32::MAX as usize, r);
    }
    compress_range(&result)
}

/// 压缩重叠区间。
///
/// 先按 begin 排序，再单次线性扫描合并（前后相邻/相交则合并），
/// 保证输出不含重叠且按 begin 升序。
pub(super) fn compress_range(ranges: &[Range]) -> Vec<Range> {
    let mut sorted: Vec<Range> = ranges.to_vec();
    sorted.sort_by_key(|r| r.begin);

    let mut result: Vec<Range> = Vec::with_capacity(sorted.len());
    for r in sorted {
        // 跳过非法段（begin >= end）
        if r.begin >= r.end {
            continue;
        }
        if let Some(last) = result.last_mut() {
            // 相邻或相交则合并
            if r.begin <= last.end {
                last.begin = last.begin.min(r.begin);
                last.end = last.end.max(r.end);
                last.count += r.count;
                last.score = last.score.max(r.score);
                continue;
            }
        }
        result.push(r);
    }
    result
}

/// 计算区间类型（关联不同文件中的相同广告）
pub(super) fn calc_range_types(ranges: &mut [Vec<Range>], links: &[Link]) {
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
    let total = total_len as i32;

    // 只过滤非法段：出现次数由阈值过滤，这里不能以 count 为准，
    // 否则 GUI 把最小次数调到 1 时标记删除会失效。
    let mut sorted: Vec<(i32, i32)> = ranges
        .iter()
        .filter(|r| r.begin < r.end)
        .map(|r| (r.begin.max(0).min(total), r.end.max(0).min(total)))
        .filter(|(b, e)| b < e)
        .collect();
    sorted.sort_by_key(|&(b, _)| b);

    // 先合并重叠/相接区间，避免补集出现负长度段
    let mut merged: Vec<(i32, i32)> = Vec::with_capacity(sorted.len());
    for (b, e) in sorted {
        match merged.last_mut() {
            Some(last) if b <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((b, e)),
        }
    }

    if merged.is_empty() {
        if total > 0 {
            result.push(Range::new_simple(0, total));
        }
        return result;
    }

    if merged[0].0 > 0 {
        result.push(Range::new_simple(0, merged[0].0));
    }

    for i in 1..merged.len() {
        let gap_begin = merged[i - 1].1;
        let gap_end = merged[i].0;
        if gap_begin < gap_end {
            result.push(Range::new_simple(gap_begin, gap_end));
        }
    }

    let last_end = merged.last().unwrap().1;
    if last_end < total {
        result.push(Range::new_simple(last_end, total));
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segments(ranges: &[Range]) -> Vec<(i32, i32)> {
        ranges.iter().map(|r| (r.begin, r.end)).collect()
    }

    #[test]
    fn reverse_range_keeps_count_one_segments() {
        // count=1 的段在 GUI 最小次数为 1 时同样应参与补集计算
        let ranges = vec![Range::new(100, 200, 1)];
        assert_eq!(segments(&reverse_range(&ranges, 1000)), vec![(0, 100), (200, 1000)]);
    }

    #[test]
    fn reverse_range_merges_overlapping_segments() {
        let ranges = vec![
            Range::new(100, 300, 2),
            Range::new(200, 400, 2),
            Range::new(350, 500, 2),
        ];
        // 重叠/相接的段合并为 100-500 后求补集，不产生负长度区间
        assert_eq!(segments(&reverse_range(&ranges, 1000)), vec![(0, 100), (500, 1000)]);
    }

    #[test]
    fn reverse_range_clamps_to_total_length() {
        let ranges = vec![Range::new(-10, 100, 2), Range::new(900, 1200, 2)];
        assert_eq!(segments(&reverse_range(&ranges, 1000)), vec![(100, 900)]);
    }

    #[test]
    fn reverse_range_without_valid_segments_returns_whole() {
        let ranges = vec![Range::new(100, 100, 2), Range::new(300, 200, 2)];
        assert_eq!(segments(&reverse_range(&ranges, 500)), vec![(0, 500)]);
        assert!(reverse_range(&ranges, 0).is_empty());
    }
}
