//! FFI 接口模块
//!
//! 提供 C ABI 接口供 C# 调用

use libc;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::path::PathBuf;
use std::slice;
use std::panic::AssertUnwindSafe;

use crate::audio::ffmpeg;
use crate::core::detect;
use crate::core::range::Range;
use crate::pipeline;

/// 构造错误 C 字符串（统一入口，避免 unwrap panic）
fn error_cstring(msg: impl Into<String>) -> *mut c_char {
    CString::new(msg.into()).unwrap_or_else(|_| CString::new("internal error").unwrap()).into_raw()
}

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
    // 入参校验：负数 count / 空指针防御
    if files_count < 0 || (files_count > 0 && files_ptr.is_null()) {
        let result_box = Box::new(CDetectionResult {
            file_count: 0,
            range_counts: std::ptr::null_mut(),
            ranges: std::ptr::null_mut(),
            lengths: std::ptr::null_mut(),
            error: error_cstring("invalid argument: files_count negative or null pointer"),
        });
        return Box::into_raw(result_box);
    }

    // catch_unwind：任何内部 panic 都不能越过 extern "C" 边界（UB）
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let files_slice = if files_count == 0 {
            &[] as &[*const c_char]
        } else {
            slice::from_raw_parts(files_ptr, files_count as usize)
        };
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

        match detect::detect_ad(&files) {
            Ok(d) => Ok(d),
            Err(e) => Err(format!("{}", e)),
        }
    }));

    let det_result = match result {
        Ok(Ok(d)) => d,
        Ok(Err(e)) => {
            let result_box = Box::new(CDetectionResult {
                file_count: 0,
                range_counts: std::ptr::null_mut(),
                ranges: std::ptr::null_mut(),
                lengths: std::ptr::null_mut(),
                error: error_cstring(format!("detect error: {}", e)),
            });
            return Box::into_raw(result_box);
        }
        Err(_) => {
            let result_box = Box::new(CDetectionResult {
                file_count: 0,
                range_counts: std::ptr::null_mut(),
                ranges: std::ptr::null_mut(),
                lengths: std::ptr::null_mut(),
                error: error_cstring("panic in mp3cutad_detect (caught)"),
            });
            return Box::into_raw(result_box);
        }
    };

    let file_count = det_result.ranges.len() as c_int;

    // malloc 判空：分配失败直接返回错误结果
    let range_counts = libc::malloc(file_count as usize * std::mem::size_of::<c_int>()) as *mut c_int;
    if file_count > 0 && range_counts.is_null() {
        let result_box = Box::new(CDetectionResult {
            file_count: 0,
            range_counts: std::ptr::null_mut(),
            ranges: std::ptr::null_mut(),
            lengths: std::ptr::null_mut(),
            error: error_cstring("out of memory"),
        });
        return Box::into_raw(result_box);
    }
    for (i, ranges) in det_result.ranges.iter().enumerate() {
        *range_counts.add(i) = ranges.len() as c_int;
    }

    let total_ranges: usize = det_result.ranges.iter().map(|r| r.len()).sum();
    let ranges_ptr = if total_ranges > 0 {
        libc::malloc(total_ranges * std::mem::size_of::<CRange>()) as *mut CRange
    } else {
        std::ptr::null_mut()
    };
    if total_ranges > 0 && ranges_ptr.is_null() {
        libc::free(range_counts as *mut c_void);
        let result_box = Box::new(CDetectionResult {
            file_count: 0,
            range_counts: std::ptr::null_mut(),
            ranges: std::ptr::null_mut(),
            lengths: std::ptr::null_mut(),
            error: error_cstring("out of memory"),
        });
        return Box::into_raw(result_box);
    }
    let mut idx = 0;
    for ranges in &det_result.ranges {
        for r in ranges {
            if !ranges_ptr.is_null() {
                *ranges_ptr.add(idx) = (*r).into();
            }
            idx += 1;
        }
    }

    let lengths = libc::malloc(file_count as usize * std::mem::size_of::<c_int>()) as *mut c_int;
    if file_count > 0 && lengths.is_null() {
        libc::free(range_counts as *mut c_void);
        if !ranges_ptr.is_null() {
            libc::free(ranges_ptr as *mut c_void);
        }
        let result_box = Box::new(CDetectionResult {
            file_count: 0,
            range_counts: std::ptr::null_mut(),
            ranges: std::ptr::null_mut(),
            lengths: std::ptr::null_mut(),
            error: error_cstring("out of memory"),
        });
        return Box::into_raw(result_box);
    }
    for (i, &len) in det_result.lengths.iter().enumerate() {
        *lengths.add(i) = len as c_int;
    }

    let result_box = Box::new(CDetectionResult {
        file_count,
        range_counts,
        ranges: ranges_ptr,
        lengths,
        error: std::ptr::null_mut(),
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
    // 入参校验：负 count / 负 total_length / 必要空指针
    if ranges_count < 0 || total_length < 0 {
        return error_cstring("invalid argument: negative ranges_count or total_length");
    }
    if input_file.is_null() || output_file.is_null() {
        return error_cstring("invalid argument: null input/output path");
    }
    if ranges_count > 0 && ranges_ptr.is_null() {
        return error_cstring("invalid argument: null ranges_ptr with positive ranges_count");
    }

    // catch_unwind：任何内部 panic 都不能越过 extern "C" 边界（UB）
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| -> Result<*mut c_char, String> {
        let input_path = CStr::from_ptr(input_file)
            .to_str()
            .map(PathBuf::from)
            .map_err(|e| format!("invalid input path: {}", e))?;
        let output_path = CStr::from_ptr(output_file)
            .to_str()
            .map(PathBuf::from)
            .map_err(|e| format!("invalid output path: {}", e))?;

        let ad_ranges: Vec<Range> = if ranges_count > 0 {
            let slice = slice::from_raw_parts(ranges_ptr, ranges_count as usize);
            slice
                .iter()
                .map(|r| Range::new(r.begin, r.end, r.count))
                .collect()
        } else {
            Vec::new()
        };

        let segments = pipeline::keep_segments(&ad_ranges, total_length as usize);
        ffmpeg::cut_and_combine(&input_path, &output_path, &segments)
            .map(|_| std::ptr::null_mut())
            .map_err(|e| format!("{}", e))
    }));

    match result {
        Ok(Ok(ptr)) => ptr,
        Ok(Err(e)) => error_cstring(format!("{}", e)),
        Err(_) => error_cstring("panic in mp3cutad_cut (caught)"),
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
