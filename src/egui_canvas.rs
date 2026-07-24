//! egui 画布层 —— 承接老 tao 渲染器的图元能力，改由 egui painter 自绘
//!
//! 老的 tao 自绘轨（渲染器 + 绘制矩形/圆/线/文本）已淘汰。本文件用 egui 的
//! `allocate_painter` 在 immediate mode 帧里划出一块定尺寸自绘区，图元通过
//! `egui::Painter` 直接画到软件光栅缓冲上。坐标为**画布局部坐标**（左上角 0,0），
//! 内部自动加上画布在窗口中的偏移。
//!
//! ## 用法（帧循环内）
//! ```text
//! 画布开始("场景", 640, 360)
//!   画布矩形(0, 0, 640, 56, 36, 42, 58)
//!   画布圆(320, 180, 28, 255, 184, 76)
//!   画布线(0, 300, 640, 300, 2, 120, 120, 120)
//!   画布文本(20, 14, "标题", 18, 235, 238, 245)
//!   如果 (画布点击() == 1) { ... 画布鼠标X() / 画布鼠标Y() ... }
//! 画布结束()
//! ```
//! 帧循环天然驱动动画：每帧重画即可，无需定时器。

use crate::egui_app::{canvas_begin, canvas_end, cstr, with_top_canvas};
use egui::{Align2, Color32, FontId, Stroke};
use std::os::raw::c_char;

/// 画布开始(id, 宽, 高)：在当前 Ui 占一块定尺寸自绘区。id 目前仅作语义标注。
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_begin_impl(_id: *const c_char, width: i64, height: i64) {
    canvas_begin(width as f32, height as f32);
}

/// 画布结束()
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_end_impl() {
    canvas_end();
}

/// 画布矩形(x, y, 宽, 高, r, g, b)：填充矩形，坐标为画布局部坐标
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_rect_impl(
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    r: i64,
    g: i64,
    b: i64,
) {
    with_top_canvas(|c| {
        let min = c.offset + egui::vec2(x as f32, y as f32);
        let rect = egui::Rect::from_min_size(min, egui::vec2(w.max(0) as f32, h.max(0) as f32));
        c.painter
            .rect_filled(rect, 0.0, Color32::from_rgb(r as u8, g as u8, b as u8));
    });
}

/// 画布圆(x, y, 半径, r, g, b)：填充圆，(x,y) 为圆心的画布局部坐标
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_circle_impl(
    x: i64,
    y: i64,
    radius: i64,
    r: i64,
    g: i64,
    b: i64,
) {
    with_top_canvas(|c| {
        let center = c.offset + egui::vec2(x as f32, y as f32);
        c.painter.circle_filled(
            center,
            radius.max(0) as f32,
            Color32::from_rgb(r as u8, g as u8, b as u8),
        );
    });
}

/// 画布线(x1, y1, x2, y2, 粗, r, g, b)：线段，端点为画布局部坐标
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_line_impl(
    x1: i64,
    y1: i64,
    x2: i64,
    y2: i64,
    width: i64,
    r: i64,
    g: i64,
    b: i64,
) {
    with_top_canvas(|c| {
        let p1 = c.offset + egui::vec2(x1 as f32, y1 as f32);
        let p2 = c.offset + egui::vec2(x2 as f32, y2 as f32);
        let stroke = Stroke::new(
            (width.max(1) as f32).max(0.5),
            Color32::from_rgb(r as u8, g as u8, b as u8),
        );
        c.painter.line_segment([p1, p2], stroke);
    });
}

/// 画布文本(x, y, 文本, 字号, r, g, b)：左上对齐绘制文本，(x,y) 为局部坐标
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_text_impl(
    x: i64,
    y: i64,
    text: *const c_char,
    size: i64,
    r: i64,
    g: i64,
    b: i64,
) {
    let s = cstr(text);
    with_top_canvas(|c| {
        let pos = c.offset + egui::vec2(x as f32, y as f32);
        c.painter.text(
            pos,
            Align2::LEFT_TOP,
            s,
            FontId::proportional((size.max(1) as f32).max(4.0)),
            Color32::from_rgb(r as u8, g as u8, b as u8),
        );
    });
}

/// 画布点击() → 整数：本帧画布是否被点击（1/0）
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_clicked_impl() -> i64 {
    with_top_canvas(|c| if c.response.clicked() { 1 } else { 0 }).unwrap_or(0)
}

/// 画布鼠标X() → 整数：鼠标在画布内的局部 X（无悬停返回 -1）
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_mouse_x_impl() -> i64 {
    with_top_canvas(|c| match c.response.hover_pos() {
        Some(p) => (p.x - c.offset.x) as i64,
        None => -1,
    })
    .unwrap_or(-1)
}

/// 画布鼠标Y() → 整数：鼠标在画布内的局部 Y（无悬停返回 -1）
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_mouse_y_impl() -> i64 {
    with_top_canvas(|c| match c.response.hover_pos() {
        Some(p) => (p.y - c.offset.y) as i64,
        None => -1,
    })
    .unwrap_or(-1)
}
