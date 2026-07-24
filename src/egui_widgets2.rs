//! egui 控件层 · 第二批控件 FFI
//!
//! 与 `egui_app.rs` 的第一批（22 个）同族：都从 thread_local 当前帧的 Ui 栈顶
//! 取 `&mut Ui` 调 egui。本文件只放"单次调用即完成"的控件；begin/end 配对的
//! 容器（滚动区/折叠区）在 egui_app.rs（需要动 FrameCtx 的容器栈）。
//!
//! 覆盖：单选 / 选择项 / 数字输入 / 浮点滑条 / 超链接 / 表格 / 柱状图 /
//!       图片显示 / 设置主题 / 界面缩放 / 设置窗口标题 / 悬浮标签

use crate::egui_app::{cstr, with_ctx, with_top_ui};
use std::cell::RefCell;
use std::collections::HashMap;
use std::os::raw::c_char;

use egui::{vec2, Id, RichText};

thread_local! {
    /// 图片纹理缓存：路径 → TextureHandle（持有句柄保活纹理；进程内不逐出）
    static IMAGES: RefCell<HashMap<String, egui::TextureHandle>> = RefCell::new(HashMap::new());
}

/// 单选按钮：selected=当前是否选中，返回 1=本帧被点击（调用方据此切换组内序号）
#[no_mangle]
pub extern "C" fn qi_gui_egui_radio_impl(text: *const c_char, selected: i32) -> i32 {
    let s = cstr(text);
    with_top_ui(|ui| {
        if ui.radio(selected != 0, s).clicked() {
            1
        } else {
            0
        }
    })
    .unwrap_or(0)
}

/// 可选中列表项（整行高亮）：返回 1=本帧被点击
#[no_mangle]
pub extern "C" fn qi_gui_egui_selectable_impl(text: *const c_char, selected: i32) -> i32 {
    let s = cstr(text);
    with_top_ui(|ui| {
        if ui.selectable_label(selected != 0, s).clicked() {
            1
        } else {
            0
        }
    })
    .unwrap_or(0)
}

/// 数字输入（拖拽/双击编辑的整数框）：返回新值
#[no_mangle]
pub extern "C" fn qi_gui_egui_drag_value_impl(_id: *const c_char, cur: i64) -> i64 {
    let mut v = cur;
    with_top_ui(|ui| {
        ui.add(egui::DragValue::new(&mut v).speed(1));
    });
    v
}

/// 浮点滑条：返回新值
#[no_mangle]
pub extern "C" fn qi_gui_egui_slider_f64_impl(
    _id: *const c_char,
    cur: f64,
    min: f64,
    max: f64,
) -> f64 {
    let mut v = cur;
    let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
    with_top_ui(|ui| {
        ui.add(egui::Slider::new(&mut v, lo..=hi));
    });
    v
}

/// 超链接：点击用系统浏览器打开（经 egui-winit 的 open_url 平台输出）
#[no_mangle]
pub extern "C" fn qi_gui_egui_hyperlink_impl(text: *const c_char, url: *const c_char) {
    let t = cstr(text);
    let u = cstr(url);
    with_top_ui(|ui| {
        ui.hyperlink_to(t, u);
    });
}

/// 带悬浮提示的标签：鼠标悬停显示气泡
#[no_mangle]
pub extern "C" fn qi_gui_egui_label_tip_impl(text: *const c_char, tip: *const c_char) {
    let s = cstr(text);
    let t = cstr(tip);
    with_top_ui(|ui| {
        ui.label(s).on_hover_text(t);
    });
}

/// 只读表格：表头 CSV（逗号分列），数据行以 '\n' 分行、逗号分列。斑马纹。
#[no_mangle]
pub extern "C" fn qi_gui_egui_table_impl(
    id: *const c_char,
    headers_csv: *const c_char,
    rows_data: *const c_char,
) {
    let id = cstr(id);
    let headers: Vec<String> = cstr(headers_csv)
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    let data = cstr(rows_data);
    with_top_ui(|ui| {
        egui::Grid::new(Id::new(("qi_table", &id)))
            .striped(true)
            .min_col_width(60.0)
            .show(ui, |ui| {
                for h in &headers {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for row in data.split('\n').filter(|r| !r.trim().is_empty()) {
                    for cell in row.split(',') {
                        ui.label(cell.trim());
                    }
                    ui.end_row();
                }
            });
    });
}

/// 柱状图：values 为 CSV 数值；宽高（点，0=自适应）
#[no_mangle]
pub extern "C" fn qi_gui_egui_bar_chart_impl(
    id: *const c_char,
    values_csv: *const c_char,
    width: i64,
    height: i64,
) {
    let id = cstr(id);
    let bars: Vec<egui_plot::Bar> = cstr(values_csv)
        .split(',')
        .filter_map(|s| s.trim().parse::<f64>().ok())
        .enumerate()
        .map(|(i, y)| egui_plot::Bar::new(i as f64, y).width(0.7))
        .collect();
    with_top_ui(|ui| {
        let mut plot = egui_plot::Plot::new(Id::new(("qi_bar", &id)));
        if width > 0 {
            plot = plot.width(width as f32);
        }
        if height > 0 {
            plot = plot.height(height as f32);
        }
        plot.show(ui, |plot_ui| {
            plot_ui.bar_chart(egui_plot::BarChart::new(bars));
        });
    });
}

/// 图片显示(路径, 宽, 高)：首次加载解码并缓存为纹理（png/jpg 等），之后直接画。
/// 宽/高传 0 = 用图片原始尺寸。加载失败画一行错误标签（不崩）。
#[no_mangle]
pub extern "C" fn qi_gui_egui_image_impl(path: *const c_char, width: i64, height: i64) {
    let p = cstr(path);
    // 先查缓存；未命中则解码 + 注册纹理
    let tex = IMAGES.with(|m| m.borrow().get(&p).cloned());
    let tex = match tex {
        Some(t) => Some(t),
        None => {
            let loaded = image::open(&p).ok().map(|img| {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw())
            });
            match loaded {
                Some(ci) => with_ctx(|ctx| ctx.load_texture(&p, ci, egui::TextureOptions::LINEAR))
                    .map(|handle| {
                        IMAGES.with(|m| m.borrow_mut().insert(p.clone(), handle.clone()));
                        handle
                    }),
                None => None,
            }
        }
    };
    with_top_ui(|ui| match &tex {
        Some(t) => {
            let orig = t.size_vec2();
            let size = match (width > 0, height > 0) {
                (true, true) => vec2(width as f32, height as f32),
                // 只给一边：按原图比例算另一边
                (true, false) => vec2(width as f32, width as f32 * orig.y / orig.x.max(1.0)),
                (false, true) => vec2(height as f32 * orig.x / orig.y.max(1.0), height as f32),
                (false, false) => orig,
            };
            ui.image(egui::load::SizedTexture::new(t.id(), size));
        }
        None => {
            ui.colored_label(egui::Color32::RED, format!("图片加载失败: {p}"));
        }
    });
}

/// 设置主题：1=深色，0=浅色。任意时刻可调（含帧内）。
#[no_mangle]
pub extern "C" fn qi_gui_egui_set_theme_impl(dark: i32) {
    with_ctx(|ctx| {
        ctx.set_visuals(if dark != 0 {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
    });
}

/// 界面缩放：百分比（100=原始，125=放大 1.25 倍…），夹在 50..300
#[no_mangle]
pub extern "C" fn qi_gui_egui_set_zoom_impl(percent: i64) {
    let z = (percent.clamp(50, 300) as f32) / 100.0;
    with_ctx(|ctx| {
        ctx.set_zoom_factor(z);
    });
}

/// 设置窗口标题（运行中随时改）
#[no_mangle]
pub extern "C" fn qi_gui_egui_window_title_impl(app_id: u64, title: *const c_char) {
    let t = cstr(title);
    crate::egui_app::set_window_title(app_id, &t);
}

/// 供 Qi 侧探测本批控件是否可用（返回批次号）
#[no_mangle]
pub extern "C" fn qi_gui_egui_widgets2_version_impl() -> i64 {
    2
}
