# Qi GUI - 奇语言图形化界面库

**单轨 egui 架构**的跨平台 GUI 库，为奇语言提供原生窗口 + 控件 + 自绘画布 + 音频。

- **窗口后端**：[winit](https://github.com/rust-windowing/winit) 0.30
- **呈现**：[softbuffer](https://github.com/rust-windowing/softbuffer) 软件帧缓冲 +
  自绘 [egui](https://github.com/emilk/egui)/epaint 网格光栅化（**不依赖 GL/GPU/Metal**，跨平台稳定）
- **控件**：egui immediate mode，由 qilang 主循环逐帧驱动
- **音频**：[rodio](https://github.com/RustAudio/rodio)（MP3/WAV/FLAC/Vorbis）

> **架构变更（2026-07-18）**：老的 **tao 自绘轨**（`创建窗口`/渲染器图元/事件回调/
> 定时器/keycode）已**彻底移除**，`tao`/`tiny-skia`/`cosmic-text` 依赖一并删除。
> 图元自绘能力由新的**画布层**（`画布开始/结束` + `画布矩形/圆/线/文本`）在 egui
> 帧循环内承接。早期基于老轨的 `qi-ui` retained 控件包同步弃用（见 `qi-ui/README.md`）。

## ✨ 特性

- ✅ **单轨 egui** —— winit + softbuffer 软件光栅，无 GPU 依赖
- ✅ **immediate mode 控件** —— 40+ 个中文 FFI：按钮/标签/输入框/滑条/复选框/
  下拉/单选/选择项/进度条/分组/水平布局/滚动区/折叠区/表格/柱状图/折线图/
  图片显示/超链接/悬浮标签/消息弹窗/设置主题/界面缩放…
- ✅ **画布层** —— `画布开始/结束` + `画布矩形/圆/线/文本` + `画布点击/鼠标X/鼠标Y`，
  帧循环天然驱动逐帧动画（无需定时器）
- ✅ **CJK 字体** —— 运行时探测系统中文字体注入 egui（macOS PingFang/STHeiti、
  Linux Noto、Windows 雅黑）
- ✅ **音频播放** —— rodio，音量/暂停/停止/状态查询
- ✅ **中文 API** —— 完全中文化，经 `标准库.图形化` 模块无缝集成
- ✅ **静态库** —— `libqi_gui.a`，零运行时开销

## 🚀 快速开始（qilang）

主循环模型：qilang 持有主循环，egui 每帧驱动。

```qi
包 主程序;
导入 标准库.图形化 作为 图形;

变量 计数: 整数 = 0;

函数 入口() {
    变量 应用: 整数 = 图形.应用创建("你好 egui", 640, 480);
    当 (图形.帧开始(应用) == 1) {       // 帧开始 抽事件；窗口关闭时返回 0
        图形.标题文本("计数器");
        如果 (图形.按钮("点我 +1") == 1) { 计数 = 计数 + 1; }
        图形.标签("当前：" + 整数转字符串(计数));
        图形.帧结束(应用);              // 一次性上屏 + 60fps 限帧
    }
    图形.关闭应用(应用);
}
```

画布逐帧动画：

```qi
图形.画布开始("场景", 640, 360);
图形.画布矩形(0, 0, 640, 360, 18, 20, 28);
图形.画布圆(球心X, 180, 28, 255, 184, 76);
图形.画布线(0, 300, 640, 300, 2, 120, 120, 120);
图形.画布文本(20, 16, "标题", 20, 235, 238, 245);
如果 (图形.画布点击() == 1) { /* 用 画布鼠标X()/画布鼠标Y() 取局部坐标 */ }
图形.画布结束();
```

Qi 示例（`qi/示例/图形界面/`）：`控件演示.qi`、`控件演示二.qi`、`动画演示.qi`、
`待办清单.qi`、`待办客户端.qi`、`自动刷新待办.qi`。

## 📖 中文 API 速览（`标准库.图形化`）

| 类别 | 函数 |
| --- | --- |
| 应用/主循环 | `应用创建(标题,宽,高)`、`帧开始(应用)`、`帧结束(应用)`、`关闭应用(应用)`、`设置窗口标题(应用,标题)` |
| 文本/标签 | `标题文本`、`标签`、`彩色标签`、`悬浮标签`、`超链接`、`消息弹窗` |
| 交互控件 | `按钮`、`输入框`、`多行输入`、`滑条`、`浮点滑条`、`数字输入`、`复选框`、`下拉选择`、`单选`、`选择项`、`进度条` |
| 布局/容器 | `水平开始/结束`、`分组开始/结束`、`滚动开始/结束`、`折叠开始/结束`、`分隔线`、`空行` |
| 数据展示 | `表格`、`柱状图`、`折线图`、`图片显示` |
| 外观 | `设置主题(深色)`、`界面缩放(百分比)` |
| 画布（自绘图元） | `画布开始(id,宽,高)`、`画布结束`、`画布矩形`、`画布圆`、`画布线`、`画布文本`、`画布点击`、`画布鼠标X`、`画布鼠标Y` |
| 音频 | `加载音频`、`播放音频`、`暂停音频`、`停止音频`、`设置音量`、`音频是否播放`、`音频是否完成`、`释放音频` |
| 杂项 | `版本()`、`释放字符串` |

## 🛠️ 开发

```bash
# 编译静态库（工作区共享 target）
cargo build            # 或 --release

# 运行 Rust 音频示例
cargo run --example audio_player path/to/audio.mp3
```

### 构建要求

- Rust 1.75+
- 平台特定：macOS（Xcode CLT）、Linux（`libxkbcommon`/`wayland` 或 X11 开发库）、Windows（MSVC）

## 架构

```
qi-gui/
├── src/
│   ├── lib.rs           # 库入口（模块声明）
│   ├── egui_app.rs      # winit pump 主循环 + softbuffer 呈现 + FrameCtx/容器栈 + 第一批控件 + 画布容器
│   ├── egui_canvas.rs   # 画布层 FFI（矩形/圆/线/文本/点击/鼠标）
│   ├── egui_widgets2.rs # 第二批控件 FFI（单选/表格/图片/主题…）
│   ├── egui_raster.rs   # epaint 网格 → softbuffer 软件光栅
│   ├── audio.rs         # rodio 播放器
│   └── audio_ffi.rs     # 音频 + 版本/释放字符串 FFI
├── examples/
│   └── audio_player.rs
└── qi_gui.h             # cbindgen 生成的 C 头文件
```

FFI 经 `qi-runtime/src/stdlib/gui_ffi.rs`（`#[cfg(has_gui)]` shim；`qi/src/runtime/
stdlib/gui_ffi.rs` 为孪生副本）转发到本库的 `*_impl`，并在
`qi/src/codegen/module_registry.rs` 注册中文名（模块 `标准库.图形化`）。

## 依赖

- [egui / egui-winit / egui_plot](https://github.com/emilk/egui) 0.29
- [winit](https://github.com/rust-windowing/winit) 0.30、[softbuffer](https://github.com/rust-windowing/softbuffer) 0.4
- [rodio](https://github.com/RustAudio/rodio) 0.19、[image](https://github.com/image-rs/image) 0.25
- [cbindgen](https://github.com/mozilla/cbindgen) —— C 头文件生成

## 许可证

MIT
