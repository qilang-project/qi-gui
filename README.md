# Qi GUI - 奇语言图形化界面库

基于 [Tao](https://github.com/tauri-apps/tao) 的跨平台 GUI 库，为奇语言提供原生窗口支持。

## ✨ 特性

- ✅ **跨平台支持** - 基于 Tao 库，支持 macOS（已完成）、Linux 和 Windows
- ✅ **原生性能** - 静态库编译，零运行时开销
- ✅ **中文 API** - 完全中文化的函数接口
- ✅ **事件驱动** - 基于事件循环的异步架构
- ✅ **丰富的事件支持** - 鼠标点击/移动/滚轮、完整键盘支持（字符、功能键、修饰键）、窗口事件监听
- ✅ **窗口控制** - 获取和设置窗口位置、大小
- ✅ **完整键盘支持** - A-Z/0-9字符键、F1-F35功能键、方向键、Shift/Ctrl/Alt/Cmd修饰键组合
- ✅ **音频播放** 🆕 - MP3、WAV、FLAC、Vorbis 格式支持，音量控制
- ✅ **2D 渲染** 🆕 - 软件渲染器，支持像素、矩形、圆形、直线、图片绘制
- ✅ **FFI 接口** - 完整的 C FFI，支持从 Qi 语言调用
- ✅ **易于集成** - 通过标准库模块系统无缝集成
- ✅ **懒加载架构** - 创新的 EventLoop 管理，解决所有权问题
- ✅ **调试友好** - 内置事件打印功能，方便开发调试
- ✅ **示例丰富** - Rust 和 Qi 语言示例程序

## 🚀 快速开始

### 安装

qi-gui 已内置于奇语言编译器中，无需额外安装。

### 最小示例

```qi
包 主程序；

导入 标准库.图形化；

函数 入口() {
    打印行("=== 最小 GUI 窗口示例 ===");

    // 创建窗口（加入队列）
    图形化.创建窗口("你好，世界！", 800, 600);

    // 启动事件循环（创建并显示所有窗口）
    图形化.运行();

    打印行("程序结束");
}
```

### 运行示例

#### Rust 示例 (新增!)

```bash
# 简单窗口示例
cargo run --example simple_window

# 键盘事件演示
cargo run --example keyboard_demo

# 窗口控制演示
cargo run --example window_control

# 音频播放示例 (需要提供音频文件)
cargo run --example audio_player path/to/audio.mp3
```

#### Qi 语言示例

```bash
# 基础窗口示例
qi run examples/minimal_window.qi

# 多窗口示例
qi run examples/hello_gui.qi

# 事件监听示例（鼠标、键盘、窗口大小）
qi run examples/event_test.qi

# 简单事件测试
qi run examples/event_test_simple.qi

# 鼠标追踪示例（鼠标移动和滚轮）
qi run examples/mouse_tracking.qi

# 窗口控制示例（位置和大小设置）
qi run examples/window_control.qi

# 键盘测试示例（字符、功能键、修饰键）
qi run examples/keyboard_test.qi
```

## 📖 API 文档

### 窗口管理

#### 创建窗口
```qi
变量 窗口ID: 整数 = 图形化.创建窗口(标题: 字符串, 宽度: 整数, 高度: 整数);
```
创建一个新窗口并加入显示队列。返回窗口ID（成功时 > 0，失败时 = 0）。

#### 运行事件循环
```qi
图形化.运行();
```
创建所有排队的窗口并启动事件循环。这是一个阻塞调用，直到所有窗口关闭才返回。

#### 销毁窗口
```qi
图形化.销毁窗口(窗口ID: 整数);
```
销毁指定的窗口并释放资源。

### 窗口属性

#### 设置标题
```qi
图形化.设置标题(窗口ID: 整数, 标题: 字符串);
```
修改窗口的标题文本。

#### 获取标题
```qi
变量 标题: 字符串 = 图形化.获取标题(窗口ID: 整数);
```
读取窗口当前的标题。

#### 显示/隐藏窗口
```qi
图形化.显示窗口(窗口ID: 整数);
图形化.隐藏窗口(窗口ID: 整数);
```
控制窗口的可见性。

#### 检查可见性
```qi
变量 可见: 整数 = 图形化.是否可见(窗口ID: 整数);
```
返回 1 表示窗口可见，0 表示不可见。

### 事件处理

#### 启用事件打印
```qi
图形化.启用事件打印(窗口ID: 整数);
```
为指定窗口启用事件打印功能。所有的窗口事件（鼠标点击、移动、滚轮、键盘按键、窗口大小改变等）都会自动打印到控制台。

**支持的事件类型：**
- **窗口关闭事件** - 用户点击关闭按钮时触发
- **窗口大小改变** - 用户拖动窗口边缘调整大小时触发，显示新的宽度和高度
- **鼠标点击事件** - 鼠标左键、右键、中键的按下和释放
- **鼠标移动事件** - 实时追踪鼠标在窗口内的位置 (x, y)
- **鼠标滚轮事件** - 检测鼠标滚轮滚动 (横向和纵向)
- **键盘事件** 🆕 v0.4.0 - 完整的键盘支持
  - **字符键（A-Z, 0-9, 符号）** - 显示实际字符
  - **功能键（F1-F35）** - 支持所有功能键
  - **特殊键** - Enter, Escape, Backspace, Tab, 方向键等
  - **修饰键组合** - 自动检测 Shift, Ctrl, Alt, Command 组合

**示例：**
```qi
变量 窗口: 整数 = 图形化.创建窗口("事件测试", 800, 600);
图形化.启用事件打印(窗口);  // 启用事件监听
图形化.运行();
```

**输出示例：**
```
[窗口 1] 鼠标移动: x=350, y=200
[窗口 1] 鼠标左键事件: 按下
[窗口 1] 鼠标左键事件: 释放
[窗口 1] 鼠标滚轮: dx=0, dy=1
[窗口 1] 键盘事件: 'a'
[窗口 1] 键盘事件: Shift+'A'
[窗口 1] 键盘事件: Ctrl+'c'
[窗口 1] 键盘事件: 0x70  // F1键
[窗口 1] 键盘事件: 0x26  // 上方向键
[窗口 1] 大小改变事件: 900x700
[窗口 1] 关闭事件
```

### 窗口位置和大小控制 🆕

#### 获取窗口位置
```qi
变量 X: 整数 = 图形化.获取位置X(窗口ID: 整数);
变量 Y: 整数 = 图形化.获取位置Y(窗口ID: 整数);
```
获取窗口当前在屏幕上的位置（屏幕坐标）。

#### 设置窗口位置
```qi
图形化.设置位置(窗口ID: 整数, X: 整数, Y: 整数);
```
将窗口移动到屏幕上的指定位置。

#### 获取窗口大小
```qi
变量 宽度: 整数 = 图形化.获取宽度(窗口ID: 整数);
变量 高度: 整数 = 图形化.获取高度(窗口ID: 整数);
```
获取窗口当前的内部大小（不包括标题栏和边框）。

#### 设置窗口大小
```qi
图形化.设置大小(窗口ID: 整数, 宽度: 整数, 高度: 整数);
```
调整窗口的大小。

**示例：**
```qi
变量 窗口: 整数 = 图形化.创建窗口("窗口控制", 800, 600);

// 设置窗口位置到屏幕左上角 (100, 100)
图形化.设置位置(窗口, 100, 100);

// 调整窗口大小
图形化.设置大小(窗口, 1024, 768);

// 读取当前位置和大小
变量 X: 整数 = 图形化.获取位置X(窗口);
变量 Y: 整数 = 图形化.获取位置Y(窗口);
变量 宽度: 整数 = 图形化.获取宽度(窗口);
变量 高度: 整数 = 图形化.获取高度(窗口);

打印行(X);
打印行(Y);
打印行(宽度);
打印行(高度);

图形化.运行();
```

### 版本信息
```qi
变量 版本: 字符串 = 图形化.版本();
```
获取 qi-gui 库的版本信息。

### 音频播放 🆕

qi-gui 提供简单的音频播放功能，支持 MP3、WAV、FLAC 和 Vorbis 格式。

#### 加载音频
```qi
变量 音频ID: 整数 = 图形化.音频_加载(文件路径: 字符串);
```
加载音频文件并返回音频播放器ID。成功返回 > 0，失败返回 0。

#### 播放控制
```qi
图形化.音频_播放(音频ID: 整数);     # 开始或继续播放
图形化.音频_暂停(音频ID: 整数);     # 暂停播放
图形化.音频_停止(音频ID: 整数);     # 停止播放
```

#### 音量控制
```qi
图形化.音频_设置音量(音频ID: 整数, 音量: 浮点数);  # 音量范围: 0.0 到 1.0
```

#### 状态查询
```qi
变量 正在播放: 整数 = 图形化.音频_正在播放(音频ID: 整数);  # 返回 1 表示正在播放
变量 已完成: 整数 = 图形化.音频_已完成(音频ID: 整数);      # 返回 1 表示播放完成
```

#### 释放资源
```qi
图形化.音频_释放(音频ID: 整数);
```

**示例：**
```qi
变量 音乐: 整数 = 图形化.音频_加载("music.mp3");
图形化.音频_设置音量(音乐, 0.7);  # 设置音量为70%
图形化.音频_播放(音乐);

# 使用完毕后释放
图形化.音频_释放(音乐);
```

### 2D 渲染 🆕 (实验性)

qi-gui 提供软件渲染器用于基本的 2D 图形绘制。

**注意：** 渲染器 FFI 接口已实现，但由于架构限制，从 FFI 创建渲染器暂不完全支持。推荐在 Rust 代码中使用渲染器 API。

#### 绘制功能

- **清除画面**: `renderer.clear(r, g, b)` - 用指定颜色填充整个画面
- **绘制像素**: `renderer.draw_pixel(x, y, r, g, b)` - 绘制单个像素点
- **绘制矩形**: `renderer.draw_rect(x, y, width, height, r, g, b)` - 绘制填充矩形
- **绘制直线**: `renderer.draw_line(x0, y0, x1, y1, r, g, b)` - 使用 Bresenham 算法绘制直线
- **绘制圆形**: `renderer.draw_circle(cx, cy, radius, r, g, b)` - 使用中点圆算法绘制圆形
- **绘制图片**: `renderer.draw_image(path, x, y)` - 从文件加载并绘制图片

**Rust 示例：**
```rust
use qi_gui::renderer::Renderer;
use std::rc::Rc;

// 创建渲染器 (需要 Rc<TaoWindow>)
let renderer = Renderer::new(window)?;

// 清除为黑色
renderer.clear(0, 0, 0);

// 绘制红色矩形
renderer.draw_rect(100, 100, 200, 150, 255, 0, 0);

// 绘制绿色圆形
renderer.draw_circle(400, 300, 50, 0, 255, 0);

// 绘制蓝色直线
renderer.draw_line(0, 0, 800, 600, 0, 0, 255);
```

## 🛠️ 开发

### 构建要求

- Rust 1.70+
- Cargo
- LLVM 工具链
- 平台特定依赖：
  - macOS: Xcode Command Line Tools
  - Linux: GTK3 开发库 (`sudo apt install libgtk-3-dev`)
  - Windows: Visual Studio Build Tools

### 编译库

```bash
# 编译 release 版本
cargo build --release

# 生成静态库
ls target/release/libqi_gui.a

# 复制到编译器运行时
cp target/release/libqi_gui.a ../qi/lib/

# 重新构建编译器
cd ../qi
cargo build --release
```

## C API

### 基础函数

```c
#include "qi_gui.h"

// 创建窗口
uint64_t qi_gui_create_window(const char* title, uint32_t width, uint32_t height);

// 销毁窗口
void qi_gui_destroy_window(uint64_t window_id);

// 设置标题
void qi_gui_set_title(uint64_t window_id, const char* title);

// 获取标题
char* qi_gui_get_title(uint64_t window_id);

// 显示/隐藏窗口
void qi_gui_show_window(uint64_t window_id);
void qi_gui_hide_window(uint64_t window_id);

// 检查可见性
int qi_gui_is_visible(uint64_t window_id);

// 运行事件循环 (阻塞)
void qi_gui_run(void);

// 释放字符串
void qi_gui_free_string(char* s);

// 获取版本
char* qi_gui_version(void);
```

### C 示例

```c
#include "qi_gui.h"
#include <stdio.h>

int main() {
    // 创建窗口
    uint64_t window = qi_gui_create_window("Hello GUI", 800, 600);

    // 显示窗口
    qi_gui_show_window(window);

    // 运行事件循环
    qi_gui_run();

    // 清理
    qi_gui_destroy_window(window);

    return 0;
}
```

编译:

```bash
# macOS
clang main.c -L./target/release -lqi_gui -framework Cocoa -framework QuartzCore -o main

# Linux
gcc main.c -L./target/release -lqi_gui -lgtk-3 -lgdk-3 -o main

# Windows
cl main.c /link qi_gui.lib
```

## 奇语言 API (计划中)

```qi
包 主程序;
导入 标准库.图形化;

函数 入口() {
    变量 窗口 = 图形化.创建窗口("你好，GUI！", 800, 600);
    图形化.显示(窗口);
    图形化.运行();
}
```

## 测试

### C 测试程序

```bash
cd examples
./test_window
```

这将创建一个窗口并运行事件循环。关闭窗口或按 Ctrl+C 退出。

## 架构

```
qi-gui/
├── src/
│   ├── lib.rs      # 库入口
│   ├── window.rs   # 窗口封装
│   ├── event.rs    # 事件循环封装
│   └── ffi.rs      # C FFI 接口
├── examples/
│   └── test_window.c  # C 测试程序
├── qi_gui.h        # 生成的 C 头文件
└── target/release/
    └── libqi_gui.a # 静态库
```

## 依赖

- [tao](https://github.com/tauri-apps/tao) v0.34 - 窗口管理
- [cbindgen](https://github.com/mozilla/cbindgen) - C 头文件生成

## 🗺️ Roadmap

### Phase 1: 基础窗口 ✅

- [x] 创建/销毁窗口
- [x] 显示/隐藏
- [x] 标题设置
- [x] 事件循环
- [x] C FFI 接口
- [x] 静态库编译

### Phase 2: 事件处理 ✅

- [x] 关闭事件回调
- [x] 键盘事件 (完整支持)
- [x] 鼠标事件 (点击、移动、滚轮)
- [x] 窗口调整大小事件
- [x] 窗口位置和大小控制

### Phase 3: 多媒体支持 ✅

- [x] 音频播放 (MP3, WAV, FLAC, Vorbis)
- [x] 音量控制
- [x] 播放状态查询

### Phase 4: 基础绘图 ✅ (部分完成)

- [x] 软件渲染器
- [x] 绘制像素
- [x] 绘制矩形
- [x] 绘制直线 (Bresenham)
- [x] 绘制圆形 (中点圆算法)
- [x] 绘制图像
- [ ] 文字渲染 (计划中)
- [ ] 反锯齿 (计划中)

### Phase 5: 高级功能 (计划中)

- [ ] 硬件加速渲染 (OpenGL/Metal/Vulkan)
- [ ] 原生菜单栏
- [ ] 对话框和文件选择器
- [ ] 系统托盘图标
- [ ] 拖放支持
- [ ] 剪贴板操作

### Phase 6: 组件库 (未来)

- [ ] 按钮、文本框等基础组件
- [ ] 布局管理器
- [ ] 主题系统
- [ ] 动画支持

## 许可证

MIT

## 贡献

欢迎贡献！请确保代码通过测试并遵循 Rust 代码规范。

```bash
# 运行测试
cargo test

# 格式化代码
cargo fmt

# 检查代码
cargo clippy
```
