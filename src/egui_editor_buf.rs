//! 托管编辑区：正文放在 Rust 这边，qi 不再每帧往返全文
//!
//! 老的 `编辑区(id, 值, 等宽, 字号) → 新值` 每帧把全文从 qi 拷进来、再拷回去，
//! qi 侧还要逐字节比一遍「变没变」。几 MB 的文档光这几趟拷贝和比较就是十几毫秒一帧。
//! 托管编辑区把缓冲留在 Rust：
//!
//! - `托管编辑区(id, 等宽, 字号) → 版本`：画编辑区，返回内容版本号（改一次 +1）
//! - `设置编辑区文本(id, 文本)` / `编辑区文本(id) → 文本` / `编辑区版本(id)`
//! - `设置编辑区高亮(id, "起,止,样式;…")`：字节区间 + 样式位，重叠处按位或
//! - `设置高亮颜色(样式位, 颜色)`：覆盖某个样式的颜色（默认从外观配色派生）
//!
//! qi 只在版本变了时取一次文本、重算高亮和统计。排版结果按
//! （版本, 高亮序号, 字号, 字族, 折行宽, 配色）缓存，静止的帧不再重排、不再哈希全文。
//!
//! 高亮区间是 qi 对着某一版正文算的；用户接着打字，正文变了而新区间还没来。
//! 这时拿「设高亮时的正文快照」和现在的正文比公共前后缀，把区间挪到新位置
//! （单处编辑完全准确）。qi 随后送来的新区间若跟挪过的一样，就不必再排一遍。

use crate::egui_app::{cstr, ret_str, with_top_ui};
use crate::egui_editor::{bold_family, editor_id, mix, set_last_cursor, unpack_rgb, weak_color};
use std::cell::RefCell;
use std::collections::HashMap;
use std::os::raw::c_char;
use std::sync::Arc;

use egui::text::{LayoutJob, LayoutSection, TextFormat, TextWrapping};
use egui::{vec2, Align, Color32, FontFamily, FontId, Galley, Stroke, TextStyle};

/// 高亮样式位（与 qi 侧 高亮.qi 的常量一致）
pub const HL_HEADING: u32 = 1;
pub const HL_BOLD: u32 = 2;
pub const HL_ITALIC: u32 = 4;
pub const HL_CODE: u32 = 8;
pub const HL_LINK: u32 = 16;
pub const HL_MARKER: u32 = 32;
pub const HL_QUOTE: u32 = 64;
pub const HL_BLOCK: u32 = 128;
pub const HL_STRIKE: u32 = 256;
pub const HL_LIST: u32 = 512;
const HL_BITS: usize = 10;

/// 颜色取哪个样式的：排在前面的优先（记号永远是淡的，哪怕在标题里）
const COLOR_PRIORITY: [u32; 9] = [
    HL_MARKER, HL_LIST, HL_LINK, HL_STRIKE, HL_CODE, HL_BLOCK, HL_QUOTE, HL_HEADING, HL_BOLD,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Span {
    start: usize,
    end: usize,
    bits: u32,
}

#[derive(Clone, Copy, PartialEq)]
struct LayoutKey {
    version: i64,
    hl_serial: u64,
    px_bits: u32,
    mono: bool,
    wrap_bits: u32,
    palette: [Color32; 11],
}

#[derive(Default)]
struct EditorBuf {
    text: String,
    version: i64,
    spans: Vec<Span>,
    /// 设高亮时的正文快照：正文之后又被改过时，靠它把区间挪到新位置
    span_base: String,
    hl_serial: u64,
    cache: Option<(LayoutKey, Arc<Galley>)>,
}

thread_local! {
    static BUFS: RefCell<HashMap<String, EditorBuf>> = RefCell::new(HashMap::new());
    /// 设置高亮颜色 给的覆盖色：样式位 → 颜色
    static HL_COLORS: RefCell<HashMap<u32, Color32>> = RefCell::new(HashMap::new());
}

// ============================================================================
// 区间：解析、挪位、合成排版任务
// ============================================================================

/// "起,止,样式;…" → 区间表。坏项跳过；区间先不校验边界，排版前统一夹
fn parse_spans(spec: &str) -> Vec<Span> {
    spec.split(';')
        .filter_map(|item| {
            let mut it = item.split(',').map(|v| v.trim().parse::<i64>().ok());
            let (s, e, b) = (it.next()??, it.next()??, it.next()??);
            (s >= 0 && e > s && b > 0).then_some(Span {
                start: s as usize,
                end: e as usize,
                bits: b as u32,
            })
        })
        .collect()
}

fn floor_boundary(text: &str, mut i: usize) -> usize {
    i = i.min(text.len());
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// 把对着 `base` 算的区间挪到 `text` 上。两者只差一处连续编辑时与 qi 重算的结果一致：
/// 公共前缀之前的位置不动，公共后缀之后的位置平移长度差，落在改动里的位置收到改动起点。
///
/// 纯插入时，恰好停在插入点上的区间终点要猜「新字算不算进去」：整行的样式（标题、引用、
/// 代码块）算；记号本身不算；内容区间（粗体里的字）在后面紧跟收尾记号时不算（`**粗**` 后面
/// 接着写），否则算（在 `**粗|**` 里接着写）。猜错也只是 qi 送来的新区间对不上、重排一次。
/// 结果夹到字符边界、丢掉空区间。
fn remap(spans: &[Span], base: &str, text: &str) -> Vec<Span> {
    let (b, t) = (base.as_bytes(), text.as_bytes());
    if b == t {
        return spans
            .iter()
            .filter_map(|s| clamp_span(text, s.start, s.end, s.bits))
            .collect();
    }
    let max = b.len().min(t.len());
    let prefix = b.iter().zip(t).take_while(|(x, y)| x == y).count();
    let suffix = b[prefix..]
        .iter()
        .rev()
        .zip(t[prefix..].iter().rev())
        .take(max - prefix)
        .take_while(|(x, y)| x == y)
        .count();
    let tail_old = b.len() - suffix;
    let tail_new = t.len() - suffix;
    let shift = |x: usize| x - tail_old + tail_new;
    let insertion = tail_old == prefix;
    let marker_ends_here = spans
        .iter()
        .any(|s| s.end == prefix && s.bits & HL_MARKER != 0);
    spans
        .iter()
        .filter_map(|s| {
            let start = if s.start < prefix {
                s.start
            } else if s.start >= tail_old {
                shift(s.start)
            } else {
                prefix
            };
            let end = if insertion && s.end == prefix {
                let line_wide = s.bits & (HL_HEADING | HL_QUOTE | HL_BLOCK) != 0;
                let grows = line_wide || (s.bits & HL_MARKER == 0 && !marker_ends_here);
                if grows {
                    shift(s.end)
                } else {
                    s.end
                }
            } else if s.end <= prefix {
                s.end
            } else if s.end >= tail_old {
                shift(s.end)
            } else {
                prefix
            };
            clamp_span(text, start, end, s.bits)
        })
        .collect()
}

fn clamp_span(text: &str, start: usize, end: usize, bits: u32) -> Option<Span> {
    let (start, end) = (floor_boundary(text, start), floor_boundary(text, end));
    (end > start).then_some(Span { start, end, bits })
}

/// 外观派生的默认配色，下标是样式位的序号；第 10 格是正文色。
/// 行内代码不铺底色：1.6 倍行高下底色块撑满整行，一块块很重，改用等宽 + 代码色
fn palette(ui: &egui::Ui) -> [Color32; 11] {
    let v = ui.visuals();
    let text = v.text_color();
    let bg = v.extreme_bg_color;
    let weak = weak_color().unwrap_or_else(|| v.weak_text_color());
    let accent = v.hyperlink_color;
    let mut p = [text; 11];
    let mut put = |bit: u32, c: Color32| p[bit.trailing_zeros() as usize] = c;
    put(HL_HEADING, v.strong_text_color());
    put(HL_BOLD, v.strong_text_color());
    put(HL_ITALIC, text);
    put(HL_CODE, mix(text, accent, 0.7));
    put(HL_LINK, accent);
    put(HL_MARKER, mix(weak, bg, 0.2));
    put(HL_QUOTE, mix(text, weak, 0.6));
    put(HL_BLOCK, mix(text, weak, 0.3));
    put(HL_STRIKE, weak);
    put(HL_LIST, weak);
    HL_COLORS.with(|m| {
        for (bit, c) in m.borrow().iter() {
            if bit.is_power_of_two() && (bit.trailing_zeros() as usize) < HL_BITS {
                p[bit.trailing_zeros() as usize] = *c;
            }
        }
    });
    p[10] = text;
    p
}

fn format_for(bits: u32, base_family: &FontFamily, px: f32, pal: &[Color32; 11]) -> TextFormat {
    let family = if bits & (HL_CODE | HL_BLOCK) != 0 {
        FontFamily::Monospace
    } else if bits & (HL_HEADING | HL_BOLD) != 0 {
        bold_family().unwrap_or_else(|| base_family.clone())
    } else {
        base_family.clone()
    };
    let color = COLOR_PRIORITY
        .iter()
        .find(|b| bits & **b != 0)
        .map(|b| pal[b.trailing_zeros() as usize])
        .unwrap_or(pal[10]);
    TextFormat {
        font_id: FontId::new(px, family),
        color,
        italics: bits & HL_ITALIC != 0,
        strikethrough: if bits & HL_STRIKE != 0 {
            Stroke::new(1.0_f32, color)
        } else {
            Stroke::NONE
        },
        // 自己排版：给每行 1.6 倍行高（TextEdit 默认行距太挤，中文尤其）
        line_height: Some(px * 1.6),
        valign: Align::Center,
        ..Default::default()
    }
}

/// 正文 + 区间 → 排版任务。扫描线：每个边界处更新各样式位的覆盖计数，
/// 相邻同样式的段合并成一节
fn build_job(
    text: &str,
    spans: &[Span],
    base_family: &FontFamily,
    px: f32,
    pal: &[Color32; 11],
    wrap_width: f32,
) -> LayoutJob {
    let mut edges: Vec<(usize, u32, bool)> = Vec::with_capacity(spans.len() * 2);
    for s in spans {
        edges.push((s.start, s.bits, true));
        edges.push((s.end, s.bits, false));
    }
    edges.sort_unstable_by_key(|e| e.0);
    let mut counts = [0u32; HL_BITS];
    // (节, 这一节的样式位)：相邻同样式的段并进上一节
    let mut sections: Vec<(LayoutSection, u32)> = Vec::new();
    let mut push = |range: std::ops::Range<usize>, bits: u32| {
        if range.is_empty() {
            return;
        }
        if let Some((last, last_bits)) = sections.last_mut() {
            if last.byte_range.end == range.start && *last_bits == bits {
                last.byte_range.end = range.end;
                return;
            }
        }
        let section = LayoutSection {
            leading_space: 0.0,
            byte_range: range,
            format: format_for(bits, base_family, px, pal),
        };
        sections.push((section, bits));
    };
    let mut at = 0usize;
    let mut cur = 0u32;
    let mut i = 0;
    while i < edges.len() {
        let pos = edges[i].0;
        push(at..pos, cur);
        while i < edges.len() && edges[i].0 == pos {
            let (_, bits, open) = edges[i];
            for (k, c) in counts.iter_mut().enumerate() {
                if bits & (1 << k) != 0 {
                    if open {
                        *c += 1;
                    } else {
                        *c = c.saturating_sub(1);
                    }
                }
            }
            i += 1;
        }
        cur = counts
            .iter()
            .enumerate()
            .filter(|(_, c)| **c > 0)
            .fold(0, |acc, (k, _)| acc | (1 << k));
        at = pos;
    }
    push(at..text.len(), cur);
    let mut sections: Vec<LayoutSection> = sections.into_iter().map(|(s, _)| s).collect();
    if sections.is_empty() {
        sections.push(LayoutSection {
            leading_space: 0.0,
            byte_range: 0..0,
            format: format_for(0, base_family, px, pal),
        });
    }
    LayoutJob {
        text: text.to_owned(),
        sections,
        wrap: TextWrapping {
            max_width: wrap_width,
            ..Default::default()
        },
        break_on_newline: true,
        ..Default::default()
    }
}

// ============================================================================
// FFI
// ============================================================================

/// 托管编辑区(id, 等宽, 字号) → 版本：撑满剩余区域的多行输入，自带纵向滚动。
/// 正文在 Rust 这边，内容每改一次版本 +1。调用后 编辑区光标() 照常可用。
#[no_mangle]
pub extern "C" fn qi_gui_egui_editor_managed_impl(id: *const c_char, mono: i64, size: i64) -> i64 {
    let id = cstr(id);
    let mut cursor = None;
    let version = BUFS.with(|bufs| {
        let mut bufs = bufs.borrow_mut();
        let buf = bufs.entry(id.clone()).or_default();
        let mut text = std::mem::take(&mut buf.text);
        let changed = with_top_ui(|ui| {
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
            let pal = palette(ui);
            let (version, hl_serial) = (buf.version, buf.hl_serial);
            let key = |wrap: f32, version: i64| LayoutKey {
                version,
                hl_serial,
                px_bits: px.to_bits(),
                mono: mono != 0,
                wrap_bits: wrap.to_bits(),
                palette: pal,
            };
            // TextEdit 每次 show 第一次调排版器时，传进来的一定是本帧改动前的正文
            // （= 缓冲里那一版），这一次可以放心走缓存；之后的调用都是改动后的新正文
            let mut first = true;
            let (spans, base, cache) = (&buf.spans, &buf.span_base, &mut buf.cache);
            let mut layouter = |ui: &egui::Ui, s: &str, wrap: f32| {
                let was_first = std::mem::replace(&mut first, false);
                if was_first {
                    if let Some((k, g)) = cache.as_ref() {
                        if *k == key(wrap, version) {
                            return g.clone();
                        }
                    }
                }
                let mapped = remap(spans, base, s);
                let job = build_job(s, &mapped, &family, px, &pal, wrap);
                let galley = ui.fonts(|f| f.layout_job(job));
                // 首次调用对应当前版本；之后的调用对应改动后的下一版
                let v = if was_first { version } else { version + 1 };
                *cache = Some((key(wrap, v), galley.clone()));
                galley
            };
            let mut changed = false;
            egui::ScrollArea::vertical()
                .id_salt(("qi_editor_scroll", &id))
                .auto_shrink([false, false])
                .max_height(avail.y)
                .show(ui, |ui| {
                    let out = egui::TextEdit::multiline(&mut text)
                        .id(editor_id(&id))
                        .layouter(&mut layouter)
                        .frame(false)
                        .margin(vec2(0.0, 0.0))
                        .lock_focus(true)
                        .desired_width(f32::INFINITY)
                        .min_size(vec2(0.0, avail.y))
                        .show(ui);
                    changed = out.response.changed();
                    cursor = out.cursor_range.map(|r| r.primary);
                });
            changed
        })
        .unwrap_or(false);
        buf.text = text;
        if changed {
            buf.version += 1;
        }
        buf.version
    });
    set_last_cursor(cursor);
    version
}

/// 设置编辑区文本(id, 文本)：整篇换掉（打开文件、新建、程序插入内容），版本 +1，高亮清空
#[no_mangle]
pub extern "C" fn qi_gui_egui_editor_set_text_impl(id: *const c_char, text: *const c_char) {
    let (id, text) = (cstr(id), cstr(text));
    BUFS.with(|bufs| {
        let mut bufs = bufs.borrow_mut();
        let buf = bufs.entry(id).or_default();
        buf.text = text;
        buf.version += 1;
        buf.spans.clear();
        buf.span_base.clear();
        buf.hl_serial += 1;
        buf.cache = None;
    });
}

/// 编辑区文本(id) → 当前正文（只在版本变了时取）
#[no_mangle]
pub extern "C" fn qi_gui_egui_editor_text_impl(id: *const c_char) -> *const c_char {
    let id = cstr(id);
    let text = BUFS.with(|b| b.borrow().get(&id).map(|buf| buf.text.clone()));
    ret_str(text.unwrap_or_default())
}

/// 编辑区版本(id) → 内容版本号；没建过的编辑区返回 0
#[no_mangle]
pub extern "C" fn qi_gui_egui_editor_version_impl(id: *const c_char) -> i64 {
    let id = cstr(id);
    BUFS.with(|b| b.borrow().get(&id).map(|buf| buf.version).unwrap_or(0))
}

/// 设置编辑区高亮(id, "起,止,样式;…")：对着当前正文算的字节区间；空串 = 不高亮。
/// 跟正文改动后挪过位的旧区间一模一样时什么都不做（不重排）
#[no_mangle]
pub extern "C" fn qi_gui_egui_editor_set_highlight_impl(id: *const c_char, spec: *const c_char) {
    let (id, spec) = (cstr(id), cstr(spec));
    BUFS.with(|bufs| {
        let mut bufs = bufs.borrow_mut();
        let buf = bufs.entry(id).or_default();
        let spans: Vec<Span> = parse_spans(&spec)
            .iter()
            .filter_map(|s| clamp_span(&buf.text, s.start, s.end, s.bits))
            .collect();
        let unchanged = remap(&buf.spans, &buf.span_base, &buf.text) == spans;
        buf.spans = spans;
        buf.span_base.clone_from(&buf.text);
        if !unchanged {
            buf.hl_serial += 1;
        }
    });
}

/// 设置高亮颜色(样式位, 颜色)：覆盖某个样式的颜色；颜色 <0 恢复从外观派生的默认色
#[no_mangle]
pub extern "C" fn qi_gui_egui_set_highlight_color_impl(bits: i64, color: i64) {
    let bit = bits as u32;
    HL_COLORS.with(|m| {
        if color < 0 {
            m.borrow_mut().remove(&bit);
        } else {
            m.borrow_mut().insert(bit, unpack_rgb(color));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egui_app::run_headless_frame;
    use crate::egui_editor::qi_gui_egui_editor_cursor_impl;
    use std::ffi::{CStr, CString};

    fn sp(start: usize, end: usize, bits: u32) -> Span {
        Span { start, end, bits }
    }

    #[test]
    fn parse_skips_bad_items() {
        let got = parse_spans("0,5,1;0,1,32;bad;3,2,1;4,9,x;;7,8,0;2,4,2");
        assert_eq!(got, vec![sp(0, 5, 1), sp(0, 1, 32), sp(2, 4, 2)]);
    }

    #[test]
    fn remap_follows_a_single_edit() {
        let bold = vec![sp(0, 2, HL_MARKER), sp(2, 5, HL_BOLD), sp(5, 7, HL_MARKER)];
        // 在 **粗|** 里接着写：粗体伸长，收尾记号后移
        assert_eq!(
            remap(&bold, "**粗**", "**粗x**"),
            vec![sp(0, 2, HL_MARKER), sp(2, 6, HL_BOLD), sp(6, 8, HL_MARKER)]
        );
        // 在 **粗**| 后面接着写：记号不伸长，新字不带样式
        assert_eq!(remap(&bold, "**粗**", "**粗**x"), bold);
        // 整行样式（标题）在行尾接着写会伸长，哪怕行尾是收尾记号
        let heading = vec![
            sp(0, 12, HL_HEADING),
            sp(5, 7, HL_MARKER),
            sp(10, 12, HL_MARKER),
        ];
        let grown = remap(&heading, "# 甲**乙**", "# 甲**乙**丙");
        assert_eq!(grown[0], sp(0, 15, HL_HEADING));
        assert_eq!(grown[2], sp(10, 12, HL_MARKER));
        // 删字：区间平移，整个被删掉的区间丢掉
        let two = vec![sp(0, 1, HL_MARKER), sp(2, 8, HL_BOLD)];
        assert_eq!(
            remap(&two, "# 甲乙", "# 乙"),
            vec![sp(0, 1, HL_MARKER), sp(2, 5, HL_BOLD)]
        );
        assert_eq!(remap(&[sp(2, 5, HL_BOLD)], "# 甲乙", "# 乙"), vec![]);
        // 越界与切进汉字中间的端点被夹到字符边界
        assert_eq!(remap(&[sp(1, 99, 1)], "甲", "甲"), vec![sp(0, 3, 1)]);
    }

    #[test]
    fn job_sections_cover_text_and_merge_equal_neighbours() {
        let pal = [Color32::BLACK; 11];
        let text = "# 题 **粗**";
        let spans = vec![
            sp(0, text.len(), HL_HEADING),
            sp(0, 1, HL_MARKER),
            sp(6, 8, HL_MARKER),
            sp(8, 11, HL_BOLD),
            sp(11, 13, HL_MARKER),
        ];
        let job = build_job(text, &spans, &FontFamily::Proportional, 16.0, &pal, 300.0);
        let ranges: Vec<_> = job.sections.iter().map(|s| s.byte_range.clone()).collect();
        assert_eq!(ranges, vec![0..1, 1..6, 6..8, 8..11, 11..13]);
        assert!(job.sections[3].format.font_id.size == 16.0);
        let plain = build_job("甲乙", &[], &FontFamily::Proportional, 16.0, &pal, 300.0);
        assert_eq!(plain.sections.len(), 1);
        assert_eq!(plain.sections[0].byte_range, 0..6);
    }

    #[test]
    fn managed_editor_keeps_text_in_rust_and_versions_changes() {
        let id = CString::new("buf").unwrap();
        let text = CString::new("# 标题\n正文 **粗**").unwrap();
        qi_gui_egui_editor_set_text_impl(id.as_ptr(), text.as_ptr());
        let v1 = qi_gui_egui_editor_version_impl(id.as_ptr());
        assert!(v1 >= 1);
        // "# 标题" 是 0..8，"**粗**" 是 16..23
        let spec = CString::new("0,8,1;0,1,32;16,18,32;18,21,2;21,23,32").unwrap();
        qi_gui_egui_editor_set_highlight_impl(id.as_ptr(), spec.as_ptr());
        let mut shown = 0;
        let frame = run_headless_frame(|| {
            shown = qi_gui_egui_editor_managed_impl(id.as_ptr(), 0, 16);
        });
        assert!(!frame.shapes.is_empty());
        assert_eq!(shown, v1, "没有输入时版本不变");
        let got = unsafe { CStr::from_ptr(qi_gui_egui_editor_text_impl(id.as_ptr())) };
        assert_eq!(got.to_str().unwrap(), "# 标题\n正文 **粗**");
        assert_eq!(qi_gui_egui_editor_cursor_impl(), -1, "没有焦点就没有光标");
        // 同样的高亮再设一遍：不算变化（不会触发重排）
        let serial = BUFS.with(|b| b.borrow().get("buf").unwrap().hl_serial);
        qi_gui_egui_editor_set_highlight_impl(id.as_ptr(), spec.as_ptr());
        assert_eq!(
            serial,
            BUFS.with(|b| b.borrow().get("buf").unwrap().hl_serial)
        );
        let unknown = CString::new("nobody").unwrap();
        assert_eq!(qi_gui_egui_editor_version_impl(unknown.as_ptr()), 0);
    }
}
