# Qi GUI 开发指南

本文档面向希望为 qi-gui 贡献代码或了解其内部实现的开发者。

## 📁 项目结构

```
qi-gui/
├── src/
│   ├── lib.rs          # 库入口，re-export 主要类型
│   ├── window.rs       # Window 包装器，封装 Tao 窗口
│   ├── event.rs        # EventLoop 管理，懒加载架构
│   ├── ffi.rs          # C FFI 接口，提供给 Qi 编译器
│   ├── keycode.rs      # 键盘键码映射
│   ├── renderer.rs     # 软件渲染器 (softbuffer)
│   └── audio.rs        # 音频播放 (rodio)
├── examples/           # Rust 示例程序
├── build.rs            # 构建脚本 (cbindgen)
├── cbindgen.toml       # C 头文件生成配置
├── qi_gui.h            # 生成的 C 头文件
└── Cargo.toml          # 项目配置
```

## 🔧 核心架构

### 1. Window 模块 (`src/window.rs`)

**设计理念：**
- 使用 `Arc<Mutex<TaoWindow>>` 包装 Tao 窗口，支持多线程访问
- 提供简洁的 API 隐藏 Tao 的复杂性

**主要功能：**
- 窗口创建、显示/隐藏
- 标题设置/获取
- 位置和大小控制
- 窗口 ID 管理

**注意事项：**
- 所有方法都使用 `lock().unwrap()` 访问内部窗口
- 这种设计支持从多个线程控制窗口

### 2. EventLoop 模块 (`src/event.rs`)

**懒加载架构：**
```
创建窗口 → 加入队列
  ↓
调用 run() → 创建 EventLoop
  ↓
从队列创建实际窗口
  ↓
运行事件循环 (阻塞)
```

**为什么使用懒加载？**
- Tao 的 `EventLoop` 不能移动或共享
- 窗口必须在 `run()` 调用前创建
- 懒加载允许在调用 `run()` 前配置多个窗口

### 3. FFI 模块 (`src/ffi.rs`)

**全局状态管理：**
```rust
struct GuiState {
    next_window_id: u64,
    pending_windows: Vec<WindowRequest>,
    window_titles: HashMap<u64, String>,
    event_callbacks: HashMap<u64, EventCallback>,
    window_id_map: HashMap<WindowId, u64>,
    created_windows: HashMap<u64, Window>,
    current_modifiers: ModifiersState,
    next_audio_id: u64,
}

static GUI_STATE: Mutex<Option<GuiState>> = Mutex::new(None);
```

**线程本地存储：**
```rust
thread_local! {
    static AUDIO_PLAYERS: RefCell<HashMap<u64, AudioPlayer>> = ...;
    static RENDERERS: RefCell<HashMap<u64, Renderer>> = ...;
}
```

- 音频和渲染器不是 `Send/Sync`，使用 thread-local 存储
- GUI 状态是全局的，使用 `Mutex` 保护

### 4. Renderer 模块 (`src/renderer.rs`)

**软件渲染器：**
- 基于 `softbuffer` 库
- CPU 渲染，无需 GPU
- 支持基本图形绘制

**实现的绘图算法：**
- **Bresenham 直线算法** - `draw_line()`
- **中点圆算法** - `draw_circle()`
- **直接像素操作** - `draw_pixel()`, `draw_rect()`

**当前限制：**
- 需要 `Rc<TaoWindow>` 创建
- 与 Window 的 `Arc<Mutex<>>` 架构不兼容
- FFI 创建暂不完全支持

### 5. Audio 模块 (`src/audio.rs`)

**音频架构：**
```rust
pub struct AudioPlayer {
    _stream: OutputStream,  // 保持流活跃
    sink: Sink,             // 音频播放控制
}
```

**支持的格式：**
- MP3 (通过 rodio)
- WAV
- FLAC
- Vorbis

**播放控制：**
- play/pause/stop
- volume (0.0-1.0)
- 状态查询

## 🛠️ 开发工作流

### 设置开发环境

```bash
# 克隆仓库
git clone <repository>
cd qi-gui

# 安装依赖（macOS）
xcode-select --install

# 安装依赖（Linux）
sudo apt install libgtk-3-dev

# 构建
cargo build
```

### 运行测试

```bash
# 单元测试
cargo test --lib

# 运行示例
cargo run --example simple_window
cargo run --example keyboard_demo
cargo run --example window_control

# 音频示例（需要音频文件）
cargo run --example audio_player test.mp3
```

### 构建发布版本

```bash
# 构建 release
cargo build --release

# 静态库位于
ls -la target/release/libqi_gui.a

# 生成的 C 头文件
cat qi_gui.h
```

## 📝 添加新功能

### 1. 添加新的 Rust API

**在 `src/window.rs` 添加方法：**
```rust
impl Window {
    pub fn new_feature(&self) {
        // 实现
    }
}
```

### 2. 添加 FFI 函数

**在 `src/ffi.rs` 添加：**
```rust
/// 函数描述
#[no_mangle]
pub extern "C" fn qi_gui_new_feature_impl(window_id: u64) {
    // 实现
}
```

**更新头文件：**
```bash
cargo build  # 自动生成新的 qi_gui.h
```

### 3. 添加示例

**创建 `examples/new_example.rs`：**
```rust
use qi_gui::{Window, EventLoop};

fn main() {
    // 示例代码
}
```

**更新 `Cargo.toml`：**
```toml
[[example]]
name = "new_example"
path = "examples/new_example.rs"
```

### 4. 添加测试

**在相应模块添加：**
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_feature() {
        // 测试代码
    }
}
```

## 🐛 调试技巧

### 1. FFI 调试

```rust
// 在 FFI 函数中添加日志
eprintln!("Debug: window_id = {}", window_id);
```

### 2. 事件调试

使用 `qi_gui_enable_event_printing_impl()` 启用事件打印：
```c
qi_gui_enable_event_printing_impl(window_id);
```

### 3. Rust 日志

```bash
RUST_LOG=debug cargo run --example simple_window
```

## 📐 代码风格

### Rust 代码

- 使用 `cargo fmt` 格式化
- 遵循 Rust 命名约定
- 为公共 API 添加文档注释

```rust
/// 创建新窗口
///
/// # 参数
///
/// * `title` - 窗口标题
/// * `width` - 窗口宽度
/// * `height` - 窗口高度
///
/// # 返回值
///
/// 返回 `Result<Window, String>`
pub fn new(title: &str, width: u32, height: u32) -> Result<Window, String> {
    // ...
}
```

### FFI 代码

- 所有 FFI 函数以 `_impl` 结尾
- 使用 `#[no_mangle]` 和 `extern "C"`
- 添加文档注释（会生成到 C 头文件）

```rust
/// Create a new window
/// Returns window ID (> 0) on success, 0 on failure
#[no_mangle]
pub extern "C" fn qi_gui_create_window_impl(
    title: *const c_char,
    width: u32,
    height: u32,
) -> u64 {
    // ...
}
```

## 🔍 常见问题

### Q: 为什么渲染器不能从 FFI 创建？

**A:** 渲染器需要 `Rc<TaoWindow>`，但我们的 `Window` 使用 `Arc<Mutex<TaoWindow>>`。

**解决方案：**
- 短期：在 Rust 代码中使用渲染器
- 长期：重构 Window 包装器

### Q: 为什么音频播放器使用 thread-local？

**A:** `rodio::OutputStream` 不是 `Send/Sync`，不能在线程间传递。

### Q: 如何添加新的键盘按键支持？

**A:** 在 `src/keycode.rs` 的 `map_key_to_code()` 添加映射：
```rust
Key::NewKey => 0x??,  // 使用合适的键码
```

### Q: 事件回调如何工作？

**A:**
1. 用户注册回调：`qi_gui_set_event_callback_impl()`
2. 存储在 `GuiState.event_callbacks`
3. `qi_gui_run_impl()` 中的事件循环调用回调

## 📊 性能考虑

### 软件渲染器

- **优点**: 简单，无需 GPU
- **缺点**: CPU 密集，大窗口性能差
- **建议**: 用于原型和简单应用

### 音频播放

- **解码**: 在单独线程进行（rodio 自动处理）
- **缓冲**: 由 rodio 管理
- **延迟**: 通常很低（< 50ms）

### 事件处理

- **Wait 模式**: 默认，节能
- **Poll 模式**: 持续轮询，适合游戏/动画

## 🚀 未来改进

### 架构重构

1. **统一窗口包装器**
   - 支持 `Rc` 和 `Arc` 两种模式
   - 根据使用场景选择

2. **渲染器抽象**
   - 软件渲染器作为备选
   - 支持 OpenGL/Metal/Vulkan

3. **事件系统改进**
   - 更细粒度的事件过滤
   - 事件队列和异步处理

### 新功能

1. **文字渲染**
   - 集成字体库（rusttype/fontdue）
   - Unicode 支持
   - 字体缓存

2. **硬件加速**
   - wgpu 集成
   - 跨平台统一 API

3. **组件系统**
   - 按钮、文本框等基础组件
   - 事件分发
   - 布局管理

## 📚 参考资料

- [Tao 文档](https://docs.rs/tao/)
- [softbuffer 文档](https://docs.rs/softbuffer/)
- [rodio 文档](https://docs.rs/rodio/)
- [cbindgen 用户指南](https://github.com/mozilla/cbindgen/blob/master/docs.md)

## 🤝 贡献指南

1. Fork 项目
2. 创建特性分支 (`git checkout -b feature/amazing-feature`)
3. 提交更改 (`git commit -m 'Add amazing feature'`)
4. 推送到分支 (`git push origin feature/amazing-feature`)
5. 创建 Pull Request

**代码审查清单：**
- [ ] 代码通过 `cargo fmt` 和 `cargo clippy`
- [ ] 所有测试通过 (`cargo test`)
- [ ] 添加了必要的文档注释
- [ ] 更新了 README.md（如有需要）
- [ ] 添加了示例程序（如果是新功能）

---

**Happy Coding! 🎉**
