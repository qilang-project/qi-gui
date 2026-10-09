//! GUI 脚本回放：按帧注入鼠标 / 键盘事件，并把渲染结果直接存成 PNG
//!
//! 设 `QI_GUI_SCRIPT=脚本文件` 时生效，不设零影响。为的是不靠辅助功能权限、
//! 不靠屏幕录制权限，也能端到端地测 GUI 程序（本机和 CI 都一样）：
//! 事件直接塞进 egui 的 RawInput，截图取的是软光栅自己的帧缓冲。
//!
//! 脚本每行 `帧号 动作 参数`，帧号从第一次 `帧开始` 起算，# 开头是注释：
//!
//! ```text
//! 10 click 400 300        # 在逻辑坐标 (400, 300) 按下，下一帧松开
//! 12 text Hello **world** # 输入文字（不含换行）
//! 13 key Enter            # 单键：键名同 按键按住()，如 Enter / Tab / A / 上
//! 20 cmd S                # Cmd（macOS）/ Ctrl（其它）+ 键
//! 21 cmd+shift S
//! 30 shot /tmp/a.png      # 这一帧画完后把帧缓冲存成 PNG
//! 40 drop /tmp/b.png      # 模拟把文件拖进窗口
//! 50 scroll 900 400 -300  # 鼠标移到 (900, 400) 滚轮滚 -300 点（负数往下翻）
//! ```

use crate::egui_keyboard::{key_from_name, KeyTarget};
use std::cell::RefCell;

use egui::{Event, Key, Modifiers, PointerButton, Pos2};

#[derive(Debug, Clone, PartialEq)]
enum Action {
    Move(f32, f32),
    Press(f32, f32),
    Release(f32, f32),
    Text(String),
    Key(Key, Modifiers),
    Shot(String),
    Drop(String),
    Scroll(f32, f32, f32),
}

#[derive(Default)]
struct Script {
    loaded: bool,
    steps: Vec<(u64, Action)>,
    frame: u64,
    shot_now: Option<String>,
}

thread_local! {
    static SCRIPT: RefCell<Script> = RefCell::new(Script::default());
}

fn command_mods(shift: bool) -> Modifiers {
    let mut m = Modifiers::COMMAND;
    if cfg!(target_os = "macos") {
        m.mac_cmd = true;
    } else {
        m.ctrl = true;
    }
    m.shift = shift;
    m
}

/// 解析脚本文本。坏行跳过并告警，不让一行写错毁掉整个回放。
fn parse(text: &str) -> Vec<(u64, Action)> {
    let mut steps = Vec::new();
    for (no, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, ' ');
        let frame = parts.next().and_then(|f| f.parse::<u64>().ok());
        let verb = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("").trim();
        let Some(frame) = frame else {
            eprintln!("qi-gui 脚本第 {} 行：帧号不是整数，跳过", no + 1);
            continue;
        };
        let xy = || -> Option<(f32, f32)> {
            let mut it = rest.split_whitespace();
            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
        };
        let key = |name: &str| match key_from_name(name) {
            Some(KeyTarget::Key(k)) => Some(k),
            _ => None,
        };
        let action = match verb {
            "move" => xy().map(|(x, y)| vec![(frame, Action::Move(x, y))]),
            "click" => xy().map(|(x, y)| {
                vec![
                    (frame, Action::Press(x, y)),
                    (frame + 1, Action::Release(x, y)),
                ]
            }),
            "text" => {
                // text 后面整行原样输入；行内 # 不当注释
                let t = raw.trim_start().splitn(3, ' ').nth(2).unwrap_or("");
                Some(vec![(frame, Action::Text(t.to_string()))])
            }
            "key" => key(rest).map(|k| vec![(frame, Action::Key(k, Modifiers::NONE))]),
            "cmd" => key(rest).map(|k| vec![(frame, Action::Key(k, command_mods(false)))]),
            "cmd+shift" => key(rest).map(|k| vec![(frame, Action::Key(k, command_mods(true)))]),
            "shot" if !rest.is_empty() => Some(vec![(frame, Action::Shot(rest.to_string()))]),
            "scroll" => {
                let mut it = rest
                    .split_whitespace()
                    .filter_map(|v| v.parse::<f32>().ok());
                match (it.next(), it.next(), it.next()) {
                    (Some(x), Some(y), Some(dy)) => Some(vec![(frame, Action::Scroll(x, y, dy))]),
                    _ => None,
                }
            }
            "drop" if !rest.is_empty() => Some(vec![(frame, Action::Drop(rest.to_string()))]),
            _ => None,
        };
        match action {
            Some(a) => steps.extend(a),
            None => eprintln!("qi-gui 脚本第 {} 行看不懂：{}", no + 1, line),
        }
    }
    steps.sort_by_key(|(f, _)| *f);
    steps
}

/// 帧开始时调用：把本帧该发生的事件塞进 RawInput。返回本帧是否注入了事件
/// （调用方据此把这一帧当成"有输入"，不让静止跳帧吞掉）。
pub(crate) fn inject(raw: &mut egui::RawInput) -> bool {
    SCRIPT.with(|s| {
        let mut s = s.borrow_mut();
        if !s.loaded {
            s.loaded = true;
            if let Ok(path) = std::env::var("QI_GUI_SCRIPT") {
                match std::fs::read_to_string(&path) {
                    Ok(text) => s.steps = parse(&text),
                    Err(e) => eprintln!("qi-gui：读不了脚本 {path}：{e}"),
                }
            }
        }
        let frame = s.frame;
        s.frame += 1;
        let mut injected = false;
        while s.steps.first().is_some_and(|(f, _)| *f <= frame) {
            let (_, action) = s.steps.remove(0);
            injected = true;
            match action {
                Action::Move(x, y) => raw.events.push(Event::PointerMoved(Pos2::new(x, y))),
                Action::Press(x, y) | Action::Release(x, y) => {
                    let pressed = matches!(action, Action::Press(..));
                    let pos = Pos2::new(x, y);
                    raw.events.push(Event::PointerMoved(pos));
                    raw.events.push(Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed,
                        modifiers: Modifiers::NONE,
                    });
                }
                Action::Text(t) => raw.events.push(Event::Text(t)),
                Action::Key(key, modifiers) => {
                    raw.modifiers = modifiers;
                    for pressed in [true, false] {
                        raw.events.push(Event::Key {
                            key,
                            physical_key: None,
                            pressed,
                            repeat: false,
                            modifiers,
                        });
                    }
                }
                Action::Shot(path) => s.shot_now = Some(path),
                Action::Scroll(x, y, dy) => {
                    raw.events.push(Event::PointerMoved(Pos2::new(x, y)));
                    raw.events.push(Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, dy),
                        modifiers: Modifiers::NONE,
                    });
                }
                Action::Drop(path) => raw.dropped_files.push(egui::DroppedFile {
                    path: Some(path.into()),
                    ..Default::default()
                }),
            }
        }
        injected
    })
}

/// 帧结束时调用：这一帧要不要截图（要的话必须真画，不能跳帧）
pub(crate) fn take_shot() -> Option<String> {
    SCRIPT.with(|s| s.borrow_mut().shot_now.take())
}

/// 把 0x00RRGGBB 帧缓冲存成 PNG
pub(crate) fn save_png(path: &str, buf: &[u32], w: u32, h: u32) {
    let mut img = image::RgbImage::new(w, h);
    for (i, px) in img.pixels_mut().enumerate() {
        let c = buf.get(i).copied().unwrap_or(0);
        *px = image::Rgb([(c >> 16) as u8, (c >> 8) as u8, c as u8]);
    }
    match img.save(path) {
        Ok(()) => eprintln!("qi-gui：截图已存 {path}"),
        Err(e) => eprintln!("qi-gui：截图存不了 {path}：{e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_action_and_sorts_by_frame() {
        let steps = parse(
            "# 注释\n\
             20 shot /tmp/x.png\n\
             10 click 400 300\n\
             12 text Hello **world** # 不是注释\n\
             13 key Enter\n\
             14 cmd S\n\
             15 cmd+shift S\n\
             16 bogus\n",
        );
        assert_eq!(steps[0], (10, Action::Press(400.0, 300.0)));
        assert_eq!(steps[1], (11, Action::Release(400.0, 300.0)));
        assert_eq!(
            steps[2],
            (12, Action::Text("Hello **world** # 不是注释".into()))
        );
        assert_eq!(steps[3], (13, Action::Key(Key::Enter, Modifiers::NONE)));
        assert!(matches!(steps[4], (14, Action::Key(Key::S, m)) if m.command && !m.shift));
        assert!(matches!(steps[5], (15, Action::Key(Key::S, m)) if m.command && m.shift));
        assert_eq!(steps[6], (20, Action::Shot("/tmp/x.png".into())));
        assert_eq!(steps.len(), 7, "看不懂的行跳过");
    }
}
