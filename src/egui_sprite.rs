//! 画布精灵层 —— 在画布上贴图片（缩放 / 旋转 / 水平镜像），纹理按路径缓存
//!
//! 面向儿童编程：这是 Scratch「角色」的平替。孩子把一张 png 摆到画布上，
//! 让它跟着鼠标转、左右走时镜像翻面，就能拼出小游戏。
//!
//! ## 为什么要有这一层（而不是直接用 `图片显示` 控件）
//! `egui_widgets2.rs` 的 `图片显示` 是**控件级**的：它走 `ui.image()`，位置由
//! 布局流决定，不能旋转、不能任意定位。游戏要的是「画布局部坐标 (x,y) 处贴一张图，
//! 绕中心转 37 度」——只能自己往 painter 上塞网格（`Shape::Mesh`）：
//! egui 的 Image shape 只认轴对齐矩形，带旋转就必须自己算四角顶点 + UV。
//!
//! ## 为什么必须缓存纹理
//! immediate mode 每帧都会重跑一遍用户的绘制代码，`画布图片("猫.png", ...)` 一秒
//! 要调 60 次。每次重新解码 PNG + 重新上传纹理会直接把帧率打到个位数。所以按
//! **路径**缓存 TextureHandle（进程内不逐出——精灵素材总量很小，且孩子会反复用）。
//!
//! ## 加载失败为什么画品红方块
//! 孩子写错路径是常态。「屏幕上什么都没有」没法查，「屏幕上有个刺眼的粉方块」
//! 一眼就知道是这张图没加载出来，而且方块的位置/大小/角度都对，能反推是哪一句。
//! 警告只在**首次**失败时打一行（路径进失败集），否则 60fps 会把终端刷爆。
//!
//! ## 路径怎么解析
//! 相对路径相对**进程当前工作目录（CWD）**解析，不是 .qi 源文件所在目录。
//! 示例都约定 `cd` 到自己目录再跑，所以 `"素材/小球.png"` 就是源文件旁边那张。
//!
//! ## 坐标与角度约定
//! 坐标是**画布局部坐标**（画布左上角为 0,0），与 `egui_canvas.rs` 的图元一致。
//! 角度是**角度制整数**，正数 = 顺时针（屏幕 Y 轴朝下，右转 90 度即朝下）。

use crate::egui_app::{cstr, with_ctx, with_top_canvas};
use egui::{Color32, Mesh, Pos2, Shape};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::os::raw::c_char;

thread_local! {
    /// 路径 → 纹理句柄（持有即保活；进程内不逐出，见文件头说明）。
    /// 原始像素尺寸不存在这里 —— 见下面 DIMENSIONS 表。
    static TEXTURES: RefCell<HashMap<String, egui::TextureHandle>> = RefCell::new(HashMap::new());
    /// 路径 → 原始像素尺寸。单独一张表：`图片宽/高` 可能在帧外调用（拿不到
    /// egui Context 建不了纹理），此时只探测文件头即可，比解码整张图便宜得多。
    static DIMENSIONS: RefCell<HashMap<String, (i64, i64)>> = RefCell::new(HashMap::new());
    /// 加载失败过的路径：不再重试、不再刷警告
    static FAILED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// 真正跑过磁盘加载的次数（解码或探头）。仅供单测断言缓存确实命中。
    static LOAD_COUNT: Cell<u64> = const { Cell::new(0) };
}

/// 加载失败时画的占位色：品红（正常素材几乎不会用这个颜色，一眼可辨）
const PLACEHOLDER: Color32 = Color32::from_rgb(255, 0, 200);

// ============================================================================
// 纯函数几何（不碰 egui 全局状态，可直接单测）
// ============================================================================

/// 绕中心旋转后的四角坐标，顺序固定为 [左上, 右上, 右下, 左下]（未旋转时的方位）。
///
/// 屏幕坐标 Y 轴朝下，所以标准旋转矩阵 `(dx·cos − dy·sin, dx·sin + dy·cos)`
/// 在视觉上表现为**顺时针**：右边的点 (1,0) 转 90 度后落到 (0,1)，也就是下方。
/// 这正好符合孩子的直觉——"转 90 度"就是右转。
fn rotated_corners(cx: f32, cy: f32, w: f32, h: f32, degrees: f32) -> [Pos2; 4] {
    let rad = degrees.to_radians();
    let (sin, cos) = rad.sin_cos();
    let hw = w / 2.0;
    let hh = h / 2.0;
    let offsets = [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)];
    offsets.map(|(dx, dy)| Pos2::new(cx + dx * cos - dy * sin, cy + dx * sin + dy * cos))
}

/// 四角的纹理坐标（UV），顺序与 `rotated_corners` 一一对应。
///
/// 翻转不动顶点、只动 UV：把左右两列的 u 对调，图案就镜像了，而形状/位置不变。
/// 这比"把宽取负"稳妥得多（负宽会让三角形绕序反过来）。
fn quad_uv(flip_h: bool, flip_v: bool) -> [Pos2; 4] {
    let (u0, u1) = if flip_h { (1.0, 0.0) } else { (0.0, 1.0) };
    let (v0, v1) = if flip_v { (1.0, 0.0) } else { (0.0, 1.0) };
    [
        Pos2::new(u0, v0),
        Pos2::new(u1, v0),
        Pos2::new(u1, v1),
        Pos2::new(u0, v1),
    ]
}

/// 由四角坐标 + 四角 UV 拼一个两三角形的贴图网格（0-1-2 / 0-2-3）
fn build_mesh(texture_id: egui::TextureId, corners: [Pos2; 4], uv: [Pos2; 4]) -> Mesh {
    let mut mesh = Mesh::with_texture(texture_id);
    for i in 0..4 {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: corners[i],
            uv: uv[i],
            color: Color32::WHITE, // 白色 = 原样输出纹理颜色（不着色）
        });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    mesh
}

// ============================================================================
// 纹理缓存
// ============================================================================

/// 缓存查表骨架：命中直接返回且**不调** loader；未命中调 loader，
/// 成功入缓存，失败入失败集（下次直接短路）并把警告交给调用方打一次。
///
/// 抽出来是为了让"第二次不再解码"这条策略可以脱离 egui 单测。
fn cached_lookup<T: Clone>(
    cache: &RefCell<HashMap<String, T>>,
    failed: &RefCell<HashSet<String>>,
    key: &str,
    loader: impl FnOnce() -> Option<T>,
) -> Option<T> {
    if let Some(v) = cache.borrow().get(key) {
        return Some(v.clone());
    }
    if failed.borrow().contains(key) {
        return None;
    }
    match loader() {
        Some(v) => {
            cache.borrow_mut().insert(key.to_string(), v.clone());
            Some(v)
        }
        None => {
            failed.borrow_mut().insert(key.to_string());
            eprintln!("[奇语·图形] 图片加载失败，画品红占位方块代替：{key}");
            None
        }
    }
}

/// 取路径对应的纹理；未命中则解码 + 上传。必须在帧内调用（要 egui Context）。
fn texture_for(path: &str) -> Option<egui::TextureHandle> {
    TEXTURES.with(|cache| {
        FAILED.with(|failed| {
            cached_lookup(cache, failed, path, || {
                LOAD_COUNT.with(|c| c.set(c.get() + 1));
                let img = image::open(path).ok()?;
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                // 顺手把尺寸也记进尺寸表，省得 图片宽/高 再读一次盘
                DIMENSIONS.with(|d| {
                    d.borrow_mut()
                        .insert(path.to_string(), (w as i64, h as i64));
                });
                let color_image = egui::ColorImage::from_rgba_unmultiplied(
                    [w as usize, h as usize],
                    rgba.as_raw(),
                );
                with_ctx(|ctx| {
                    ctx.load_texture(
                        format!("qi_sprite:{path}"),
                        color_image,
                        egui::TextureOptions::LINEAR,
                    )
                })
            })
        })
    })
}

/// 取原始像素尺寸。优先用已解码纹理，否则只读文件头（不解码像素）。
fn dimensions_for(path: &str) -> Option<(i64, i64)> {
    DIMENSIONS.with(|cache| {
        FAILED.with(|failed| {
            cached_lookup(cache, failed, path, || {
                LOAD_COUNT.with(|c| c.set(c.get() + 1));
                image::image_dimensions(path)
                    .ok()
                    .map(|(w, h)| (w as i64, h as i64))
            })
        })
    })
}

// ============================================================================
// 绘制核心：一个函数吃下三个 FFI 的所有情况
// ============================================================================

/// 在栈顶画布上画一张精灵。`cx/cy` 是**中心**的画布局部坐标。
/// 加载不出来就在同样的位置/大小/角度上画品红占位方块。
fn draw_sprite(path: &str, cx: f32, cy: f32, w: f32, h: f32, degrees: f32, flip_h: bool) {
    let tex = texture_for(path);
    with_top_canvas(|canvas| {
        let (ox, oy) = (canvas.offset.x, canvas.offset.y);
        let corners = rotated_corners(ox + cx, oy + cy, w, h, degrees);
        match &tex {
            Some(t) => {
                let mesh = build_mesh(t.id(), corners, quad_uv(flip_h, false));
                canvas.painter.add(Shape::mesh(mesh));
            }
            None => {
                // 占位：实心品红 + 深色描边，位置/尺寸/角度与真图一致，好反推是哪一句
                canvas.painter.add(Shape::convex_polygon(
                    corners.to_vec(),
                    PLACEHOLDER,
                    egui::Stroke::new(2.0, Color32::from_rgb(90, 0, 70)),
                ));
            }
        }
    });
}

/// 左上角对齐的入口共用：把 (x,y,宽,高) 换算成中心点
fn top_left_to_center(x: f32, y: f32, w: f32, h: f32) -> (f32, f32) {
    (x + w / 2.0, y + h / 2.0)
}

// ============================================================================
// FFI
// ============================================================================

/// 画布图片(路径, x, y, 宽, 高)：左上角对齐，拉伸到给定宽高。
/// 宽或高传 <=0 时按图片原始尺寸的比例补齐；两个都 <=0 就用原始尺寸。
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_image_impl(
    path: *const c_char,
    x: i64,
    y: i64,
    width: i64,
    height: i64,
) {
    let p = cstr(path);
    let (w, h) = resolve_size(&p, width, height);
    let (cx, cy) = top_left_to_center(x as f32, y as f32, w, h);
    draw_sprite(&p, cx, cy, w, h, 0.0, false);
}

/// 画布图片旋转(路径, 中心x, 中心y, 宽, 高, 角度)：绕**中心**旋转，角度制，正数顺时针。
/// 做转向的小车 / 朝鼠标的飞机就用它。
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_image_rotated_impl(
    path: *const c_char,
    cx: i64,
    cy: i64,
    width: i64,
    height: i64,
    degrees: i64,
) {
    let p = cstr(path);
    let (w, h) = resolve_size(&p, width, height);
    draw_sprite(&p, cx as f32, cy as f32, w, h, degrees as f32, false);
}

/// 画布图片翻转(路径, x, y, 宽, 高, 水平翻)：水平翻 != 0 时左右镜像。
/// 角色向左走时翻一下，就不用为左右各画一张图。
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_image_flipped_impl(
    path: *const c_char,
    x: i64,
    y: i64,
    width: i64,
    height: i64,
    flip_h: i64,
) {
    let p = cstr(path);
    let (w, h) = resolve_size(&p, width, height);
    let (cx, cy) = top_left_to_center(x as f32, y as f32, w, h);
    draw_sprite(&p, cx, cy, w, h, 0.0, flip_h != 0);
}

/// 图片宽(路径) → 整数：原始像素宽。读不到返回 0（等比缩放前先判一下就不会除零）。
#[no_mangle]
pub extern "C" fn qi_gui_egui_image_width_impl(path: *const c_char) -> i64 {
    dimensions_for(&cstr(path)).map(|(w, _)| w).unwrap_or(0)
}

/// 图片高(路径) → 整数：原始像素高。读不到返回 0。
#[no_mangle]
pub extern "C" fn qi_gui_egui_image_height_impl(path: *const c_char) -> i64 {
    dimensions_for(&cstr(path)).map(|(_, h)| h).unwrap_or(0)
}

/// 宽/高 <=0 时用原始尺寸补齐（只给一边就按原图比例算另一边）。
/// 读不到原始尺寸就退到 64x64 —— 占位方块也得有个看得见的大小。
fn resolve_size(path: &str, width: i64, height: i64) -> (f32, f32) {
    if width > 0 && height > 0 {
        return (width as f32, height as f32);
    }
    let (ow, oh) = dimensions_for(path).unwrap_or((64, 64));
    let (ow, oh) = (ow.max(1) as f32, oh.max(1) as f32);
    match (width > 0, height > 0) {
        (true, false) => (width as f32, width as f32 * oh / ow),
        (false, true) => (height as f32 * ow / oh, height as f32),
        _ => (ow, oh),
    }
}

/// 供 Qi 侧探测精灵层是否可用（返回批次号）
#[no_mangle]
pub extern "C" fn qi_gui_egui_sprite_version_impl() -> i64 {
    1
}

// ============================================================================
// 单测
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    /// 顶点坐标断言：容差 0.01
    fn assert_near(got: Pos2, want: (f32, f32), what: &str) {
        assert!(
            (got.x - want.0).abs() < 0.01 && (got.y - want.1).abs() < 0.01,
            "{what}: 期望 {want:?}，实得 ({}, {})",
            got.x,
            got.y
        );
    }

    #[test]
    fn zero_degrees_is_axis_aligned() {
        let c = rotated_corners(100.0, 100.0, 40.0, 20.0, 0.0);
        assert_near(c[0], (80.0, 90.0), "左上");
        assert_near(c[1], (120.0, 90.0), "右上");
        assert_near(c[2], (120.0, 110.0), "右下");
        assert_near(c[3], (80.0, 110.0), "左下");
    }

    #[test]
    fn ninety_degrees_rotates_clockwise() {
        // 屏幕 Y 轴朝下：原来的"右上角"转 90 度后应落到右下方
        let c = rotated_corners(100.0, 100.0, 40.0, 20.0, 90.0);
        assert_near(c[0], (110.0, 80.0), "左上→");
        assert_near(c[1], (110.0, 120.0), "右上→");
        assert_near(c[2], (90.0, 120.0), "右下→");
        assert_near(c[3], (90.0, 80.0), "左下→");
        // 顺时针的判据：原来在正右方的边中点，转 90 度后跑到正下方
        let mid_right_x = (c[1].x + c[2].x) / 2.0;
        let mid_right_y = (c[1].y + c[2].y) / 2.0;
        assert!(
            (mid_right_x - 100.0).abs() < 0.01 && mid_right_y > 100.0,
            "右边中点应转到正下方，实得 ({mid_right_x}, {mid_right_y})"
        );
    }

    #[test]
    fn one_eighty_degrees_is_point_symmetric() {
        let c = rotated_corners(100.0, 100.0, 40.0, 20.0, 180.0);
        assert_near(c[0], (120.0, 110.0), "左上→右下");
        assert_near(c[2], (80.0, 90.0), "右下→左上");
    }

    #[test]
    fn rotation_preserves_size_and_center() {
        let c = rotated_corners(50.0, 60.0, 80.0, 30.0, 37.0);
        let edge_w = (c[1] - c[0]).length();
        let edge_h = (c[3] - c[0]).length();
        assert!((edge_w - 80.0).abs() < 0.01, "宽边变了：{edge_w}");
        assert!((edge_h - 30.0).abs() < 0.01, "高边变了：{edge_h}");
        let cx = (c[0].x + c[2].x) / 2.0;
        let cy = (c[0].y + c[2].y) / 2.0;
        assert_near(Pos2::new(cx, cy), (50.0, 60.0), "对角线中点=中心");
    }

    #[test]
    fn flip_only_swaps_uv_axis() {
        let normal = quad_uv(false, false);
        assert_near(normal[0], (0.0, 0.0), "左上 UV");
        assert_near(normal[1], (1.0, 0.0), "右上 UV");
        assert_near(normal[2], (1.0, 1.0), "右下 UV");
        assert_near(normal[3], (0.0, 1.0), "左下 UV");

        let flipped = quad_uv(true, false);
        // 横坐标对调，纵坐标原样
        for i in 0..4 {
            assert!(
                (flipped[i].x - (1.0 - normal[i].x)).abs() < 0.01,
                "第 {i} 角 u 未镜像"
            );
            assert!(
                (flipped[i].y - normal[i].y).abs() < 0.01,
                "第 {i} 角 v 被动了"
            );
        }

        let flipped_v = quad_uv(false, true);
        for i in 0..4 {
            assert!(
                (flipped_v[i].x - normal[i].x).abs() < 0.01,
                "第 {i} 角 u 被动了"
            );
            assert!(
                (flipped_v[i].y - (1.0 - normal[i].y)).abs() < 0.01,
                "第 {i} 角 v 未镜像"
            );
        }
    }

    #[test]
    fn mesh_is_two_triangles() {
        let corners = rotated_corners(10.0, 10.0, 20.0, 20.0, 45.0);
        let mesh = build_mesh(egui::TextureId::default(), corners, quad_uv(false, false));
        assert_eq!(mesh.vertices.len(), 4, "四个顶点");
        assert_eq!(mesh.indices, vec![0, 1, 2, 0, 2, 3], "两个三角形");
        assert!(mesh.vertices.iter().all(|v| v.color == Color32::WHITE));
    }

    /// 造一张 n×n 纯色 PNG（不下载素材、不进仓库；单测现场生成）
    fn write_png(path: &std::path::Path, w: u32, h: u32) {
        let buf = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 60, 90, 255]));
        buf.save(path).expect("写测试 PNG 失败");
    }

    #[test]
    fn second_size_query_hits_cache() {
        let dir = std::env::temp_dir().join(format!("qi_sprite_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("cache_hit.png");
        write_png(&png, 4, 6);
        let c_path = CString::new(png.to_str().unwrap()).unwrap();

        let before = LOAD_COUNT.with(|c| c.get());
        assert_eq!(qi_gui_egui_image_width_impl(c_path.as_ptr()), 4);
        assert_eq!(qi_gui_egui_image_height_impl(c_path.as_ptr()), 6);
        let after_first = LOAD_COUNT.with(|c| c.get());
        assert_eq!(after_first - before, 1, "首次应恰好读盘一次");

        // 再问 10 次：一次盘都不该读
        for _ in 0..10 {
            assert_eq!(qi_gui_egui_image_width_impl(c_path.as_ptr()), 4);
            assert_eq!(qi_gui_egui_image_height_impl(c_path.as_ptr()), 6);
        }
        assert_eq!(
            LOAD_COUNT.with(|c| c.get()),
            after_first,
            "缓存命中后不该再读盘"
        );
        let _ = std::fs::remove_file(&png);
    }

    #[test]
    fn missing_path_never_panics_and_retries_once() {
        let missing = CString::new("/绝不存在的目录/没有这张图.png").unwrap();
        let before = LOAD_COUNT.with(|c| c.get());
        assert_eq!(qi_gui_egui_image_width_impl(missing.as_ptr()), 0);
        assert_eq!(qi_gui_egui_image_height_impl(missing.as_ptr()), 0);
        assert_eq!(
            LOAD_COUNT.with(|c| c.get()) - before,
            1,
            "失败路径应进失败集，不该反复重试"
        );
        // 帧外画一张不存在的图：只是无操作，绝不能 panic
        qi_gui_egui_canvas_image_impl(missing.as_ptr(), 0, 0, 32, 32);
        qi_gui_egui_canvas_image_rotated_impl(missing.as_ptr(), 10, 10, 32, 32, 45);
        qi_gui_egui_canvas_image_flipped_impl(missing.as_ptr(), 0, 0, 32, 32, 1);
    }

    #[test]
    fn null_path_never_panics() {
        assert_eq!(qi_gui_egui_image_width_impl(std::ptr::null()), 0);
        qi_gui_egui_canvas_image_impl(std::ptr::null(), 0, 0, 16, 16);
    }

    #[test]
    fn one_sided_size_keeps_aspect_ratio() {
        let dir = std::env::temp_dir().join(format!("qi_sprite_ratio_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("ratio.png");
        write_png(&png, 40, 20); // 2:1
        let p = png.to_str().unwrap();

        assert_eq!(resolve_size(p, 80, 0), (80.0, 40.0), "给宽补高");
        assert_eq!(resolve_size(p, 0, 40), (80.0, 40.0), "给高补宽");
        assert_eq!(resolve_size(p, 0, 0), (40.0, 20.0), "都不给=原尺寸");
        assert_eq!(resolve_size(p, 7, 9), (7.0, 9.0), "都给就照给的来");
        let _ = std::fs::remove_file(&png);
    }

    // ── 跑一帧看真画了什么（脱离窗口，见 egui_app::run_headless_frame）──

    /// 从一帧的 shapes 里挑出所有贴图网格
    fn meshes_of(shapes: &[egui::epaint::ClippedShape]) -> Vec<&Mesh> {
        shapes
            .iter()
            .filter_map(|s| match &s.shape {
                Shape::Mesh(m) => Some(m),
                _ => None,
            })
            .collect()
    }

    /// 只挑精灵网格：四顶点 + 非默认纹理（字体图集也会产网格，要排掉）
    fn sprite_meshes(shapes: &[egui::epaint::ClippedShape]) -> Vec<&Mesh> {
        meshes_of(shapes)
            .into_iter()
            .filter(|m| m.vertices.len() == 4 && m.texture_id != egui::TextureId::default())
            .collect()
    }

    /// 挑出唯一那张精灵网格
    fn only_sprite(shapes: &[egui::epaint::ClippedShape]) -> &Mesh {
        let v = sprite_meshes(shapes);
        assert_eq!(v.len(), 1, "应当只有一张精灵网格，实得 {}", v.len());
        v[0]
    }

    /// 四个顶点的包围盒（宽, 高）—— 旋转有没有真发生，看这个最直接
    fn bbox(mesh: &Mesh) -> (f32, f32) {
        let xs: Vec<f32> = mesh.vertices.iter().map(|v| v.pos.x).collect();
        let ys: Vec<f32> = mesh.vertices.iter().map(|v| v.pos.y).collect();
        let w = xs.iter().cloned().fold(f32::MIN, f32::max)
            - xs.iter().cloned().fold(f32::MAX, f32::min);
        let h = ys.iter().cloned().fold(f32::MIN, f32::max)
            - ys.iter().cloned().fold(f32::MAX, f32::min);
        (w, h)
    }

    fn temp_png(name: &str, w: u32, h: u32) -> CString {
        let dir = std::env::temp_dir().join(format!("qi_sprite_frame_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join(name);
        write_png(&png, w, h);
        CString::new(png.to_str().unwrap()).unwrap()
    }

    #[test]
    fn frame_draw_emits_textured_quad() {
        let path = temp_png("draw.png", 8, 8);
        let frame = crate::egui_app::run_headless_frame(|| {
            let id = CString::new("stage").unwrap();
            crate::egui_canvas::qi_gui_egui_canvas_begin_impl(id.as_ptr(), 400, 300);
            qi_gui_egui_canvas_image_impl(path.as_ptr(), 20, 30, 40, 20);
            crate::egui_canvas::qi_gui_egui_canvas_end_impl();
        });
        let shapes = frame.shapes;
        let meshes = meshes_of(&shapes);
        // 字体图集也会产出网格，所以按"四个顶点 + 非默认纹理"筛我们那一张
        let sprite: Vec<&&Mesh> = meshes
            .iter()
            .filter(|m| m.vertices.len() == 4 && m.texture_id != egui::TextureId::default())
            .collect();
        assert_eq!(
            sprite.len(),
            1,
            "应当只有一张精灵网格，实得 {}",
            sprite.len()
        );
        let m = sprite[0];
        assert_eq!(m.indices, vec![0, 1, 2, 0, 2, 3], "两个三角形");
        let (w, h) = bbox(m);
        assert!(
            (w - 40.0).abs() < 0.01 && (h - 20.0).abs() < 0.01,
            "尺寸应为 40x20，实得 {w}x{h}"
        );
        // 左上角对齐：四角的最小点 = 画布偏移 + (20,30)。偏移未知，但两点之差已被上面钉死，
        // 这里再钉一次 x/y 的相对关系（左上角必须在中心的左上方）
        let cx = m.vertices.iter().map(|v| v.pos.x).sum::<f32>() / 4.0;
        assert!(
            m.vertices.iter().any(|v| v.pos.x < cx),
            "应有顶点在中心左侧"
        );
    }

    #[test]
    fn frame_rotation_turns_the_quad_on_its_side() {
        let path = temp_png("rot.png", 8, 8);
        let frame = crate::egui_app::run_headless_frame(|| {
            let id = CString::new("stage").unwrap();
            crate::egui_canvas::qi_gui_egui_canvas_begin_impl(id.as_ptr(), 400, 300);
            // 40 宽 20 高，转 90 度 → 包围盒应变成 20 宽 40 高
            qi_gui_egui_canvas_image_rotated_impl(path.as_ptr(), 200, 150, 40, 20, 90);
            crate::egui_canvas::qi_gui_egui_canvas_end_impl();
        });
        let shapes = frame.shapes;
        let m = only_sprite(&shapes);
        let (w, h) = bbox(m);
        assert!(
            (w - 20.0).abs() < 0.01 && (h - 40.0).abs() < 0.01,
            "转 90 度后应为 20x40，实得 {w}x{h}"
        );
    }

    #[test]
    fn frame_flip_swaps_uv_but_keeps_geometry() {
        let path = temp_png("flip.png", 8, 8);
        let grab = |flip: i64| {
            let p = path.clone();
            let frame = crate::egui_app::run_headless_frame(move || {
                let id = CString::new("stage").unwrap();
                crate::egui_canvas::qi_gui_egui_canvas_begin_impl(id.as_ptr(), 400, 300);
                qi_gui_egui_canvas_image_flipped_impl(p.as_ptr(), 10, 10, 40, 20, flip);
                crate::egui_canvas::qi_gui_egui_canvas_end_impl();
            });
            let shapes = frame.shapes;
            let m = only_sprite(&shapes);
            (
                m.vertices
                    .iter()
                    .map(|v| (v.pos.x, v.pos.y))
                    .collect::<Vec<_>>(),
                m.vertices
                    .iter()
                    .map(|v| (v.uv.x, v.uv.y))
                    .collect::<Vec<_>>(),
            )
        };
        let (pos_a, uv_a) = grab(0);
        let (pos_b, uv_b) = grab(1);
        assert_eq!(pos_a, pos_b, "翻转不该动顶点位置");
        assert_ne!(uv_a, uv_b, "翻转必须动 UV");
        for (a, b) in uv_a.iter().zip(uv_b.iter()) {
            assert!((a.0 - (1.0 - b.0)).abs() < 0.01, "u 应镜像");
            assert!((a.1 - b.1).abs() < 0.01, "v 不该动");
        }
    }

    #[test]
    fn frame_missing_image_draws_magenta_placeholder() {
        let missing = CString::new("/绝不存在的目录/占位.png").unwrap();
        let frame = crate::egui_app::run_headless_frame(|| {
            let id = CString::new("stage").unwrap();
            crate::egui_canvas::qi_gui_egui_canvas_begin_impl(id.as_ptr(), 400, 300);
            qi_gui_egui_canvas_image_impl(missing.as_ptr(), 20, 20, 40, 40);
            crate::egui_canvas::qi_gui_egui_canvas_end_impl();
        });
        let shapes = frame.shapes;
        // 占位是实心多边形，不是贴图网格
        let has_placeholder = shapes.iter().any(|s| match &s.shape {
            Shape::Path(p) => p.fill == PLACEHOLDER && p.points.len() == 4,
            _ => false,
        });
        assert!(has_placeholder, "加载失败时应画品红占位四边形");
        assert!(
            meshes_of(&shapes)
                .iter()
                .all(|m| m.texture_id == egui::TextureId::default()),
            "不该凭空冒出贴图网格"
        );
    }

    #[test]
    fn frame_same_path_decodes_once_across_many_draws() {
        let path = temp_png("cache.png", 8, 8);
        let before = LOAD_COUNT.with(|c| c.get());
        let frame = crate::egui_app::run_headless_frame(|| {
            let id = CString::new("stage").unwrap();
            crate::egui_canvas::qi_gui_egui_canvas_begin_impl(id.as_ptr(), 400, 300);
            // 同一张图画 30 次（模拟 immediate mode 逐帧重画 + 一帧里多个同款精灵）
            for k in 0..30 {
                qi_gui_egui_canvas_image_rotated_impl(path.as_ptr(), 50 + k, 50, 16, 16, k * 3);
            }
            crate::egui_canvas::qi_gui_egui_canvas_end_impl();
        });
        let shapes = frame.shapes;
        assert_eq!(
            LOAD_COUNT.with(|c| c.get()) - before,
            1,
            "30 次绘制只该解码一次"
        );
        let sprites = sprite_meshes(&shapes);
        assert_eq!(sprites.len(), 30, "30 次绘制应产出 30 张网格");
        // 而且它们共用同一个纹理 id —— 这就是缓存生效的直接证据
        let first = sprites[0].texture_id;
        assert!(
            sprites.iter().all(|m| m.texture_id == first),
            "应共用同一纹理"
        );
    }

    // ── 一路走到像素：tessellate + 软光栅，断言屏幕上真出现了这些颜色 ──
    // （截图要屏幕录制权限，CI 上没有；直接跑软光栅比截图更准也更稳）

    /// 取缓冲里 (x,y) 处的 RGB
    fn pixel(buf: &[u32], w: usize, x: usize, y: usize) -> (u8, u8, u8) {
        let p = buf[y * w + x];
        (
            ((p >> 16) & 0xff) as u8,
            ((p >> 8) & 0xff) as u8,
            (p & 0xff) as u8,
        )
    }

    fn close(a: (u8, u8, u8), b: (u8, u8, u8), tol: i32) -> bool {
        (a.0 as i32 - b.0 as i32).abs() <= tol
            && (a.1 as i32 - b.1 as i32).abs() <= tol
            && (a.2 as i32 - b.2 as i32).abs() <= tol
    }

    const FB_W: usize = 480;
    const FB_H: usize = 360;
    /// 传给光栅器的清屏色。注意：egui 的 CentralPanel 会用自己的 panel_fill
    /// 把整块盖掉，所以"空白处是什么颜色"不能拿这个常量断言 —— 要现场采样。
    const BG: [u8; 3] = [10, 10, 10];
    /// write_png 画的就是这个颜色
    const SPRITE_RGB: (u8, u8, u8) = (200, 60, 90);

    #[test]
    fn rasterized_sprite_paints_real_pixels() {
        let path = temp_png("raster.png", 8, 8);
        let offset = std::rc::Rc::new(Cell::new((0.0f32, 0.0f32)));
        let seen = offset.clone();
        let frame = crate::egui_app::run_headless_frame(move || {
            let id = CString::new("stage").unwrap();
            crate::egui_canvas::qi_gui_egui_canvas_begin_impl(id.as_ptr(), 300, 200);
            // 记下画布左上角在窗口里的位置，好精确定位待断言的像素
            crate::egui_app::with_top_canvas(|c| seen.set((c.offset.x, c.offset.y)));
            qi_gui_egui_canvas_image_impl(path.as_ptr(), 40, 30, 60, 40);
            crate::egui_canvas::qi_gui_egui_canvas_end_impl();
        });
        let buf = frame.rasterize(FB_W, FB_H, BG);
        let (ox, oy) = offset.get();

        // 精灵中心（局部 70,50）必须是素材的颜色
        let cx = (ox + 70.0) as usize;
        let cy = (oy + 50.0) as usize;
        let got = pixel(&buf, FB_W, cx, cy);
        assert!(
            close(got, SPRITE_RGB, 6),
            "精灵中心应为 {SPRITE_RGB:?}，实得 {got:?}"
        );

        // "空白"是什么颜色现场采一个（局部 250,180 那里什么都没画）。
        // 不能拿 BG 常量断言：CentralPanel 会用自己的 panel_fill 把清屏色盖掉。
        let empty = pixel(&buf, FB_W, (ox + 250.0) as usize, (oy + 180.0) as usize);
        assert!(!close(empty, SPRITE_RGB, 30), "采样点不能恰好落在精灵上");

        // 精灵框外（局部 200,160）应还是空白，说明没糊满整块
        let outside = pixel(&buf, FB_W, (ox + 200.0) as usize, (oy + 160.0) as usize);
        assert!(
            close(outside, empty, 2),
            "精灵框外应是空白，实得 {outside:?}"
        );

        // 左上角对齐：局部 (38,28) 在框外、(43,33) 在框内
        assert!(
            close(
                pixel(&buf, FB_W, (ox + 38.0) as usize, (oy + 28.0) as usize),
                empty,
                2
            ),
            "左上角再往外一点应还是空白 —— 说明是左上角对齐而不是居中"
        );
        assert!(
            close(
                pixel(&buf, FB_W, (ox + 43.0) as usize, (oy + 33.0) as usize),
                SPRITE_RGB,
                12
            ),
            "左上角往里一点应已是精灵"
        );
    }

    #[test]
    fn rasterized_missing_image_paints_magenta() {
        let missing = CString::new("/绝不存在的目录/像素占位.png").unwrap();
        let offset = std::rc::Rc::new(Cell::new((0.0f32, 0.0f32)));
        let seen = offset.clone();
        let frame = crate::egui_app::run_headless_frame(move || {
            let id = CString::new("stage").unwrap();
            crate::egui_canvas::qi_gui_egui_canvas_begin_impl(id.as_ptr(), 300, 200);
            crate::egui_app::with_top_canvas(|c| seen.set((c.offset.x, c.offset.y)));
            qi_gui_egui_canvas_image_impl(missing.as_ptr(), 40, 30, 60, 40);
            crate::egui_canvas::qi_gui_egui_canvas_end_impl();
        });
        let buf = frame.rasterize(FB_W, FB_H, BG);
        let (ox, oy) = offset.get();
        let got = pixel(&buf, FB_W, (ox + 70.0) as usize, (oy + 50.0) as usize);
        let want = (PLACEHOLDER.r(), PLACEHOLDER.g(), PLACEHOLDER.b());
        assert!(
            close(got, want, 6),
            "路径不存在时中心应是品红占位 {want:?}，实得 {got:?}"
        );
    }

    #[test]
    fn rasterized_rotation_moves_pixels_off_the_axis() {
        // 转 45 度后，原本在四角的像素应该空出来（菱形），正中心仍是实心
        let path = temp_png("raster_rot.png", 8, 8);
        let offset = std::rc::Rc::new(Cell::new((0.0f32, 0.0f32)));
        let seen = offset.clone();
        let frame = crate::egui_app::run_headless_frame(move || {
            let id = CString::new("stage").unwrap();
            crate::egui_canvas::qi_gui_egui_canvas_begin_impl(id.as_ptr(), 300, 200);
            crate::egui_app::with_top_canvas(|c| seen.set((c.offset.x, c.offset.y)));
            qi_gui_egui_canvas_image_rotated_impl(path.as_ptr(), 120, 100, 80, 80, 45);
            crate::egui_canvas::qi_gui_egui_canvas_end_impl();
        });
        let buf = frame.rasterize(FB_W, FB_H, BG);
        let (ox, oy) = offset.get();
        let at = |dx: f32, dy: f32| {
            pixel(
                &buf,
                FB_W,
                (ox + 120.0 + dx) as usize,
                (oy + 100.0 + dy) as usize,
            )
        };
        assert!(
            close(at(0.0, 0.0), SPRITE_RGB, 6),
            "中心应是精灵，实得 {:?}",
            at(0.0, 0.0)
        );
        // 空白色现场采（局部 20,20 那里没画东西）
        let empty = pixel(&buf, FB_W, (ox + 20.0) as usize, (oy + 20.0) as usize);
        // 未旋转时 (±36,±36) 还在方块内；转 45 度后这四个角必然露出空白
        for (dx, dy) in [(-36.0, -36.0), (36.0, -36.0), (36.0, 36.0), (-36.0, 36.0)] {
            let c = at(dx, dy);
            assert!(
                close(c, empty, 2),
                "转 45 度后角上 ({dx},{dy}) 应露空白，实得 {c:?}"
            );
        }
        // 而正上/下/左/右 38 像素处（菱形的尖）应仍在图内
        for (dx, dy) in [(0.0, -38.0), (38.0, 0.0), (0.0, 38.0), (-38.0, 0.0)] {
            let c = at(dx, dy);
            assert!(
                close(c, SPRITE_RGB, 20),
                "菱形四个尖 ({dx},{dy}) 应仍是精灵，实得 {c:?}"
            );
        }
    }

    #[test]
    fn unknown_size_falls_back_to_visible_box() {
        // 读不到原始尺寸也要给个看得见的方块，否则占位等于没画
        let (w, h) = resolve_size("/绝不存在的目录/x.png", 0, 0);
        assert!(w >= 1.0 && h >= 1.0, "占位尺寸必须可见，实得 {w}x{h}");
    }
}
