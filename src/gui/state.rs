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
    /// 事件分类结果（标签 + 置信度）
    pub event_ranges: Vec<EventRange>,
    pub classified: bool,
}

/// 按匹配度选择广告段的筛选模式
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MatchMode {
    /// 高置信度：不低于高阈值
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
            MatchMode::High => score >= th.score_high,
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

    /// 是否包含满足阈值的广告段
    pub fn has_effective_ads(&self, th: Thresholds) -> bool {
        self.effective_ads(th).next().is_some()
    }
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
        assert!(MatchMode::High.matches(0.9, 0.7, th));
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
        for mode in [MatchMode::High, MatchMode::Medium, MatchMode::Low] {
            assert!(!mode.matches(0.0, 0.0, th));
        }
        assert!(!MatchMode::Custom.matches(0.0, 0.0, th));
    }
}
