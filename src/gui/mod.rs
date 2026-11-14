//! GUI 模块（需要 `gui` feature）
//!
//! 按职责分组：
//! - [`toolbar`] / [`status_bar`]：顶部工具栏与底部状态栏
//! - [`panels`]：文件列表、结果视图与设置/训练/匹配选择窗口
//! - [`widgets`]：音频条、图例与频谱可视化
//! - [`tasks`]：检测、分类与保存后台任务

mod app;
mod fonts;
mod panels;
mod playback;
mod state;
mod status_bar;
mod sysmon;
mod tasks;
mod toolbar;
mod widgets;

pub use app::run;
