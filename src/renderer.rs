/// Software renderer module using softbuffer
/// Provides image rendering and basic 2D drawing capabilities

use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::rc::Rc;
use tao::window::Window as TaoWindow;

/// Software renderer for a window
pub struct Renderer {
    surface: Surface<Rc<TaoWindow>, Rc<TaoWindow>>,
    width: u32,
    height: u32,
}

impl Renderer {
    /// Create a new renderer for a window
    pub fn new(window: Rc<TaoWindow>) -> Result<Self, Box<dyn std::error::Error>> {
        let size = window.inner_size();

        let context = Context::new(window.clone())?;
        let surface = Surface::new(&context, window.clone())?;
        let width = size.width;
        let height = size.height;

        Ok(Renderer {
            surface,
            width,
            height,
        })
    }

    /// Resize the rendering surface
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;

        if width > 0 && height > 0 {
            let _ = self.surface.resize(
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

        let mut buffer = self.surface.buffer_mut().unwrap();
        buffer.fill(color);
        let _ = buffer.present();
    }

    /// Draw a filled rectangle
    pub fn draw_rect(&mut self, x: u32, y: u32, width: u32, height: u32, r: u8, g: u8, b: u8) {
        if self.width == 0 || self.height == 0 {
            return;
        }

        let color = u32::from_be_bytes([0, r, g, b]);
        let mut buffer = self.surface.buffer_mut().unwrap();

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
        let mut buffer = self.surface.buffer_mut().unwrap();

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
        let mut buffer = self.surface.buffer_mut().unwrap();

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
        let mut buffer = self.surface.buffer_mut().unwrap();

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

        let mut buffer = self.surface.buffer_mut().unwrap();

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
}
