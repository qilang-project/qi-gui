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
    // 清背景（不透明）
    let clear = ((bg[0] as u32) << 16) | ((bg[1] as u32) << 8) | (bg[2] as u32);
    for p in buf.iter_mut() {
        *p = clear;
    }

    for job in jobs {
        // 裁剪矩形 → 物理像素并夹到帧内
        let cx0 = (job.clip_rect.min.x * ppp).floor().max(0.0) as usize;
        let cy0 = (job.clip_rect.min.y * ppp).floor().max(0.0) as usize;
        let cx1 = (job.clip_rect.max.x * ppp).ceil().min(fb_w as f32) as usize;
        let cy1 = (job.clip_rect.max.y * ppp).ceil().min(fb_h as f32) as usize;
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
                let or = ((sr + dr * inv).clamp(0.0, 1.0) * 255.0) as u32;
                let og = ((sg + dg * inv).clamp(0.0, 1.0) * 255.0) as u32;
                let ob = ((sb + db * inv).clamp(0.0, 1.0) * 255.0) as u32;
                buf[dst_i] = (or << 16) | (og << 8) | ob;
            }
        }
    }
}

/// 便捷：把 egui 背景色转成 [u8;3]
pub fn color32_to_rgb(c: Color32) -> [u8; 3] {
    [c.r(), c.g(), c.b()]
}
