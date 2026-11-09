//! MP3CutAd Rust 核心库
//!
//! 提供音频广告检测和裁剪功能。代码分层：
//! - [`core`]: 纯算法（频谱、LSH、特征、Viterbi、检测）
//! - [`audio`]: FFmpeg 调用与检测缓存
//! - [`pipeline`]: 检测到裁剪的高层编排
//! - [`result_io`]: 检测结果文件读写
//! - [`ffi`]: C ABI 接口
//! - [`cli`] / [`gui`]: 需要对应 feature 的界面层

pub mod audio;
pub mod core;
pub mod ffi;
pub mod pipeline;
pub mod result_io;

#[cfg(feature = "cli")]
pub mod cli;

#[cfg(feature = "gui")]
pub mod gui;

// 重新导出主要接口
pub use audio::ffmpeg::{audio_to_wav, cut_and_combine};
pub use core::detect::{detect_ad, reverse_range, DetectionResult};
pub use core::range::Range;
pub use pipeline::{collect_audio_files, cut_file, is_supported_audio, keep_segments};
