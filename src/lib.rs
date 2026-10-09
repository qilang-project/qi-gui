//! Qi GUI Library
//!
//! 奇语言的图形化界面库。单轨 egui 架构：winit 0.30 窗口、softbuffer 软件帧缓冲、
//! 自绘 epaint 光栅（无 GL/GPU），immediate mode 控件由 qilang 主循环驱动，
//! 画布层承接图元自绘，键盘层提供逐帧按键查询（做小游戏用）。另含 rodio 音频
//! 播放 FFI。老 tao 自绘轨已移除。
//!
//! 窗口内视频（ffmpeg 子进程解码、rodio 出声，见 `video` / `egui_video`），
//! 原生文件对话框（rfd，`native_dialog`），原生菜单栏（muda，`native_menu`）。

pub mod audio;
pub mod audio_ffi;
pub mod egui_app;
pub mod egui_canvas;
pub mod egui_damage;
pub mod egui_editor;
pub mod egui_editor_buf;
pub mod egui_ime;
pub mod egui_keyboard;
pub mod egui_raster;
pub mod egui_script;
pub mod egui_sprite;
pub mod egui_video;
pub mod egui_widgets2;
pub mod native_dialog;
pub mod native_menu;
pub mod video;
