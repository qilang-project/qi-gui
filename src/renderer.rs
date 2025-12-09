/// Software renderer module using softbuffer
/// Provides image rendering and basic 2D drawing capabilities

use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::rc::Rc;
use std::cell::RefCell;
use std::ops::Deref;
use tao::window::Window as TaoWindow;
/// Software renderer for a window
pub struct Renderer {
    _window: Arc<Mutex<TaoWindow>>,  // Keep window alive
    surface: Rc<RefCell<Surface<Rc<TaoWindow>, Rc<TaoWindow>>>>,
    width: u32,
    height: u32,
}

impl Renderer {
    /// Create a new renderer from Arc<Mutex<TaoWindow>>
    /// This uses an unsafe extraction but is safe in our single-threaded event loop context
    pub fn new_from_arc_mutex(window: Arc<Mutex<TaoWindow>>) -> Result<Self, Box<dyn std::error::Error>> {
        let size = {
            let window_guard = window.lock().unwrap();
            window_guard.inner_size()
        };

        // SAFETY: We're creating an Rc from a raw pointer
        // This is safe because:
        // 1. The Arc<Mutex<>> keeps the window alive
        // 2. The event loop is single-threaded
        // 3. We never move the TaoWindow after creation
        let rc_window: Rc<TaoWindow> = unsafe {
            let ptr = window.lock().unwrap().deref() as *const TaoWindow;
            // Create Rc from the raw pointer
            // Note: This doesn't actually own the window, so we need to be careful
            Rc::from_raw(ptr)
        };

        let context = Context::new(rc_window.clone())?;
        let surface = Surface::new(&context, rc_window.clone())?;

        // Forget the Rc to avoid double-free (Arc still owns it)
        std::mem::forget(rc_window);

        Ok(Renderer {
            _window: window,
            surface: Rc::new(RefCell::new(surface)),
            width: size.width,
            height: size.height,
        })
    }

    /// Create a new renderer for a window (for Rc<TaoWindow>)
    pub fn new(window: Rc<TaoWindow>) -> Result<Self, Box<dyn std::error::Error>> {
        let size = window.inner_size();

        let context = Context::new(window.clone())?;
        let surface = Surface::new(&context, window.clone())?;

        // For this case, we wrap the Rc in Arc<Mutex<>> to match our struct
        let arc_window = Arc::new(Mutex::new(unsafe {
            std::ptr::read(Rc::as_ptr(&window))
        }));
        std::mem::forget(window);

        Ok(Renderer {
            _window: arc_window,
            surface: Rc::new(RefCell::new(surface)),
            width: size.width,
            height: size.height,
        })
    }

    /// Resize the rendering surface
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;

        if width > 0 && height > 0 {
            let _ = self.surface.borrow_mut().resize(
                NonZeroU32::new(width).unwrap(),
                NonZeroU32::new(height).unwrap(),
            );
        }
    }

    /// Get current size
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Clear the surface with a color (RGBA format)
    pub fn clear(&mut self, r: u8, g: u8, b: u8) {
        if self.width == 0 || self.height == 0 {
            return;
        }

        let color = u32::from_be_bytes([0, r, g, b]);

        let mut surface = self.surface.borrow_mut();
        let mut buffer = surface.buffer_mut().unwrap();
        buffer.fill(color);
        let _ = buffer.present();
    }

    /// Draw a filled rectangle
    pub fn draw_rect(&mut self, x: u32, y: u32, width: u32, height: u32, r: u8, g: u8, b: u8) {
        if self.width == 0 || self.height == 0 {
            return;
        }

        let color = u32::from_be_bytes([0, r, g, b]);
        let mut surface = self.surface.borrow_mut();
        let mut buffer = surface.buffer_mut().unwrap();

        for py in y..(y + height).min(self.height) {
            for px in x..(x + width).min(self.width) {
                let idx = (py * self.width + px) as usize;
                if idx < buffer.len() {
                    buffer[idx] = color;
                }
            }
        }

        let _ = buffer.present();
    }

    /// Draw a single pixel
    pub fn draw_pixel(&mut self, x: u32, y: u32, r: u8, g: u8, b: u8) {
        if self.width == 0 || self.height == 0 || x >= self.width || y >= self.height {
            return;
        }

        let color = u32::from_be_bytes([0, r, g, b]);
        let mut surface = self.surface.borrow_mut();
        let mut buffer = surface.buffer_mut().unwrap();

        let idx = (y * self.width + x) as usize;
        if idx < buffer.len() {
            buffer[idx] = color;
        }

        let _ = buffer.present();
    }

    /// Draw a line (simple Bresenham algorithm)
    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, r: u8, g: u8, b: u8) {
        if self.width == 0 || self.height == 0 {
            return;
        }

        let color = u32::from_be_bytes([0, r, g, b]);
        let mut surface = self.surface.borrow_mut();
        let mut buffer = surface.buffer_mut().unwrap();

        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;

        let mut x = x0;
        let mut y = y0;

        loop {
            // Draw pixel if within bounds
            if x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.height {
                let idx = (y as u32 * self.width + x as u32) as usize;
                if idx < buffer.len() {
                    buffer[idx] = color;
                }
            }

            if x == x1 && y == y1 {
                break;
            }

            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }

        let _ = buffer.present();
    }

    /// Draw a circle (midpoint circle algorithm)
    pub fn draw_circle(&mut self, cx: i32, cy: i32, radius: u32, r: u8, g: u8, b: u8) {
        if self.width == 0 || self.height == 0 || radius == 0 {
            return;
        }

        let color = u32::from_be_bytes([0, r, g, b]);
        let mut surface = self.surface.borrow_mut();
        let mut buffer = surface.buffer_mut().unwrap();

        let put_pixel = |buffer: &mut [u32], x: i32, y: i32| {
            if x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.height {
                let idx = (y as u32 * self.width + x as u32) as usize;
                if idx < buffer.len() {
                    buffer[idx] = color;
                }
            }
        };

        let r = radius as i32;
        let mut x = 0;
        let mut y = r;
        let mut d = 3 - 2 * r;

        while x <= y {
            // Draw 8-way symmetry
            put_pixel(&mut buffer, cx + x, cy + y);
            put_pixel(&mut buffer, cx - x, cy + y);
            put_pixel(&mut buffer, cx + x, cy - y);
            put_pixel(&mut buffer, cx - x, cy - y);
            put_pixel(&mut buffer, cx + y, cy + x);
            put_pixel(&mut buffer, cx - y, cy + x);
            put_pixel(&mut buffer, cx + y, cy - x);
            put_pixel(&mut buffer, cx - y, cy - x);

            if d < 0 {
                d = d + 4 * x + 6;
            } else {
                d = d + 4 * (x - y) + 10;
                y -= 1;
            }
            x += 1;
        }

        let _ = buffer.present();
    }

    /// Load and draw an image from file
    pub fn draw_image(&mut self, path: &str, x: u32, y: u32) -> Result<(), Box<dyn std::error::Error>> {
        if self.width == 0 || self.height == 0 {
            return Ok(());
        }

        // Load image
        let img = image::open(path)?;
        let img = img.to_rgba8();
        let (img_width, img_height) = img.dimensions();

        let mut surface = self.surface.borrow_mut();
        let mut buffer = surface.buffer_mut().unwrap();

        // Copy image pixels to buffer
        for py in 0..img_height {
            for px in 0..img_width {
                let screen_x = x + px;
                let screen_y = y + py;

                if screen_x < self.width && screen_y < self.height {
                    let pixel = img.get_pixel(px, py);
                    let r = pixel[0];
                    let g = pixel[1];
                    let b = pixel[2];
                    // Note: ignoring alpha channel for now

                    let color = u32::from_be_bytes([0, r, g, b]);
                    let idx = (screen_y * self.width + screen_x) as usize;
                    if idx < buffer.len() {
                        buffer[idx] = color;
                    }
                }
            }
        }

        buffer.present()?;
        Ok(())
    }

    /// Draw text using a simple built-in bitmap font
    /// This is a basic 8x16 pixel font that supports ASCII characters
    pub fn draw_text(&mut self, text: &str, x: i32, y: i32, r: u8, g: u8, b: u8) {
        if self.width == 0 || self.height == 0 {
            return;
        }

        let color = u32::from_be_bytes([0, r, g, b]);
        let mut surface = self.surface.borrow_mut();
        let mut buffer = surface.buffer_mut().unwrap();

        let char_width = 8;
        let mut current_x = x;

        for ch in text.chars() {
            // Only render printable ASCII for now
            if ch.is_ascii() && !ch.is_ascii_control() {
                let glyph = get_basic_glyph(ch);

                // Draw the glyph
                for (gy, row) in glyph.iter().enumerate() {
                    for gx in 0..8 {
                        if (row >> (7 - gx)) & 1 == 1 {
                            let px = current_x + gx;
                            let py = y + gy as i32;

                            if px >= 0 && py >= 0 && (px as u32) < self.width && (py as u32) < self.height {
                                let idx = (py as u32 * self.width + px as u32) as usize;
                                if idx < buffer.len() {
                                    buffer[idx] = color;
                                }
                            }
                        }
                    }
                }
            }

            current_x += char_width;
        }

        let _ = buffer.present();
    }

    /// Draw text with a custom font size (scaled version of basic font)
    pub fn draw_text_scaled(&mut self, text: &str, x: i32, y: i32, scale: u32, r: u8, g: u8, b: u8) {
        if self.width == 0 || self.height == 0 || scale == 0 {
            return;
        }

        let color = u32::from_be_bytes([0, r, g, b]);
        let mut surface = self.surface.borrow_mut();
        let mut buffer = surface.buffer_mut().unwrap();

        let char_width = 8 * scale;
        let mut current_x = x;

        for ch in text.chars() {
            if ch.is_ascii() && !ch.is_ascii_control() {
                let glyph = get_basic_glyph(ch);

                // Draw scaled glyph
                for (gy, row) in glyph.iter().enumerate() {
                    for gx in 0..8 {
                        if (row >> (7 - gx)) & 1 == 1 {
                            // Draw a scale x scale block for each pixel
                            for sy in 0..scale {
                                for sx in 0..scale {
                                    let px = current_x + (gx * scale as i32) + sx as i32;
                                    let py = y + (gy as u32 * scale) as i32 + sy as i32;

                                    if px >= 0 && py >= 0 && (px as u32) < self.width && (py as u32) < self.height {
                                        let idx = (py as u32 * self.width + px as u32) as usize;
                                        if idx < buffer.len() {
                                            buffer[idx] = color;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            current_x += char_width as i32;
        }

        let _ = buffer.present();
    }
}

/// Get a basic 8x16 bitmap glyph for an ASCII character
/// Returns a 16-element array where each element is a byte representing a row
fn get_basic_glyph(ch: char) -> [u8; 16] {
    match ch {
        ' ' => [0x00; 16],
        '!' => [
            0x00, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18,
            0x18, 0x00, 0x18, 0x18, 0x00, 0x00, 0x00, 0x00,
        ],
        'A' => [
            0x00, 0x00, 0x18, 0x24, 0x24, 0x42, 0x42, 0x7E,
            0x42, 0x42, 0x42, 0x42, 0x00, 0x00, 0x00, 0x00,
        ],
        'B' => [
            0x00, 0x00, 0x7C, 0x42, 0x42, 0x7C, 0x42, 0x42,
            0x42, 0x42, 0x42, 0x7C, 0x00, 0x00, 0x00, 0x00,
        ],
        'C' => [
            0x00, 0x00, 0x3C, 0x42, 0x42, 0x40, 0x40, 0x40,
            0x40, 0x42, 0x42, 0x3C, 0x00, 0x00, 0x00, 0x00,
        ],
        'H' => [
            0x00, 0x00, 0x42, 0x42, 0x42, 0x42, 0x7E, 0x42,
            0x42, 0x42, 0x42, 0x42, 0x00, 0x00, 0x00, 0x00,
        ],
        'e' => [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x3C, 0x42, 0x42,
            0x7E, 0x40, 0x42, 0x3C, 0x00, 0x00, 0x00, 0x00,
        ],
        'l' => [
            0x00, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18,
            0x18, 0x18, 0x18, 0x18, 0x00, 0x00, 0x00, 0x00,
        ],
        'o' => [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x3C, 0x42, 0x42,
            0x42, 0x42, 0x42, 0x3C, 0x00, 0x00, 0x00, 0x00,
        ],
        'W' => [
            0x00, 0x00, 0x82, 0x82, 0x82, 0x92, 0x92, 0xAA,
            0xAA, 0xC6, 0xC6, 0x82, 0x00, 0x00, 0x00, 0x00,
        ],
        'r' => [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x5C, 0x62, 0x40,
            0x40, 0x40, 0x40, 0x40, 0x00, 0x00, 0x00, 0x00,
        ],
        'd' => [
            0x00, 0x02, 0x02, 0x02, 0x02, 0x3E, 0x42, 0x42,
            0x42, 0x42, 0x46, 0x3A, 0x00, 0x00, 0x00, 0x00,
        ],
        // Add more characters as needed...
        // For now, use a simple box for unknown characters
        _ => [
            0x00, 0x00, 0x7E, 0x42, 0x42, 0x42, 0x42, 0x42,
            0x42, 0x42, 0x42, 0x7E, 0x00, 0x00, 0x00, 0x00,
        ],
    }
}
