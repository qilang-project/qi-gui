/// Software renderer module using tiny-skia and softbuffer
/// Provides high-quality 2D drawing capabilities
use softbuffer::{Context, Surface};
use std::cell::RefCell;
use std::num::NonZeroU32;
use std::ops::Deref;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use tao::window::Window as TaoWindow;
use tiny_skia::{Color, LineCap, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke, Transform};

use cosmic_text::{Attrs, Buffer as TextBuffer, Color as TextColor, FontSystem, Metrics, Shaping, SwashCache};
use once_cell::sync::Lazy;
use std::sync::Mutex as StdMutex;

// 全局字体系统 / 字形缓存。FontSystem::new() 会加载系统字体（含 PingFang SC /
// Noto / 雅黑 等 CJK 字体），首次稍慢，之后复用。GUI 走主线程事件循环，单线程
// 使用，这里用 Mutex 仅为满足 Sync 约束。
static 字体系统: Lazy<StdMutex<FontSystem>> = Lazy::new(|| StdMutex::new(FontSystem::new()));
static 字形缓存: Lazy<StdMutex<SwashCache>> = Lazy::new(|| StdMutex::new(SwashCache::new()));

/// Software renderer for a window
pub struct Renderer {
    _window: Arc<Mutex<TaoWindow>>, // Keep window alive
    surface: Rc<RefCell<Surface<Rc<TaoWindow>, Rc<TaoWindow>>>>,
    width: u32,
    height: u32,
    pixmap: Pixmap,
    // 设备像素比（retina 上为 2.0）。pixmap 是物理像素尺寸，而上层用逻辑坐标绘制，
    // 故所有绘制按此比例放大，逻辑内容才能铺满整个窗口（否则只占左上角）。
    scale: f32,
}

impl Renderer {
    /// 把逻辑坐标缩放到物理像素的变换（供矢量绘制使用）
    fn 缩放变换(&self) -> Transform {
        Transform::from_scale(self.scale, self.scale)
    }
}

impl Renderer {
    /// Create a new renderer from Arc<Mutex<TaoWindow>>
    pub fn new_from_arc_mutex(
        window: Arc<Mutex<TaoWindow>>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (size, scale_factor) = {
            let window_guard = window.lock().unwrap();
            (
                window_guard.inner_size(),
                window_guard.scale_factor() as f32,
            )
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
            scale: scale_factor.max(1.0),
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

            let t = self.缩放变换();
            self.pixmap.fill_rect(rect, &paint, t, None);
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

            let t = self.缩放变换();
            self.pixmap.stroke_path(&path, &paint, &stroke, t, None);
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

            let t = self.缩放变换();
            self.pixmap.stroke_path(&path, &paint, &stroke, t, None);
        }
    }

    /// Draw an image from file
    pub fn draw_image(
        &mut self,
        path: &str,
        x: u32,
        y: u32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Load image using the 'image' crate (already a dependency)
        let img = image::open(path)?.to_rgba8();
        let (w, h) = img.dimensions();

        // Convert to tiny-skia pixmap
        // tiny-skia expects pre-multiplied alpha, but for opaque images it doesn't matter much if we just load RGB
        // For correctness with transparent PNGs, we should handle premultiplication,
        // but image crate gives straight RGBA.
        // Let's create a pixmap from the raw data.

        if let Some(src_pixmap) =
            Pixmap::from_vec(img.into_raw(), tiny_skia::IntSize::from_wh(w, h).unwrap())
        {
            let s = self.scale;
            self.pixmap.draw_pixmap(
                (x as f32 * s) as i32,
                (y as f32 * s) as i32,
                src_pixmap.as_ref(),
                &PixmapPaint::default(),
                Transform::from_scale(s, s),
                None,
            );
        }

        Ok(())
    }

    /// 绘制文本（cosmic-text 真字体整形 + 栅格化，支持中文等 CJK，系统字体回退）。
    pub fn draw_text(&mut self, text: &str, x: i32, y: i32, r: u8, g: u8, b: u8) {
        self.draw_text_scaled(text, x, y, 1, r, g, b);
    }

    pub fn draw_text_scaled(
        &mut self,
        text: &str,
        x: i32,
        y: i32,
        scale: u32,
        r: u8,
        g: u8,
        b: u8,
    ) {
        if text.is_empty() {
            return;
        }
        // 逻辑字号：scale=1≈14px，每级约 +8px；再乘设备像素比，retina 上字形才清晰。
        let 逻辑字号 = (14.0 + (scale.saturating_sub(1) as f32) * 8.0).max(10.0);
        let font_size = 逻辑字号 * self.scale;
        let line_height = font_size * 1.25;
        // 逻辑坐标 → 物理像素
        let x = (x as f32 * self.scale).round() as i32;
        let y = (y as f32 * self.scale).round() as i32;

        let mut fs = 字体系统.lock().unwrap();
        let mut cache = 字形缓存.lock().unwrap();

        let mut tb = TextBuffer::new(&mut fs, Metrics::new(font_size, line_height));
        let max_w = self.pixmap.width().max(1) as f32;
        tb.set_size(&mut fs, Some(max_w), Some(line_height + 4.0));
        tb.set_text(&mut fs, text, Attrs::new(), Shaping::Advanced);
        tb.shape_until_scroll(&mut fs, false);

        let pixmap_w = self.pixmap.width() as i32;
        let pixmap_h = self.pixmap.height() as i32;
        let pixels = self.pixmap.pixels_mut();
        let text_color = TextColor::rgb(r, g, b);

        tb.draw(&mut fs, &mut cache, text_color, |gx, gy, w, h, color| {
            let sa = color.a() as u32;
            if sa == 0 {
                return;
            }
            let inv = 255 - sa;
            for dy in 0..h as i32 {
                for dx in 0..w as i32 {
                    let px = x + gx + dx;
                    let py = y + gy + dy;
                    if px < 0 || py < 0 || px >= pixmap_w || py >= pixmap_h {
                        continue;
                    }
                    let idx = (py * pixmap_w + px) as usize;
                    let dst = pixels[idx];
                    // 画布不透明(a=255) → premultiplied 等价 straight，按直通 alpha 混合
                    let nr = ((color.r() as u32 * sa + dst.red() as u32 * inv) / 255) as u8;
                    let ng = ((color.g() as u32 * sa + dst.green() as u32 * inv) / 255) as u8;
                    let nb = ((color.b() as u32 * sa + dst.blue() as u32 * inv) / 255) as u8;
                    if let Some(c) = tiny_skia::PremultipliedColorU8::from_rgba(nr, ng, nb, 255) {
                        pixels[idx] = c;
                    }
                }
            }
        });
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
        '!' => [
            0x00, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00, 0x18, 0x18, 0x00, 0x00,
            0x00, 0x00,
        ],
        'A' => [
            0x00, 0x00, 0x18, 0x24, 0x24, 0x42, 0x42, 0x7E, 0x42, 0x42, 0x42, 0x42, 0x00, 0x00,
            0x00, 0x00,
        ],
        'B' => [
            0x00, 0x00, 0x7C, 0x42, 0x42, 0x7C, 0x42, 0x42, 0x42, 0x42, 0x42, 0x7C, 0x00, 0x00,
            0x00, 0x00,
        ],
        'C' => [
            0x00, 0x00, 0x3C, 0x42, 0x42, 0x40, 0x40, 0x40, 0x40, 0x42, 0x42, 0x3C, 0x00, 0x00,
            0x00, 0x00,
        ],
        'D' => [
            0x00, 0x7C, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x7C, 0x00, 0x00,
            0x00, 0x00,
        ],
        'E' => [
            0x00, 0x7E, 0x40, 0x40, 0x40, 0x7E, 0x40, 0x40, 0x40, 0x40, 0x40, 0x7E, 0x00, 0x00,
            0x00, 0x00,
        ],
        'F' => [
            0x00, 0x7E, 0x40, 0x40, 0x40, 0x7E, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x00, 0x00,
            0x00, 0x00,
        ],
        'G' => [
            0x00, 0x3C, 0x42, 0x42, 0x40, 0x40, 0x4E, 0x42, 0x42, 0x42, 0x42, 0x3C, 0x00, 0x00,
            0x00, 0x00,
        ],
        'H' => [
            0x00, 0x00, 0x42, 0x42, 0x42, 0x42, 0x7E, 0x42, 0x42, 0x42, 0x42, 0x42, 0x00, 0x00,
            0x00, 0x00,
        ],
        'I' => [
            0x00, 0x3C, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00, 0x00,
            0x00, 0x00,
        ],
        'Q' => [
            0x00, 0x3C, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x4A, 0x44, 0x3A, 0x00, 0x00,
            0x00, 0x00,
        ],
        'U' => [
            0x00, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x3C, 0x00, 0x00,
            0x00, 0x00,
        ],
        'W' => [
            0x00, 0x00, 0x82, 0x82, 0x82, 0x92, 0x92, 0xAA, 0xAA, 0xC6, 0xC6, 0x82, 0x00, 0x00,
            0x00, 0x00,
        ],
        'a' => [
            0x00, 0x00, 0x00, 0x00, 0x3C, 0x02, 0x3E, 0x42, 0x42, 0x42, 0x42, 0x3E, 0x00, 0x00,
            0x00, 0x00,
        ],
        'c' => [
            0x00, 0x00, 0x00, 0x00, 0x3C, 0x42, 0x40, 0x40, 0x40, 0x40, 0x42, 0x3C, 0x00, 0x00,
            0x00, 0x00,
        ],
        'd' => [
            0x00, 0x02, 0x02, 0x02, 0x02, 0x3E, 0x42, 0x42, 0x42, 0x42, 0x46, 0x3A, 0x00, 0x00,
            0x00, 0x00,
        ],
        'e' => [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x3C, 0x42, 0x42, 0x7E, 0x40, 0x42, 0x3C, 0x00, 0x00,
            0x00, 0x00,
        ],
        'h' => [
            0x00, 0x40, 0x40, 0x40, 0x40, 0x5C, 0x62, 0x42, 0x42, 0x42, 0x42, 0x42, 0x00, 0x00,
            0x00, 0x00,
        ],
        'i' => [
            0x00, 0x18, 0x00, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00, 0x00,
            0x00, 0x00,
        ],
        'l' => [
            0x00, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00, 0x00,
            0x00, 0x00,
        ],
        'n' => [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x5C, 0x62, 0x42, 0x42, 0x42, 0x42, 0x42, 0x00, 0x00,
            0x00, 0x00,
        ],
        'o' => [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x3C, 0x42, 0x42, 0x42, 0x42, 0x42, 0x3C, 0x00, 0x00,
            0x00, 0x00,
        ],
        'r' => [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x5C, 0x62, 0x40, 0x40, 0x40, 0x40, 0x40, 0x00, 0x00,
            0x00, 0x00,
        ],
        's' => [
            0x00, 0x00, 0x00, 0x00, 0x3C, 0x42, 0x40, 0x3C, 0x02, 0x02, 0x42, 0x3C, 0x00, 0x00,
            0x00, 0x00,
        ],
        't' => [
            0x00, 0x00, 0x10, 0x10, 0x3C, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x0C, 0x00, 0x00,
            0x00, 0x00,
        ],
        'w' => [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x82, 0x82, 0x92, 0x92, 0xAA, 0xAA, 0x44, 0x00, 0x00,
            0x00, 0x00,
        ],
        'x' => [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x42, 0x24, 0x18, 0x18, 0x24, 0x42, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'y' => [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x42, 0x42, 0x42, 0x42, 0x3E, 0x02, 0x3C, 0x00, 0x00,
            0x00, 0x00,
        ],
        _ => [
            0x00, 0x00, 0x7E, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x7E, 0x00, 0x00,
            0x00, 0x00,
        ],
    }
}
