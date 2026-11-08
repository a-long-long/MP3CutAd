//! FFI 接口模块
//!
//! 提供 C ABI 接口供 C# 调用

use libc;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::path::PathBuf;
use std::slice;

use crate::audio::ffmpeg;
use crate::core::detect;
use crate::core::range::Range;
use crate::pipeline;

/// 检测结果结构（C 兼容）
#[repr(C)]
pub struct CDetectionResult {
    /// 文件数量
    pub file_count: c_int,
    /// 每个文件的区间数量数组
    pub range_counts: *mut c_int,
    /// 所有区间数据（扁平化存储）
    pub ranges: *mut CRange,
    /// 每个文件的总帧数
    pub lengths: *mut c_int,
    /// 错误信息（null 表示无错误）
    pub error: *mut c_char,
}

/// 区间结构（C 兼容）
#[repr(C)]
pub struct CRange {
    pub begin: c_int,
    pub end: c_int,
    pub count: c_int,
    pub type_id: c_int,
}

impl From<Range> for CRange {
    fn from(r: Range) -> Self {
        Self {
            begin: r.begin,
            end: r.end,
            count: r.count,
            type_id: r.type_id,
        }
    }
}

/// 进度回调函数类型
pub type ProgressCallback = extern "C" fn(used: c_int, left: c_int, user_data: *mut c_void);

/// 检测广告
///
/// # Safety
/// 此函数接收原始指针，调用者需确保指针有效
#[no_mangle]
pub unsafe extern "C" fn mp3cutad_detect(
    files_ptr: *const *const c_char,
    files_count: c_int,
    _progress_cb: Option<ProgressCallback>,
    _user_data: *mut c_void,
) -> *mut CDetectionResult {
    // 解析文件路径
    let files_slice = slice::from_raw_parts(files_ptr, files_count as usize);
    let files: Vec<PathBuf> = files_slice
        .iter()
        .filter_map(|&p| {
            if p.is_null() {
                None
            } else {
                CStr::from_ptr(p).to_str().ok().map(PathBuf::from)
            }
        })
        .collect();

    // 执行检测
    let result = detect::detect_ad(&files);

    // 分配结果结构
    let result_box = Box::new(match result {
        Ok(det_result) => {
            let file_count = det_result.ranges.len() as c_int;

            // 分配区间计数数组
            let range_counts =
                libc::malloc(file_count as usize * std::mem::size_of::<c_int>()) as *mut c_int;
            for (i, ranges) in det_result.ranges.iter().enumerate() {
                *range_counts.add(i) = ranges.len() as c_int;
            }

            // 计算总区间数
            let total_ranges: usize = det_result.ranges.iter().map(|r| r.len()).sum();

            // 分配区间数组
            let ranges_ptr =
                libc::malloc(total_ranges * std::mem::size_of::<CRange>()) as *mut CRange;
            let mut idx = 0;
            for ranges in &det_result.ranges {
                for r in ranges {
                    *ranges_ptr.add(idx) = (*r).into();
                    idx += 1;
                }
            }

            // 分配长度数组
            let lengths =
                libc::malloc(file_count as usize * std::mem::size_of::<c_int>()) as *mut c_int;
            for (i, &len) in det_result.lengths.iter().enumerate() {
                *lengths.add(i) = len as c_int;
            }

            CDetectionResult {
                file_count,
                range_counts,
                ranges: ranges_ptr,
                lengths,
                error: std::ptr::null_mut(),
            }
        }
        Err(e) => {
            let error_msg = CString::new(format!("{}", e)).unwrap_or_default();
            CDetectionResult {
                file_count: 0,
                range_counts: std::ptr::null_mut(),
                ranges: std::ptr::null_mut(),
                lengths: std::ptr::null_mut(),
                error: error_msg.into_raw(),
            }
        }
    });

    Box::into_raw(result_box)
}

/// 裁剪音频文件
///
/// # Safety
/// 此函数接收原始指针，调用者需确保指针有效
#[no_mangle]
pub unsafe extern "C" fn mp3cutad_cut(
    input_file: *const c_char,
    output_file: *const c_char,
    ranges_ptr: *const CRange,
    ranges_count: c_int,
    total_length: c_int,
) -> *mut c_char {
    let input_path = match CStr::from_ptr(input_file).to_str() {
        Ok(s) => PathBuf::from(s),
        Err(_) => return CString::new("Invalid input file path").unwrap().into_raw(),
    };

    let output_path = match CStr::from_ptr(output_file).to_str() {
        Ok(s) => PathBuf::from(s),
        Err(_) => return CString::new("Invalid output file path").unwrap().into_raw(),
    };

    // 转换区间
    let ranges_slice = slice::from_raw_parts(ranges_ptr, ranges_count as usize);
    let ad_ranges: Vec<Range> = ranges_slice
        .iter()
        .map(|r| Range::new(r.begin, r.end, r.count))
        .collect();

    // 获取要保留的区间（反转广告区间）
    let segments = pipeline::keep_segments(&ad_ranges, total_length as usize);

    // 执行裁剪
    match ffmpeg::cut_and_combine(&input_path, &output_path, &segments) {
        Ok(_) => std::ptr::null_mut(),
        Err(e) => CString::new(format!("{}", e)).unwrap().into_raw(),
    }
}

/// 释放检测结果
///
/// # Safety
/// 此函数接收原始指针，调用者需确保指针有效
#[no_mangle]
pub unsafe extern "C" fn mp3cutad_free_result(result: *mut CDetectionResult) {
    if result.is_null() {
        return;
    }

    let result = Box::from_raw(result);

    if !result.range_counts.is_null() {
        libc::free(result.range_counts as *mut c_void);
    }

    if !result.ranges.is_null() {
        libc::free(result.ranges as *mut c_void);
    }

    if !result.lengths.is_null() {
        libc::free(result.lengths as *mut c_void);
    }

    if !result.error.is_null() {
        let _ = CString::from_raw(result.error);
    }
}

/// 释放字符串
///
/// # Safety
/// 此函数接收原始指针，调用者需确保指针有效
#[no_mangle]
pub unsafe extern "C" fn mp3cutad_free_string(s: *mut c_char) {
    if !s.is_null() {
        let _ = CString::from_raw(s);
    }
}

/// 检查 FFmpeg 是否可用
#[no_mangle]
pub extern "C" fn mp3cutad_check_ffmpeg() -> c_int {
    if ffmpeg::check_ffmpeg() {
        1
    } else {
        0
    }
}

/// 获取版本信息
#[no_mangle]
pub extern "C" fn mp3cutad_version() -> *const c_char {
    static VERSION: &str = concat!("mp3cutad-rs ", env!("CARGO_PKG_VERSION"), "\0");
    VERSION.as_ptr() as *const c_char
}
