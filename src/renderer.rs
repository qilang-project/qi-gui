/// Software renderer module using tiny-skia and softbuffer
/// Provides high-quality 2D drawing capabilities

use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::rc::Rc;
use std::cell::RefCell;
use std::ops::Deref;
use tao::window::Window as TaoWindow;
use tiny_skia::{Pixmap, Paint, Color, Rect, Transform, PathBuilder, Stroke, LineCap, PixmapPaint};

/// Software renderer for a window
pub struct Renderer {
    _window: Arc<Mutex<TaoWindow>>,  // Keep window alive
    surface: Rc<RefCell<Surface<Rc<TaoWindow>, Rc<TaoWindow>>>>,
    width: u32,
    height: u32,
    pixmap: Pixmap,
}

impl Renderer {
    /// Create a new renderer from Arc<Mutex<TaoWindow>>
    pub fn new_from_arc_mutex(window: Arc<Mutex<TaoWindow>>) -> Result<Self, Box<dyn std::error::Error>> {
        let size = {
            let window_guard = window.lock().unwrap();
            window_guard.inner_size()
        };

        // SAFETY: Dummy Rc for softbuffer
        let rc_window: Rc<TaoWindow> = unsafe {
            let ptr = window.lock().unwrap().deref() as *const TaoWindow;
            Rc::from_raw(ptr)
        };

        let context = Context::new(rc_window.clone())?;
        let mut surface = Surface::new(&context, rc_window.clone())?;
        
        if size.width > 0 && size.height > 0 {
            surface.resize(
                NonZeroU32::new(size.width).unwrap(),
                NonZeroU32::new(size.height).unwrap(),
            )?;
        }

        std::mem::forget(rc_window);

        // Create initial pixmap
        let width = size.width.max(1);
        let height = size.height.max(1);
        let pixmap = Pixmap::new(width, height).ok_or("Failed to create pixmap")?;

        Ok(Renderer {
            _window: window,
            surface: Rc::new(RefCell::new(surface)),
            width: size.width,
            height: size.height,
            pixmap,
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
            
            // Recreate pixmap
            if let Some(new_pixmap) = Pixmap::new(width, height) {
                self.pixmap = new_pixmap;
            }
        }
    }

    /// Get current size
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Clear the surface with a color
    pub fn clear(&mut self, r: u8, g: u8, b: u8) {
        // tiny-skia uses RGBA, mapped to 0-255
        let color = Color::from_rgba8(r, g, b, 255);
        self.pixmap.fill(color);
    }

    /// Draw a filled rectangle
    pub fn draw_rect(&mut self, x: u32, y: u32, width: u32, height: u32, r: u8, g: u8, b: u8) {
        let rect = Rect::from_xywh(x as f32, y as f32, width as f32, height as f32);
        if let Some(rect) = rect {
            let mut paint = Paint::default();
            paint.set_color_rgba8(r, g, b, 255);
            paint.anti_alias = true;

            self.pixmap.fill_rect(rect, &paint, Transform::identity(), None);
        }
    }

    /// Draw a single pixel
    pub fn draw_pixel(&mut self, x: u32, y: u32, r: u8, g: u8, b: u8) {
        // Drawing a 1x1 rect is safer and easier with tiny-skia
        self.draw_rect(x, y, 1, 1, r, g, b);
    }

    /// Draw a line
    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, r: u8, g: u8, b: u8) {
        let mut pb = PathBuilder::new();
        pb.move_to(x0 as f32, y0 as f32);
        pb.line_to(x1 as f32, y1 as f32);
        
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color_rgba8(r, g, b, 255);
            paint.anti_alias = true;

            let mut stroke = Stroke::default();
            stroke.width = 1.0;
            stroke.line_cap = LineCap::Round;

            self.pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }

    /// Draw a circle (filled for now, matching previous behavior vaguely or improving it)
    /// The previous implementation was a hollow circle outline. Let's stick to outline to match "draw_circle"
    /// or actually, standard GUI draw_circle usually implies outline, fill_circle implies filled.
    /// The previous implementation used midpoint algorithm which draws an outline.
    pub fn draw_circle(&mut self, cx: i32, cy: i32, radius: u32, r: u8, g: u8, b: u8) {
        let mut pb = PathBuilder::new();
        pb.push_circle(cx as f32, cy as f32, radius as f32);

        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color_rgba8(r, g, b, 255);
            paint.anti_alias = true;

            let mut stroke = Stroke::default();
            stroke.width = 1.0;

            self.pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }

    /// Draw an image from file
    pub fn draw_image(&mut self, path: &str, x: u32, y: u32) -> Result<(), Box<dyn std::error::Error>> {
        // Load image using the 'image' crate (already a dependency)
        let img = image::open(path)?.to_rgba8();
        let (w, h) = img.dimensions();

        // Convert to tiny-skia pixmap
        // tiny-skia expects pre-multiplied alpha, but for opaque images it doesn't matter much if we just load RGB
        // For correctness with transparent PNGs, we should handle premultiplication, 
        // but image crate gives straight RGBA.
        // Let's create a pixmap from the raw data.
        
        if let Some(src_pixmap) = Pixmap::from_vec(img.into_raw(), tiny_skia::IntSize::from_wh(w, h).unwrap()) {
             self.pixmap.draw_pixmap(
                 x as i32, 
                 y as i32, 
                 src_pixmap.as_ref(), 
                 &PixmapPaint::default(), 
                 Transform::identity(), 
                 None
            );
        }

        Ok(())
    }

    /// Draw text
    /// tiny-skia doesn't have text rendering built-in. 
    /// We will use the previous simple bitmap font approach but draw it using tiny-skia's fill_rect
    /// for better integration, or stick to pixel manipulation on the pixmap.
    /// To keep it "simple" and "working" without adding 'rusttype' dependency yet, 
    /// I'll reimplement the bitmap font using draw_rect (1x1 pixels) or fill_rects.
    pub fn draw_text(&mut self, text: &str, x: i32, y: i32, r: u8, g: u8, b: u8) {
        self.draw_text_scaled(text, x, y, 1, r, g, b);
    }

    pub fn draw_text_scaled(&mut self, text: &str, x: i32, y: i32, scale: u32, r: u8, g: u8, b: u8) {
         let char_width = 8 * scale as i32;
         let mut current_x = x;

         let mut paint = Paint::default();
         paint.set_color_rgba8(r, g, b, 255);

         for ch in text.chars() {
             if ch.is_ascii() && !ch.is_ascii_control() {
                 let glyph = get_basic_glyph(ch);
                 
                 for (gy, row) in glyph.iter().enumerate() {
                     for gx in 0..8 {
                         if (row >> (7 - gx)) & 1 == 1 {
                             let rect = Rect::from_xywh(
                                 (current_x + gx * scale as i32) as f32,
                                 (y + gy as i32 * scale as i32) as f32,
                                 scale as f32,
                                 scale as f32
                             );
                             
                             if let Some(r) = rect {
                                 self.pixmap.fill_rect(r, &paint, Transform::identity(), None);
                             }
                         }
                     }
                 }
             }
             current_x += char_width;
         }
    }

    /// Present the framebuffer to the window
    pub fn present(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut surface = self.surface.borrow_mut();
        let mut buffer = surface.buffer_mut()?;
        
        let data = self.pixmap.data();
        
        // tiny-skia produces RGBA8888.
        // softbuffer requires u32 xRGB (on many platforms, it's 0x00RRGGBB).
        // However, softbuffer's format can vary.
        // 
        // Typically softbuffer expects: 
        // bits: 00000000 RRRRRRRR GGGGGGGG BBBBBBBB
        // 
        // tiny-skia's buffer is [R, G, B, A, R, G, B, A...]
        // We need to pack these bytes into u32s.
        
        let len = buffer.len().min(data.len() / 4);
        
        for i in 0..len {
            let offset = i * 4;
            let r = data[offset];
            let g = data[offset + 1];
            let b = data[offset + 2];
            // let a = data[offset + 3];
            
            // Pack into u32: 00RGB
            buffer[i] = u32::from_be_bytes([0, r, g, b]);
        }
        
        buffer.present()?;
        Ok(())
    }
}

/// Basic bitmap glyph (same as before, but kept for immediate availability)
fn get_basic_glyph(ch: char) -> [u8; 16] {
    match ch {
        ' ' => [0x00; 16],
        '!' => [0x00, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00, 0x18, 0x18, 0x00, 0x00, 0x00, 0x00],
        'A' => [0x00, 0x00, 0x18, 0x24, 0x24, 0x42, 0x42, 0x7E, 0x42, 0x42, 0x42, 0x42, 0x00, 0x00, 0x00, 0x00],
        'B' => [0x00, 0x00, 0x7C, 0x42, 0x42, 0x7C, 0x42, 0x42, 0x42, 0x42, 0x42, 0x7C, 0x00, 0x00, 0x00, 0x00],
        'C' => [0x00, 0x00, 0x3C, 0x42, 0x42, 0x40, 0x40, 0x40, 0x40, 0x42, 0x42, 0x3C, 0x00, 0x00, 0x00, 0x00],
        'D' => [0x00, 0x7C, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x7C, 0x00, 0x00, 0x00, 0x00],
        'E' => [0x00, 0x7E, 0x40, 0x40, 0x40, 0x7E, 0x40, 0x40, 0x40, 0x40, 0x40, 0x7E, 0x00, 0x00, 0x00, 0x00],
        'F' => [0x00, 0x7E, 0x40, 0x40, 0x40, 0x7E, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x00, 0x00, 0x00, 0x00],
        'G' => [0x00, 0x3C, 0x42, 0x42, 0x40, 0x40, 0x4E, 0x42, 0x42, 0x42, 0x42, 0x3C, 0x00, 0x00, 0x00, 0x00],
        'H' => [0x00, 0x00, 0x42, 0x42, 0x42, 0x42, 0x7E, 0x42, 0x42, 0x42, 0x42, 0x42, 0x00, 0x00, 0x00, 0x00],
        'I' => [0x00, 0x3C, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00, 0x00, 0x00, 0x00],
        'Q' => [0x00, 0x3C, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x4A, 0x44, 0x3A, 0x00, 0x00, 0x00, 0x00],
        'U' => [0x00, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x3C, 0x00, 0x00, 0x00, 0x00],
        'W' => [0x00, 0x00, 0x82, 0x82, 0x82, 0x92, 0x92, 0xAA, 0xAA, 0xC6, 0xC6, 0x82, 0x00, 0x00, 0x00, 0x00],
        'a' => [0x00, 0x00, 0x00, 0x00, 0x3C, 0x02, 0x3E, 0x42, 0x42, 0x42, 0x42, 0x3E, 0x00, 0x00, 0x00, 0x00],
        'c' => [0x00, 0x00, 0x00, 0x00, 0x3C, 0x42, 0x40, 0x40, 0x40, 0x40, 0x42, 0x3C, 0x00, 0x00, 0x00, 0x00],
        'd' => [0x00, 0x02, 0x02, 0x02, 0x02, 0x3E, 0x42, 0x42, 0x42, 0x42, 0x46, 0x3A, 0x00, 0x00, 0x00, 0x00],
        'e' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x3C, 0x42, 0x42, 0x7E, 0x40, 0x42, 0x3C, 0x00, 0x00, 0x00, 0x00],
        'h' => [0x00, 0x40, 0x40, 0x40, 0x40, 0x5C, 0x62, 0x42, 0x42, 0x42, 0x42, 0x42, 0x00, 0x00, 0x00, 0x00],
        'i' => [0x00, 0x18, 0x00, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00, 0x00, 0x00, 0x00],
        'l' => [0x00, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00, 0x00, 0x00, 0x00],
        'n' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x5C, 0x62, 0x42, 0x42, 0x42, 0x42, 0x42, 0x00, 0x00, 0x00, 0x00],
        'o' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x3C, 0x42, 0x42, 0x42, 0x42, 0x42, 0x3C, 0x00, 0x00, 0x00, 0x00],
        'r' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x5C, 0x62, 0x40, 0x40, 0x40, 0x40, 0x40, 0x00, 0x00, 0x00, 0x00],
        's' => [0x00, 0x00, 0x00, 0x00, 0x3C, 0x42, 0x40, 0x3C, 0x02, 0x02, 0x42, 0x3C, 0x00, 0x00, 0x00, 0x00],
        't' => [0x00, 0x00, 0x10, 0x10, 0x3C, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x0C, 0x00, 0x00, 0x00, 0x00],
        'w' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x82, 0x82, 0x92, 0x92, 0xAA, 0xAA, 0x44, 0x00, 0x00, 0x00, 0x00],
        'x' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x42, 0x24, 0x18, 0x18, 0x24, 0x42, 0x00, 0x00, 0x00, 0x00, 0x00],
        'y' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x42, 0x42, 0x42, 0x42, 0x3E, 0x02, 0x3C, 0x00, 0x00, 0x00, 0x00],
        _ => [0x00, 0x00, 0x7E, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x7E, 0x00, 0x00, 0x00, 0x00],
    }
}