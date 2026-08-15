//! egui 控件层 —— immediate mode GUI，由 qilang 主循环驱动
//!
//! ## 架构
//! - **窗口后端**：winit 0.30（`pump_app_events` 非阻塞逐帧抽事件）。qi-gui 唯一
//!   窗口栈（老 tao 自绘轨已于 2026-07-18 移除，图元能力由画布层承接）。
//! - **呈现**：softbuffer 软件帧缓冲 + `egui_raster` 自绘 epaint 网格（无 GL/GPU）。
//! - **主循环**：qilang 侧 `当(帧开始(句柄)){ ...控件... 帧结束(句柄) }`。
//!     - `帧开始`：pump 事件 → `ctx.begin_pass` → 建根 `Ui` 压栈 → 返回窗口是否存活。
//!     - 控件 FFI：从 thread_local 的当前帧 `Ui` 栈顶取 `&mut Ui` 调 egui 控件。
//!     - `帧结束`：`end_pass` → tessellate → 光栅化 → present → 60fps 限帧。
//! - **immediate mode 语义**：控件状态由 qilang 侧每帧传入/取回（输入框等）。egui 内部
//!   仅保留焦点/光标等瞬态，用 id 串标识。
//!
//! ## 中文字体
//! 运行时探测系统 CJK 字体注入 egui（macOS PingFang/STHeiti、Linux Noto、Windows
//! 雅黑）。找不到则退回 egui 默认并告警（中文会显示豆腐块）。不内嵌字体进仓库。

use crate::egui_raster::{self, TextureStore};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::num::NonZeroU32;
use std::os::raw::c_char;
use std::rc::Rc;
use std::time::{Duration, Instant};

use egui::{vec2, Align, Color32, Id, LayerId, Layout, Stroke, UiBuilder, ViewportId};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::platform::pump_events::{EventLoopExtPumpEvents, PumpStatus};
use winit::window::{Window, WindowId};

type SbSurface = softbuffer::Surface<Rc<Window>, Rc<Window>>;

/// winit 应用处理器：持有窗口/呈现面/egui-winit 状态，逐帧抽事件时更新
struct EguiHandler {
    title: String,
    init_w: u32,
    init_h: u32,
    window: Option<Rc<Window>>,
    surface: Option<SbSurface>,
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    size: (u32, u32),
    close_requested: bool,
}

impl EguiHandler {
    fn new(ctx: egui::Context, title: String, w: u32, h: u32) -> Self {
        EguiHandler {
            title,
            init_w: w,
            init_h: h,
            window: None,
            surface: None,
            egui_ctx: ctx,
            egui_state: None,
            size: (w, h),
            close_requested: false,
        }
    }

    fn create(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(&self.title)
            .with_inner_size(LogicalSize::new(self.init_w, self.init_h));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                eprintln!("egui: 创建窗口失败: {e}");
                return;
            }
        };
        let ph = window.inner_size();
        self.size = (ph.width.max(1), ph.height.max(1));

        let context = match softbuffer::Context::new(window.clone()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("egui: softbuffer 上下文失败: {e}");
                return;
            }
        };
        let surface = match softbuffer::Surface::new(&context, window.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("egui: softbuffer 呈现面失败: {e}");
                return;
            }
        };

        let state = egui_winit::State::new(
            self.egui_ctx.clone(),
            ViewportId::ROOT,
            &*window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );

        self.window = Some(window);
        self.surface = Some(surface);
        self.egui_state = Some(state);
    }
}

impl ApplicationHandler for EguiHandler {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.create(event_loop);
    }

    fn window_event(&mut self, _el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let (Some(win), Some(state)) = (&self.window, &mut self.egui_state) {
            let _ = state.on_window_event(win, &event);
        }
        match event {
            WindowEvent::CloseRequested => self.close_requested = true,
            WindowEvent::Resized(sz) => {
                self.size = (sz.width.max(1), sz.height.max(1));
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {}
}

/// 一个 egui 应用实例（窗口 + 事件循环 + 纹理仓库 + 限帧状态）
struct EguiApp {
    event_loop: EventLoop<()>,
    handler: EguiHandler,
    textures: TextureStore,
    last_frame: Instant,
    alive: bool,
    /// 帧循环起始时刻（**第一次 `帧开始` 时才置位**），配合 `autoclose` 做自动关窗。
    /// 为什么不用"应用创建时刻"：`应用创建` 自己就要花掉一大截时间 —— 建事件循环、
    /// 泵到窗口真正出来、读系统 CJK 字体（PingFang.ttc 是几十 MB）、建字体图集，
    /// macOS 上实测 2.4 秒左右。从创建时刻计时的话，`QI_GUI_AUTOCLOSE_MS=2000`
    /// 会**一帧都没渲染就退出**，而程序照样打印"窗口已关闭"、退出码 0 ——
    /// 一条什么都没验到的假绿。从第一帧起算，语义就是干脆的"帧循环跑 N 毫秒"。
    loop_start: Option<Instant>,
    /// 自动关窗时限。来自环境变量 `QI_GUI_AUTOCLOSE_MS`，未设为 None（零行为变化）。
    autoclose: Option<Duration>,
}

/// 读测试钩子 `QI_GUI_AUTOCLOSE_MS`。
///
/// **为什么需要它**：GUI 的自动化验收有个死结 —— 程序开了窗就等用户去关，
/// CI 里没有用户，脚本只能挂死或者被 timeout 杀掉（拿不到干净的退出码）。
/// 设了这个变量后，`帧开始` 在**帧循环**跑满该毫秒数时返回 0，效果**等同于用户关窗**：
/// qi 侧 `当(帧开始(应用))` 循环正常结束，走完 `关闭应用` 后正常退出，退出码 0。
/// 计时从第一次 `帧开始` 起算（不含窗口/字体的启动开销，理由见 `loop_start`）。
///
/// 不设时完全不生效，正常使用零影响。CI 与后续所有 GUI 自动化都靠这个钩子。
fn read_autoclose() -> Option<Duration> {
    let raw = std::env::var("QI_GUI_AUTOCLOSE_MS").ok()?;
    match raw.trim().parse::<u64>() {
        Ok(ms) => Some(Duration::from_millis(ms)),
        Err(_) => {
            eprintln!("egui: QI_GUI_AUTOCLOSE_MS 不是合法毫秒数「{raw}」，忽略。");
            None
        }
    }
}

/// 当前帧上下文：控件 FFI 从这里取 Ui 栈顶
struct FrameCtx {
    ctx: egui::Context,
    ppp: f32,
    ui_stack: Vec<*mut egui::Ui>,
    /// begin/end 式容器（滚动区/折叠区）的配对元数据栈
    containers: Vec<Container>,
}

/// begin/end 容器元数据：end 时据此收尾（滚动条绘制/光标推进/是否需弹 Ui）
enum Container {
    /// 滚动区：id 用于在 egui 内存里持久化滚动偏移，viewport 是可视窗口
    Scroll { id: Id, viewport: egui::Rect },
    /// 折叠区：展开时压了子 Ui（收起时没压，end 不弹）
    Collapse { pushed: bool },
    /// 画布：定尺寸自绘区。painter 承接老图元能力（矩形/圆/线/文本），
    /// offset 是画布左上角全局坐标（局部坐标 + offset = 全局），
    /// response 存点击/悬停查询。allocate_painter 已占位并推进父光标，故 end 只弹元数据。
    Canvas(CanvasCtx),
}

/// 画布上下文：绘制 FFI 从这里取 painter，查询 FFI 从这里读 response
pub(crate) struct CanvasCtx {
    pub painter: egui::Painter,
    pub offset: egui::Pos2,
    pub response: egui::Response,
}

thread_local! {
    static APPS: RefCell<HashMap<u64, EguiApp>> = RefCell::new(HashMap::new());
    static NEXT_ID: RefCell<u64> = const { RefCell::new(1) };
    static FRAME: RefCell<Option<FrameCtx>> = const { RefCell::new(None) };
    /// 字符串返回复用缓冲：Qi 侧调用点会立刻 qi_string_from_cstr 拷贝，
    /// 故单槽复用即可，零逐帧泄漏。
    static RET_BUF: RefCell<Option<CString>> = const { RefCell::new(None) };
}

pub(crate) fn ret_str(s: String) -> *const c_char {
    RET_BUF.with(|b| {
        let c = CString::new(s).unwrap_or_default();
        let ptr = c.as_ptr();
        *b.borrow_mut() = Some(c);
        ptr
    })
}

pub(crate) fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p).to_string_lossy().into_owned() }
}

/// 探测并注入系统 CJK 字体
fn install_cjk_fonts(ctx: &egui::Context) {
    let candidates: &[&str] = &[
        // macOS
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
        // Linux
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        // Windows
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\msyh.ttf",
        "C:\\Windows\\Fonts\\simhei.ttf",
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("qi_cjk".to_owned(), egui::FontData::from_owned(bytes));
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "qi_cjk".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("qi_cjk".to_owned());
            ctx.set_fonts(fonts);
            return;
        }
    }
    eprintln!("egui: 未找到系统 CJK 字体，中文可能显示为方块。");
}

// ============================================================================
// FFI —— 应用生命周期
// ============================================================================

/// 创建 egui 应用窗口，返回句柄（>0 成功，0 失败）
#[no_mangle]
pub extern "C" fn qi_gui_egui_app_create_impl(
    title: *const c_char,
    width: u32,
    height: u32,
) -> u64 {
    let title = if title.is_null() {
        "Qi".to_string()
    } else {
        cstr(title)
    };

    let event_loop = match EventLoop::builder().build() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("egui: 创建事件循环失败: {e}");
            return 0;
        }
    };
    event_loop.set_control_flow(ControlFlow::Poll);

    let ctx = egui::Context::default();
    install_cjk_fonts(&ctx);
    ctx.set_pixels_per_point(1.0); // 由 egui-winit 按窗口 scale 覆盖

    let mut app = EguiApp {
        event_loop,
        handler: EguiHandler::new(ctx, title, width.max(1), height.max(1)),
        textures: TextureStore::new(),
        last_frame: Instant::now(),
        alive: true,
        loop_start: None,
        autoclose: read_autoclose(),
    };

    // 泵事件直到窗口创建（resumed 触发）
    let mut tries = 0;
    while app.handler.window.is_none() && tries < 200 {
        let status = app
            .event_loop
            .pump_app_events(Some(Duration::from_millis(5)), &mut app.handler);
        if let PumpStatus::Exit(_) = status {
            break;
        }
        tries += 1;
    }
    if app.handler.window.is_none() {
        eprintln!("egui: 窗口未能创建");
        return 0;
    }

    let id = NEXT_ID.with(|n| {
        let mut n = n.borrow_mut();
        let id = *n;
        *n += 1;
        id
    });
    APPS.with(|a| a.borrow_mut().insert(id, app));
    id
}

/// 帧开始：pump 事件 + begin_pass + 建根 Ui。返回 1=窗口存活，0=已关闭。
#[no_mangle]
pub extern "C" fn qi_gui_egui_frame_begin_impl(app_id: u64) -> i32 {
    APPS.with(|apps| {
        let mut apps = apps.borrow_mut();
        let Some(app) = apps.get_mut(&app_id) else {
            return 0;
        };
        if !app.alive {
            return 0;
        }

        // 测试钩子：帧循环跑满时限就假装用户关了窗（详见 read_autoclose 的注释）
        if let Some(limit) = app.autoclose {
            let started = *app.loop_start.get_or_insert_with(Instant::now);
            if started.elapsed() >= limit {
                app.alive = false;
                return 0;
            }
        }

        // 非阻塞抽干事件
        let status = app
            .event_loop
            .pump_app_events(Some(Duration::ZERO), &mut app.handler);
        if let PumpStatus::Exit(_) = status {
            app.alive = false;
            return 0;
        }
        if app.handler.close_requested {
            app.alive = false;
            return 0;
        }

        let (Some(window), Some(state)) =
            (app.handler.window.clone(), app.handler.egui_state.as_mut())
        else {
            return 0;
        };

        let raw_input = state.take_egui_input(&window);
        let ctx = app.handler.egui_ctx.clone();
        ctx.begin_pass(raw_input);
        // 抓这一帧的键盘快照（必须在 begin_pass 之后，那时 InputState 才是本帧的）
        crate::egui_keyboard::capture(&ctx);
        let ppp = ctx.pixels_per_point();

        // 建根 Ui：占满可用区域，留 10pt 边距，纵向布局
        let mut rect = ctx.available_rect();
        rect = rect.shrink(10.0);
        let root = egui::Ui::new(
            ctx.clone(),
            LayerId::background(),
            Id::new("qi_root"),
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::top_down(Align::Min)),
        );
        let root_ptr = Box::into_raw(Box::new(root));

        FRAME.with(|f| {
            *f.borrow_mut() = Some(FrameCtx {
                ctx,
                ppp,
                ui_stack: vec![root_ptr],
                containers: Vec::new(),
            });
        });
        1
    })
}

/// 帧结束：end_pass + tessellate + 光栅化 + present + 限帧
#[no_mangle]
pub extern "C" fn qi_gui_egui_frame_end_impl(app_id: u64) {
    // 先收掉当前帧的 Ui 栈（还原并释放所有 Box<Ui>）
    let frame = FRAME.with(|f| f.borrow_mut().take());
    let Some(frame) = frame else {
        return;
    };
    for ptr in frame.ui_stack.into_iter().rev() {
        unsafe {
            drop(Box::from_raw(ptr));
        }
    }
    let ctx = frame.ctx;
    let ppp = frame.ppp;

    APPS.with(|apps| {
        let mut apps = apps.borrow_mut();
        let Some(app) = apps.get_mut(&app_id) else {
            return;
        };
        let (Some(window), Some(state), Some(surface)) = (
            app.handler.window.clone(),
            app.handler.egui_state.as_mut(),
            app.handler.surface.as_mut(),
        ) else {
            return;
        };

        let output = ctx.end_pass();
        state.handle_platform_output(&window, output.platform_output);
        app.textures
            .apply(&output.textures_delta.set, &output.textures_delta.free);
        let jobs = ctx.tessellate(output.shapes, ppp);

        let (w, h) = app.handler.size;
        if let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) {
            let _ = surface.resize(nw, nh);
            if let Ok(mut buffer) = surface.buffer_mut() {
                let bg = egui_raster::color32_to_rgb(ctx.style().visuals.panel_fill);
                egui_raster::paint(
                    &mut buffer,
                    w as usize,
                    h as usize,
                    ppp,
                    bg,
                    &jobs,
                    &app.textures,
                );
                let _ = buffer.present();
            }
        }
        window.request_redraw();

        // 60fps 限帧
        let target = Duration::from_micros(1_000_000 / 60);
        let elapsed = app.last_frame.elapsed();
        if elapsed < target {
            std::thread::sleep(target - elapsed);
        }
        app.last_frame = Instant::now();
    });
}

/// 改窗口标题（供 egui_widgets2 的 设置窗口标题 用）
pub(crate) fn set_window_title(app_id: u64, title: &str) {
    APPS.with(|a| {
        if let Some(app) = a.borrow().get(&app_id) {
            if let Some(w) = &app.handler.window {
                w.set_title(title);
            }
        }
    });
}

/// 关闭应用（销毁窗口，释放资源）
#[no_mangle]
pub extern "C" fn qi_gui_egui_app_close_impl(app_id: u64) {
    let _ = FRAME.with(|f| f.borrow_mut().take());
    APPS.with(|a| {
        a.borrow_mut().remove(&app_id);
    });
}

// ============================================================================
// 帧内 Ui 栈辅助
// ============================================================================

pub(crate) fn with_top_ui<R>(f: impl FnOnce(&mut egui::Ui) -> R) -> Option<R> {
    FRAME.with(|fr| {
        let b = fr.borrow();
        let frame = b.as_ref()?;
        let ptr = *frame.ui_stack.last()?;
        Some(f(unsafe { &mut *ptr }))
    })
}

pub(crate) fn with_ctx<R>(f: impl FnOnce(&egui::Context) -> R) -> Option<R> {
    FRAME.with(|fr| {
        let b = fr.borrow();
        let frame = b.as_ref()?;
        Some(f(&frame.ctx))
    })
}

// ============================================================================
// 画布容器辅助（供 egui_canvas.rs 的 FFI 调用）
// ============================================================================

/// 画布开始：在当前 Ui 顶上分配一块定尺寸自绘区，painter/response 压入容器栈。
/// allocate_painter 会占位并推进父光标，因此结束时无需再手动推进。
pub(crate) fn canvas_begin(width: f32, height: f32) {
    FRAME.with(|fr| {
        let mut b = fr.borrow_mut();
        let Some(frame) = b.as_mut() else {
            return;
        };
        let Some(&parent_ptr) = frame.ui_stack.last() else {
            return;
        };
        let parent = unsafe { &mut *parent_ptr };
        let (response, mut painter) =
            parent.allocate_painter(vec2(width.max(1.0), height.max(1.0)), egui::Sense::click());
        let offset = response.rect.min;
        // 裁剪到画布矩形：越界图元不画
        painter.set_clip_rect(response.rect.intersect(parent.clip_rect()));
        frame.containers.push(Container::Canvas(CanvasCtx {
            painter,
            offset,
            response,
        }));
    });
}

/// 画布结束：弹出画布容器元数据（占位已在 begin 完成）
pub(crate) fn canvas_end() {
    FRAME.with(|fr| {
        let mut b = fr.borrow_mut();
        let Some(frame) = b.as_mut() else {
            return;
        };
        // 只在栈顶确实是画布时弹，避免配对错乱破坏别的容器
        if matches!(frame.containers.last(), Some(Container::Canvas(_))) {
            frame.containers.pop();
        }
    });
}

/// 取栈顶画布上下文（从后往前找最近的 Canvas），供绘制/查询 FFI 使用
pub(crate) fn with_top_canvas<R>(f: impl FnOnce(&CanvasCtx) -> R) -> Option<R> {
    FRAME.with(|fr| {
        let b = fr.borrow();
        let frame = b.as_ref()?;
        for c in frame.containers.iter().rev() {
            if let Container::Canvas(cc) = c {
                return Some(f(cc));
            }
        }
        None
    })
}

/// 压入一个子布局 Ui（复刻 egui `scope_dyn`/`horizontal` 的做法：`new_child` +
/// 结束时 `advance_cursor_after_rect`）。水平布局把子 Ui 高度约束到一行，避免
/// 纵向居中把内容撑满整列。
fn push_layout(is_horizontal: bool, indent: f32) {
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
        let builder = if is_horizontal {
            let h = parent.spacing().interact_size.y;
            let rect = egui::Rect::from_min_size(avail.min, vec2(avail.width(), h));
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::left_to_right(Align::Center))
        } else {
            let mut rect = avail;
            rect.min.x += indent;
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::top_down(Align::Min))
        };
        let child = parent.new_child(builder);
        frame.ui_stack.push(Box::into_raw(Box::new(child)));
    });
}

fn pop_layout(draw_frame: bool) {
    FRAME.with(|fr| {
        let mut b = fr.borrow_mut();
        let Some(frame) = b.as_mut() else {
            return;
        };
        if frame.ui_stack.len() <= 1 {
            return; // 从不弹根
        }
        let child_ptr = frame.ui_stack.pop().unwrap();
        let child = unsafe { Box::from_raw(child_ptr) };
        let rect = child.min_rect();
        drop(child);
        let parent_ptr = *frame.ui_stack.last().unwrap();
        let parent = unsafe { &mut *parent_ptr };
        if draw_frame {
            let framed = rect.expand(6.0);
            let stroke = Stroke::new(1.0, parent.visuals().widgets.noninteractive.bg_stroke.color);
            parent.painter().rect_stroke(framed, 4.0, stroke);
            parent.advance_cursor_after_rect(framed);
        } else {
            parent.advance_cursor_after_rect(rect);
        }
    });
}

// ============================================================================
// FFI —— 容器：滚动区 / 折叠区（begin/end 配对，元数据走 containers 栈）
// ============================================================================

/// 滚动开始(id, 高度pt)：固定高度的垂直滚动视口。内容超高时出滚动条，
/// 滚轮悬停滚动。偏移量按 id 持久化在 egui 内存里（跨帧保持）。
#[no_mangle]
pub extern "C" fn qi_gui_egui_scroll_begin_impl(id: *const c_char, height: i64) {
    let sid = Id::new(("qi_scroll", cstr(id)));
    FRAME.with(|fr| {
        let mut b = fr.borrow_mut();
        let Some(frame) = b.as_mut() else {
            return;
        };
        let Some(&parent_ptr) = frame.ui_stack.last() else {
            return;
        };
        let parent = unsafe { &mut *parent_ptr };
        let offset: f32 = parent
            .ctx()
            .data_mut(|d| *d.get_persisted_mut_or(sid, 0.0f32));
        let avail = parent.available_rect_before_wrap();
        let h = (height.max(40) as f32).min(avail.height().max(40.0));
        let viewport = egui::Rect::from_min_size(avail.min, vec2(avail.width(), h));
        // 内容 Ui：从视口顶上移 offset 起排，右侧留 14pt 滚道；高度无限（由内容撑）
        let content_rect = egui::Rect::from_min_size(
            viewport.min - vec2(0.0, offset),
            vec2((viewport.width() - 14.0).max(20.0), f32::INFINITY),
        );
        let mut child = parent.new_child(
            UiBuilder::new()
                .max_rect(content_rect)
                .layout(Layout::top_down(Align::Min)),
        );
        // 裁剪到视口：视口外的内容不画（光栅器按 clip_rect 裁）
        child.set_clip_rect(viewport.intersect(parent.clip_rect()));
        frame.ui_stack.push(Box::into_raw(Box::new(child)));
        frame
            .containers
            .push(Container::Scroll { id: sid, viewport });
    });
}

/// 滚动结束：收内容高度 → 处理滚轮 → 画滚动条 → 光标推进过视口
#[no_mangle]
pub extern "C" fn qi_gui_egui_scroll_end_impl() {
    FRAME.with(|fr| {
        let mut b = fr.borrow_mut();
        let Some(frame) = b.as_mut() else {
            return;
        };
        let Some(Container::Scroll { id, viewport }) = frame.containers.pop() else {
            return; // 配对错乱：忽略（不弹 Ui，避免把别的容器弄塌）
        };
        if frame.ui_stack.len() <= 1 {
            return;
        }
        let child = unsafe { Box::from_raw(frame.ui_stack.pop().unwrap()) };
        let content_h = child.min_rect().height();
        drop(child);
        let parent = unsafe { &mut **frame.ui_stack.last().unwrap() };

        // 滚轮（悬停视口时生效）；偏移夹在 [0, 内容高-视口高]
        let mut offset: f32 = parent
            .ctx()
            .data_mut(|d| *d.get_persisted_mut_or(id, 0.0f32));
        if parent.rect_contains_pointer(viewport) {
            let dy = parent.ctx().input(|i| i.smooth_scroll_delta.y);
            offset -= dy;
        }
        let max_off = (content_h - viewport.height()).max(0.0);
        offset = offset.clamp(0.0, max_off);
        parent.ctx().data_mut(|d| d.insert_persisted(id, offset));

        // 滚动条：右缘 6pt 滑道 + 按比例的滑块
        if max_off > 0.0 {
            let track = egui::Rect::from_min_max(
                egui::pos2(viewport.max.x - 8.0, viewport.min.y + 2.0),
                egui::pos2(viewport.max.x - 2.0, viewport.max.y - 2.0),
            );
            let track_h = track.height();
            let thumb_h = (viewport.height() / content_h * track_h).max(24.0);
            let thumb_y = track.min.y + (offset / max_off) * (track_h - thumb_h);
            let thumb = egui::Rect::from_min_size(
                egui::pos2(track.min.x, thumb_y),
                vec2(track.width(), thumb_h),
            );
            let weak = parent.visuals().widgets.noninteractive.bg_fill;
            let strong = parent.visuals().widgets.inactive.fg_stroke.color;
            parent.painter().rect_filled(track, 3.0, weak);
            parent.painter().rect_filled(thumb, 3.0, strong);
        }
        parent.advance_cursor_after_rect(viewport);
    });
}

/// 折叠开始(标题)：可点开合的分区头。返回 1=展开（子控件会显示）/ 0=收起。
/// 收起时调用方照常写子控件调用也无妨（会落到父 Ui），但推荐用返回值跳过。
#[no_mangle]
pub extern "C" fn qi_gui_egui_collapse_begin_impl(title: *const c_char) -> i32 {
    let t = cstr(title);
    let cid = Id::new(("qi_collapse", &t));
    let mut open_now = false;
    FRAME.with(|fr| {
        let mut b = fr.borrow_mut();
        let Some(frame) = b.as_mut() else {
            return;
        };
        let Some(&parent_ptr) = frame.ui_stack.last() else {
            return;
        };
        let parent = unsafe { &mut *parent_ptr };
        let mut open: bool = parent
            .ctx()
            .data_mut(|d| *d.get_persisted_mut_or(cid, false));
        let arrow = if open { "▼" } else { "▶" };
        if parent
            .selectable_label(false, format!("{arrow} {t}"))
            .clicked()
        {
            open = !open;
            parent.ctx().data_mut(|d| d.insert_persisted(cid, open));
        }
        if open {
            // 展开：压缩进 12pt 的纵向子 Ui（与 push_layout(false, 12.0) 同构，
            // 就地实现避免嵌套借用 FRAME）
            let avail = parent.available_rect_before_wrap();
            let mut rect = avail;
            rect.min.x += 12.0;
            let child = parent.new_child(
                UiBuilder::new()
                    .max_rect(rect)
                    .layout(Layout::top_down(Align::Min)),
            );
            frame.ui_stack.push(Box::into_raw(Box::new(child)));
        }
        frame.containers.push(Container::Collapse { pushed: open });
        open_now = open;
    });
    if open_now {
        1
    } else {
        0
    }
}

/// 折叠结束：与 折叠开始 配对；展开时弹出子 Ui 并推进父光标
#[no_mangle]
pub extern "C" fn qi_gui_egui_collapse_end_impl() {
    let need_pop = FRAME.with(|fr| {
        let mut b = fr.borrow_mut();
        let Some(frame) = b.as_mut() else {
            return false;
        };
        match frame.containers.pop() {
            Some(Container::Collapse { pushed }) => pushed,
            _ => false,
        }
    });
    if need_pop {
        pop_layout(false);
    }
}

// ============================================================================
// FFI —— 控件
// ============================================================================

/// 普通标签
#[no_mangle]
pub extern "C" fn qi_gui_egui_label_impl(text: *const c_char) {
    let s = cstr(text);
    with_top_ui(|ui| {
        ui.label(s);
    });
}

/// 大号标题
#[no_mangle]
pub extern "C" fn qi_gui_egui_heading_impl(text: *const c_char) {
    let s = cstr(text);
    with_top_ui(|ui| {
        ui.heading(s);
    });
}

/// 彩色标签
#[no_mangle]
pub extern "C" fn qi_gui_egui_colored_label_impl(text: *const c_char, r: i64, g: i64, b: i64) {
    let s = cstr(text);
    let col = Color32::from_rgb(r as u8, g as u8, b as u8);
    with_top_ui(|ui| {
        ui.colored_label(col, s);
    });
}

/// 按钮：返回本帧是否被点击（1/0）
#[no_mangle]
pub extern "C" fn qi_gui_egui_button_impl(text: *const c_char) -> i32 {
    let s = cstr(text);
    with_top_ui(|ui| if ui.button(s).clicked() { 1 } else { 0 }).unwrap_or(0)
}

/// 单行输入框：传入当前值，返回编辑后的新值
#[no_mangle]
pub extern "C" fn qi_gui_egui_text_edit_impl(
    id: *const c_char,
    value: *const c_char,
) -> *const c_char {
    let id = cstr(id);
    let mut buf = cstr(value);
    with_top_ui(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut buf)
                .id(Id::new(("qi_edit", id)))
                .desired_width(f32::INFINITY),
        );
    });
    ret_str(buf)
}

/// 多行输入框
#[no_mangle]
pub extern "C" fn qi_gui_egui_text_edit_multiline_impl(
    id: *const c_char,
    value: *const c_char,
) -> *const c_char {
    let id = cstr(id);
    let mut buf = cstr(value);
    with_top_ui(|ui| {
        ui.add(
            egui::TextEdit::multiline(&mut buf)
                .id(Id::new(("qi_edit_ml", id)))
                .desired_width(f32::INFINITY),
        );
    });
    ret_str(buf)
}

/// 整数滑条：返回新值
#[no_mangle]
pub extern "C" fn qi_gui_egui_slider_impl(_id: *const c_char, cur: i64, min: i64, max: i64) -> i64 {
    let mut v = cur;
    let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
    with_top_ui(|ui| {
        ui.add(egui::Slider::new(&mut v, lo..=hi));
    });
    v
}

/// 复选框：返回新的勾选状态（1/0）
#[no_mangle]
pub extern "C" fn qi_gui_egui_checkbox_impl(
    _id: *const c_char,
    text: *const c_char,
    cur: i32,
) -> i32 {
    let s = cstr(text);
    let mut checked = cur != 0;
    with_top_ui(|ui| {
        ui.checkbox(&mut checked, s);
    });
    if checked {
        1
    } else {
        0
    }
}

/// 下拉选择：options 为 CSV（逗号分隔），cur 为当前序号，返回新序号
#[no_mangle]
pub extern "C" fn qi_gui_egui_combo_impl(
    id: *const c_char,
    options_csv: *const c_char,
    cur: i64,
) -> i64 {
    let id = cstr(id);
    let opts: Vec<String> = cstr(options_csv)
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    let mut sel = cur.clamp(0, opts.len().saturating_sub(1) as i64) as usize;
    let selected_text = opts.get(sel).cloned().unwrap_or_default();
    with_top_ui(|ui| {
        egui::ComboBox::from_id_salt(Id::new(("qi_combo", id)))
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                for (i, opt) in opts.iter().enumerate() {
                    ui.selectable_value(&mut sel, i, opt);
                }
            });
    });
    sel as i64
}

/// 分隔线
#[no_mangle]
pub extern "C" fn qi_gui_egui_separator_impl() {
    with_top_ui(|ui| {
        ui.separator();
    });
}

/// 空行（纵向间距）
#[no_mangle]
pub extern "C" fn qi_gui_egui_space_impl() {
    with_top_ui(|ui| {
        ui.add_space(8.0);
    });
}

/// 水平布局开始
#[no_mangle]
pub extern "C" fn qi_gui_egui_horizontal_begin_impl() {
    push_layout(true, 0.0);
}

/// 水平布局结束
#[no_mangle]
pub extern "C" fn qi_gui_egui_horizontal_end_impl() {
    pop_layout(false);
}

/// 分组开始（带标题的边框容器）
#[no_mangle]
pub extern "C" fn qi_gui_egui_group_begin_impl(title: *const c_char) {
    let t = cstr(title);
    if !t.is_empty() {
        with_top_ui(|ui| {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(t).strong());
        });
    }
    push_layout(false, 10.0);
}

/// 分组结束
#[no_mangle]
pub extern "C" fn qi_gui_egui_group_end_impl() {
    pop_layout(true);
    with_top_ui(|ui| {
        ui.add_space(4.0);
    });
}

/// 进度条：percent 0..100
#[no_mangle]
pub extern "C" fn qi_gui_egui_progress_impl(percent: i64) {
    let frac = (percent.clamp(0, 100) as f32) / 100.0;
    with_top_ui(|ui| {
        ui.add(egui::ProgressBar::new(frac).show_percentage());
    });
}

/// 折线图：id 标识，values 为 CSV 数值，宽高（点）。用于画损失曲线等。
#[no_mangle]
pub extern "C" fn qi_gui_egui_plot_impl(
    id: *const c_char,
    values_csv: *const c_char,
    width: i64,
    height: i64,
) {
    let id = cstr(id);
    let ys: Vec<f64> = cstr(values_csv)
        .split(',')
        .filter_map(|s| s.trim().parse::<f64>().ok())
        .collect();
    let points: Vec<[f64; 2]> = ys.iter().enumerate().map(|(i, y)| [i as f64, *y]).collect();
    with_top_ui(|ui| {
        let mut plot = egui_plot::Plot::new(Id::new(("qi_plot", id)));
        if width > 0 {
            plot = plot.width(width as f32);
        }
        if height > 0 {
            plot = plot.height(height as f32);
        }
        plot.show(ui, |plot_ui| {
            plot_ui.line(egui_plot::Line::new(egui_plot::PlotPoints::from(points)));
        });
    });
}

/// 消息弹窗：浮动窗口显示文本（需每帧调用以保持显示）
#[no_mangle]
pub extern "C" fn qi_gui_egui_message_impl(text: *const c_char) {
    let s = cstr(text);
    with_ctx(|ctx| {
        egui::Window::new("提示")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(s);
            });
    });
}
