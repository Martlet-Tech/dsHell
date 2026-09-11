//! 关闭确认弹窗（系统原生）。
//!
//! 用三键 `YesNoCancelCustom`：能同时表达"退出 / 托盘 / 取消"三态，
//! 且系统的 × 按钮天然映射到 Cancel（= 取消），符合用户"点错了"的预期。
//!
//! 为什么不用两键 `OkCancelCustom`：× 与第二键都返回 Cancel，无法区分，
//! 会把"点错了"错当成"关到托盘"。

use tauri::{AppHandle, Runtime};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult};

use crate::lifecycle::{self, CloseChoice, CloseDialogSpec};

/// 弹出关闭确认框，并把结果应用到 `lifecycle`。
///
/// 用**回调版**而非 `blocking_show_with_result()`，避免阻塞 UI 线程。
pub fn ask_close_choice<R: Runtime>(app: &AppHandle<R>) {
    let spec = CloseDialogSpec::load();
    let yes = spec.yes.to_string();
    let no = spec.no.to_string();

    app.dialog()
        .message(spec.body)
        .title(spec.title)
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::YesNoCancelCustom(
            spec.yes.to_string(),
            spec.no.to_string(),
            spec.cancel.to_string(),
        ))
        .show_with_result({
            let app = app.clone();
            move |result| {
                lifecycle::apply_close_choice(&app, interpret(&result, &yes, &no));
            }
        });
}

/// 把对话框结果翻译成三条出路。
///
/// 实现在 Windows 上（rfd 的 win_cid 后端 + 插件的映射）会给自定义按钮返回
/// `Custom(<按钮文字>)`，而不是 `Yes`/`No`。两种形态都认，避免随版本变化而失效。
fn interpret(result: &MessageDialogResult, yes: &str, no: &str) -> CloseChoice {
    match result {
        // 主路径：插件把自定义按钮文字包成 Custom
        MessageDialogResult::Custom(text) => {
            if text == yes {
                CloseChoice::Quit
            } else if text == no {
                CloseChoice::HideToTray
            } else {
                // 第三键（取消）与任何未知文字 → 什么都不做
                CloseChoice::Cancel
            }
        }
        // 兜底：若某版本直接返回系统枚举
        MessageDialogResult::Yes => CloseChoice::Quit,
        MessageDialogResult::No => CloseChoice::HideToTray,
        // Cancel（含点 × 关闭对话框）与其余一切 → 什么都不做
        _ => CloseChoice::Cancel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const YES: &str = "完全退出";
    const NO: &str = "关到托盘";

    #[test]
    fn custom_yes_is_quit() {
        let r = MessageDialogResult::Custom(YES.to_string());
        assert_eq!(interpret(&r, YES, NO), CloseChoice::Quit);
    }

    #[test]
    fn custom_no_is_hide() {
        let r = MessageDialogResult::Custom(NO.to_string());
        assert_eq!(interpret(&r, YES, NO), CloseChoice::HideToTray);
    }

    #[test]
    fn custom_cancel_is_cancel() {
        let r = MessageDialogResult::Custom("取消".to_string());
        assert_eq!(interpret(&r, YES, NO), CloseChoice::Cancel);
    }

    /// 点 × 关闭对话框时，rfd 返回 Cancel —— 必须什么都不做，而不是缩到托盘。
    /// 这是本功能最容易错的一点（见方案 §14 R6）。
    #[test]
    fn window_x_is_cancel_not_hide() {
        assert_eq!(interpret(&MessageDialogResult::Cancel, YES, NO), CloseChoice::Cancel);
    }

    #[test]
    fn plain_enum_fallback() {
        assert_eq!(interpret(&MessageDialogResult::Yes, YES, NO), CloseChoice::Quit);
        assert_eq!(interpret(&MessageDialogResult::No, YES, NO), CloseChoice::HideToTray);
    }
}
