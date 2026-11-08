//! 广告段预览播放

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::core::range::Range;

use super::state::SharedState;

/// 单次预览播放的最大检查次数
const PLAYBACK_CHECK_INTERVAL: Duration = Duration::from_millis(100);

/// 预览播放广告段（阻塞当前线程，直到播放结束或被停止）
fn preview_ad_segment(file_path: &Path, ad: &Range, stop_flag: Arc<Mutex<bool>>) {
    use rodio::{Decoder, OutputStream, Sink, Source};

    let Ok((_stream, stream_handle)) = OutputStream::try_default() else {
        eprintln!("无法打开音频输出");
        return;
    };

    let Ok(file) = File::open(file_path) else {
        eprintln!("无法打开文件: {}", file_path.display());
        return;
    };

    let Ok(source) = Decoder::new(BufReader::new(file)) else {
        eprintln!("无法解码 MP3 文件");
        return;
    };

    let start_secs = ad.begin as f64 / 10.0;
    let end_secs = ad.end as f64 / 10.0;
    let duration_secs = end_secs - start_secs;

    let source = source
        .skip_duration(Duration::from_secs_f64(start_secs))
        .take_duration(Duration::from_secs_f64(duration_secs));

    let sink = Sink::try_new(&stream_handle).unwrap();
    sink.append(source);
    sink.play();

    let max_checks = (duration_secs * 10.0) as u64 + 10;
    for _ in 0..max_checks {
        if *stop_flag.lock().unwrap() {
            sink.stop();
            return;
        }
        thread::sleep(PLAYBACK_CHECK_INTERVAL);
        if sink.empty() {
            break;
        }
    }
    sink.stop();
}

/// 切换指定广告段的播放状态：正在播放则停止，否则开始播放
pub fn toggle_ad_playback(state: &SharedState, file_idx: usize, ad_idx: usize) {
    let (same_target, any_playing, file_path, ad, file_name, stop_flag) = {
        let s = state.lock().unwrap();
        let Some(file) = s.files.get(file_idx) else {
            return;
        };
        let Some(ad) = file.effective_ads().nth(ad_idx) else {
            return;
        };

        (
            s.playback.playing_file == Some(file_idx) && s.playback.playing_ad == Some(ad_idx),
            s.playback.playing_file.is_some(),
            file.path.clone(),
            *ad,
            file.name.clone(),
            s.playback.stop_flag.clone(),
        )
    };

    // 点击正在播放的段 → 停止
    if same_target {
        stop_playback(state);
        return;
    }

    // 有其他段在播放 → 先让旧线程退出，避免两个音频叠加
    if any_playing {
        *stop_flag.lock().unwrap() = true;
        thread::sleep(Duration::from_millis(150));
    }
    *stop_flag.lock().unwrap() = false;

    {
        let mut s = state.lock().unwrap();
        s.playback.playing_file = Some(file_idx);
        s.playback.playing_ad = Some(ad_idx);
        s.status_message = format!("正在播放: {} 广告{}", file_name, ad_idx + 1);
    }

    let thread_state = state.clone();
    let thread_stop_flag = stop_flag.clone();
    thread::spawn(move || {
        preview_ad_segment(&file_path, &ad, thread_stop_flag);

        // 只清理自己的播放状态，避免覆盖之后开始的新播放
        let mut s = thread_state.lock().unwrap();
        if s.playback.playing_file == Some(file_idx) && s.playback.playing_ad == Some(ad_idx) {
            s.playback.playing_file = None;
            s.playback.playing_ad = None;
        }
    });
}

/// 停止当前播放
pub fn stop_playback(state: &SharedState) {
    let stop_flag = {
        let s = state.lock().unwrap();
        s.playback.stop_flag.clone()
    };
    *stop_flag.lock().unwrap() = true;

    let mut s = state.lock().unwrap();
    s.playback.playing_file = None;
    s.playback.playing_ad = None;
    s.status_message = "播放已停止".to_string();
}
