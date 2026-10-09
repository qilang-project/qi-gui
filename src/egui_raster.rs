//! egui 网格软件光栅化器
//!
//! 把 egui/epaint 生成的 `ClippedPrimitive`（三角网格 + 字体图集纹理）光栅化到
//! softbuffer 的 `u32` 帧缓冲（0x00RRGGBB）。不依赖 GL/GPU，跨平台稳定。
//!
//! ## 颜色约定
//! egui 的 `Color32` 是**预乘 alpha 的 sRGBA**。纹理（字体图集）也按预乘处理：
//!   - 字体/实心形状 → `TextureId::Managed(_)` 的覆盖度(coverage)图，白色像素覆盖度=1
//!     供实心形状采样；文字处覆盖度<1 形成抗锯齿。
//!   - 彩色图（用户纹理）→ 预乘 sRGBA。
//! 片元 src = 顶点色(预乘) ⊗ 纹素(预乘)，再用预乘 over 混合到不透明背景：
//!   out.rgb = src.rgb + dst.rgb * (1 - src.a)
//! 直接在 sRGB 空间混合（省略 gamma 校正）——文字/控件足够清晰，教学与截图验证够用。

use egui::epaint::{ClippedPrimitive, Color32, ImageData, ImageDelta, Mesh, Primitive, TextureId};
use std::collections::HashMap;

/// 单张纹理的像素数据
enum TexData {
    /// 覆盖度图（字体图集）：每像素一个 0..1 的覆盖度
    Coverage(Vec<f32>),
    /// 预乘 sRGBA 彩色图
    Color(Vec<[u8; 4]>),
}

struct Tex {
    w: usize,
    h: usize,
    data: TexData,
}

/// 纹理仓库：随 `TexturesDelta` 增量更新
#[derive(Default)]
pub struct TextureStore {
    map: HashMap<TextureId, Tex>,
}

impl TextureStore {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    /// 应用一批纹理增量（set / free）
    pub fn apply(&mut self, set: &[(TextureId, ImageDelta)], free: &[TextureId]) {
        for (id, delta) in set {
            self.set(*id, delta);
        }
        for id in free {
            self.map.remove(id);
        }
    }

    fn set(&mut self, id: TextureId, delta: &ImageDelta) {
        match &delta.image {
            ImageData::Font(font) => {
                let [dw, dh] = font.size;
                // 覆盖度：epaint FontImage.pixels 是 0..1 的线性覆盖度
                let patch: Vec<f32> = font.pixels.clone();
                if let Some([px, py]) = delta.pos {
                    // 局部更新：写入已有覆盖度纹理的子矩形
                    if let Some(tex) = self.map.get_mut(&id) {
                        if let TexData::Coverage(buf) = &mut tex.data {
                            for row in 0..dh {
                                for col in 0..dw {
                                    let dst = (py + row) * tex.w + (px + col);
                                    let srcv = patch[row * dw + col];
                                    if dst < buf.len() {
                                        buf[dst] = srcv;
                                    }
                                }
                            }
                        }
                    }
                } else {
                    self.map.insert(
                        id,
                        Tex {
                            w: dw,
                            h: dh,
                            data: TexData::Coverage(patch),
                        },
                    );
                }
            }
            ImageData::Color(color) => {
                let [dw, dh] = color.size;
                let patch: Vec<[u8; 4]> = color.pixels.iter().map(|c| c.to_array()).collect();
                if let Some([px, py]) = delta.pos {
                    if let Some(tex) = self.map.get_mut(&id) {
                        if let TexData::Color(buf) = &mut tex.data {
                            for row in 0..dh {
                                for col in 0..dw {
                                    let dst = (py + row) * tex.w + (px + col);
                                    if dst < buf.len() {
                                        buf[dst] = patch[row * dw + col];
                                    }
                                }
                            }
                        }
                    }
                } else {
                    self.map.insert(
                        id,
                        Tex {
                            w: dw,
                            h: dh,
                            data: TexData::Color(patch),
                        },
                    );
                }
            }
        }
    }

    /// 彩色纹理的原始像素（预乘 sRGBA），给快速贴图路径用；字体图集等返回 None
    fn color(&self, id: TextureId) -> Option<(usize, usize, &[[u8; 4]])> {
        match self.map.get(&id)? {
            Tex {
                w,
                h,
                data: TexData::Color(buf),
            } if *w > 0 && *h > 0 => Some((*w, *h, buf)),
            _ => None,
        }
    }

    /// 双线性采样，返回预乘 sRGBA（0..1）
    fn sample(&self, id: TextureId, u: f32, v: f32) -> [f32; 4] {
        let Some(tex) = self.map.get(&id) else {
            return [1.0, 1.0, 1.0, 1.0];
        };
        if tex.w == 0 || tex.h == 0 {
            return [1.0, 1.0, 1.0, 1.0];
        }
        // uv → 纹理像素坐标（-0.5 对齐纹素中心）
        let fx = (u * tex.w as f32 - 0.5).clamp(0.0, tex.w as f32 - 1.0);
        let fy = (v * tex.h as f32 - 0.5).clamp(0.0, tex.h as f32 - 1.0);
        let x0 = fx.floor() as usize;
        let y0 = fy.floor() as usize;
        let x1 = (x0 + 1).min(tex.w - 1);
        let y1 = (y0 + 1).min(tex.h - 1);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let get = |x: usize, y: usize| -> [f32; 4] {
            match &tex.data {
                TexData::Coverage(buf) => {
                    let c = buf[y * tex.w + x];
                    [c, c, c, c]
                }
                TexData::Color(buf) => {
                    let p = buf[y * tex.w + x];
                    [
                        p[0] as f32 / 255.0,
                        p[1] as f32 / 255.0,
                        p[2] as f32 / 255.0,
                        p[3] as f32 / 255.0,
                    ]
                }
            }
        };
        let a = get(x0, y0);
        let b = get(x1, y0);
        let c = get(x0, y1);
        let d = get(x1, y1);
        let mut out = [0.0f32; 4];
        for i in 0..4 {
            let top = a[i] * (1.0 - tx) + b[i] * tx;
            let bot = c[i] * (1.0 - tx) + d[i] * tx;
            out[i] = top * (1.0 - ty) + bot * ty;
        }
        out
    }
}

/// 把一批裁剪图元光栅化到帧缓冲。坐标以「点」为单位，乘 `ppp` 得到物理像素。
pub fn paint(
    buf: &mut [u32],
    fb_w: usize,
    fb_h: usize,
    ppp: f32,
    bg: [u8; 3],
    jobs: &[ClippedPrimitive],
    textures: &TextureStore,
) {
    paint_region(buf, fb_w, ppp, bg, jobs, textures, (0, 0, fb_w, fb_h));
}

/// 只重画帧缓冲里的一块（物理像素，左上含、右下不含）：先清成底色，再把所有图元
/// 裁到这块里画一遍。图元画出来的像素只取决于图元本身，所以一块区域里"重画全部
/// 图元"跟整屏重画在这块里的结果逐像素相同 —— 局部重画（`egui_damage`）靠的就是这个。
#[allow(clippy::too_many_arguments)]
pub fn paint_region(
    buf: &mut [u32],
    fb_w: usize,
    ppp: f32,
    bg: [u8; 3],
    jobs: &[ClippedPrimitive],
    textures: &TextureStore,
    region: (usize, usize, usize, usize),
) {
    let (rx0, ry0, rx1, ry1) = region;
    let fb_h = buf.len() / fb_w.max(1);
    let (rx1, ry1) = (rx1.min(fb_w), ry1.min(fb_h));
    if rx0 >= rx1 || ry0 >= ry1 {
        return;
    }
    // 清背景（不透明）
    let clear = ((bg[0] as u32) << 16) | ((bg[1] as u32) << 8) | (bg[2] as u32);
    for y in ry0..ry1 {
        buf[y * fb_w + rx0..y * fb_w + rx1].fill(clear);
    }

    for job in jobs {
        // 裁剪矩形 → 物理像素并夹到要画的区域内
        let cx0 = ((job.clip_rect.min.x * ppp).floor().max(0.0) as usize).max(rx0);
        let cy0 = ((job.clip_rect.min.y * ppp).floor().max(0.0) as usize).max(ry0);
        let cx1 = ((job.clip_rect.max.x * ppp).ceil().max(0.0) as usize).min(rx1);
        let cy1 = ((job.clip_rect.max.y * ppp).ceil().max(0.0) as usize).min(ry1);
        if cx0 >= cx1 || cy0 >= cy1 {
            continue;
        }
        match &job.primitive {
            Primitive::Mesh(mesh) => {
                raster_mesh(buf, fb_w, ppp, (cx0, cy0, cx1, cy1), mesh, textures);
            }
            Primitive::Callback(_) => {
                // 本软件后端不支持 paint callback（无 GPU 上下文）——忽略
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn raster_mesh(
    buf: &mut [u32],
    fb_w: usize,
    ppp: f32,
    clip: (usize, usize, usize, usize),
    mesh: &Mesh,
    textures: &TextureStore,
) {
    if blit_quad(buf, fb_w, ppp, clip, mesh, textures) {
        return;
    }
    let (cx0, cy0, cx1, cy1) = clip;
    let verts = &mesh.vertices;
    let idx = &mesh.indices;
    let tex_id = mesh.texture_id;
    let mut t = 0;
    while t + 2 < idx.len() {
        let v0 = &verts[idx[t] as usize];
        let v1 = &verts[idx[t + 1] as usize];
        let v2 = &verts[idx[t + 2] as usize];
        t += 3;

        let p0 = (v0.pos.x * ppp, v0.pos.y * ppp);
        let p1 = (v1.pos.x * ppp, v1.pos.y * ppp);
        let p2 = (v2.pos.x * ppp, v2.pos.y * ppp);

        // 三角形包围盒 ∩ 裁剪矩形
        let minx = p0.0.min(p1.0).min(p2.0).floor().max(cx0 as f32) as usize;
        let maxx = p0.0.max(p1.0).max(p2.0).ceil().min(cx1 as f32) as usize;
        let miny = p0.1.min(p1.1).min(p2.1).floor().max(cy0 as f32) as usize;
        let maxy = p0.1.max(p1.1).max(p2.1).ceil().min(cy1 as f32) as usize;
        if minx >= maxx || miny >= maxy {
            continue;
        }

        // 重心坐标分母
        let denom = (p1.1 - p2.1) * (p0.0 - p2.0) + (p2.0 - p1.0) * (p0.1 - p2.1);
        if denom.abs() < 1e-6 {
            continue;
        }
        let inv_denom = 1.0 / denom;

        let c0 = v0.color.to_array();
        let c1 = v1.color.to_array();
        let c2 = v2.color.to_array();

        // 纯色三角形（三个顶点同色、同一个 uv —— 实心矩形/底色块都是这样）：
        // 颜色不用逐像素插值、纹理只采一次，像素循环里只剩"在不在三角形里"的判断。
        // 大块底色是局部重画时最主要的开销（视频框底下垫着卡片底和纸色两层）
        if c0 == c1 && c1 == c2 && v0.uv == v1.uv && v1.uv == v2.uv {
            let tex = textures.sample(tex_id, v0.uv.x, v0.uv.y);
            let src = [
                c0[0] as f32 / 255.0 * tex[0],
                c0[1] as f32 / 255.0 * tex[1],
                c0[2] as f32 / 255.0 * tex[2],
                c0[3] as f32 / 255.0 * tex[3],
            ];
            if src[3] <= 0.0 {
                continue;
            }
            let opaque = src[3] >= 1.0;
            let solid = if opaque {
                let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
                (q(src[0]) << 16) | (q(src[1]) << 8) | q(src[2])
            } else {
                0
            };
            let inv = 1.0 - src[3];
            for y in miny..maxy {
                let py = y as f32 + 0.5;
                for x in minx..maxx {
                    let px = x as f32 + 0.5;
                    let w0 =
                        ((p1.1 - p2.1) * (px - p2.0) + (p2.0 - p1.0) * (py - p2.1)) * inv_denom;
                    let w1 =
                        ((p2.1 - p0.1) * (px - p2.0) + (p0.0 - p2.0) * (py - p2.1)) * inv_denom;
                    if w0 < 0.0 || w1 < 0.0 || 1.0 - w0 - w1 < 0.0 {
                        continue;
                    }
                    let dst_i = y * fb_w + x;
                    if opaque {
                        buf[dst_i] = solid;
                        continue;
                    }
                    let dst = buf[dst_i];
                    let ch = |s: f32, shift: u32| {
                        let d = ((dst >> shift) & 0xFF) as f32 / 255.0;
                        ((s + d * inv).clamp(0.0, 1.0) * 255.0 + 0.5) as u32
                    };
                    buf[dst_i] = (ch(src[0], 16) << 16) | (ch(src[1], 8) << 8) | ch(src[2], 0);
                }
            }
            continue;
        }

        for y in miny..maxy {
            let py = y as f32 + 0.5;
            for x in minx..maxx {
                let px = x as f32 + 0.5;
                // 重心权重
                let w0 = ((p1.1 - p2.1) * (px - p2.0) + (p2.0 - p1.0) * (py - p2.1)) * inv_denom;
                let w1 = ((p2.1 - p0.1) * (px - p2.0) + (p0.0 - p2.0) * (py - p2.1)) * inv_denom;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }

                // 插值顶点色（预乘 sRGBA）
                let vr = (c0[0] as f32 * w0 + c1[0] as f32 * w1 + c2[0] as f32 * w2) / 255.0;
                let vg = (c0[1] as f32 * w0 + c1[1] as f32 * w1 + c2[1] as f32 * w2) / 255.0;
                let vb = (c0[2] as f32 * w0 + c1[2] as f32 * w1 + c2[2] as f32 * w2) / 255.0;
                let va = (c0[3] as f32 * w0 + c1[3] as f32 * w1 + c2[3] as f32 * w2) / 255.0;

                // 插值 uv → 采样纹理（预乘）
                let u = v0.uv.x * w0 + v1.uv.x * w1 + v2.uv.x * w2;
                let vv = v0.uv.y * w0 + v1.uv.y * w1 + v2.uv.y * w2;
                let tex = textures.sample(tex_id, u, vv);

                // src = 顶点色 ⊗ 纹素（均预乘）
                let sr = vr * tex[0];
                let sg = vg * tex[1];
                let sb = vb * tex[2];
                let sa = va * tex[3];
                if sa <= 0.0 {
                    continue;
                }

                let dst_i = y * fb_w + x;
                let dst = buf[dst_i];
                let dr = ((dst >> 16) & 0xFF) as f32 / 255.0;
                let dg = ((dst >> 8) & 0xFF) as f32 / 255.0;
                let db = (dst & 0xFF) as f32 / 255.0;
                let inv = 1.0 - sa;
                // 四舍五入而不是截断：截断会让同一种颜色在相邻像素间 ±1 抖动，
                // 大面积浅色底上能看出一道道细纹，颜色也系统性偏暗
                let or = ((sr + dr * inv).clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
                let og = ((sg + dg * inv).clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
                let ob = ((sb + db * inv).clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
                buf[dst_i] = (or << 16) | (og << 8) | ob;
            }
        }
    }
}

/// 轴对齐、不着色（顶点全白）的单张贴图四边形 → 四角坐标与 uv：
/// `(x0, y0, x1, y1, u0, v0, u1, v1)`，(x0,y0) 处的 uv 是 (u0,v0)。
/// `Painter::image` 画的图片、视频、不旋转的精灵都是这个形状。
fn as_blit_quad(mesh: &Mesh) -> Option<[f32; 8]> {
    if mesh.vertices.len() != 4 || mesh.indices.len() != 6 {
        return None;
    }
    let v = &mesh.vertices;
    if v.iter().any(|p| p.color != Color32::WHITE) {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in v {
        x0 = x0.min(p.pos.x);
        y0 = y0.min(p.pos.y);
        x1 = x1.max(p.pos.x);
        y1 = y1.max(p.pos.y);
    }
    if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
        return None;
    }
    let at = |x: f32, y: f32| v.iter().find(|p| p.pos.x == x && p.pos.y == y);
    let (a, b, c, d) = (at(x0, y0)?, at(x1, y1)?, at(x1, y0)?, at(x0, y1)?);
    // 另两个角的 uv 必须由对角两点线性决定，否则就是旋转/扭曲过的，走通用路径
    if c.uv != egui::pos2(b.uv.x, a.uv.y) || d.uv != egui::pos2(a.uv.x, b.uv.y) {
        return None;
    }
    // 两个三角形得正好铺满这个矩形（对角线两端各出现两次）
    let mut seen = [0u8; 4];
    for &i in &mesh.indices {
        *seen.get_mut(i as usize)? += 1;
    }
    let mut counts = seen;
    counts.sort_unstable();
    if counts != [1, 1, 2, 2] {
        return None;
    }
    Some([x0, y0, x1, y1, a.uv.x, a.uv.y, b.uv.x, b.uv.y])
}

/// 快速贴图：轴对齐贴图四边形不走逐像素重心坐标，按行列直接做定点双线性采样。
/// 视频每帧要贴满几十万像素，通用路径一帧要十来毫秒，这里一两毫秒。
/// 采样公式与 `TextureStore::sample` 相同（纹素中心对齐 + 夹边），只是换成整数权重。
fn blit_quad(
    buf: &mut [u32],
    fb_w: usize,
    ppp: f32,
    clip: (usize, usize, usize, usize),
    mesh: &Mesh,
    textures: &TextureStore,
) -> bool {
    let Some([x0, y0, x1, y1, u0, v0, u1, v1]) = as_blit_quad(mesh) else {
        return false;
    };
    let Some((tw, th, tex)) = textures.color(mesh.texture_id) else {
        return false;
    };
    let (px0, py0, px1, py1) = (x0 * ppp, y0 * ppp, x1 * ppp, y1 * ppp);
    // 像素中心落在矩形里的才画（与三角形光栅的覆盖规则一致）
    let sx = ((px0 - 0.5).ceil().max(0.0) as usize).max(clip.0);
    let sy = ((py0 - 0.5).ceil().max(0.0) as usize).max(clip.1);
    let ex = ((px1 - 0.5).ceil().max(0.0) as usize).min(clip.2);
    let ey = ((py1 - 0.5).ceil().max(0.0) as usize).min(clip.3);
    if sx >= ex || sy >= ey {
        return true;
    }
    // 一维的"像素 → 纹素下标 + 8 位小数权重"表
    let axis = |p: usize, p0: f32, p1: f32, t0: f32, t1: f32, n: usize| -> (usize, usize, u32) {
        let t = t0 + ((p as f32 + 0.5) - p0) / (p1 - p0) * (t1 - t0);
        let f = (t * n as f32 - 0.5).clamp(0.0, n as f32 - 1.0);
        let i = f.floor() as usize;
        (i, (i + 1).min(n - 1), ((f - i as f32) * 256.0) as u32)
    };
    let cols: Vec<(usize, usize, u32)> = (sx..ex).map(|x| axis(x, px0, px1, u0, u1, tw)).collect();
    for y in sy..ey {
        let (r0, r1, fy) = axis(y, py0, py1, v0, v1, th);
        let (row0, row1) = (&tex[r0 * tw..r0 * tw + tw], &tex[r1 * tw..r1 * tw + tw]);
        let line = &mut buf[y * fb_w + sx..y * fb_w + ex];
        for (dst, &(c0, c1, fx)) in line.iter_mut().zip(&cols) {
            let (a, b, c, d) = (row0[c0], row0[c1], row1[c0], row1[c1]);
            let mut px = [0u32; 4];
            for (k, out) in px.iter_mut().enumerate() {
                let top = a[k] as u32 * (256 - fx) + b[k] as u32 * fx;
                let bot = c[k] as u32 * (256 - fx) + d[k] as u32 * fx;
                *out = (top * (256 - fy) + bot * fy + (1 << 15)) >> 16;
            }
            let sa = px[3];
            if sa == 0 {
                continue;
            }
            *dst = if sa >= 255 {
                (px[0] << 16) | (px[1] << 8) | px[2]
            } else {
                // 预乘 over：out = src + dst * (1 - a)
                let inv = 255 - sa;
                let ch = |src: u32, shift: u32| {
                    let d = (*dst >> shift) & 0xFF;
                    (src + (d * inv + 127) / 255).min(255)
                };
                (ch(px[0], 16) << 16) | (ch(px[1], 8) << 8) | ch(px[2], 0)
            };
        }
    }
    true
}

/// 便捷：把 egui 背景色转成 [u8;3]
pub fn color32_to_rgb(c: Color32) -> [u8; 3] {
    [c.r(), c.g(), c.b()]
}
