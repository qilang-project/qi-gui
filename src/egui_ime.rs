//! 输入法（IME）：组字、提交、候选框位置
//!
//! 链路本身是 egui-winit 现成的：TextEdit 有焦点时往平台输出里填 `ime`，
//! `handle_platform_output` 据此调 `set_ime_allowed` / `set_ime_cursor_area`；
//! winit 的 `WindowEvent::Ime` 经 `on_window_event` 变成 `egui::Event::Ime`，
//! TextEdit 把组字串当成选中的一段临时文字插进正文，提交时换成定稿。
//! qi-gui 在这条链上补两处（egui 0.29 / egui-winit 0.29 / winit 0.30 实测的缺口）：
//!
//! 1. **候选框位置**：egui-winit 拿 `ime.rect`（整个输入框）当光标区域交给系统。
//!    撑满窗口的编辑区里，系统就把候选框摆到输入框底下 —— 窗口底边甚至屏幕外，
//!    而不是光标旁边。这里在交出去之前把 `rect` 换成 `cursor_rect`（光标那一小块）。
//! 2. **组字清空**：macOS 上组字串被删光（退格删到底、Esc 取消）时 winit 发
//!    `Preedit("", None)`，egui-winit 只把它翻成 `Ime::Disabled`，TextEdit 不动正文，
//!    于是最后那个拼音字母（或整串拼音）留在正文里。这里在它前面补一个
//!    `Preedit("")`，TextEdit 就会把临时文字删掉。提交时 winit 也先发这个空组字再发
//!    `Commit`，补上之后提交路径照样对（先删临时文字，再在原处插定稿）。
//!
//! `QI_GUI_IME_LOG=1` 时把收到的 winit 输入法事件、`set_ime_allowed` 的翻转和交给系统的
//! 光标区域（物理像素）打到标准错误，用来确认候选框定位那条路确实走到了。

use winit::event::Ime;

pub(crate) struct ImeTracker {
    /// 交给 egui 的组字串非空、还没提交或清掉
    preedit_active: bool,
    log: bool,
    last_allowed: bool,
    last_area: Option<egui::Rect>,
}

impl ImeTracker {
    pub(crate) fn new() -> Self {
        ImeTracker {
            preedit_active: false,
            log: std::env::var("QI_GUI_IME_LOG").is_ok_and(|v| v == "1"),
            last_allowed: false,
            last_area: None,
        }
    }

    /// winit 的输入法事件交给 egui-winit **之前**调用
    pub(crate) fn before_winit_event(&mut self, input: &mut egui::RawInput, ime: &Ime) {
        if self.log {
            eprintln!("qi-gui ime: winit {ime:?}");
        }
        // egui-winit 在 Linux 上整个忽略输入法事件（emilk/egui#5008），这里也不插手
        if cfg!(target_os = "linux") {
            return;
        }
        match ime {
            Ime::Preedit(text, Some(_)) if !text.is_empty() => self.preedit_active = true,
            Ime::Preedit(text, None) if text.is_empty() && self.preedit_active => {
                input
                    .events
                    .push(egui::Event::Ime(egui::ImeEvent::Preedit(String::new())));
                self.preedit_active = false;
                if self.log {
                    eprintln!("qi-gui ime: 组字串清空 → 补 Preedit(\"\")");
                }
            }
            Ime::Commit(_) | Ime::Disabled => self.preedit_active = false,
            _ => {}
        }
    }

    /// 平台输出交给 egui-winit **之前**调用：候选框跟着光标走
    pub(crate) fn before_platform_output(&mut self, out: &mut egui::PlatformOutput, ppp: f32) {
        if let Some(ime) = out.ime.as_mut() {
            ime.rect = ime.cursor_rect;
        }
        if !self.log {
            return;
        }
        let allowed = out.ime.is_some();
        if allowed != self.last_allowed {
            self.last_allowed = allowed;
            eprintln!("qi-gui ime: set_ime_allowed({allowed})");
        }
        match out.ime.as_ref() {
            Some(ime) => {
                let px = ime.rect * ppp;
                if self.last_area != Some(px) {
                    self.last_area = Some(px);
                    eprintln!(
                        "qi-gui ime: set_ime_cursor_area x={:.0} y={:.0} w={:.0} h={:.0}（物理像素，光标所在）",
                        px.min.x,
                        px.min.y,
                        px.width(),
                        px.height()
                    );
                }
            }
            None => self.last_area = None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_preedit_after_composition_clears_it_once() {
        let mut t = ImeTracker::new();
        let mut raw = egui::RawInput::default();
        // 没在组字时的空组字（比如刚启用）不能补：那会删掉用户选中的字
        t.before_winit_event(&mut raw, &Ime::Preedit(String::new(), None));
        assert!(raw.events.is_empty());
        t.before_winit_event(&mut raw, &Ime::Preedit("ni".into(), Some((2, 2))));
        t.before_winit_event(&mut raw, &Ime::Preedit(String::new(), None));
        let cleared = egui::Event::Ime(egui::ImeEvent::Preedit(String::new()));
        if cfg!(target_os = "linux") {
            assert!(raw.events.is_empty());
        } else {
            assert_eq!(raw.events, vec![cleared]);
        }
        t.before_winit_event(&mut raw, &Ime::Preedit(String::new(), None));
        assert!(raw.events.len() <= 1, "只补一次");
    }

    #[test]
    fn candidate_window_follows_the_cursor() {
        let mut t = ImeTracker::new();
        let mut out = egui::PlatformOutput {
            ime: Some(egui::output::IMEOutput {
                rect: egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 5000.0)),
                cursor_rect: egui::Rect::from_min_size(
                    egui::pos2(120.0, 40.0),
                    egui::vec2(2.0, 26.0),
                ),
            }),
            ..Default::default()
        };
        t.before_platform_output(&mut out, 2.0);
        let ime = out.ime.unwrap();
        assert_eq!(ime.rect, ime.cursor_rect);
    }
}
