//! egui 编辑器层 FFI：富文本行 / 代码块 / 分栏 / 编辑区 / 组合键
//!
//! 给文字编辑器一类程序用，但每一样都是通用原语：
//! - 富文本行：`rich_begin` → 若干 `rich_span`（每段独立的字号/粗斜体/等宽/删除线/
//!   下划线/行内代码底色/颜色）→ `rich_end`（整行自动折行，可缩进、可画引用竖线）
//! - 代码块：等宽字体 + 底色框
//! - 分栏：`columns_begin(左栏千分比)` / `columns_next` / `columns_end`，
//!   两栏占满剩余区域，中间画分隔线
//! - 编辑区：撑满剩余区域的多行输入，自带纵向滚动；能读写光标（字符下标），
//!   状态栏「行:列」和以后的协同编辑都靠它。大文档用 egui_editor_buf.rs 的托管编辑区
//!   （正文留在 Rust、按版本取、带源码高亮），这里的编辑区每帧往返全文
//! - 组合键：Cmd（macOS）/ Ctrl（其它）+ 键，可选 Shift；按下即消费，不再落进输入框
//! - 外观：`设置外观` 一次给全套配色和基准字号；`设置窗口边距`；`区域开始/结束`
//!   （定高或撑满、内边距、底色、最大宽居中）；`右对齐开始/结束`；`空白(像素)`

use crate::egui_app::{cstr, ret_str, with_ctx, with_top_ui, Container, FRAME};
use crate::egui_keyboard::{key_from_name, KeyTarget};
use std::cell::{Cell, RefCell};
use std::os::raw::c_char;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{
    pos2, vec2, Align, Color32, FontFamily, FontId, Id, Layout, Modifiers, Rect, Sense, Stroke,
    TextStyle, UiBuilder,
};

/// 片段样式位（与 qi 侧 `图形.富文本片段` 的样式参数一致）
pub const FLAG_BOLD: i64 = 1;
pub const FLAG_ITALIC: i64 = 2;
pub const FLAG_MONO: i64 = 4;
pub const FLAG_STRIKE: i64 = 8;
pub const FLAG_UNDERLINE: i64 = 16;
pub const FLAG_CODE_BG: i64 = 32;

/// 粗体字族名。只有找到系统粗体字体并注册成功后才用，否则退回常规字族 ——
/// egui 遇到没注册的 FontFamily::Name 会直接 panic。
const BOLD_FAMILY: &str = "qi_bold";
static BOLD_READY: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// 根 Ui 离窗口边缘的距离（帧开始时读）
    static WINDOW_MARGIN: Cell<f32> = const { Cell::new(10.0) };
    /// 正在拼的富文本行
    static RICH: RefCell<Option<LayoutJob>> = const { RefCell::new(None) };
    /// 最近一次编辑区调用后的光标（字符下标）；没有焦点时为 -1
    static LAST_CURSOR: Cell<i64> = const { Cell::new(-1) };
    /// 同一个光标的（行, 列），都从 1 起；没有焦点时 (-1, -1)
    static LAST_ROW_COL: Cell<(i64, i64)> = const { Cell::new((-1, -1)) };
}

/// 注册粗体字族（字体安装时调用）。找不到粗体字体就不注册，片段的粗体退回常规字重。
pub(crate) fn install_bold_font(fonts: &mut egui::FontDefinitions) {
    let candidates: &[&str] = &[
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Bold.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Bold.otf",
        "C:\\Windows\\Fonts\\msyhbd.ttc",
        "C:\\Windows\\Fonts\\msyhbd.ttf",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("qi_bold_font".to_owned(), egui::FontData::from_owned(bytes));
            // 粗体字体打头，后面接常规字族的全部回退（emoji、符号）
            let mut family = vec!["qi_bold_font".to_owned()];
            if let Some(regular) = fonts.families.get(&FontFamily::Proportional) {
                family.extend(regular.iter().cloned());
            }
            fonts
                .families
                .insert(FontFamily::Name(BOLD_FAMILY.into()), family);
            BOLD_READY.store(true, Ordering::Relaxed);
            return;
        }
    }
}

pub(crate) fn unpack_rgb(c: i64) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// 粗体字族；系统里没找到粗体字体时为 None（调用方退回常规字族）
pub(crate) fn bold_family() -> Option<FontFamily> {
    BOLD_READY
        .load(Ordering::Relaxed)
        .then(|| FontFamily::Name(BOLD_FAMILY.into()))
}

/// 设置外观 给的弱文字色；没设过为 None
pub(crate) fn weak_color() -> Option<Color32> {
    let c = WEAK.with(|c| c.get());
    (c != Color32::TRANSPARENT).then_some(c)
}

/// 记下编辑区光标：字符下标 + 逻辑行（按换行分的段）与段内第几个字符
pub(crate) fn set_last_cursor(cursor: Option<egui::epaint::text::cursor::Cursor>) {
    LAST_CURSOR.with(|c| c.set(cursor.map_or(-1, |p| p.ccursor.index as i64)));
    LAST_ROW_COL.with(|c| {
        c.set(cursor.map_or((-1, -1), |p| {
            (p.pcursor.paragraph as i64 + 1, p.pcursor.offset as i64 + 1)
        }))
    });
}

fn span_format(ui: &egui::Ui, size: i64, flags: i64, color: i64) -> TextFormat {
    let body = ui
        .style()
        .text_styles
        .get(&TextStyle::Body)
        .map(|f| f.size)
        .unwrap_or(14.0);
    let size = if size > 0 { size as f32 } else { body };
    let family = if flags & FLAG_MONO != 0 {
        FontFamily::Monospace
    } else if flags & FLAG_BOLD != 0 && BOLD_READY.load(Ordering::Relaxed) {
        FontFamily::Name(BOLD_FAMILY.into())
    } else {
        FontFamily::Proportional
    };
    let v = ui.visuals();
    let color = if color >= 0 {
        unpack_rgb(color)
    } else if flags & FLAG_BOLD != 0 {
        v.strong_text_color()
    } else {
        v.text_color()
    };
    // 行高：正文 1.6 倍读起来松快，大字号（标题）1.3 倍；等宽（行内代码）跟着正文走
    let line_height = if size >= 20.0 { size * 1.3 } else { size * 1.6 };
    let line = |on: bool| {
        if on {
            Stroke::new(1.0, color)
        } else {
            Stroke::NONE
        }
    };
    TextFormat {
        font_id: FontId::new(size, family),
        color,
        italics: flags & FLAG_ITALIC != 0,
        underline: line(flags & FLAG_UNDERLINE != 0),
        strikethrough: line(flags & FLAG_STRIKE != 0),
        background: if flags & FLAG_CODE_BG != 0 {
            v.code_bg_color
        } else {
            Color32::TRANSPARENT
        },
        line_height: Some(line_height),
        valign: Align::Center,
        ..Default::default()
    }
}

/// 富文本开始()：开始拼一行
#[no_mangle]
pub extern "C" fn qi_gui_egui_rich_begin_impl() {
    RICH.with(|r| *r.borrow_mut() = Some(LayoutJob::default()));
}

/// 富文本片段(文本, 字号, 样式位, 颜色)：字号 ≤0 = 正文字号；颜色 <0 = 主题文字色
#[no_mangle]
pub extern "C" fn qi_gui_egui_rich_span_impl(
    text: *const c_char,
    size: i64,
    flags: i64,
    color: i64,
) {
    let t = cstr(text);
    if t.is_empty() {
        return;
    }
    with_top_ui(|ui| {
        let fmt = span_format(ui, size, flags, color);
        RICH.with(|r| {
            if let Some(job) = r.borrow_mut().as_mut() {
                job.append(&t, 0.0, fmt);
            }
        });
    });
}

/// 富文本结束(缩进, 竖线)：整行按可用宽度折行后放下。缩进单位是像素；
/// 竖线=1 时在缩进区画一条引用竖线（Markdown 的 > 引用）。
#[no_mangle]
pub extern "C" fn qi_gui_egui_rich_end_impl(indent: i64, bar: i64) {
    let Some(mut job) = RICH.with(|r| r.borrow_mut().take()) else {
        return;
    };
    with_top_ui(|ui| {
        let indent = indent.max(0) as f32;
        let avail = ui.available_width();
        job.wrap.max_width = (avail - indent).max(20.0);
        let galley = ui.fonts(|f| f.layout_job(job));
        let (rect, _) = ui.allocate_exact_size(vec2(avail, galley.size().y), Sense::hover());
        if bar != 0 {
            let x = rect.min.x + (indent * 0.4).max(2.0);
            let color = ui.visuals().widgets.noninteractive.bg_stroke.color;
            ui.painter().line_segment(
                [pos2(x, rect.min.y), pos2(x, rect.max.y)],
                Stroke::new(3.0, color),
            );
        }
        let text_color = ui.visuals().text_color();
        ui.painter()
            .galley(pos2(rect.min.x + indent, rect.min.y), galley, text_color);
    });
}

/// 代码块(文本, 字号)：等宽 + 底色框，占满可用宽度；字号 <=0 用等宽默认字号
#[no_mangle]
pub extern "C" fn qi_gui_egui_code_block_impl(text: *const c_char, size: i64) {
    let t = cstr(text);
    with_top_ui(|ui| {
        let mut rich = egui::RichText::new(t).monospace();
        if size > 0 {
            rich = rich.size(size as f32);
        }
        egui::Frame::none()
            .fill(ui.visuals().code_bg_color)
            .rounding(4.0)
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(egui::Label::new(rich).wrap());
            });
    });
}

fn column_child(parent: &mut egui::Ui, rect: Rect) -> egui::Ui {
    let mut child = parent.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    );
    child.set_clip_rect(rect.intersect(parent.clip_rect()));
    child
}

/// 分栏开始(左栏千分比)：把剩余区域左右分成两栏（100–900‰），先进左栏
#[no_mangle]
pub extern "C" fn qi_gui_egui_columns_begin_impl(left_permille: i64) {
    FRAME.with(|fr| {
        let mut b = fr.borrow_mut();
        let Some(frame) = b.as_mut() else {
            return;
        };
        let Some(&parent_ptr) = frame.ui_stack.last() else {
            return;
        };
        let parent = unsafe { &mut *parent_ptr };
        let full = parent.available_rect_before_wrap();
        let gap = 1.0;
        let ratio = left_permille.clamp(100, 900) as f32 / 1000.0;
        let left_w = ((full.width() - gap) * ratio).max(40.0);
        let left = Rect::from_min_size(full.min, vec2(left_w, full.height()));
        let right = Rect::from_min_max(pos2(left.max.x + gap, full.min.y), full.max);
        let x = left.max.x + 0.5;
        let color = parent.visuals().widgets.noninteractive.bg_stroke.color;
        parent.painter().line_segment(
            [pos2(x, full.min.y), pos2(x, full.max.y)],
            Stroke::new(1.0, color),
        );
        let child = column_child(parent, left);
        frame.ui_stack.push(Box::into_raw(Box::new(child)));
        frame.containers.push(Container::Columns {
            full,
            right,
            on_right: false,
        });
    });
}

/// 分栏下一栏()：从左栏切到右栏
#[no_mangle]
pub extern "C" fn qi_gui_egui_columns_next_impl() {
    FRAME.with(|fr| {
        let mut b = fr.borrow_mut();
        let Some(frame) = b.as_mut() else {
            return;
        };
        let right = match frame.containers.last_mut() {
            Some(Container::Columns {
                right, on_right, ..
            }) if !*on_right => {
                *on_right = true;
                *right
            }
            _ => return, // 配对错乱：忽略
        };
        if frame.ui_stack.len() <= 1 {
            return;
        }
        drop(unsafe { Box::from_raw(frame.ui_stack.pop().unwrap()) });
        let parent = unsafe { &mut **frame.ui_stack.last().unwrap() };
        let child = column_child(parent, right);
        frame.ui_stack.push(Box::into_raw(Box::new(child)));
    });
}

/// 分栏结束()：收起当前栏，父光标推进过整个分栏区域
#[no_mangle]
pub extern "C" fn qi_gui_egui_columns_end_impl() {
    FRAME.with(|fr| {
        let mut b = fr.borrow_mut();
        let Some(frame) = b.as_mut() else {
            return;
        };
        if !matches!(frame.containers.last(), Some(Container::Columns { .. })) {
            return;
        }
        let Some(Container::Columns { full, .. }) = frame.containers.pop() else {
            return;
        };
        if frame.ui_stack.len() <= 1 {
            return;
        }
        drop(unsafe { Box::from_raw(frame.ui_stack.pop().unwrap()) });
        let parent = unsafe { &mut **frame.ui_stack.last().unwrap() };
        advance_flush(parent, full);
    });
}

pub(crate) fn editor_id(id: &str) -> Id {
    Id::new(("qi_editor", id))
}

/// 编辑区(id, 当前值, 等宽, 字号) → 新值：撑满剩余区域的多行输入，自带纵向滚动。
/// Tab 键插入制表符而不是切焦点。调用后可用 编辑区光标() 取光标位置。
#[no_mangle]
pub extern "C" fn qi_gui_egui_editor_impl(
    id: *const c_char,
    value: *const c_char,
    mono: i64,
    size: i64,
) -> *const c_char {
    let id = cstr(id);
    let mut buf = cstr(value);
    let mut cursor = None;
    with_top_ui(|ui| {
        let avail = ui.available_size();
        let family = if mono != 0 {
            FontFamily::Monospace
        } else {
            FontFamily::Proportional
        };
        let body = ui
            .style()
            .text_styles
            .get(&TextStyle::Body)
            .map(|f| f.size)
            .unwrap_or(14.0);
        let px = if size > 0 { size as f32 } else { body };
        let font = FontId::new(px, family);
        let color = ui.visuals().text_color();
        // 自己排版：给每行 1.6 倍行高（TextEdit 默认行距太挤，中文尤其）
        let mut layouter = |ui: &egui::Ui, text: &str, wrap_width: f32| {
            let mut job = LayoutJob::simple(text.to_owned(), font.clone(), color, wrap_width);
            for section in &mut job.sections {
                section.format.line_height = Some(px * 1.6);
                section.format.valign = Align::Center;
            }
            ui.fonts(|f| f.layout_job(job))
        };
        egui::ScrollArea::vertical()
            .id_salt(("qi_editor_scroll", &id))
            .auto_shrink([false, false])
            .max_height(avail.y)
            .show(ui, |ui| {
                let out = egui::TextEdit::multiline(&mut buf)
                    .id(editor_id(&id))
                    .layouter(&mut layouter)
                    .frame(false)
                    .margin(vec2(0.0, 0.0))
                    .lock_focus(true)
                    .desired_width(f32::INFINITY)
                    .min_size(vec2(0.0, avail.y))
                    .show(ui);
                cursor = out.cursor_range.map(|r| r.primary);
            });
    });
    set_last_cursor(cursor);
    ret_str(buf)
}

/// 编辑区光标() → 最近一次 编辑区 调用后的光标字符下标；没有光标时 -1
#[no_mangle]
pub extern "C" fn qi_gui_egui_editor_cursor_impl() -> i64 {
    LAST_CURSOR.with(|c| c.get())
}

/// 编辑区光标行() → 光标在第几行（按换行分，从 1 起，折行不算）；没有光标时 -1。
/// 状态栏「行:列」直接用它，qi 不必为了数行把正文切开
#[no_mangle]
pub extern "C" fn qi_gui_egui_editor_cursor_row_impl() -> i64 {
    LAST_ROW_COL.with(|c| c.get().0)
}

/// 编辑区光标列() → 光标是本行第几个字符（从 1 起）；没有光标时 -1
#[no_mangle]
pub extern "C" fn qi_gui_egui_editor_cursor_col_impl() -> i64 {
    LAST_ROW_COL.with(|c| c.get().1)
}

/// 设置编辑区光标(id, 字符下标)：下一帧生效（协同编辑合入远端改动后挪光标用）
#[no_mangle]
pub extern "C" fn qi_gui_egui_editor_set_cursor_impl(id: *const c_char, pos: i64) {
    let id = editor_id(&cstr(id));
    with_ctx(|ctx| {
        let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
        let c = CCursor::new(pos.max(0) as usize);
        state.cursor.set_char_range(Some(CCursorRange::one(c)));
        state.store(ctx, id);
    });
}

/// 组合键(键名, 上档) → 1/0：Cmd（macOS）/ Ctrl（其它）+ 键本帧刚按下。
/// 上档=1 要求同时按 Shift，上档=0 要求没按 Shift（⌘S 与 ⇧⌘S 互不串）。
/// 命中即消费该按键事件，不会再落进输入框。
#[no_mangle]
pub extern "C" fn qi_gui_egui_shortcut_impl(name: *const c_char, shift: i64) -> i64 {
    let n = cstr(name);
    let Some(KeyTarget::Key(key)) = key_from_name(&n) else {
        return 0;
    };
    with_ctx(|ctx| {
        ctx.input_mut(|i| {
            if i.modifiers.shift != (shift != 0) {
                return false;
            }
            let mods = if shift != 0 {
                Modifiers::COMMAND | Modifiers::SHIFT
            } else {
                Modifiers::COMMAND
            };
            i.consume_key(mods, key)
        })
    })
    .map(i64::from)
    .unwrap_or(0)
}

// ============================================================================
// 外观与布局
// ============================================================================

/// 推进父光标但不加纵向条目间距：区域、分栏是拼版用的块，上下要严丝合缝
/// （工具栏、分隔线、正文、状态栏之间不该露出一条条底色缝）
fn advance_flush(parent: &mut egui::Ui, rect: Rect) {
    let saved = parent.spacing().item_spacing;
    parent.spacing_mut().item_spacing.y = 0.0;
    parent.advance_cursor_after_rect(rect);
    parent.spacing_mut().item_spacing = saved;
}

pub(crate) fn window_margin() -> f32 {
    WINDOW_MARGIN.with(|m| m.get())
}

/// 设置窗口边距(像素)：根区域离窗口边缘多远（默认 10；做贴边的工具栏/状态栏设 0）
#[no_mangle]
pub extern "C" fn qi_gui_egui_window_margin_impl(px: i64) {
    WINDOW_MARGIN.with(|m| m.set(px.clamp(0, 200) as f32));
}

/// 两色按比例混合：t=0 全是 a，t=1 全是 b
pub(crate) fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// 设置外观(底色, 文字, 弱文字, 强调, 边框, 字号)：一次给全套浅/深色配色和基准字号。
/// 颜色是打包整数 r*65536+g*256+b。代码底色、斑马纹、悬停底色都从底色和文字按比例混出来；
/// 按钮平时没有底，悬停才显出来（工具栏那种扁平按钮）。每帧调用也无妨。
#[no_mangle]
pub extern "C" fn qi_gui_egui_set_appearance_impl(
    bg: i64,
    text: i64,
    weak: i64,
    accent: i64,
    border: i64,
    size: i64,
) {
    let (bg, text, weak, accent, border) = (
        unpack_rgb(bg),
        unpack_rgb(text),
        unpack_rgb(weak),
        unpack_rgb(accent),
        unpack_rgb(border),
    );
    let size = if size > 0 { size as f32 } else { 14.0 };
    with_ctx(|ctx| {
        ctx.style_mut(|style| {
            let prop = |px: f32| FontId::new(px, FontFamily::Proportional);
            style.text_styles = [
                (TextStyle::Small, prop(size - 2.0)),
                (TextStyle::Body, prop(size)),
                (TextStyle::Button, prop(size)),
                (TextStyle::Heading, prop(size * 1.45)),
                (
                    TextStyle::Monospace,
                    FontId::new(size - 1.0, FontFamily::Monospace),
                ),
            ]
            .into();
            style.spacing.item_spacing = vec2(8.0, 6.0);
            style.spacing.button_padding = vec2(10.0, 4.0);
            style.spacing.interact_size.y = size + 12.0;
            // 滚动条浮在内容上、不悬停就隐去（写作时不该有一根深色竖条杵在旁边）
            style.spacing.scroll = egui::style::ScrollStyle {
                // 滑块用浅底色系而不是正文色 —— 默认的深色滑块在白纸上像一道划痕
                foreground_color: false,
                ..egui::style::ScrollStyle::floating()
            };

            let v = &mut style.visuals;
            v.dark_mode = (bg.r() as u32 + bg.g() as u32 + bg.b() as u32) < 384;
            v.override_text_color = None;
            v.panel_fill = bg;
            v.window_fill = bg;
            v.extreme_bg_color = bg;
            v.faint_bg_color = mix(bg, text, 0.035);
            v.code_bg_color = mix(bg, text, 0.06);
            v.hyperlink_color = accent;
            v.window_stroke = Stroke::new(1.0, border);
            // 选中态（选区、选择项）：淡淡一层强调色，字保持正文色 —— 红底红字太扎眼
            v.selection.bg_fill = mix(bg, accent, 0.14);
            v.selection.stroke = Stroke::new(1.0, text);
            v.text_cursor.stroke = Stroke::new(2.0, accent);
            let round = egui::Rounding::same(5.0);
            let hover = mix(bg, text, 0.07);
            let press = mix(bg, text, 0.12);
            let w = &mut v.widgets;
            w.noninteractive.bg_fill = bg;
            w.noninteractive.weak_bg_fill = bg;
            w.noninteractive.bg_stroke = Stroke::new(1.0, border);
            w.noninteractive.fg_stroke = Stroke::new(1.0, text);
            w.noninteractive.rounding = round;
            w.inactive.bg_fill = mix(bg, text, 0.05);
            w.inactive.weak_bg_fill = Color32::TRANSPARENT;
            w.inactive.bg_stroke = Stroke::NONE;
            w.inactive.fg_stroke = Stroke::new(1.0, text);
            w.inactive.rounding = round;
            w.hovered.bg_fill = hover;
            w.hovered.weak_bg_fill = hover;
            w.hovered.bg_stroke = Stroke::NONE;
            w.hovered.fg_stroke = Stroke::new(1.5, text);
            w.hovered.rounding = round;
            w.hovered.expansion = 0.0;
            w.active.bg_fill = press;
            w.active.weak_bg_fill = press;
            w.active.bg_stroke = Stroke::NONE;
            w.active.fg_stroke = Stroke::new(1.5, text);
            w.active.rounding = round;
            w.active.expansion = 0.0;
            w.open = w.active;
            // 弱文字（状态栏、提示）egui 没有单独的槽，记下来给 弱标签 用
            WEAK.with(|c| c.set(weak));
        });
    });
}

thread_local! {
    /// 设置外观给的弱文字色（弱文字标签用）；没设过就从主题算
    static WEAK: Cell<Color32> = const { Cell::new(Color32::TRANSPARENT) };
}

/// 弱标签(文本)：次要信息（状态栏、提示）用的浅色文字
#[no_mangle]
pub extern "C" fn qi_gui_egui_weak_label_impl(text: *const c_char) {
    let t = cstr(text);
    with_top_ui(|ui| {
        let set = WEAK.with(|c| c.get());
        let color = if set == Color32::TRANSPARENT {
            ui.visuals().weak_text_color()
        } else {
            set
        };
        ui.label(egui::RichText::new(t).color(color));
    });
}

/// 空白(像素)：沿当前布局方向留一段空
#[no_mangle]
pub extern "C" fn qi_gui_egui_add_space_impl(px: i64) {
    with_top_ui(|ui| ui.add_space(px.max(0) as f32));
}

/// 区域开始(高度, 内边距, 背景色, 最大宽)：
/// - 高度 >0 定高；0 随内容长；-1 撑满剩余高度；-(N+1) 撑满但底下让出 N 像素
///   （给状态栏留地方）。在滚动区里剩余高度无限，撑满也按随内容长处理
/// - 背景色 <0 不画底
/// - 最大宽 >0 且可用宽度更宽时，内容区按最大宽水平居中（阅读栏）
#[no_mangle]
pub extern "C" fn qi_gui_egui_region_begin_impl(height: i64, pad: i64, bg: i64, max_width: i64) {
    FRAME.with(|fr| {
        let mut b = fr.borrow_mut();
        let Some(frame) = b.as_mut() else {
            return;
        };
        let Some(&parent_ptr) = frame.ui_stack.last() else {
            return;
        };
        let parent = unsafe { &mut *parent_ptr };
        let avail = parent.available_rect_before_wrap();
        let fixed = if height > 0 {
            Some(height as f32)
        } else if height < 0 && avail.height().is_finite() {
            let leave = (-height - 1) as f32;
            Some((avail.height() - leave).max(0.0))
        } else {
            None
        };
        let outer = Rect::from_min_size(
            avail.min,
            vec2(avail.width(), fixed.unwrap_or(f32::INFINITY)),
        );
        let pad = pad.max(0) as f32;
        let mut inner = outer.shrink(pad);
        if max_width > 0 && inner.width() > max_width as f32 {
            let extra = (inner.width() - max_width as f32) / 2.0;
            inner = inner.shrink2(vec2(extra, 0.0));
        }
        // 先占一个形状位：底色要画在内容下面，而随内容长的区域到结束才知道多高
        let bg_slot = if bg >= 0 {
            Some((parent.painter().add(egui::Shape::Noop), unpack_rgb(bg)))
        } else {
            None
        };
        let mut child = parent.new_child(
            UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::top_down(Align::Min)),
        );
        if fixed.is_some() {
            child.set_clip_rect(outer.intersect(parent.clip_rect()));
        }
        frame.ui_stack.push(Box::into_raw(Box::new(child)));
        frame.containers.push(Container::Region {
            outer,
            pad,
            fixed: fixed.is_some(),
            bg_slot,
        });
    });
}

/// 区域结束()：补画底色，父光标推进过整个区域
#[no_mangle]
pub extern "C" fn qi_gui_egui_region_end_impl() {
    FRAME.with(|fr| {
        let mut b = fr.borrow_mut();
        let Some(frame) = b.as_mut() else {
            return;
        };
        if !matches!(frame.containers.last(), Some(Container::Region { .. })) {
            return;
        }
        let Some(Container::Region {
            outer,
            pad,
            fixed,
            bg_slot,
        }) = frame.containers.pop()
        else {
            return;
        };
        if frame.ui_stack.len() <= 1 {
            return;
        }
        let child = unsafe { Box::from_raw(frame.ui_stack.pop().unwrap()) };
        let content_bottom = child.min_rect().max.y;
        drop(child);
        let parent = unsafe { &mut **frame.ui_stack.last().unwrap() };
        let rect = if fixed {
            outer
        } else {
            Rect::from_min_max(outer.min, pos2(outer.max.x, content_bottom + pad))
        };
        if let Some((slot, color)) = bg_slot {
            parent
                .painter()
                .set(slot, egui::Shape::rect_filled(rect, 0.0, color));
        }
        advance_flush(parent, rect);
    });
}

/// 右对齐开始()：在当前这一行剩下的宽度里从右往左排（先写的在最右）
#[no_mangle]
pub extern "C" fn qi_gui_egui_right_begin_impl() {
    FRAME.with(|fr| {
        let mut b = fr.borrow_mut();
        let Some(frame) = b.as_mut() else {
            return;
        };
        let Some(&parent_ptr) = frame.ui_stack.last() else {
            return;
        };
        let parent = unsafe { &mut *parent_ptr };
        let avail = parent.available_rect_before_wrap();
        let row_h = parent.spacing().interact_size.y;
        let rect = Rect::from_min_size(avail.min, vec2(avail.width(), row_h));
        let child = parent.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::right_to_left(Align::Center)),
        );
        frame.ui_stack.push(Box::into_raw(Box::new(child)));
        frame.containers.push(Container::Right);
    });
}

/// 右对齐结束()
#[no_mangle]
pub extern "C" fn qi_gui_egui_right_end_impl() {
    FRAME.with(|fr| {
        let mut b = fr.borrow_mut();
        let Some(frame) = b.as_mut() else {
            return;
        };
        if !matches!(frame.containers.last(), Some(Container::Right)) {
            return;
        }
        frame.containers.pop();
        if frame.ui_stack.len() <= 1 {
            return;
        }
        let child = unsafe { Box::from_raw(frame.ui_stack.pop().unwrap()) };
        let used = child.min_rect();
        drop(child);
        let parent = unsafe { &mut **frame.ui_stack.last().unwrap() };
        parent.advance_cursor_after_rect(used);
    });
}

/// 适应图片(路径, 最大宽) → 1 显示了 / 0 读不了：按原图比例缩到可用宽度
/// （再不超过最大宽，<=0 不限），不放大；圆角
#[no_mangle]
pub extern "C" fn qi_gui_egui_image_fit_impl(path: *const c_char, max_width: i64) -> i64 {
    let p = cstr(path);
    let Some(tex) = crate::egui_widgets2::image_texture(&p) else {
        return 0;
    };
    with_top_ui(|ui| {
        let orig = tex.size_vec2();
        let mut w = orig.x.min(ui.available_width());
        if max_width > 0 {
            w = w.min(max_width as f32);
        }
        let size = vec2(w, w * orig.y / orig.x.max(1.0));
        ui.add(egui::Image::new(&tex).fit_to_exact_size(size).rounding(6.0));
    });
    1
}

/// 进度拖条(id, 千分比, 宽) → 本帧被点/拖到的千分比，没动返回 -1。
/// 细轨 + 强调色已走部分 + 圆头，音视频播放进度用。宽 <=0 占满可用宽度
#[no_mangle]
pub extern "C" fn qi_gui_egui_seek_bar_impl(_id: *const c_char, permille: i64, width: i64) -> i64 {
    with_top_ui(|ui| {
        let w = if width > 0 {
            width as f32
        } else {
            ui.available_width()
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 18.0), Sense::click_and_drag());
        let frac = (permille.clamp(0, 1000) as f32) / 1000.0;
        let y = rect.center().y;
        let v = ui.visuals();
        let track = Rect::from_min_max(pos2(rect.min.x, y - 2.0), pos2(rect.max.x, y + 2.0));
        let done = Rect::from_min_max(track.min, pos2(rect.min.x + w * frac, track.max.y));
        let painter = ui.painter();
        // 没走的那段用淡灰：卡片底本身就是浅色，再用代码底色就看不见了
        painter.rect_filled(track, 2.0, v.weak_text_color().gamma_multiply(0.3));
        painter.rect_filled(done, 2.0, v.hyperlink_color);
        let r = if resp.hovered() || resp.dragged() {
            6.0
        } else {
            4.5
        };
        painter.circle_filled(pos2(done.max.x, y), r, v.hyperlink_color);
        if resp.clicked() || resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                let f = ((p.x - rect.min.x) / w.max(1.0)).clamp(0.0, 1.0);
                return (f * 1000.0).round() as i64;
            }
        }
        -1
    })
    .unwrap_or(-1)
}

/// 拖入文件() → 这一帧拖进窗口的文件路径，多个用换行分开；没有则空串
#[no_mangle]
pub extern "C" fn qi_gui_egui_dropped_files_impl() -> *const c_char {
    let paths = with_ctx(|ctx| {
        ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.as_ref().map(|p| p.display().to_string()))
                .collect::<Vec<_>>()
                .join("\n")
        })
    })
    .unwrap_or_default();
    ret_str(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egui_app::run_headless_frame;
    use std::ffi::{CStr, CString};

    #[test]
    fn rich_line_and_code_block_render_text() {
        let s = |t: &str| CString::new(t).unwrap();
        let (a, b, code) = (s("普通 "), s("粗体"), s("let x = 1;"));
        let frame = run_headless_frame(|| {
            qi_gui_egui_rich_begin_impl();
            qi_gui_egui_rich_span_impl(a.as_ptr(), 0, 0, -1);
            qi_gui_egui_rich_span_impl(b.as_ptr(), 24, FLAG_BOLD | FLAG_ITALIC, 0xd6442f);
            qi_gui_egui_rich_end_impl(16, 1);
            qi_gui_egui_code_block_impl(code.as_ptr(), 14);
        });
        assert!(!frame.shapes.is_empty(), "富文本与代码块应当产生形状");
    }

    #[test]
    fn columns_are_balanced_and_editor_round_trips() {
        let id = CString::new("ed").unwrap();
        let text = CString::new("第一行\n第二行").unwrap();
        let mut got = String::new();
        run_headless_frame(|| {
            qi_gui_egui_columns_begin_impl(500);
            let p = qi_gui_egui_editor_impl(id.as_ptr(), text.as_ptr(), 1, 0);
            got = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
            qi_gui_egui_columns_next_impl();
            qi_gui_egui_rich_begin_impl();
            qi_gui_egui_rich_end_impl(0, 0);
            qi_gui_egui_columns_end_impl();
        });
        assert_eq!(got, "第一行\n第二行", "没有输入时编辑区原样返回");
        assert_eq!(qi_gui_egui_editor_cursor_impl(), -1, "没有焦点就没有光标");
    }

    #[test]
    fn region_and_right_align_stay_balanced() {
        let t = CString::new("右边").unwrap();
        let frame = run_headless_frame(|| {
            qi_gui_egui_set_appearance_impl(0xffffff, 0x24292f, 0x8c8c8c, 0xd6442f, 0xe7e5e0, 14);
            qi_gui_egui_region_begin_impl(44, 8, 0xf6f5f2, 0);
            qi_gui_egui_right_begin_impl();
            qi_gui_egui_weak_label_impl(t.as_ptr());
            qi_gui_egui_right_end_impl();
            qi_gui_egui_region_end_impl();
            qi_gui_egui_region_begin_impl(-29, 24, -1, 720);
            qi_gui_egui_add_space_impl(10);
            qi_gui_egui_region_end_impl();
            // 配对错乱不崩
            qi_gui_egui_region_end_impl();
            qi_gui_egui_right_end_impl();
        });
        assert!(!frame.shapes.is_empty(), "区域底色与文字应当产生形状");
    }

    #[test]
    fn unbalanced_column_calls_are_ignored() {
        run_headless_frame(|| {
            qi_gui_egui_columns_next_impl();
            qi_gui_egui_columns_end_impl();
        });
    }
}
