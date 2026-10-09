//! 局部重画：只光栅化这一帧跟上次真画时不一样的那几块
//!
//! 静止跳帧（见 `egui_app` 的 `帧结束`）解决的是"整屏没变"；视频播放、光标闪烁、
//! 进度条走动这类场景，每帧都有**一小块**在变，以前只能整屏重画 —— 软光栅一帧
//! 十来毫秒，视频 30fps 就把一个核吃满。
//!
//! 判据跟静止跳帧同一条推理：像素 = f(覆盖它的图元, 纹理)。逐个比较这一帧和上次
//! 真画那帧的形状列表，变了的形状取新旧两个外框，用到的纹理内容变了的形状取它的
//! 外框，并起来就是这一帧所有可能变化的像素；框外每个像素的输入都没变，结果必然不变。
//!
//! 保守退回整屏重画的情形：形状个数变了（增删会让下标错位，逐个比较失去意义）、
//! 字体图集或纹理释放这类全局变化、脏区超过半屏（局部画不比整屏省）。
//! 设 `QI_GUI_FULL_REDRAW=1` 可以整个关掉，排查花屏时用。

use egui::epaint::{ClippedShape, ImageData, ImageDelta, Shape, TextureId};
use egui::Rect;

/// 物理像素矩形：左上含、右下不含
pub type PxRect = (usize, usize, usize, usize);

/// 抗锯齿羽化、斜体字出框之类的余量（点）
const MARGIN: f32 = 2.0;

fn uses_texture(shape: &Shape, ids: &[TextureId]) -> bool {
    match shape {
        Shape::Vec(v) => v.iter().any(|s| uses_texture(s, ids)),
        Shape::Mesh(_) | Shape::Rect(_) => ids.contains(&shape.texture_id()),
        _ => false,
    }
}

fn bounds(cs: &ClippedShape) -> Rect {
    let mut r = cs.shape.visual_bounding_rect();
    if let Shape::Rect(rs) = &cs.shape {
        r = r.expand(rs.blur_width);
    }
    r.expand(MARGIN).intersect(cs.clip_rect)
}

fn to_px(r: Rect, ppp: f32, fb_w: usize, fb_h: usize) -> Option<PxRect> {
    if !r.is_positive() {
        return None;
    }
    let x0 = ((r.min.x * ppp).floor() - 1.0).max(0.0) as usize;
    let y0 = ((r.min.y * ppp).floor() - 1.0).max(0.0) as usize;
    let x1 = (((r.max.x * ppp).ceil() + 1.0).max(0.0) as usize).min(fb_w);
    let y1 = (((r.max.y * ppp).ceil() + 1.0).max(0.0) as usize).min(fb_h);
    (x0 < x1 && y0 < y1).then_some((x0, y0, x1, y1))
}

/// 这一帧需要重画的区域。`None` 表示整屏重画。
///
/// - `old` / `new`：上次真画那帧 / 这一帧的形状
/// - `tex_set` / `tex_free`：这一帧的纹理增量
pub fn dirty_rects(
    old: &[ClippedShape],
    new: &[ClippedShape],
    tex_set: &[(TextureId, ImageDelta)],
    tex_free: &[TextureId],
    ppp: f32,
    fb_w: usize,
    fb_h: usize,
) -> Option<Vec<PxRect>> {
    if old.len() != new.len() || !tex_free.is_empty() {
        return None;
    }
    // 字体图集变了，所有文字都可能变；只认整张替换的彩色纹理（图片/视频换帧）
    let mut changed_tex = Vec::new();
    for (id, delta) in tex_set {
        match &delta.image {
            ImageData::Color(_) if delta.pos.is_none() => changed_tex.push(*id),
            _ => return None,
        }
    }
    let mut dirty: Vec<Rect> = Vec::new();
    for (a, b) in old.iter().zip(new) {
        if a != b {
            dirty.push(bounds(a));
            dirty.push(bounds(b));
        } else if !changed_tex.is_empty() && uses_texture(&b.shape, &changed_tex) {
            dirty.push(bounds(b));
        }
    }
    let mut rects: Vec<PxRect> = dirty
        .into_iter()
        .filter_map(|r| to_px(r, ppp, fb_w, fb_h))
        .collect();
    merge_overlapping(&mut rects);
    let area: usize = rects.iter().map(|r| (r.2 - r.0) * (r.3 - r.1)).sum();
    if area * 2 > fb_w * fb_h {
        return None;
    }
    Some(rects)
}

fn overlaps(a: &PxRect, b: &PxRect) -> bool {
    a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3
}

/// 相交的框合并成外包框，直到两两不相交。重叠区域只画一次，框数也不会爆
fn merge_overlapping(rects: &mut Vec<PxRect>) {
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                if overlaps(&rects[i], &rects[j]) {
                    let (a, b) = (rects[i], rects.swap_remove(j));
                    rects[i] = (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3));
                    merged = true;
                    break 'outer;
                }
            }
        }
    }
}

/// `QI_GUI_FULL_REDRAW=1` 关掉局部重画
pub fn disabled() -> bool {
    std::env::var("QI_GUI_FULL_REDRAW")
        .map(|v| v == "1")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egui_app::run_headless_frame;
    use crate::egui_raster::{paint, paint_region, TextureStore};
    use std::ffi::CString;

    const W: usize = 800;
    const H: usize = 600;
    const BG: [u8; 3] = [250, 250, 250];

    fn scene(x: i64, label: &str) -> crate::egui_app::HeadlessFrame {
        let label = label.to_string();
        run_headless_frame(move || {
            let t = CString::new(label).unwrap();
            crate::egui_app::qi_gui_egui_label_impl(t.as_ptr());
            let id = CString::new("场景").unwrap();
            crate::egui_canvas::qi_gui_egui_canvas_begin_impl(id.as_ptr(), 400, 200);
            crate::egui_canvas::qi_gui_egui_canvas_rect_impl(x, 20, 40, 30, 200, 40, 40);
            crate::egui_canvas::qi_gui_egui_canvas_end_impl();
        })
    }

    /// 地基：在 A 的像素上只重画脏区得到的结果，必须跟整屏画 B 逐像素相同
    #[test]
    fn partial_repaint_matches_full_repaint() {
        let a = scene(10, "位置 0:01");
        let b = scene(70, "位置 0:02");
        let rects = dirty_rects(&a.shapes, &b.shapes, &[], &[], 1.0, W, H)
            .expect("只挪了一个方块、改了一行字，应当局部重画");
        assert!(!rects.is_empty());
        let area: usize = rects.iter().map(|r| (r.2 - r.0) * (r.3 - r.1)).sum();
        assert!(area < W * H / 10, "脏区应当很小，实得 {area} 像素");

        let mut store = TextureStore::new();
        store.apply(&a.textures_delta.set, &a.textures_delta.free);
        store.apply(&b.textures_delta.set, &b.textures_delta.free);
        let jobs_a = a.ctx.tessellate(a.shapes.clone(), 1.0);
        let jobs_b = b.ctx.tessellate(b.shapes.clone(), 1.0);

        let mut partial = vec![0u32; W * H];
        paint(&mut partial, W, H, 1.0, BG, &jobs_a, &store);
        for r in &rects {
            paint_region(&mut partial, W, 1.0, BG, &jobs_b, &store, *r);
        }
        let mut full = vec![0u32; W * H];
        paint(&mut full, W, H, 1.0, BG, &jobs_b, &store);
        let diff = partial.iter().zip(&full).filter(|(p, f)| p != f).count();
        assert_eq!(diff, 0, "局部重画与整屏重画有 {diff} 个像素不同");
    }

    #[test]
    fn same_frame_has_no_damage_and_count_change_is_full() {
        let a = scene(10, "一样");
        let b = scene(10, "一样");
        assert_eq!(
            dirty_rects(&a.shapes, &b.shapes, &[], &[], 1.0, W, H),
            Some(vec![])
        );
        let c = run_headless_frame(|| {});
        assert_eq!(dirty_rects(&a.shapes, &c.shapes, &[], &[], 1.0, W, H), None);
        assert_eq!(
            dirty_rects(
                &a.shapes,
                &b.shapes,
                &[],
                &[TextureId::Managed(7)],
                1.0,
                W,
                H
            ),
            None,
            "释放纹理按整屏处理"
        );
    }

    #[test]
    fn overlapping_rects_merge() {
        let mut r = vec![(0, 0, 10, 10), (5, 5, 20, 20), (100, 100, 110, 110)];
        merge_overlapping(&mut r);
        r.sort();
        assert_eq!(r, vec![(0, 0, 20, 20), (100, 100, 110, 110)]);
    }
}
