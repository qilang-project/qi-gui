//! Window management module

use tao::window::{Window as TaoWindow, WindowBuilder, WindowId};
use tao::dpi::LogicalSize;
use std::sync::{Arc, Mutex};

/// Window wrapper
#[derive(Clone)]
pub struct Window {
    inner: Arc<Mutex<TaoWindow>>,
}

impl Window {
    /// Create a new window with title and size
    pub fn new(event_loop: &tao::event_loop::EventLoop<()>, title: &str, width: u32, height: u32) -> Result<Self, String> {
        let window = WindowBuilder::new()
            .with_title(title)
            .with_inner_size(LogicalSize::new(width, height))
            .build(event_loop)
            .map_err(|e| format!("Failed to create window: {}", e))?;

        Ok(Window {
            inner: Arc::new(Mutex::new(window)),
        })
    }

    /// Get the window title
    pub fn title(&self) -> String {
        self.inner.lock().unwrap().title()
    }

    /// Set the window title
    pub fn set_title(&self, title: &str) {
        self.inner.lock().unwrap().set_title(title);
    }

    /// Show the window
    pub fn show(&self) {
        self.inner.lock().unwrap().set_visible(true);
    }

    /// Hide the window
    pub fn hide(&self) {
        self.inner.lock().unwrap().set_visible(false);
    }

    /// Check if window is visible
    pub fn is_visible(&self) -> bool {
        self.inner.lock().unwrap().is_visible()
    }

    /// Get window ID
    pub fn id(&self) -> WindowId {
        self.inner.lock().unwrap().id()
    }

    /// Get window position
    pub fn position(&self) -> (i32, i32) {
        let pos = self.inner.lock().unwrap().outer_position().unwrap_or_default();
        (pos.x, pos.y)
    }

    /// Set window position
    pub fn set_position(&self, x: i32, y: i32) {
        use tao::dpi::PhysicalPosition;
        self.inner.lock().unwrap().set_outer_position(PhysicalPosition::new(x, y));
    }

    /// Get window size (inner size)
    pub fn size(&self) -> (u32, u32) {
        let size = self.inner.lock().unwrap().inner_size();
        (size.width, size.height)
    }

    /// Set window size (inner size)
    pub fn set_size(&self, width: u32, height: u32) {
        use tao::dpi::PhysicalSize;
        self.inner.lock().unwrap().set_inner_size(PhysicalSize::new(width, height));
    }

    /// Get inner Tao window (for internal use)
    pub fn inner(&self) -> Arc<Mutex<TaoWindow>> {
        self.inner.clone()
    }
}
