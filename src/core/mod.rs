//! 纯算法层
//!
//! 只负责频谱、哈希、特征与检测算法本身，不执行外部程序、不读写检测缓存。
//! 音频文件转换与缓存由 [`crate::audio`] 负责，流程编排由 [`crate::pipeline`] 负责。
//!
//! 按功能域分组：
//! - [`dsp`]：频谱变换、特征提取、分段特征与源分离
//! - [`matching`]：LSH 哈希与 GPU 批量相似度
//! - [`detect`]：广告区间检测
//! - [`classify`]：事件分类、模型文件与随机森林

pub mod classify;
pub mod detect;
pub mod dsp;
pub mod matching;
pub mod range;
pub mod viterbi;
