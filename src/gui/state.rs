//! GUI 状态模型

use crate::core::range::Range;
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

    /// 有效广告段（出现次数大于 1 的片段）
    pub fn effective_ads(&self) -> impl Iterator<Item = &Range> {
        self.ad_ranges.iter().filter(|r| r.count > 1)
    }

    /// 有效广告段数量
    pub fn effective_ad_count(&self) -> usize {
        self.effective_ads().count()
    }

    /// 是否包含有效广告段
    pub fn has_effective_ads(&self) -> bool {
        self.effective_ads().next().is_some()
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
    pub min_length: i32,
    pub min_count: i32,
    pub playback: PlaybackState,
    /// (file_idx, 有效广告段索引)
    pub ads_to_delete: Vec<(usize, usize)>,
    pub overwrite_original: bool,
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
            min_length: 5,
            min_count: 2,
            playback: PlaybackState::default(),
            ads_to_delete: Vec::new(),
            overwrite_original: false,
        }
    }
}

/// 界面各模块共享的状态句柄
pub type SharedState = Arc<Mutex<AppState>>;
