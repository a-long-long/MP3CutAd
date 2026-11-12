//! 状态栏用的系统占用率采样（CPU / GPU）
//!
//! Windows 下通过 `GetSystemTimes` 计算 CPU 使用率，通过 PDH 的
//! `GPU Engine(*)\Utilization Percentage` 计数器取最忙引擎的占用率。
//! 其他平台或计数器不可用时返回 `None`，界面只显示可用项。

/// 一次采样结果：CPU 百分比、GPU 百分比
#[derive(Debug, Clone, Copy, Default)]
pub struct Snapshot {
    pub cpu: Option<f32>,
    pub gpu: Option<f32>,
}

#[cfg(windows)]
mod imp {
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
        PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA,
    };
    use windows_sys::Win32::System::Threading::GetSystemTimes;

    use super::Snapshot;

    const SAMPLE_INTERVAL: Duration = Duration::from_millis(900);

    pub struct SysMonitor {
        last_sample: Option<Instant>,
        last_cpu: Option<(u64, u64)>, // (idle, total)
        query: isize,
        gpu_counter: Option<GpuCounter>,
        snapshot: Snapshot,
    }

    /// windows-sys 0.52 直接用 `isize` 表示 PDH 句柄
    pub struct GpuCounter(isize);

    impl SysMonitor {
        pub fn new() -> Self {
            let (query, gpu_counter) = open_gpu_query();
            Self {
                last_sample: None,
                last_cpu: None,
                query,
                gpu_counter,
                snapshot: Snapshot::default(),
            }
        }

        pub fn snapshot(&self) -> Snapshot {
            self.snapshot
        }

        /// 每约 1 秒采样一次，其余调用直接复用上次结果
        pub fn sample(&mut self) {
            if let Some(last) = self.last_sample {
                if last.elapsed() < SAMPLE_INTERVAL {
                    return;
                }
            }
            self.last_sample = Some(Instant::now());

            self.snapshot.cpu = self.sample_cpu();
            self.snapshot.gpu = self.sample_gpu();
        }

        fn sample_cpu(&mut self) -> Option<f32> {
            let (idle, total) = system_times()?;
            let prev = self.last_cpu.replace((idle, total));
            let (prev_idle, prev_total) = prev?;

            let delta_total = total.saturating_sub(prev_total);
            let delta_idle = idle.saturating_sub(prev_idle);
            if delta_total == 0 {
                return None;
            }
            let busy = delta_total.saturating_sub(delta_idle);
            Some((busy as f64 / delta_total as f64 * 100.0) as f32)
        }

        fn sample_gpu(&mut self) -> Option<f32> {
            let counter = self.gpu_counter.as_ref()?;
            unsafe {
                if PdhCollectQueryData(self.query) != 0 {
                    return None;
                }

                let mut size = 0u32;
                let mut count = 0u32;
                let status = PdhGetFormattedCounterArrayW(
                    counter.0,
                    PDH_FMT_DOUBLE,
                    &mut size,
                    &mut count,
                    std::ptr::null_mut(),
                );
                if status != PDH_MORE_DATA || size == 0 {
                    return None;
                }

                let item_size = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
                let capacity = (size as usize / item_size) + 1;
                let empty: PDH_FMT_COUNTERVALUE_ITEM_W = std::mem::zeroed();
                let mut buffer = vec![empty; capacity];
                let status = PdhGetFormattedCounterArrayW(
                    counter.0,
                    PDH_FMT_DOUBLE,
                    &mut size,
                    &mut count,
                    buffer.as_mut_ptr(),
                );
                if status != 0 {
                    return None;
                }
                buffer.truncate((count as usize).min(capacity));

                let mut max = 0.0f64;
                for item in &buffer {
                    if item.FmtValue.CStatus == 0 {
                        let value = item.FmtValue.Anonymous.doubleValue;
                        if value > max {
                            max = value;
                        }
                    }
                }
                Some(max.clamp(0.0, 100.0) as f32)
            }
        }
    }

    impl Drop for SysMonitor {
        fn drop(&mut self) {
            if self.query != 0 {
                unsafe { PdhCloseQuery(self.query) };
            }
        }
    }

    fn open_gpu_query() -> (isize, Option<GpuCounter>) {
        unsafe {
            let mut query: isize = 0;
            if PdhOpenQueryW(std::ptr::null(), 0, &mut query) != 0 {
                return (0, None);
            }

            // 英文计数器名，避免系统语言影响
            let path: Vec<u16> = "\\GPU Engine(*)\\Utilization Percentage\0"
                .encode_utf16()
                .collect();
            let mut counter: isize = 0;
            if PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) != 0 {
                PdhCloseQuery(query);
                return (0, None);
            }

            (query, Some(GpuCounter(counter)))
        }
    }

    /// 返回 (idle, total)，单位为 100ns
    fn system_times() -> Option<(u64, u64)> {
        unsafe {
            let mut idle = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let mut kernel = idle;
            let mut user = idle;
            if GetSystemTimes(&mut idle, &mut kernel, &mut user) == 0 {
                return None;
            }
            let idle = filetime_to_u64(idle);
            let total = filetime_to_u64(kernel) + filetime_to_u64(user);
            Some((idle, total))
        }
    }

    fn filetime_to_u64(t: FILETIME) -> u64 {
        ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Snapshot;

    pub struct SysMonitor {
        snapshot: Snapshot,
    }

    impl SysMonitor {
        pub fn new() -> Self {
            Self {
                snapshot: Snapshot::default(),
            }
        }

        pub fn snapshot(&self) -> Snapshot {
            self.snapshot
        }

        pub fn sample(&mut self) {}
    }
}

pub use imp::SysMonitor;
