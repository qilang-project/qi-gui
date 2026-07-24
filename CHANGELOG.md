# Qi GUI 更新日志

## 2026-07-18 · 单轨 egui 收敛：移除 tao + 新增画布层（累计 47 个 egui FFI）

### 移除老 tao 自绘轨（彻底淘汰）
- 删除 `window.rs`/`event.rs`/`renderer.rs`/`keycode.rs` 及 `ffi.rs` 的窗口/渲染器/
  事件回调/定时器/帧率相关 FFI；音频 FFI 剥离到独立的 `audio_ffi.rs`（去 tao/GuiState 依赖，
  句柄改用原子计数器）。
- `Cargo.toml` 移除 `tao`、`tiny-skia`、`cosmic-text`、`raw-window-handle`、`once_cell`
  依赖；删除三个 tao Rust 示例（`simple_window`/`keyboard_demo`/`window_control`）。
- `qi-runtime`（及孪生 `qi/src/runtime/stdlib/gui_ffi.rs`）删除老轨 extern/shim；
  `module_registry.rs` 删除老轨中文注册（创建窗口/窗口控制/事件/定时器/渲染器/运行）。
- 现为**单轨 egui**：winit 0.30 + softbuffer 软件光栅，无 GL/GPU 依赖。

### 新增画布层（9 个 FFI，承接老图元能力）
- `画布开始(id, 宽, 高)/画布结束()`：`ui.allocate_painter` 在当前 Ui 占一块定尺寸自绘区，
  painter/response 存入 FrameCtx 新容器变体 `Container::Canvas`，自动裁剪到画布矩形。
- `画布矩形/画布圆/画布线/画布文本`：egui `Painter` 直出，坐标为画布**局部坐标**（自动加 offset）。
- `画布点击()→1/0`、`画布鼠标X()/画布鼠标Y()→局部坐标`（无悬停返回 -1），查询 response。
- 帧循环天然驱动动画：逐帧重画即可，无需定时器/设置帧率。

### 示例迁移
- `动画演示.qi` 改用画布层做逐帧弹跳动画；`待办清单/待办客户端/自动刷新待办.qi`
  改用 egui 控件（复选框/滚动区），HTTP/异步落库逻辑保留，自动刷新改由帧循环轮询。

## 2026-07-18 · egui 第二批控件（16 个新 FFI，累计 38 个）

### 容器（begin/end 配对，FrameCtx 容器栈保证配对收尾）
- **滚动区** `滚动开始(id, 高度)/滚动结束`：固定高度垂直视口，滚轮滚动、
  比例滑块滚动条、偏移按 id 跨帧持久化（手写实现，不依赖 egui ScrollArea 闭包 API）
- **折叠区** `折叠开始(标题)→展开?/折叠结束`：▶/▼ 头部点击开合，状态持久化，
  收起时不压子 Ui（返回值供 Qi 侧跳过子控件）

### 控件
- `单选`（radio）/ `选择项`（整行高亮列表项）/ `数字输入`（DragValue）/
  `浮点滑条`（f64）/ `超链接`（系统浏览器打开）/ `悬浮标签`（hover 气泡）
- `表格`：表头 CSV + 行数据（换行分行/逗号分列），斑马纹
- `柱状图`（egui_plot BarChart，与既有 折线图 并列）
- `图片显示(路径,宽,高)`：png/jpg 解码→纹理缓存→彩色纹理采样（软件光栅直出）；
  0=原尺寸/单边按比例

### 外观与窗口
- `设置主题(深色)` 深/浅一键切换、`界面缩放(百分比)`（50–300）、
  `设置窗口标题(应用,标题)` 运行中改标题

配套：qi-runtime `gui_ffi.rs` 16 个 shim（cfg(has_gui) 门控，孪生同步）；
编译器 `module_registry.rs` 中文名注册；示例 `控件演示二.qi`（全家福）。

## [当前版本] - 2025-11-17

### 新增功能 ✨

#### 渲染器 FFI 接口
- 添加了完整的 2D 渲染器 FFI 接口
- 支持清除画面、绘制像素、矩形、圆形、直线
- 支持从文件加载并绘制图片
- 包含以下 FFI 函数：
  - `qi_gui_renderer_create_impl()` - 创建渲染器
  - `qi_gui_renderer_clear_impl()` - 清除画面
  - `qi_gui_renderer_draw_rect_impl()` - 绘制矩形
  - `qi_gui_renderer_draw_pixel_impl()` - 绘制像素
  - `qi_gui_renderer_draw_line_impl()` - 绘制直线
  - `qi_gui_renderer_draw_circle_impl()` - 绘制圆形
  - `qi_gui_renderer_draw_image_impl()` - 绘制图片
  - `qi_gui_renderer_resize_impl()` - 调整渲染器大小
  - `qi_gui_renderer_get_width_impl()` / `qi_gui_renderer_get_height_impl()` - 获取尺寸
  - `qi_gui_renderer_free_impl()` - 释放渲染器

#### Rust 示例程序
- `simple_window.rs` - 简单窗口示例，展示基本窗口创建和事件循环
- `keyboard_demo.rs` - 键盘事件演示，支持字符键、功能键、修饰键组合
- `window_control.rs` - 窗口控制演示，展示位置、大小、标题的动态控制
- `audio_player.rs` - 音频播放器示例，演示音频加载、播放控制、音量调节

### 改进 🔧

#### 代码质量
- 修复了测试代码中的函数命名错误（从 `qi_gui_*` 改为 `qi_gui_*_impl`）
- 清理了未使用的导入和变量
- 所有测试通过（7个单元测试）

#### 文档更新
- 完善了 README.md，添加了音频和渲染器 API 文档
- 更新了特性列表，标注了所有已完成的功能
- 重新组织了路线图，明确了各阶段的完成状态
- 添加了 Rust 示例运行说明

#### 构建系统
- 在 Cargo.toml 中正确配置了示例程序
- Release 版本构建成功，所有依赖正常

### 已知限制 ⚠️

#### 渲染器集成
- 渲染器的 FFI 创建由于架构限制暂不完全支持
- 原因：`Renderer::new()` 需要 `Rc<TaoWindow>`，但 `Window` 使用 `Arc<Mutex<TaoWindow>>`
- 解决方案：
  - 短期：推荐在 Rust 代码中直接使用渲染器
  - 长期：需要重构 Window 包装器以支持 FFI 渲染器创建

### 技术细节 🔬

#### 模块结构
```
qi-gui/
├── src/
│   ├── lib.rs          # 库入口
│   ├── window.rs       # 窗口管理
│   ├── event.rs        # 事件循环
│   ├── ffi.rs          # C FFI 接口（窗口、音频、渲染器）
│   ├── keycode.rs      # 键盘映射
│   ├── renderer.rs     # 2D 渲染器
│   └── audio.rs        # 音频播放
├── examples/           # Rust 示例程序（新增）
│   ├── simple_window.rs
│   ├── keyboard_demo.rs
│   ├── window_control.rs
│   └── audio_player.rs
└── target/
    └── release/
        └── libqi_gui.a # 静态库
```

#### FFI 接口统计
- 窗口管理：10 个函数
- 音频播放：7 个函数
- 渲染器：10 个函数
- 工具函数：2 个函数
- **总计：29 个 FFI 导出函数**

### 测试覆盖 ✅
- FFI 版本测试
- FFI 窗口创建测试
- FFI 窗口标题测试
- 键盘映射测试（字符键、功能键、特殊键）
- 音频播放器创建测试

### 下一步计划 📋

#### 高优先级
1. 重构 Window 以支持渲染器 FFI 创建
2. 实现文字渲染功能
3. 添加更多 Qi 语言示例

#### 中优先级
1. 硬件加速渲染支持
2. 原生菜单栏
3. 文件对话框

#### 低优先级
1. 组件系统
2. 布局管理器
3. 主题支持

---

## 版本历史

### v0.1.0（之前的版本）
- 基础窗口管理
- 事件处理（键盘、鼠标、窗口）
- 音频播放
- 软件渲染器
- C FFI 接口
