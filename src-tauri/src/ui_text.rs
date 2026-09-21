//! 面向用户的文案。**所有面向用户的中文都只在这里出现。**
//!
//! 为什么对外是函数而不是 `pub const`：将来做 i18n 时，这里改成"按当前语言查表"，
//! 所有调用点（`ui_text::menu_quit()`）**一行都不用改**。

/// 当前语言。现在就留好入口，将来由设置窗口切换。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    ZhCn,
}

/// 取当前语言。将来改为读取用户设置。
pub fn current() -> Lang {
    Lang::ZhCn
}

// ── 托盘 ──────────────────────────────────────────────

pub fn tray_tooltip() -> &'static str {
    match current() {
        Lang::ZhCn => "DShell",
    }
}

pub fn menu_open_in_browser() -> &'static str {
    match current() {
        Lang::ZhCn => "在浏览器中打开",
    }
}

pub fn menu_quit() -> &'static str {
    match current() {
        Lang::ZhCn => "完全退出",
    }
}

pub fn menu_restart_dsh() -> &'static str {
    match current() {
        Lang::ZhCn => "重启 dsh 后端",
    }
}

/// 打开主窗口内的设置面板（覆盖层，不是独立窗口）。
pub fn menu_settings() -> &'static str {
    match current() {
        Lang::ZhCn => "设置",
    }
}

// ── 关闭确认弹窗（三键） ────────────────────────────────

pub fn close_dialog_title() -> &'static str {
    match current() {
        Lang::ZhCn => "DShell",
    }
}

pub fn close_dialog_body() -> &'static str {
    match current() {
        Lang::ZhCn => "要退出 DShell，还是只在后台运行？",
    }
}

/// 第一键 → 正常退出
pub fn close_btn_quit() -> &'static str {
    match current() {
        Lang::ZhCn => "完全退出",
    }
}

/// 第二键 → 缩到托盘
pub fn close_btn_tray() -> &'static str {
    match current() {
        Lang::ZhCn => "关到托盘",
    }
}

/// 第三键 → 取消（点 × 也归这里）
pub fn close_btn_cancel() -> &'static str {
    match current() {
        Lang::ZhCn => "取消",
    }
}
