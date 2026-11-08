//! Viterbi 时序一致性约束
//!
//! 对候选匹配进行动态规划过滤，只保留形成时序一致链的匹配点，
//! 消除随机噪声导致的孤立假阳性。

/// 候选匹配：文件1的位置 pos1 与文件2的位置 pos2 之间的匹配，得分为 score
#[derive(Debug, Clone, Copy)]
pub struct Candidate {
    pub pos1: usize,
    pub pos2: usize,
    pub score: f64,
}

/// 时序一致性过滤参数
#[derive(Debug, Clone, Copy)]
pub struct ViterbiConfig {
    /// 相邻匹配允许的最大偏移漂移（帧）
    pub max_drift: usize,
    /// 向前搜索的最大跨度（帧）
    pub max_lookback: usize,
    /// 保留链的最小累计得分
    pub min_chain_score: f64,
    /// 保留链的最小匹配数量
    pub min_chain_length: usize,
}

impl Default for ViterbiConfig {
    fn default() -> Self {
        Self {
            max_drift: 3,
            max_lookback: 200,
            min_chain_score: 3.0,
            min_chain_length: 3,
        }
    }
}

/// 对候选匹配进行 Viterbi 时序一致性过滤
///
/// 返回通过过滤的候选匹配（保持原始索引对应关系）
pub fn viterbi_filter(candidates: &[Candidate], config: &ViterbiConfig) -> Vec<Candidate> {
    let n = candidates.len();
    if n < config.min_chain_length {
        return Vec::new();
    }

    // 按 pos1 排序，保留原始索引
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| candidates[i].pos1);

    // dp[k] = 以 order[k] 结尾的最佳链得分
    let mut dp = vec![0.0f64; n];
    // parent[k] = 链中 order[k] 的前驱在 order 中的位置，None 表示起点
    let mut parent: Vec<Option<usize>> = vec![None; n];
    // chain_len[k] = 以 order[k] 结尾的链长度
    let mut chain_len = vec![0usize; n];

    for k in 0..n {
        let ci = &candidates[order[k]];
        dp[k] = ci.score;
        chain_len[k] = 1;

        // 向前搜索一致的候选
        for m in (0..k).rev() {
            let cm = &candidates[order[m]];

            // 超出搜索范围则停止
            if ci.pos1 - cm.pos1 > config.max_lookback {
                break;
            }

            // 时序一致性检查：delta_pos1 ≈ delta_pos2
            let delta1 = ci.pos1 as isize - cm.pos1 as isize;
            let delta2 = ci.pos2 as isize - cm.pos2 as isize;
            let drift = (delta1 - delta2).unsigned_abs();

            if drift <= config.max_drift && delta1 > 0 {
                let new_score = dp[m] + ci.score;
                if new_score > dp[k] {
                    dp[k] = new_score;
                    parent[k] = Some(m);
                    chain_len[k] = chain_len[m] + 1;
                }
            }
        }
    }

    // 回溯标记：找出所有属于合格链的候选
    let mut keep = vec![false; n];
    for k in 0..n {
        if dp[k] >= config.min_chain_score && chain_len[k] >= config.min_chain_length {
            let mut cur = Some(k);
            while let Some(c) = cur {
                keep[c] = true;
                cur = parent[c];
            }
        }
    }

    // 收集通过的候选
    order
        .into_iter()
        .enumerate()
        .filter(|(k, _)| keep[*k])
        .map(|(_, orig_idx)| candidates[orig_idx])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_consistent_chain() {
        // 完美一致链：pos1-pos2 偏移恒为 10
        let candidates = vec![
            Candidate {
                pos1: 10,
                pos2: 0,
                score: 1.0,
            },
            Candidate {
                pos1: 30,
                pos2: 20,
                score: 1.0,
            },
            Candidate {
                pos1: 50,
                pos2: 40,
                score: 1.0,
            },
            Candidate {
                pos1: 70,
                pos2: 60,
                score: 1.0,
            },
        ];

        let config = ViterbiConfig {
            max_drift: 2,
            min_chain_score: 2.0,
            min_chain_length: 3,
            ..Default::default()
        };

        let result = viterbi_filter(&candidates, &config);
        assert_eq!(result.len(), 4, "all 4 should pass");
    }

    #[test]
    fn test_reject_noise() {
        // 3 个一致 + 1 个噪声
        let candidates = vec![
            Candidate {
                pos1: 10,
                pos2: 0,
                score: 1.0,
            },
            Candidate {
                pos1: 30,
                pos2: 20,
                score: 1.0,
            },
            Candidate {
                pos1: 50,
                pos2: 40,
                score: 1.0,
            },
            // 噪声：偏移完全不同
            Candidate {
                pos1: 60,
                pos2: 100,
                score: 1.0,
            },
        ];

        let config = ViterbiConfig {
            max_drift: 2,
            min_chain_score: 2.5,
            min_chain_length: 3,
            ..Default::default()
        };

        let result = viterbi_filter(&candidates, &config);
        assert_eq!(result.len(), 3, "noise should be filtered out");
        assert!(result.iter().all(|c| c.pos2 != 100));
    }

    #[test]
    fn test_too_few_candidates() {
        let candidates = vec![Candidate {
            pos1: 10,
            pos2: 0,
            score: 1.0,
        }];
        let config = ViterbiConfig::default();
        let result = viterbi_filter(&candidates, &config);
        assert!(result.is_empty());
    }
}
