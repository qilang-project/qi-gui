//! 原生文件对话框 FFI（rfd）
//!
//! macOS 是 NSOpenPanel / NSSavePanel，Windows 是 IFileDialog，Linux 走
//! xdg-desktop-portal（D-Bus，纯 Rust，不链 GTK），没有 portal 时 rfd 自己退到 zenity。
//! 对话框是模态的：弹着的时候 qi 主循环停在这一次调用里，窗口不刷新。
//! 选中返回路径，取消返回空串。

use crate::egui_app::{cstr, ret_str};
use std::os::raw::c_char;
use std::path::PathBuf;

/// 过滤串 → (名称, 扩展名表) 组。
///
/// 写法：`png,jpg` 是一组；`图片:png,jpg;音频:mp3,wav` 是两组，冒号前是名称；
/// 扩展名前面带不带点都行，空串表示不过滤
pub(crate) fn parse_filters(spec: &str) -> Vec<(String, Vec<String>)> {
    spec.split(';')
        .filter_map(|group| {
            let group = group.trim();
            let (name, exts) = match group.split_once(':') {
                Some((n, e)) => (n.trim().to_string(), e),
                None => (String::new(), group),
            };
            let exts: Vec<String> = exts
                .split(',')
                .map(|e| e.trim().trim_start_matches('.').to_string())
                .filter(|e| !e.is_empty())
                .collect();
            if exts.is_empty() {
                return None;
            }
            let name = if name.is_empty() {
                exts.join(", ")
            } else {
                name
            };
            Some((name, exts))
        })
        .collect()
}

fn dialog(title: &str) -> rfd::FileDialog {
    let d = rfd::FileDialog::new();
    if title.is_empty() {
        d
    } else {
        d.set_title(title)
    }
}

fn path_str(p: Option<PathBuf>) -> String {
    p.map(|p| p.display().to_string()).unwrap_or_default()
}

/// 选择打开文件(标题, 扩展名过滤) → 路径，取消返回空串
#[no_mangle]
pub extern "C" fn qi_gui_dialog_open_file_impl(
    title: *const c_char,
    filters: *const c_char,
) -> *const c_char {
    let mut d = dialog(&cstr(title));
    for (name, exts) in parse_filters(&cstr(filters)) {
        d = d.add_filter(name, &exts);
    }
    ret_str(path_str(d.pick_file()))
}

/// 选择保存文件(标题, 默认名) → 路径，取消返回空串。默认名可以带目录，
/// 带目录时对话框从那个目录开始
#[no_mangle]
pub extern "C" fn qi_gui_dialog_save_file_impl(
    title: *const c_char,
    default_name: *const c_char,
) -> *const c_char {
    let mut d = dialog(&cstr(title));
    let name = cstr(default_name);
    let path = PathBuf::from(&name);
    match (path.parent(), path.file_name()) {
        (Some(dir), Some(file)) if !dir.as_os_str().is_empty() => {
            d = d
                .set_directory(dir)
                .set_file_name(file.to_string_lossy().into_owned());
        }
        _ if !name.is_empty() => d = d.set_file_name(name),
        _ => {}
    }
    ret_str(path_str(d.save_file()))
}

/// 选择文件夹(标题) → 路径，取消返回空串
#[no_mangle]
pub extern "C" fn qi_gui_dialog_pick_folder_impl(title: *const c_char) -> *const c_char {
    ret_str(path_str(dialog(&cstr(title)).pick_folder()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_spec_forms() {
        assert!(parse_filters("").is_empty());
        assert_eq!(
            parse_filters("md, .txt"),
            vec![("md, txt".to_string(), vec!["md".into(), "txt".into()])]
        );
        assert_eq!(
            parse_filters("图片:png,jpg;音频:mp3"),
            vec![
                ("图片".to_string(), vec!["png".into(), "jpg".into()]),
                ("音频".to_string(), vec!["mp3".into()]),
            ]
        );
        assert!(parse_filters("空组:;  ;").is_empty());
    }
}
