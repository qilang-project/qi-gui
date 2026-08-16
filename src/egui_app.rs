//! egui 控件层 —— immediate mode GUI，由 qilang 主循环驱动
//!
//! ## 架构
//! - **窗口后端**：winit 0.30（`pump_app_events` 逐帧抽事件：动画时非阻塞，
//!   静止时阻塞等事件）。qi-gui 唯一
//!   窗口栈（老 tao 自绘轨已于 2026-07-18 移除，图元能力由画布层承接）。
//! - **呈现**：softbuffer 软件帧缓冲 + `egui_raster` 自绘 epaint 网格（无 GL/GPU）。
//! - **主循环**：qilang 侧 `当(帧开始(句柄)){ ...控件... 帧结束(句柄) }`。
//!     - `帧开始`：pump 事件 → `ctx.begin_pass` → 建根 `Ui` 压栈 → 返回窗口是否存活。
//!     - 控件 FFI：从 thread_local 的当前帧 `Ui` 栈顶取 `&mut Ui` 调 egui 控件。
//!     - `帧结束`：`end_pass` → 静止判定 → tessellate → 光栅化 → present → 60fps 限帧。
//! - **静止跳帧**：画面跟上一次真画时逐字段相同就不 tessellate/不光栅化/不上屏，
//!   同时把抽事件切到阻塞档。静止时 CPU 从 ~104% 降到 3~4%，动画场景零影响。
//!   判定与两档事件循环的来龙去脉见 `qi_gui_egui_frame_begin_impl` /
//!   `qi_gui_egui_frame_end_impl` 里的注释；`QI_GUI_STATS=1` 可看到跳帧账。
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
    /// 这一轮 pump 里收到过**真输入/窗口状态**事件（鼠标、键盘、改尺寸、焦点…）。
    /// 静止跳帧的"立刻醒过来"闸门：`帧结束` 读一次就清零。
    ///
    /// 只认真事件，不认 `RedrawRequested` —— 那是我们自己每帧 `request_redraw`
    /// 招来的，把它算进去就永远静不下来（自激）。
    input_dirty: bool,
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
            input_dirty: true, // 第一帧必须真画
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
        // 白名单：哪些事件算"用户真的动了/窗口状态真的变了"，需要立刻恢复重画。
        // 用白名单而不是黑名单 —— 漏判一个新事件的后果只是"这一帧靠形状比对兜底"，
        // 而黑名单漏一个自激事件（比如 RedrawRequested）就会让跳帧彻底失效。
        // 注意鼠标移动：winit 只在指针**在本窗口内**时才发 CursorMoved，
        // 所以"鼠标在窗口外乱晃不算"是天然成立的，不用额外判。
        if matches!(
            event,
            WindowEvent::CursorMoved { .. }
                | WindowEvent::CursorEntered { .. }
                | WindowEvent::CursorLeft { .. }
                | WindowEvent::MouseInput { .. }
                | WindowEvent::MouseWheel { .. }
                | WindowEvent::KeyboardInput { .. }
                | WindowEvent::ModifiersChanged(_)
                | WindowEvent::Ime(_)
                | WindowEvent::Touch(_)
                | WindowEvent::TouchpadPressure { .. }
                | WindowEvent::PinchGesture { .. }
                | WindowEvent::PanGesture { .. }
                | WindowEvent::RotationGesture { .. }
                | WindowEvent::DoubleTapGesture { .. }
                | WindowEvent::Resized(_)
                | WindowEvent::ScaleFactorChanged { .. }
                | WindowEvent::Focused(_)
                | WindowEvent::Occluded(_)
                | WindowEvent::ThemeChanged(_)
                | WindowEvent::DroppedFile(_)
                | WindowEvent::HoveredFile(_)
                | WindowEvent::HoveredFileCancelled
        ) {
            self.input_dirty = true;
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

    // ── 静止跳帧（见 `qi_gui_egui_frame_end_impl` 里的判定注释）────────────
    /// 上一次**真正光栅化**那帧的形状列表。跳帧判定的主信号。
    last_shapes: Option<Vec<egui::epaint::ClippedShape>>,
    /// 上一次真正光栅化时的 (宽, 高, ppp 的位模式, 底色) —— 光栅结果的其余入参。
    last_paint_key: Option<(u32, u32, u32, [u8; 3])>,
    /// 上一次真正上屏的时刻，安全阀的基准。
    last_paint: Instant,
    /// 真正光栅化 + 上屏的帧数 / 判定为"画面没变"而跳过的帧数。
    /// `QI_GUI_STATS=1` 时关窗打一行，是这项优化唯一的可观测出口。
    painted: u64,
    skipped: u64,
    /// egui 自己说"要重绘"的帧数（`repaint_delay == 0`）。只统计不参与判定，
    /// 理由见 `qi_gui_egui_frame_end_impl`。
    egui_wanted: u64,
    /// 上一帧是不是被跳掉了 —— 决定下一次抽事件用阻塞档还是零超时档
    /// （见 `qi_gui_egui_frame_begin_impl` 里的超时注释）。
    last_skipped: bool,
    stats: bool,
    /// 抽事件 / 光栅化各自的累计耗时。跟计数一起在 `QI_GUI_STATS=1` 时报出来 ——
    /// 定位"CPU 到底烧在哪"时，这两个数一眼就能分清是渲染贵还是事件循环贵。
    t_pump: Duration,
    t_raster: Duration,
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
    // `Wait` 而不是 `Poll` —— 这一行是省电的关键，值得解释清楚。
    //
    // 帧的节奏由 qilang 主循环 + 限帧掌握，winit 这边不需要自己催。而 macOS 后端
    // 在 `Poll` 下每轮 runloop 结束都把唤醒时刻设成"立刻"（`app_timeout =
    // Some(Instant::now())`），加上 winit 那个重复间隔 0.1µs 的唤醒定时器，
    // CFRunLoop 就永远在 fire→重排→`mk_timer_arm` 之间打转，线程根本睡不着 ——
    // 实测这就是静止时 100% CPU 的大头，比软光栅还贵。
    //
    // 之后每帧 `帧开始` 会按动画/静止两档重设（见那里的注释）；这里设一次是给
    // 下面"泵到窗口创建出来"的那段循环用的。
    event_loop.set_control_flow(ControlFlow::Wait);

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
        last_shapes: None,
        last_paint_key: None,
        last_paint: Instant::now(),
        painted: 0,
        skipped: 0,
        egui_wanted: 0,
        last_skipped: false,
        t_pump: Duration::ZERO,
        t_raster: Duration::ZERO,
        stats: std::env::var("QI_GUI_STATS")
            .map(|v| v == "1")
            .unwrap_or(false),
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

        // ── 抽事件：动画档 / 静止档 ───────────────────────────────────────
        //
        // **这一段才是省电的大头，不是少光栅化。** 一开始想当然地以为静止时
        // 100% CPU 是软光栅烧的，做完形状比对一测：光栅化从 441 帧降到 13 帧，
        // CPU 纹丝不动，还是 103%。上 `sample` 一看，94% 的时间在
        // `pump_app_events` 里 —— NSApplication 的 run loop 起停、CFRunLoop
        // 观察者回调、`mk_timer_arm` 反复给唤醒定时器上弦。一次
        // `pump_app_events(Some(ZERO))` 实测就要烧掉 ~20ms 的 CPU，比一帧
        // 800×600 的软光栅（~23ms）还贵，而它每帧都要跑一次。
        //
        // 两档的分法：
        //
        //   * **动画档**（上一帧真画了）：`ControlFlow::Wait` + 超时 `ZERO`。
        //     winit 走 `stop_before_wait`，抽干事件立刻返回，一点帧率都不让 ——
        //     跟改动前完全一样（实测动画演示 327 帧、接小球 190 帧，无变化）。
        //
        //   * **静止档**（上一帧被判定"画面没变"跳过了）：`WaitUntil(现在+IDLE_TICK)`
        //     + 超时 `None`。此时 winit 走 `stop_after_wait`，线程真的阻塞在
        //     `mach_msg` 上睡觉，来事件立刻醒。CPU 从 43% 掉到 3~4%。
        //
        // 为什么静止档非得用 `None` 而不是"给 pump 一个非零超时"：winit 在
        // macOS 上取 `min(pump 的超时, ControlFlow 的到期时间)` 当唤醒时刻，
        // 而它的唤醒定时器重复间隔是 0.1µs —— 走 pump 超时那条路 run loop
        // 会一直在 fire→重排→上弦之间打转（实测给 500ms 超时，pump 平均
        // 26ms 就返回一次，CPU 43%）。改用 `WaitUntil` 把到期时间交给
        // ControlFlow，定时器才真的被设到未来，线程才睡得着。
        //
        // 静止档的代价：画面没变时 qilang 主循环从 60Hz 降到 `IDLE_TICK` 的
        // 30Hz 左右。这是有意的 —— 画面既然没变，主循环跑得再快也只是空转
        //（改动前软光栅本来也只跑到 32~40fps）。任何真输入都会立刻唤醒
        //（延迟不受 IDLE_TICK 影响），画面一动就切回动画档。
        let t_pump = Instant::now();
        let status = if app.last_skipped {
            app.event_loop
                .set_control_flow(ControlFlow::WaitUntil(Instant::now() + IDLE_TICK));
            app.event_loop.pump_app_events(None, &mut app.handler)
        } else {
            app.event_loop.set_control_flow(ControlFlow::Wait);
            app.event_loop
                .pump_app_events(Some(Duration::ZERO), &mut app.handler)
        };
        app.t_pump += t_pump.elapsed();
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
        // 纹理增量必须**无条件**吃掉：字体图集/图片是增量下发的，跳帧时丢一份，
        // 后面真画的那一帧就会去引用一块不存在的纹理（花屏或缺字）。
        let tex_changed =
            !output.textures_delta.set.is_empty() || !output.textures_delta.free.is_empty();
        app.textures
            .apply(&output.textures_delta.set, &output.textures_delta.free);

        let (w, h) = app.handler.size;
        let bg = egui_raster::color32_to_rgb(ctx.style().visuals.panel_fill);
        let paint_key = (w, h, ppp.to_bits(), bg);

        // ── 静止跳帧判定 ────────────────────────────────────────────────
        // 软光栅是纯 CPU 的：一帧 800×600 的 tessellate + 逐三角形扫描线混合要
        // 十几到二十几毫秒，于是"什么都没变的静止画面"也能把一个核吃满
        // （实测控件演示恒定 ~104% CPU）。课堂上的旧笔记本就是被这个烧的。
        //
        // 判据取**形状本身**，不取 egui 的重绘请求。理由：
        //   * 光栅结果是个纯函数 —— 像素 = f(shapes, ppp, 画布尺寸, 纹理, 底色)。
        //     这几项跟上次真画时逐字段相同，这一帧画出来必然是同样的像素，
        //     再画一遍纯属白烧 CPU。这条推理对任何场景都成立，不会误判。
        //   * egui 的 `repaint_delay` 只知道 egui 自己的控件要不要动，**不知道
        //     qi 侧画布这一帧画了什么**。海龟/小游戏每帧改的是画布图元，egui
        //     完全可能报"无需重绘"—— 信它就会把动画冻住。反过来它也常年报
        //     "要重绘"（悬停动画、光标闪烁的余波），信它又一帧都省不下来。
        //     所以它只被统计（`egui_wanted`），不参与判定。
        //   * 输入事件只作为额外的"立刻醒过来"闸门（`input_dirty`）。严格说它是
        //     冗余的（用户点了按钮 → 按钮外观变 → 形状就变了），留着是保险：
        //     万一哪天有种交互改的是像素之外的东西，不至于要等安全阀。
        if let Some(v) = output.viewport_output.get(&ViewportId::ROOT) {
            if v.repaint_delay == Duration::ZERO {
                app.egui_wanted += 1;
            }
        }
        let input_dirty = std::mem::take(&mut app.handler.input_dirty);
        let same_pixels = app.last_paint_key == Some(paint_key)
            && app.last_shapes.as_ref() == Some(&output.shapes);

        if should_skip(
            same_pixels,
            tex_changed,
            input_dirty,
            app.last_paint.elapsed(),
        ) {
            app.skipped += 1;
            app.last_skipped = true;
            // 跳帧时不 request_redraw：那既会招来一个白跑的 RedrawRequested，
            // 又会让下一次阻塞抽事件被 `stop_on_redraw` 立刻打断（白等于没等）。
            limit_fps(app);
            return;
        }
        app.last_skipped = false;

        // 真画：先留一份形状快照做下一帧的比对基准，再交给 tessellate（它要所有权）
        let t_raster = Instant::now();
        app.last_shapes = Some(output.shapes.clone());
        app.last_paint_key = Some(paint_key);
        let jobs = ctx.tessellate(output.shapes, ppp);

        if let (Some(nw), Some(nh)) = (NonZeroU32::new(w), NonZeroU32::new(h)) {
            let _ = surface.resize(nw, nh);
            if let Ok(mut buffer) = surface.buffer_mut() {
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
        app.t_raster += t_raster.elapsed();
        window.request_redraw();
        app.painted += 1;
        app.last_paint = Instant::now();

        limit_fps(app);
    });
}

/// 静止跳帧的安全阀间隔：再怎么判"没变"，也至少这么久真画一帧。
const SKIP_SAFETY_INTERVAL: Duration = Duration::from_secs(1);

/// 这一帧该不该跳过（不 tessellate / 不光栅化 / 不上屏）。
///
/// - `same_pixels`：形状 + 尺寸 + ppp + 底色都跟上次真画时逐字段相同
///   （相同 ⇒ 画出来必然是同样的像素，见调用处的推理）
/// - `tex_changed`：这一帧 egui 下发了纹理增量（字体图集扩了/图片换了）
/// - `input_dirty`：这一轮抽到过真输入或窗口状态事件
/// - `since_last_paint`：距上次真画过去了多久 —— 安全阀
///
/// 抽成独立函数是为了能单测：这四个条件的组合就是整套优化的全部风险面，
/// 错一个的后果是"动画冻住"或者"一点没省"，都值得钉死。
fn should_skip(
    same_pixels: bool,
    tex_changed: bool,
    input_dirty: bool,
    since_last_paint: Duration,
) -> bool {
    // 安全阀：判成"没变"也每秒至少真画一帧。
    // 这条不是为了正确性（`same_pixels` 的纯函数推理已经够），是为了兜底最坏情况 ——
    // 万一将来 egui 换版本让形状比对失真、或者哪个 FFI 绕过 shapes 直接改了
    // 呈现面，后果被限制在"最多迟滞 1 秒"，而不是"窗口永久停在旧画面/白屏"。
    // 代价是静止时 1 帧/秒的光栅化，占用可以忽略。
    same_pixels && !tex_changed && !input_dirty && since_last_paint < SKIP_SAFETY_INTERVAL
}

/// 每帧的时间配额（60fps）—— 动画档的限帧目标。
const FRAME_BUDGET: Duration = Duration::from_micros(1_000_000 / 60);

/// 静止档主循环的兜底心跳：画面没变时，最长睡这么久就得醒一次。
///
/// 它不是输入延迟 —— 任何鼠标/键盘/窗口事件都会立刻把阻塞中的 run loop 叫醒。
/// 它管的是"完全没有事件时，qilang 主循环多久转一圈"：安全阀要靠它按时到点，
/// `QI_GUI_AUTOCLOSE_MS` 要靠它按时收工，qi 侧写在循环里的非画面逻辑
/// （轮询、计时）也要靠它推进。
///
/// 取 33ms（30Hz）：阻塞档的一次抽事件实测只要几十微秒，所以 30Hz 跟 10Hz 的
/// CPU 几乎一样（控件演示都是 4%），没必要为了省那一点点把主循环拖慢 ——
/// 拖慢会连带影响两件事：qi 侧写在循环里的非画面逻辑（轮询、计时）会变迟钝，
/// 以及 `qi/tests/gui自动化/断言.sh` 那条"2 秒至少 20 帧"的硬门槛会贴着线走。
/// 30Hz 下静止的键盘演示 10 秒仍跑 358 轮（改动前 316 轮），门槛反而更宽。
const IDLE_TICK: Duration = Duration::from_millis(33);

/// 60fps 限帧。跳帧与真画共用 —— 跳帧省的是光栅化，帧循环的节奏不变，
/// 这样输入延迟还是一帧（~16ms），交互手感跟改动前一样。
///
/// 静止时这一段通常睡 0 —— 配额已经在 `帧开始` 那次阻塞抽事件里等掉了。
/// 留着它是backstop：万一某个平台的 pump 提前返回，节奏也不会失控成忙等。
fn limit_fps(app: &mut EguiApp) {
    let elapsed = app.last_frame.elapsed();
    if elapsed < FRAME_BUDGET {
        std::thread::sleep(FRAME_BUDGET - elapsed);
    }
    app.last_frame = Instant::now();
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
        let app = a.borrow_mut().remove(&app_id);
        // `QI_GUI_STATS=1` 时报一行跳帧账。默认一个字都不打（零行为变化）。
        // 这是静止跳帧唯一的可观测出口：示例自己打的「共渲染 N 帧」数的是
        // **主循环轮数**（qi 侧的计数器），跟真正光栅化了几帧不是一回事。
        if let Some(app) = app {
            if app.stats {
                let total = app.painted + app.skipped;
                eprintln!(
                    "egui: 帧统计 —— 主循环 {total} 帧，光栅化 {} 帧，静止跳过 {} 帧；\
                     其中 egui 自称需要重绘 {} 帧",
                    app.painted, app.skipped, app.egui_wanted
                );
                eprintln!(
                    "egui: 耗时 —— 抽事件累计 {:?}，光栅化累计 {:?}",
                    app.t_pump, app.t_raster
                );
            }
        }
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

// ============================================================================
// 单测用：脱离窗口跑一帧
// ============================================================================

/// 仅供单测：不开窗、不碰 winit/softbuffer，直接用一个裸 `egui::Context` 跑一帧，
/// 把根 Ui 压进 FRAME，于是所有控件/画布/精灵 FFI 都能像在真帧里一样被调用。
/// 返回这一帧产生的 `ClippedShape` 列表 —— 测试据此断言"到底画出了什么"。
///
/// 存在的理由：CI 和本地都可能没有屏幕录制权限，截图验不了画面；而"精灵有没有
/// 真的变成一块带纹理的网格、四个顶点在不在该在的位置"恰恰是本层最该被钉住的
/// 东西。跑一帧拿 shapes 比截图更准，也更稳定。
#[cfg(test)]
pub(crate) struct HeadlessFrame {
    pub shapes: Vec<egui::epaint::ClippedShape>,
    pub ctx: egui::Context,
    pub textures_delta: egui::TexturesDelta,
}

#[cfg(test)]
impl HeadlessFrame {
    /// 把这一帧真的光栅化成 RGB 像素（走的就是窗口里那条 tessellate + 软光栅路径），
    /// 返回 (像素缓冲, 宽, 高)。像素是 0x00RRGGBB。
    ///
    /// 有了它，"精灵到底有没有出现在屏幕上、是什么颜色"可以直接断言像素，
    /// 不必依赖截图权限。
    pub fn rasterize(self, w: usize, h: usize, bg: [u8; 3]) -> Vec<u32> {
        let mut store = crate::egui_raster::TextureStore::new();
        store.apply(&self.textures_delta.set, &self.textures_delta.free);
        let jobs = self.ctx.tessellate(self.shapes, 1.0);
        let mut buf = vec![0u32; w * h];
        crate::egui_raster::paint(&mut buf, w, h, 1.0, bg, &jobs, &store);
        buf
    }
}

#[cfg(test)]
pub(crate) fn run_headless_frame(f: impl FnOnce()) -> HeadlessFrame {
    let ctx = egui::Context::default();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            vec2(800.0, 600.0),
        )),
        ..Default::default()
    };
    // Context::run 要 FnMut，而 f 是 FnOnce —— 装进 Option 里 take 一次
    let mut once = Some(f);
    let output = ctx.run(input, move |ctx| {
        let Some(body) = once.take() else {
            return;
        };
        egui::CentralPanel::default().show(ctx, |ui| {
            FRAME.with(|fr| {
                *fr.borrow_mut() = Some(FrameCtx {
                    ctx: ctx.clone(),
                    ppp: 1.0,
                    ui_stack: vec![ui as *mut egui::Ui],
                    containers: Vec::new(),
                });
            });
            body();
            FRAME.with(|fr| {
                *fr.borrow_mut() = None;
            });
        });
    });
    HeadlessFrame {
        shapes: output.shapes,
        ctx,
        textures_delta: output.textures_delta,
    }
}

// ============================================================================
// 静止跳帧的单测
// ============================================================================

#[cfg(test)]
mod skip_tests {
    use super::*;
    use std::ffi::CString;

    const FRESH: Duration = Duration::from_millis(10);

    #[test]
    fn 画面没变就跳过() {
        assert!(should_skip(true, false, false, FRESH));
    }

    #[test]
    fn 画面变了必须画() {
        assert!(!should_skip(false, false, false, FRESH));
    }

    #[test]
    fn 纹理增量必须画() {
        // 字体图集刚扩过/图片刚换过，跳掉这一帧屏幕上就是旧图集
        assert!(!should_skip(true, true, false, FRESH));
    }

    #[test]
    fn 有输入就立刻恢复重画() {
        assert!(!should_skip(true, false, true, FRESH));
    }

    #[test]
    fn 安全阀到点必须画() {
        // 哪怕四项都判"没变"，超过安全阀间隔也得真画一帧 —— 防误判导致画面卡死
        assert!(!should_skip(true, false, false, SKIP_SAFETY_INTERVAL));
        assert!(!should_skip(
            true,
            false,
            false,
            SKIP_SAFETY_INTERVAL + Duration::from_millis(1)
        ));
        // 差一点点还不到，仍然跳
        assert!(should_skip(
            true,
            false,
            false,
            SKIP_SAFETY_INTERVAL - Duration::from_millis(1)
        ));
    }

    #[test]
    fn 静止档心跳不能长过安全阀() {
        // 主循环在静止档最长睡 IDLE_TICK 才转一圈；它要是比安全阀还长，
        // 安全阀就永远迟到，"每秒至少真画一帧"这条保证会失效。
        assert!(
            IDLE_TICK < SKIP_SAFETY_INTERVAL,
            "静止心跳 {IDLE_TICK:?} 必须短于安全阀 {SKIP_SAFETY_INTERVAL:?}"
        );
    }

    /// 跳帧判据的地基：**同样的界面画两帧，产生的形状必须逐字段相等**。
    /// 这一条要是不成立（比如 egui 哪天在形状里塞进时间戳/随机 id），
    /// 静止跳帧就一帧都省不下来 —— 是回归而不是崩溃，只有测试能发现。
    #[test]
    fn 同样的界面两帧形状相同() {
        let draw = || {
            let text = CString::new("静止的一行字").unwrap();
            qi_gui_egui_label_impl(text.as_ptr());
        };
        let a = run_headless_frame(draw);
        let b = run_headless_frame(draw);
        assert_eq!(a.shapes, b.shapes, "同样的界面两帧形状竟然不同");
    }

    /// 反过来：画布内容变了，形状必须跟着变。海龟/小游戏就是靠这个不被冻住 ——
    /// egui 自己的重绘信号不知道 qi 侧画布画了什么，只有形状比对认得出来。
    #[test]
    fn 画布内容变了形状就变() {
        let frame_at = |x: i64| {
            run_headless_frame(move || {
                let id = CString::new("场景").unwrap();
                crate::egui_canvas::qi_gui_egui_canvas_begin_impl(id.as_ptr(), 200, 100);
                crate::egui_canvas::qi_gui_egui_canvas_rect_impl(x, 10, 20, 20, 255, 0, 0);
                crate::egui_canvas::qi_gui_egui_canvas_end_impl();
            })
        };
        let a = frame_at(10);
        let b = frame_at(10);
        let c = frame_at(40);
        assert_eq!(a.shapes, b.shapes, "同一位置的方块两帧形状应相同");
        assert_ne!(
            a.shapes, c.shapes,
            "方块挪了位置，形状却没变 —— 动画会被冻住"
        );
    }
}
