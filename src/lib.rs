//! Qi GUI Library
//!
//! 奇语言的图形化界面库。单轨 egui 架构：winit 0.30 窗口 + softbuffer 软件帧缓冲
//! + 自绘 epaint 光栅（无 GL/GPU），immediate mode 控件由 qilang 主循环驱动，
//! 画布层承接图元自绘。另含 rodio 音频播放 FFI。老 tao 自绘轨已移除。

pub mod audio;
pub mod audio_ffi;
pub mod egui_app;
pub mod egui_canvas;
pub mod egui_raster;
pub mod egui_sprite;
pub mod egui_widgets2;
