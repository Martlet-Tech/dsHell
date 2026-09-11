//! 窗口生命周期：关闭行为的唯一裁决点。
//!
//! **本文件是 `hide()` 与 `app.exit()` 的唯一调用处。**
//! 任何其他文件都不得直接 hide/close/exit 主窗口 —— 关窗行为只有这里说了算。
//!
//! 为什么必须集中：`ExitRequested` 在**最后一个窗口关闭**时触发，而它负责收掉
//! dsh 进程树。若「关到托盘」误用 `close()`，窗口被销毁 → `ExitRequested` 触发
//! → dsh 被杀 → 托盘图标变成一个点不开的空壳。把这条约束收在一个文件里，
//! 将来新增退出路径时无法绕过它。

use tauri::{AppHandle, Manager, Runtime};

use crate::ui_text;

/// 「关到托盘」：隐藏窗口，**绝不能用 close()**。
pub fn hide_to_tray<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window(crate::MAIN_WINDOW) {
        let _ = w.hide();
        crate::log("close behavior: hidden to tray (dsh kept alive)");
    }
}

/// 「完全退出」：走既有 `ExitRequested` 清理链，**不新增清理逻辑**。
pub fn quit<R: Runtime>(app: &AppHandle<R>) {
    crate::log("close behavior: full quit");
    app.exit(0);
}

/// 托盘左键：重新展开主界面并置顶。
pub fn restore_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window(crate::MAIN_WINDOW) {
        let _ = w.show();
        let _ = w.set_focus();
        crate::log("tray: restored main window");
    }
}

/// 用户在关闭弹窗里选了什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseChoice {
    /// 「完全退出」
    Quit,
    /// 「关到托盘」
    HideToTray,
    /// 「取消」或点 × —— 什么都不做
    Cancel,
}

/// 关闭弹窗的动作表：把"用户选了什么"翻译成"该干什么"。
/// 新增选项时改这里一处即可。
pub fn apply_close_choice<R: Runtime>(app: &AppHandle<R>, choice: CloseChoice) {
    match choice {
        CloseChoice::Quit => quit(app),
        CloseChoice::HideToTray => hide_to_tray(app),
        CloseChoice::Cancel => {
            crate::log("close behavior: cancelled (window stays visible)");
        }
    }
}

/// 弹窗文案与按钮。
///
/// 集中在这里，`close_dialog.rs` 只负责调用；文案本身来自 `ui_text`。
pub struct CloseDialogSpec {
    pub title: &'static str,
    pub body: &'static str,
    /// 第一键：完全退出
    pub yes: &'static str,
    /// 第二键：关到托盘
    pub no: &'static str,
    /// 第三键：取消（点 × 也归这里）
    pub cancel: &'static str,
}

impl CloseDialogSpec {
    pub fn load() -> Self {
        Self {
            title: ui_text::close_dialog_title(),
            body: ui_text::close_dialog_body(),
            yes: ui_text::close_btn_quit(),
            no: ui_text::close_btn_tray(),
            cancel: ui_text::close_btn_cancel(),
        }
    }
}
