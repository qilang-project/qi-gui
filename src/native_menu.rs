//! 原生菜单栏 FFI（muda）
//!
//! macOS 是屏幕顶部的系统菜单栏，Windows 是窗口标题下的菜单栏。Linux 上 muda 要
//! GTK，不开 —— 那里所有 FFI 返回 0，qi 侧照常靠工具栏和组合键。
//!
//! - `菜单添加(菜单名, 项名, 快捷键)` → 编号：菜单不存在就新建，按调用顺序排。
//!   快捷键写 `Cmd+S` / `Cmd+Shift+S`，Cmd 在 macOS 是 ⌘、其它系统是 Ctrl；空串不设。
//! - `菜单被点()` → 编号或 0：逐帧轮询，一次取一个。
//! - `菜单添加编辑项(菜单名)`：撤销/重做/剪切/拷贝/粘贴/全选。点了就往下一帧的
//!   egui 输入里塞对应事件，输入框照常处理 —— 不用 macOS 的 `copy:` 响应链
//!   （winit 的 NSView 不实现它们，那样的菜单项是灰的）。
//!
//! **快捷键不会触发两次**：macOS 上菜单项的快捷键由 NSMenu 的 key equivalent 截走，
//! 按键事件根本不进窗口，egui 和 `组合键()` 都看不到；Windows 上我们不装
//! `TranslateAccelerator` 钩子，菜单里的快捷键只是显示用，按键照常进 egui，
//! 由 `组合键()` 处理。两边各只有一条路。

use crate::egui_app::cstr;
use std::os::raw::c_char;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod imp {
    use egui::{Event, Key, Modifiers};
    use muda::accelerator::Accelerator;
    use muda::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};
    use std::str::FromStr;

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub(super) enum Builtin {
        Undo,
        Redo,
        Cut,
        Copy,
        Paste,
        SelectAll,
        Quit,
    }

    struct State {
        menu: Menu,
        submenus: Vec<(String, Submenu)>,
        user: HashMap<MenuId, i64>,
        builtin: HashMap<MenuId, Builtin>,
        clicks: VecDeque<i64>,
        next_no: i64,
        installed: bool,
        quit: bool,
    }

    thread_local! {
        static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    }

    /// `Cmd+Shift+S` → muda 的写法（Cmd 换成 CmdOrCtrl）；空串或认不出来返回 None
    pub(super) fn accelerator(spec: &str) -> Option<Accelerator> {
        let spec = spec.trim();
        if spec.is_empty() {
            return None;
        }
        let mapped: Vec<String> = spec
            .split('+')
            .map(|t| match t.trim().to_uppercase().as_str() {
                "CMD" | "COMMAND" | "⌘" => "CmdOrCtrl".to_string(),
                "⇧" => "Shift".to_string(),
                "OPTION" | "⌥" => "Alt".to_string(),
                _ => t.trim().to_string(),
            })
            .collect();
        match Accelerator::from_str(&mapped.join("+")) {
            Ok(a) => Some(a),
            Err(e) => {
                eprintln!("qi-gui 菜单：快捷键「{spec}」认不出来（{e}），不设快捷键");
                None
            }
        }
    }

    fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            let st = s.get_or_insert_with(|| {
                let mut st = State {
                    menu: Menu::new(),
                    submenus: Vec::new(),
                    user: HashMap::new(),
                    builtin: HashMap::new(),
                    clicks: VecDeque::new(),
                    next_no: 1,
                    installed: false,
                    quit: false,
                };
                app_menu(&mut st);
                st
            });
            f(st)
        })
    }

    /// macOS 的第一个菜单固定显示成程序名，放隐藏/退出这些。
    /// 退出用自己的项：系统的 `terminate:` 会直接结束进程，qi 侧收不到关窗
    #[cfg(target_os = "macos")]
    fn app_menu(st: &mut State) {
        let app = Submenu::new("App", true);
        let quit = MenuItem::new("退出", true, accelerator("Cmd+Q"));
        st.builtin.insert(quit.id().clone(), Builtin::Quit);
        let _ = app.append_items(&[
            &PredefinedMenuItem::hide(Some("隐藏")),
            &PredefinedMenuItem::hide_others(Some("隐藏其他")),
            &PredefinedMenuItem::show_all(Some("全部显示")),
            &PredefinedMenuItem::separator(),
            &quit,
        ]);
        let _ = st.menu.append(&app);
    }

    #[cfg(not(target_os = "macos"))]
    fn app_menu(_st: &mut State) {}

    fn submenu(st: &mut State, name: &str) -> Submenu {
        if let Some((_, m)) = st.submenus.iter().find(|(n, _)| n == name) {
            return m.clone();
        }
        let m = Submenu::new(name, true);
        let _ = st.menu.append(&m);
        st.submenus.push((name.to_string(), m.clone()));
        m
    }

    /// macOS：菜单栏挂到 NSApp 上（要在事件循环建好之后，即 `应用创建` 之后）
    #[cfg(target_os = "macos")]
    fn install(st: &mut State) {
        if !st.installed && crate::egui_app::has_window() {
            st.menu.init_for_nsapp();
            st.installed = true;
        }
    }

    /// Windows：菜单栏挂到窗口上
    #[cfg(target_os = "windows")]
    fn install(st: &mut State) {
        if st.installed {
            return;
        }
        if let Some(hwnd) = crate::egui_app::main_hwnd() {
            // SAFETY: hwnd 来自 winit 的活窗口，在主线程上调用
            if unsafe { st.menu.init_for_hwnd(hwnd) }.is_ok() {
                st.installed = true;
            }
        }
    }

    pub(super) fn add(menu: &str, item: &str, accel: &str) -> i64 {
        with_state(|st| {
            let sub = submenu(st, menu);
            let it = MenuItem::new(item, true, accelerator(accel));
            if sub.append(&it).is_err() {
                return 0;
            }
            let no = st.next_no;
            st.next_no += 1;
            st.user.insert(it.id().clone(), no);
            install(st);
            no
        })
    }

    pub(super) fn separator(menu: &str) {
        with_state(|st| {
            let sub = submenu(st, menu);
            let _ = sub.append(&PredefinedMenuItem::separator());
        });
    }

    pub(super) fn add_edit(menu: &str) -> i64 {
        let copy = if cfg!(target_os = "macos") {
            "拷贝"
        } else {
            "复制"
        };
        let items = [
            ("撤销", "Cmd+Z", Builtin::Undo),
            ("重做", "Cmd+Shift+Z", Builtin::Redo),
            ("", "", Builtin::Undo),
            ("剪切", "Cmd+X", Builtin::Cut),
            (copy, "Cmd+C", Builtin::Copy),
            ("粘贴", "Cmd+V", Builtin::Paste),
            ("全选", "Cmd+A", Builtin::SelectAll),
        ];
        with_state(|st| {
            let sub = submenu(st, menu);
            for (text, accel, kind) in items {
                if text.is_empty() {
                    let _ = sub.append(&PredefinedMenuItem::separator());
                    continue;
                }
                let it = MenuItem::new(text, true, accelerator(accel));
                let _ = sub.append(&it);
                st.builtin.insert(it.id().clone(), kind);
            }
            install(st);
            1
        })
    }

    pub(super) fn next_click() -> i64 {
        STATE.with(|s| {
            s.borrow_mut()
                .as_mut()
                .and_then(|st| st.clicks.pop_front())
                .unwrap_or(0)
        })
    }

    fn command(shift: bool) -> Modifiers {
        let mut m = Modifiers::COMMAND;
        if cfg!(target_os = "macos") {
            m.mac_cmd = true;
        } else {
            m.ctrl = true;
        }
        m.shift = shift;
        m
    }

    fn key_events(key: Key, modifiers: Modifiers) -> [Event; 2] {
        [true, false].map(|pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        })
    }

    /// 内置项被点了 → 要塞进 egui 的事件
    pub(super) fn builtin_events(kind: Builtin, clipboard: Option<String>) -> Vec<Event> {
        match kind {
            Builtin::Undo => key_events(Key::Z, command(false)).to_vec(),
            Builtin::Redo => key_events(Key::Z, command(true)).to_vec(),
            Builtin::SelectAll => key_events(Key::A, command(false)).to_vec(),
            Builtin::Cut => vec![Event::Cut],
            Builtin::Copy => vec![Event::Copy],
            Builtin::Paste => clipboard
                .filter(|t| !t.is_empty())
                .map(|t| vec![Event::Paste(t)])
                .unwrap_or_default(),
            Builtin::Quit => Vec::new(),
        }
    }

    pub(super) fn poll(
        raw: &mut egui::RawInput,
        clipboard: &mut dyn FnMut() -> Option<String>,
    ) -> bool {
        let mut any = false;
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            any = true;
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                let Some(st) = s.as_mut() else { return };
                if let Some(no) = st.user.get(&ev.id) {
                    st.clicks.push_back(*no);
                } else if let Some(kind) = st.builtin.get(&ev.id).copied() {
                    if kind == Builtin::Quit {
                        st.quit = true;
                    } else {
                        let clip = if kind == Builtin::Paste {
                            clipboard()
                        } else {
                            None
                        };
                        raw.events.extend(builtin_events(kind, clip));
                    }
                }
            });
        }
        // Windows 上菜单可能先于窗口建好，窗口出来后补挂
        STATE.with(|s| {
            if let Some(st) = s.borrow_mut().as_mut() {
                install(st);
            }
        });
        any
    }

    pub(super) fn take_quit() -> bool {
        STATE.with(|s| {
            s.borrow_mut()
                .as_mut()
                .map(|st| std::mem::take(&mut st.quit))
                .unwrap_or(false)
        })
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod imp {
    pub(super) fn add(_menu: &str, _item: &str, _accel: &str) -> i64 {
        0
    }
    pub(super) fn separator(_menu: &str) {}
    pub(super) fn add_edit(_menu: &str) -> i64 {
        0
    }
    pub(super) fn next_click() -> i64 {
        0
    }
    pub(super) fn poll(
        _raw: &mut egui::RawInput,
        _clipboard: &mut dyn FnMut() -> Option<String>,
    ) -> bool {
        false
    }
    pub(super) fn take_quit() -> bool {
        false
    }
}

/// 帧开始时调用：收菜单事件。返回这一轮有没有菜单事件（有就当这一帧有输入）
pub(crate) fn poll(
    raw: &mut egui::RawInput,
    clipboard: &mut dyn FnMut() -> Option<String>,
) -> bool {
    imp::poll(raw, clipboard)
}

/// 菜单里点了「退出」
pub(crate) fn take_quit() -> bool {
    imp::take_quit()
}

/// 菜单添加(菜单名, 项名, 快捷键) → 编号（>0）；本平台没有原生菜单返回 0
#[no_mangle]
pub extern "C" fn qi_gui_menu_add_impl(
    menu: *const c_char,
    item: *const c_char,
    accel: *const c_char,
) -> i64 {
    let (m, i) = (cstr(menu), cstr(item));
    if m.is_empty() || i.is_empty() {
        return 0;
    }
    imp::add(&m, &i, &cstr(accel))
}

/// 菜单分隔线(菜单名)
#[no_mangle]
pub extern "C" fn qi_gui_menu_separator_impl(menu: *const c_char) {
    let m = cstr(menu);
    if !m.is_empty() {
        imp::separator(&m);
    }
}

/// 菜单添加编辑项(菜单名) → 1 / 0（本平台没有原生菜单）
#[no_mangle]
pub extern "C" fn qi_gui_menu_add_edit_impl(menu: *const c_char) -> i64 {
    let m = cstr(menu);
    if m.is_empty() {
        return 0;
    }
    imp::add_edit(&m)
}

/// 菜单被点() → 编号，没有返回 0
#[no_mangle]
pub extern "C" fn qi_gui_menu_clicked_impl() -> i64 {
    imp::next_click()
}

#[cfg(all(test, any(target_os = "macos", target_os = "windows")))]
mod tests {
    use super::imp::*;
    use egui::{Event, Key};

    #[test]
    fn accelerator_spec_maps_cmd_to_platform_key() {
        assert!(accelerator("").is_none());
        assert!(accelerator("Cmd+S").is_some());
        assert!(accelerator("Cmd+Shift+S").is_some());
        assert!(accelerator("⌘⇧乱写").is_none());
        assert_eq!(accelerator("cmd+o"), accelerator("CmdOrCtrl+O"));
    }

    #[test]
    fn edit_items_turn_into_egui_events() {
        let undo = builtin_events(Builtin::Undo, None);
        assert!(matches!(
            undo[0],
            Event::Key { key: Key::Z, pressed: true, modifiers, .. } if modifiers.command && !modifiers.shift
        ));
        assert!(matches!(
            builtin_events(Builtin::Redo, None)[0],
            Event::Key { key: Key::Z, modifiers, .. } if modifiers.shift
        ));
        assert_eq!(builtin_events(Builtin::Copy, None), vec![Event::Copy]);
        assert_eq!(
            builtin_events(Builtin::Paste, Some("剪贴板".into())),
            vec![Event::Paste("剪贴板".into())]
        );
        assert!(
            builtin_events(Builtin::Paste, None).is_empty(),
            "剪贴板空就不贴"
        );
    }
}
