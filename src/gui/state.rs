//! GUI 状态模型

use crate::core::classify::EventRange;
use crate::core::range::Range;
use crate::training::TrainSample;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// 文件信息
#[derive(Clone, Default)]
pub struct FileInfo {
    pub path: PathBuf,
    pub name: String,
    pub length: usize,
    /// 检测到的广告段（保持不变）
    pub ad_ranges: Vec<Range>,
    pub detected: bool,
    pub needs_save: bool,
    /// 被标记删除的广告索引（在有效广告段中的索引）
    pub marked_for_deletion: Vec<usize>,
    /// 事件分类结果（标签 + 置信度）
    pub event_ranges: Vec<EventRange>,
    pub classified: bool,
}

/// 按匹配度选择广告段的筛选模式
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MatchMode {
    /// 极高置信度：不低于极高阈值
    VeryHigh,
    /// 高置信度：介于高阈值与极高阈值之间
    High,
    /// 中等置信度：介于低阈值与高阈值之间
    Medium,
    /// 低置信度：低于低阈值
    Low,
    /// 自定义下限
    Custom,
}

impl MatchMode {
    /// 判断某个匹配度是否落在当前模式内（未知匹配度不参与）
    pub fn matches(self, score: f64, min_score: f64, th: Thresholds) -> bool {
        if score <= 0.0 {
            return false;
        }
        match self {
            MatchMode::VeryHigh => score >= th.score_top,
            MatchMode::High => score >= th.score_high && score < th.score_top,
            MatchMode::Medium => score >= th.score_low && score < th.score_high,
            MatchMode::Low => score < th.score_low,
            MatchMode::Custom => score >= min_score,
        }
    }
}

/// 广告段的过滤与展示阈值
#[derive(Clone, Copy, Debug)]
pub struct Thresholds {
    /// 最小广告长度（秒），0 表示不限制
    pub min_length_sec: i32,
    /// 最小出现次数
    pub min_count: i32,
    /// 极高匹配度阈值
    pub score_top: f64,
    /// 高匹配度阈值
    pub score_high: f64,
    /// 低匹配度阈值
    pub score_low: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            min_length_sec: 0,
            min_count: 2,
            score_top: 0.9,
            score_high: 0.8,
            score_low: 0.6,
        }
    }
}

impl Thresholds {
    /// 区间是否满足长度与出现次数阈值
    pub fn accepts(&self, r: &Range) -> bool {
        if r.count < self.min_count.max(1) {
            return false;
        }
        if self.min_length_sec > 0 {
            let secs = r.end.saturating_sub(r.begin) as f64 / 10.0;
            if secs < self.min_length_sec as f64 {
                return false;
            }
        }
        true
    }
}

impl FileInfo {
    /// 由文件路径创建文件信息
    pub fn from_path(path: PathBuf) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        Self {
            path,
            name,
            ..Default::default()
        }
    }

    /// 满足阈值的广告段
    pub fn effective_ads(&self, th: Thresholds) -> impl Iterator<Item = &Range> {
        self.ad_ranges.iter().filter(move |r| th.accepts(r))
    }

    /// 满足阈值的广告段数量
    pub fn effective_ad_count(&self, th: Thresholds) -> usize {
        self.effective_ads(th).count()
    }

    /// 已标记删除的有效广告段（保存与界面统计共用）
    pub fn marked_segments(&self, th: Thresholds) -> Vec<Range> {
        self.effective_ads(th)
            .enumerate()
            .filter(|(idx, _)| self.marked_for_deletion.contains(idx))
            .map(|(_, r)| *r)
            .collect()
    }

    /// 是否有需要保存的标记（已检测且至少标记了一个段）
    pub fn needs_cut(&self) -> bool {
        self.detected && !self.marked_for_deletion.is_empty()
    }
}

/// 把选中的广告段标记为删除
///
/// 选中项为 `(文件索引, 有效广告段索引)`；下标越界或重复的选中项会被忽略。
/// 返回本次新增的标记数量。
pub fn mark_ads_for_deletion(
    files: &mut [FileInfo],
    th: Thresholds,
    selection: &[(usize, usize)],
) -> usize {
    let mut added = 0usize;
    for &(file_idx, ad_idx) in selection {
        let Some(file) = files.get_mut(file_idx) else {
            continue;
        };
        if ad_idx >= file.effective_ad_count(th) {
            continue;
        }
        if file.marked_for_deletion.contains(&ad_idx) {
            continue;
        }
        file.marked_for_deletion.push(ad_idx);
        file.needs_save = true;
        added += 1;
    }
    added
}

/// 播放状态
#[derive(Clone)]
pub struct PlaybackState {
    pub playing_file: Option<usize>,
    pub playing_ad: Option<usize>,
    pub stop_flag: Arc<Mutex<bool>>,
}

impl Default for PlaybackState {
    fn default() -> Self {
        Self {
            playing_file: None,
            playing_ad: None,
            stop_flag: Arc::new(Mutex::new(false)),
        }
    }
}

/// 应用状态
pub struct AppState {
    pub files: Vec<FileInfo>,
    pub selected_file: Option<usize>,
    pub selected_ad: Option<usize>,
    pub is_processing: bool,
    pub progress: f32,
    pub status_message: String,
    /// 广告段过滤与分色阈值
    pub thresholds: Thresholds,
    /// 是否显示阈值设置窗口
    pub show_settings: bool,
    pub playback: PlaybackState,
    /// (file_idx, 有效广告段索引)
    pub ads_to_delete: Vec<(usize, usize)>,
    pub overwrite_original: bool,
    /// 是否在结果视图显示能量包络与 Mel 频谱
    pub show_spectrogram: bool,
    /// 结果列表是否按平均匹配度从高到低排序
    pub sort_by_score: bool,
    /// 当前选中文件的可视化纹理缓存
    pub viz_cache: Option<super::spectrogram::VizCache>,
    /// 事件分类模型路径
    pub model_path: Option<PathBuf>,
    /// 分类置信度阈值
    pub classify_confidence: f64,
    /// 是否启用 HPSS 源分离（打击乐轨道检测广告段）
    pub enable_hpss: bool,
    /// 训练样本列表
    pub train_samples: Vec<TrainSample>,
    /// 是否显示训练窗口
    pub show_train: bool,
    /// 新增样本使用的标签
    pub train_label: String,
    /// 随机森林树数量
    pub train_trees: usize,
    /// 单棵树最大深度
    pub train_max_depth: usize,
    /// 测试集比例
    pub train_test_split: f64,
    /// 导出广告段时选中的区间序号
    pub train_export_index: usize,
    /// 最近一次训练的结果摘要
    pub train_report: Option<String>,
    /// 是否显示按匹配度选择窗口
    pub show_match_select: bool,
    /// 按匹配度选择的筛选模式
    pub match_mode: MatchMode,
    /// 自定义模式下的匹配度下限
    pub match_min_score: f64,
    /// 是否对所有文件生效（否则仅当前文件）
    pub match_all_files: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            files: Vec::new(),
            selected_file: None,
            selected_ad: None,
            is_processing: false,
            progress: 0.0,
            status_message: "就绪".to_string(),
            thresholds: Thresholds::default(),
            show_settings: false,
            playback: PlaybackState::default(),
            ads_to_delete: Vec::new(),
            overwrite_original: false,
            show_spectrogram: false,
            sort_by_score: false,
            viz_cache: None,
            model_path: None,
            classify_confidence: 0.55,
            enable_hpss: false,
            train_samples: Vec::new(),
            show_train: false,
            train_label: "广告".to_string(),
            train_trees: 100,
            train_max_depth: 32,
            train_test_split: 0.2,
            train_export_index: 0,
            train_report: None,
            show_match_select: false,
            match_mode: MatchMode::Low,
            match_min_score: 0.7,
            match_all_files: true,
        }
    }
}

/// 界面各模块共享的状态句柄
pub type SharedState = Arc<Mutex<AppState>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_mode_classifies_scores() {
        let th = Thresholds::default();

        assert!(MatchMode::VeryHigh.matches(0.95, 0.7, th));
        assert!(MatchMode::VeryHigh.matches(0.9, 0.7, th));
        assert!(!MatchMode::VeryHigh.matches(0.89, 0.7, th));

        assert!(MatchMode::High.matches(0.85, 0.7, th));
        assert!(!MatchMode::High.matches(0.95, 0.7, th));
        assert!(!MatchMode::High.matches(0.7, 0.7, th));

        assert!(MatchMode::Medium.matches(0.7, 0.7, th));
        assert!(!MatchMode::Medium.matches(0.85, 0.7, th));

        assert!(MatchMode::Low.matches(0.5, 0.7, th));
        assert!(!MatchMode::Low.matches(0.6, 0.7, th));

        assert!(MatchMode::Custom.matches(0.75, 0.7, th));
        assert!(!MatchMode::Custom.matches(0.65, 0.7, th));
    }

    #[test]
    fn unknown_score_is_never_matched() {
        let th = Thresholds::default();
        for mode in [
            MatchMode::VeryHigh,
            MatchMode::High,
            MatchMode::Medium,
            MatchMode::Low,
        ] {
            assert!(!mode.matches(0.0, 0.0, th));
        }
        assert!(!MatchMode::Custom.matches(0.0, 0.0, th));
    }

    fn range(begin: i32, end: i32, count: i32) -> Range {
        Range::new(begin, end, count)
    }

    fn file_with(ranges: Vec<Range>) -> FileInfo {
        FileInfo {
            detected: true,
            ad_ranges: ranges,
            ..Default::default()
        }
    }

    fn segments(ranges: &[Range]) -> Vec<(i32, i32)> {
        ranges.iter().map(|r| (r.begin, r.end)).collect()
    }

    #[test]
    fn mark_ads_marks_each_segment_once() {
        let mut files = vec![file_with(vec![
            range(0, 10, 2),
            range(100, 120, 2),
            range(200, 230, 2),
        ])];

        let added = mark_ads_for_deletion(
            &mut files,
            Thresholds::default(),
            &[(0, 0), (0, 0), (0, 2), (0, 2)],
        );

        assert_eq!(added, 2);
        assert_eq!(files[0].marked_for_deletion, vec![0, 2]);
        assert!(files[0].needs_save);
        assert!(files[0].needs_cut());
    }

    #[test]
    fn mark_ads_ignores_invalid_selection() {
        let mut files = vec![file_with(vec![range(0, 10, 2)])];

        // 文件索引越界、广告索引越界都应被忽略
        let added = mark_ads_for_deletion(&mut files, Thresholds::default(), &[(5, 0), (0, 3)]);

        assert_eq!(added, 0);
        assert!(files[0].marked_for_deletion.is_empty());
        assert!(!files[0].needs_cut());
    }

    #[test]
    fn marked_index_maps_to_effective_segment() {
        // 第一段 count=1 会被默认阈值过滤，标记索引 0 应落在第二段（50–70）
        let mut files = vec![file_with(vec![
            range(0, 10, 1),
            range(50, 70, 3),
            range(100, 120, 2),
        ])];
        let th = Thresholds::default();

        mark_ads_for_deletion(&mut files, th, &[(0, 0), (0, 1)]);

        assert_eq!(
            segments(&files[0].marked_segments(th)),
            vec![(50, 70), (100, 120)]
        );
    }

    #[test]
    fn marked_segments_are_cut_and_surrounding_audio_joined() {
        let mut files = vec![file_with(vec![range(100, 200, 2), range(400, 450, 2)])];
        let th = Thresholds::default();
        mark_ads_for_deletion(&mut files, th, &[(0, 0)]);

        let cut = files[0].marked_segments(th);
        assert_eq!(segments(&cut), vec![(100, 200)]);

        // 保存时保留的是补集：删除段前后被拼接起来
        let keep = crate::pipeline::keep_segments(&cut, 1000);
        assert_eq!(keep, vec![(0, 100), (200, 1000)]);
    }

    #[test]
    fn stale_mark_yields_no_segment_after_threshold_change() {
        let mut files = vec![file_with(vec![range(0, 10, 2), range(100, 120, 1)])];

        let mut lenient = Thresholds::default();
        lenient.min_count = 1;
        mark_ads_for_deletion(&mut files, lenient, &[(0, 1)]);
        assert_eq!(files[0].marked_segments(lenient).len(), 1);

        // 阈值收紧后该段不再有效，保存时会跳过而不是删错段
        let strict = Thresholds::default();
        assert!(files[0].marked_segments(strict).is_empty());
    }

    #[test]
    fn needs_cut_requires_detection() {
        let mut files = vec![file_with(vec![range(0, 10, 2)])];
        files[0].detected = false;

        mark_ads_for_deletion(&mut files, Thresholds::default(), &[(0, 0)]);

        assert_eq!(files[0].marked_for_deletion, vec![0]);
        assert!(!files[0].needs_cut());
    }
}
