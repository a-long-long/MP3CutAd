//! 区间数据结构

use std::fmt;

/// 表示一个时间区间
#[derive(Debug, Clone, Copy)]
pub struct Range {
    pub begin: i32,
    pub end: i32,
    pub count: i32,
    pub type_id: i32,
    /// 匹配度（0.0 ~ 1.0），由检测阶段的相似度得分聚合而来
    pub score: f64,
}

impl Range {
    pub fn new(begin: i32, end: i32, count: i32) -> Self {
        Self {
            begin,
            end,
            count,
            type_id: -1,
            score: 0.0,
        }
    }

    pub fn new_simple(begin: i32, end: i32) -> Self {
        Self::new(begin, end, 1)
    }

    /// 检查另一个区间是否完全包含在本区间内
    pub fn contains_range(&self, r: &Range) -> bool {
        self.begin <= r.begin && r.end <= self.end
    }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Range({}-{} count={})", self.begin, self.end, self.count)
    }
}

/// 链接关系，用于关联不同文件中的相似区间
#[derive(Debug, Clone)]
pub struct Link {
    pub f1: usize,
    pub f2: usize,
    pub r1: Range,
    pub r2: Range,
}

impl Link {
    pub fn new(f1: usize, f2: usize, r1: Range, r2: Range) -> Self {
        Self { f1, f2, r1, r2 }
    }
}
