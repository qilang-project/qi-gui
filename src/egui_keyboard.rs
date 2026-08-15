//! egui 键盘层 —— 给 qilang 主循环提供逐帧键盘查询
//!
//! ## 为什么要单独一层
//! 画布层只有鼠标（点击 / 悬停坐标），做不了任何需要"按住方向键走路"的小游戏。
//! 这一层把 egui 每帧的键盘输入抓成一份**快照**，再由三个 FFI 查询函数读快照：
//!
//! - `按键按住(键名)`：这一帧该键处于按下状态（持续，适合移动）
//! - `按键刚按(键名)`：这一帧刚按下的边沿（适合跳跃 / 开火 / 切换）
//! - `任意键刚按()`：有任何键刚按下（"按任意键开始"）
//!
//! ## 为什么是快照而不是直接查 ctx
//! 查询 FFI 在帧内任意位置被调用（可能在画布容器里、也可能在控件之间），
//! 每次都去借 `FRAME` 再 `ctx.input(...)` 会反复加锁且依赖 FRAME 存活。
//! 在 `帧开始` 里一次性抓完存进 thread_local，查询就是纯读，零借用冲突。
//!
//! ## "刚按"的语义
//! 用 egui 自己的 `InputState::key_pressed`（本帧收到按下事件），**不是**自己拿
//! `keys_down` 前后帧差分。差分会漏掉"同一帧内按下又抬起"的快按，也会在丢帧时
//! 把边沿吃掉；egui 的事件队列是权威来源。
//!
//! ## Shift 是唯一的例外
//! egui 0.29 的 `Key` 枚举里**没有** Shift/Ctrl/Alt —— 修饰键单独放在
//! `i.modifiers` 里，且只有状态没有边沿事件。所以只有 Shift 的"刚按"只能靠
//! 前后帧差分（见 `capture` 里的 `shift_pressed`）。其余键一律走 `key_pressed`。

use std::cell::RefCell;
use std::collections::HashSet;
use std::os::raw::c_char;

use crate::egui_app::cstr;
use egui::Key;

/// 键名解析结果。Shift 单列，因为 egui 把修饰键放在 `modifiers` 而非 `Key` 里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyTarget {
    Key(Key),
    Shift,
}

/// 键名 → egui 键。大小写不敏感（ASCII），首尾空白忽略，中文名优先。
///
/// 这是纯函数，没有任何全局状态 —— 键盘正确性的单测就落在这里（见文件末 tests）。
pub fn key_from_name(name: &str) -> Option<KeyTarget> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 只把 ASCII 转大写：中文键名（"上"/"空格"）原样保留
    let upper: String = trimmed.to_uppercase();

    // 单个 ASCII 字母 / 数字先走快路（"a" / "W" / "3"）
    if upper.chars().count() == 1 {
        let c = upper.chars().next().unwrap();
        if let Some(k) = ascii_char_key(c) {
            return Some(KeyTarget::Key(k));
        }
    }

    let k = match upper.as_str() {
        // ── 方向键：中文名 + 英文别名 ──
        "上" | "UP" | "ARROWUP" => Key::ArrowUp,
        "下" | "DOWN" | "ARROWDOWN" => Key::ArrowDown,
        "左" | "LEFT" | "ARROWLEFT" => Key::ArrowLeft,
        "右" | "RIGHT" | "ARROWRIGHT" => Key::ArrowRight,
        // ── 常用功能键 ──
        "空格" | "SPACE" => Key::Space,
        "回车" | "ENTER" | "RETURN" => Key::Enter,
        "ESC" | "ESCAPE" | "退出键" => Key::Escape,
        "TAB" | "制表" => Key::Tab,
        // Shift 不在 Key 枚举里，单独返回
        "SHIFT" | "上档" => return Some(KeyTarget::Shift),
        _ => return None,
    };
    Some(KeyTarget::Key(k))
}

/// 单个 ASCII 字符（已大写）→ 键
fn ascii_char_key(c: char) -> Option<Key> {
    let k = match c {
        'A' => Key::A,
        'B' => Key::B,
        'C' => Key::C,
        'D' => Key::D,
        'E' => Key::E,
        'F' => Key::F,
        'G' => Key::G,
        'H' => Key::H,
        'I' => Key::I,
        'J' => Key::J,
        'K' => Key::K,
        'L' => Key::L,
        'M' => Key::M,
        'N' => Key::N,
        'O' => Key::O,
        'P' => Key::P,
        'Q' => Key::Q,
        'R' => Key::R,
        'S' => Key::S,
        'T' => Key::T,
        'U' => Key::U,
        'V' => Key::V,
        'W' => Key::W,
        'X' => Key::X,
        'Y' => Key::Y,
        'Z' => Key::Z,
        '0' => Key::Num0,
        '1' => Key::Num1,
        '2' => Key::Num2,
        '3' => Key::Num3,
        '4' => Key::Num4,
        '5' => Key::Num5,
        '6' => Key::Num6,
        '7' => Key::Num7,
        '8' => Key::Num8,
        '9' => Key::Num9,
        _ => return None,
    };
    Some(k)
}

/// 一帧的键盘快照
#[derive(Default)]
struct KeySnapshot {
    /// 本帧处于按下状态的键
    down: HashSet<Key>,
    /// 本帧刚按下（边沿）的键
    pressed: HashSet<Key>,
    shift_down: bool,
    shift_pressed: bool,
    /// 本帧有任何键刚按下
    any_pressed: bool,
}

thread_local! {
    static SNAPSHOT: RefCell<KeySnapshot> = RefCell::new(KeySnapshot::default());
    /// 已经警告过的未知键名 —— 每个名字只吼一次，否则 60fps 会把终端刷爆
    static WARNED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// 抓一帧键盘快照。必须在 `ctx.begin_pass` **之后**调用（那时 InputState 才是本帧的）。
pub(crate) fn capture(ctx: &egui::Context) {
    // 上一帧的 shift 状态：Shift 没有边沿事件，只能差分（见文件头说明）
    let prev_shift = SNAPSHOT.with(|s| s.borrow().shift_down);

    let snap = ctx.input(|i| {
        // 按住：直接用 egui 维护的 keys_down 集合
        let down: HashSet<Key> = i.keys_down.iter().copied().collect();
        // 刚按：走 egui 的事件队列（`key_pressed` 内部也是扫这个队列）。
        // 额外滤掉 `repeat: true` —— 那是操作系统的**按住自动重复**，
        // 按住空格半秒后会连发。小游戏里"跳一下"必须是一次，不能变成连跳。
        let mut pressed = HashSet::new();
        for ev in &i.events {
            if let egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                ..
            } = ev
            {
                pressed.insert(*key);
            }
        }
        let shift_down = i.modifiers.shift;
        KeySnapshot {
            any_pressed: !pressed.is_empty() || (shift_down && !prev_shift),
            shift_pressed: shift_down && !prev_shift,
            shift_down,
            down,
            pressed,
        }
    });
    SNAPSHOT.with(|s| *s.borrow_mut() = snap);
}

/// 认不出的键名：返回 None，并且每个名字只警告一次
fn resolve_or_warn(name: &str) -> Option<KeyTarget> {
    if let Some(t) = key_from_name(name) {
        return Some(t);
    }
    let key = name.trim().to_string();
    WARNED.with(|w| {
        if w.borrow_mut().insert(key) {
            eprintln!(
                "图形化: 认不出的键名「{}」—— 可用：上/下/左/右、A~Z、0~9、空格/回车/ESC/SHIFT/TAB",
                name.trim()
            );
        }
    });
    None
}

// ============================================================================
// FFI —— 键盘查询（帧内调用；帧外读到的是上一帧的快照）
// ============================================================================

/// 按键按住(键名) → 1/0：这一帧该键是否处于按下状态（持续触发，适合移动）
#[no_mangle]
pub extern "C" fn qi_gui_egui_key_down_impl(name: *const c_char) -> i64 {
    let n = cstr(name);
    match resolve_or_warn(&n) {
        Some(KeyTarget::Key(k)) => SNAPSHOT.with(|s| i64::from(s.borrow().down.contains(&k))),
        Some(KeyTarget::Shift) => SNAPSHOT.with(|s| i64::from(s.borrow().shift_down)),
        None => 0,
    }
}

/// 按键刚按(键名) → 1/0：这一帧刚按下的边沿（适合跳跃 / 开火 / 切换）
#[no_mangle]
pub extern "C" fn qi_gui_egui_key_pressed_impl(name: *const c_char) -> i64 {
    let n = cstr(name);
    match resolve_or_warn(&n) {
        Some(KeyTarget::Key(k)) => SNAPSHOT.with(|s| i64::from(s.borrow().pressed.contains(&k))),
        Some(KeyTarget::Shift) => SNAPSHOT.with(|s| i64::from(s.borrow().shift_pressed)),
        None => 0,
    }
}

/// 任意键刚按() → 1/0：本帧有任何键刚按下（"按任意键开始"）
#[no_mangle]
pub extern "C" fn qi_gui_egui_any_key_pressed_impl() -> i64 {
    SNAPSHOT.with(|s| i64::from(s.borrow().any_pressed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn k(name: &str) -> Option<Key> {
        match key_from_name(name) {
            Some(KeyTarget::Key(key)) => Some(key),
            _ => None,
        }
    }

    #[test]
    fn 方向键中英文都认() {
        assert_eq!(k("上"), Some(Key::ArrowUp));
        assert_eq!(k("下"), Some(Key::ArrowDown));
        assert_eq!(k("左"), Some(Key::ArrowLeft));
        assert_eq!(k("右"), Some(Key::ArrowRight));
        assert_eq!(k("UP"), Some(Key::ArrowUp));
        assert_eq!(k("DOWN"), Some(Key::ArrowDown));
        assert_eq!(k("LEFT"), Some(Key::ArrowLeft));
        assert_eq!(k("RIGHT"), Some(Key::ArrowRight));
        assert_eq!(k("ArrowUp"), Some(Key::ArrowUp));
        assert_eq!(k("arrowright"), Some(Key::ArrowRight));
    }

    #[test]
    fn 字母全表大小写不敏感() {
        let upper = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
        let expect = [
            Key::A,
            Key::B,
            Key::C,
            Key::D,
            Key::E,
            Key::F,
            Key::G,
            Key::H,
            Key::I,
            Key::J,
            Key::K,
            Key::L,
            Key::M,
            Key::N,
            Key::O,
            Key::P,
            Key::Q,
            Key::R,
            Key::S,
            Key::T,
            Key::U,
            Key::V,
            Key::W,
            Key::X,
            Key::Y,
            Key::Z,
        ];
        for (i, c) in upper.chars().enumerate() {
            assert_eq!(k(&c.to_string()), Some(expect[i]), "大写 {c}");
            assert_eq!(
                k(&c.to_lowercase().to_string()),
                Some(expect[i]),
                "小写 {c}"
            );
        }
    }

    #[test]
    fn 数字全表() {
        let expect = [
            Key::Num0,
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ];
        for (i, e) in expect.iter().enumerate() {
            assert_eq!(k(&i.to_string()), Some(*e), "数字 {i}");
        }
    }

    #[test]
    fn 功能键全表() {
        assert_eq!(k("空格"), Some(Key::Space));
        assert_eq!(k("SPACE"), Some(Key::Space));
        assert_eq!(k("space"), Some(Key::Space));
        assert_eq!(k("回车"), Some(Key::Enter));
        assert_eq!(k("ENTER"), Some(Key::Enter));
        assert_eq!(k("return"), Some(Key::Enter));
        assert_eq!(k("ESC"), Some(Key::Escape));
        assert_eq!(k("esc"), Some(Key::Escape));
        assert_eq!(k("Escape"), Some(Key::Escape));
        assert_eq!(k("退出键"), Some(Key::Escape));
        assert_eq!(k("TAB"), Some(Key::Tab));
        assert_eq!(k("tab"), Some(Key::Tab));
        assert_eq!(k("制表"), Some(Key::Tab));
    }

    #[test]
    fn shift走修饰键分支() {
        assert_eq!(key_from_name("SHIFT"), Some(KeyTarget::Shift));
        assert_eq!(key_from_name("shift"), Some(KeyTarget::Shift));
        assert_eq!(key_from_name("Shift"), Some(KeyTarget::Shift));
        assert_eq!(key_from_name("上档"), Some(KeyTarget::Shift));
        // Shift 不该落进 Key 分支
        assert_eq!(k("SHIFT"), None);
    }

    #[test]
    fn 首尾空白忽略() {
        assert_eq!(k("  空格 "), Some(Key::Space));
        assert_eq!(k("\tW\n"), Some(Key::W));
        assert_eq!(k(" 左 "), Some(Key::ArrowLeft));
    }

    // ── 快照语义测试：不用开窗 ────────────────────────────────────
    // egui::Context 本身是无头的，喂它一份合成 RawInput 走 begin_pass/end_pass，
    // 就能在 cargo test 里验「按住 vs 刚按」的真实行为 —— 这比只测键名表更值钱，
    // 因为最容易写错的正是"按住不松第二帧还算不算刚按"。

    fn key_event(key: Key, pressed: bool, repeat: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat,
            modifiers: egui::Modifiers::default(),
        }
    }

    /// 喂一帧输入并抓快照
    fn feed(ctx: &egui::Context, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        ctx.begin_pass(input);
        capture(ctx);
        let _ = ctx.end_pass();
    }

    fn down(name: &str) -> i64 {
        let c = CString::new(name).unwrap();
        qi_gui_egui_key_down_impl(c.as_ptr())
    }

    fn just(name: &str) -> i64 {
        let c = CString::new(name).unwrap();
        qi_gui_egui_key_pressed_impl(c.as_ptr())
    }

    #[test]
    fn 按住持续而刚按只有一帧() {
        let ctx = egui::Context::default();

        // 第 1 帧：按下 W
        feed(&ctx, vec![key_event(Key::W, true, false)]);
        assert_eq!(down("W"), 1, "第 1 帧应当按住");
        assert_eq!(just("W"), 1, "第 1 帧应当刚按");
        assert_eq!(qi_gui_egui_any_key_pressed_impl(), 1, "任意键刚按");

        // 第 2 帧：不松手，也没有新事件
        feed(&ctx, vec![]);
        assert_eq!(down("W"), 1, "还按着，按住仍为 1（不然方块走一帧就停）");
        assert_eq!(just("W"), 0, "刚按只在边沿那一帧为 1（不然空格会连发）");
        assert_eq!(qi_gui_egui_any_key_pressed_impl(), 0);

        // 第 3 帧：系统按键自动重复 —— 不该算成新的一次"刚按"
        feed(&ctx, vec![key_event(Key::W, true, true)]);
        assert_eq!(down("W"), 1);
        assert_eq!(just("W"), 0, "自动重复不是刚按");

        // 第 4 帧：松手
        feed(&ctx, vec![key_event(Key::W, false, false)]);
        assert_eq!(down("W"), 0, "松手后按住归 0");
        assert_eq!(just("W"), 0);
    }

    #[test]
    fn 别的键不受影响且键名照常解析() {
        let ctx = egui::Context::default();
        feed(
            &ctx,
            vec![
                key_event(Key::ArrowLeft, true, false),
                key_event(Key::Space, true, false),
            ],
        );
        // 中文名 / 英文名 / 大小写 走的是同一张表，查出来必须一致
        assert_eq!(down("左"), 1);
        assert_eq!(down("LEFT"), 1);
        assert_eq!(down("left"), 1);
        assert_eq!(just("空格"), 1);
        assert_eq!(just("space"), 1);
        // 没按的键必须是 0
        assert_eq!(down("右"), 0);
        assert_eq!(down("W"), 0);
        assert_eq!(just("ESC"), 0);
        // 认不出的键名一律 0，不该 panic
        assert_eq!(down("F13"), 0);
        assert_eq!(just("鼠标左键"), 0);
    }

    #[test]
    fn 认不出的键名返回空() {
        for bad in [
            "",
            "   ",
            "F1",
            "CTRL",
            "控制",
            "鼠标左键",
            "上上",
            "AB",
            "10",
            "空",
            "!",
            "方向上",
        ] {
            assert_eq!(key_from_name(bad), None, "「{bad}」不该被认出来");
        }
    }
}
