//! Qi GUI Library
//!
//! 基于 Tao 的跨平台图形化窗口库，为奇语言提供 GUI 支持

pub mod window;
pub mod event;
pub mod ffi;
pub mod keycode;
pub mod renderer;
pub mod audio;

// Re-export main types
pub use window::Window;
pub use event::EventLoop;
