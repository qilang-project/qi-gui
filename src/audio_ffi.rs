//! 音频 FFI —— rodio 播放器的 C 接口（与窗口/渲染无关，独立于 egui 轨）
//!
//! 老 tao 自绘轨移除后，音频 FFI 从 `ffi.rs` 剥离到本文件，去掉对 tao/GuiState
//! 的依赖：句柄 id 用一个进程内原子计数器发放，播放器存 thread_local。

use crate::audio::AudioPlayer;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_char, CStr, CString};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_AUDIO_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static AUDIO_PLAYERS: RefCell<HashMap<u64, AudioPlayer>> = RefCell::new(HashMap::new());
}

/// 库版本（保留供 版本() 使用）
#[no_mangle]
pub extern "C" fn qi_gui_version_impl() -> *mut c_char {
    CString::new(concat!("qi-gui ", env!("CARGO_PKG_VERSION")))
        .unwrap()
        .into_raw()
}

/// 释放由本库返回的字符串
#[no_mangle]
pub extern "C" fn qi_gui_free_string_impl(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    unsafe {
        let _ = CString::from_raw(s);
    }
}

/// 加载音频文件，返回播放器 id（>0 成功，0 失败）。支持 MP3/WAV/FLAC/Vorbis。
#[no_mangle]
pub extern "C" fn qi_gui_audio_load_impl(file_path: *const c_char) -> u64 {
    if file_path.is_null() {
        return 0;
    }
    let c_str = unsafe { CStr::from_ptr(file_path) };
    let path_str = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let player = match AudioPlayer::new(path_str) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to load audio file {}: {}", path_str, e);
            return 0;
        }
    };
    let audio_id = NEXT_AUDIO_ID.fetch_add(1, Ordering::Relaxed);
    AUDIO_PLAYERS.with(|players| {
        players.borrow_mut().insert(audio_id, player);
    });
    audio_id
}

/// 播放
#[no_mangle]
pub extern "C" fn qi_gui_audio_play_impl(audio_id: u64) {
    if audio_id == 0 {
        return;
    }
    AUDIO_PLAYERS.with(|players| {
        if let Some(player) = players.borrow().get(&audio_id) {
            player.play();
        }
    });
}

/// 暂停
#[no_mangle]
pub extern "C" fn qi_gui_audio_pause_impl(audio_id: u64) {
    if audio_id == 0 {
        return;
    }
    AUDIO_PLAYERS.with(|players| {
        if let Some(player) = players.borrow().get(&audio_id) {
            player.pause();
        }
    });
}

/// 停止
#[no_mangle]
pub extern "C" fn qi_gui_audio_stop_impl(audio_id: u64) {
    if audio_id == 0 {
        return;
    }
    AUDIO_PLAYERS.with(|players| {
        if let Some(player) = players.borrow().get(&audio_id) {
            player.stop();
        }
    });
}

/// 设置音量（0.0..1.0）
#[no_mangle]
pub extern "C" fn qi_gui_audio_set_volume_impl(audio_id: u64, volume: f32) {
    if audio_id == 0 {
        return;
    }
    AUDIO_PLAYERS.with(|players| {
        if let Some(player) = players.borrow().get(&audio_id) {
            player.set_volume(volume);
        }
    });
}

/// 是否正在播放（1/0）
#[no_mangle]
pub extern "C" fn qi_gui_audio_is_playing_impl(audio_id: u64) -> i32 {
    if audio_id == 0 {
        return 0;
    }
    AUDIO_PLAYERS.with(|players| {
        if let Some(player) = players.borrow().get(&audio_id) {
            i32::from(player.is_playing())
        } else {
            0
        }
    })
}

/// 是否播放完成（1/0）
#[no_mangle]
pub extern "C" fn qi_gui_audio_is_finished_impl(audio_id: u64) -> i32 {
    if audio_id == 0 {
        return 0;
    }
    AUDIO_PLAYERS.with(|players| {
        if let Some(player) = players.borrow().get(&audio_id) {
            i32::from(player.is_finished())
        } else {
            0
        }
    })
}

/// 释放播放器
#[no_mangle]
pub extern "C" fn qi_gui_audio_free_impl(audio_id: u64) {
    if audio_id == 0 {
        return;
    }
    AUDIO_PLAYERS.with(|players| {
        players.borrow_mut().remove(&audio_id);
    });
}
