//! GUI 模块（需要 `gui` feature）
//!
//! 拆分为状态、播放、菜单、保存、文件列表与结果视图，避免单个文件承担全部界面逻辑。

mod app;
mod file_list;
mod fonts;
mod match_select;
mod menu;
mod playback;
mod results;
mod save;
mod settings;
mod spectrogram;
mod state;
mod train;

pub use app::run;
