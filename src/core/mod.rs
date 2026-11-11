//! 纯算法层
//!
//! 只负责频谱、哈希、特征与检测算法本身，不执行外部程序、不读写检测缓存。
//! 音频文件转换与缓存由 [`crate::audio`] 负责，流程编排由 [`crate::pipeline`] 负责。

pub mod detect;
pub mod features;
pub mod fft;
pub mod lsh;
pub mod range;
pub mod viterbi;
pub mod segfeat;
pub mod forest;
pub mod model;
pub mod classify;
pub mod separation;
