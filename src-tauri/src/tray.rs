//! 托盘图标：构建、菜单、点击。
//!
//! 本模块**只把托盘事件翻译成语义动作**（`TrayAction`），再由 `lifecycle` 执行。
//! 托盘不知道窗口怎么显示、也不知道程序怎么退出 —— 两边可独立修改。

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Runtime,
};

use crate::{lifecycle, ui_text};

/// 托盘菜单项 id（集中一处，避免字符串散落）
const MENU_ID_QUIT: &str = "tray-quit";
/// 托盘图标 id
const TRAY_ID: &str = "main-tray";

/// 托盘能产生的动作。新增动作时在这里加一项，编译器会提示所有该处理的地方。
enum TrayAction {
    /// 左键单击 → 重新展开主界面
    RestoreWindow,
    /// 菜单「完全退出」
    Quit,
}

/// 建托盘。在 `setup` 里调用一次。
pub fn init<R: Runtime>(app: &App<R>) -> tauri::Result<()> {
    let quit_item = MenuItem::with_id(app, MENU_ID_QUIT, ui_text::menu_quit(), true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&quit_item])?;

    // 复用 exe 图标：不新增任何资源文件。
    // 拿不到时（tauri.conf.json 的 bundle.icon 为空）托盘无图标，但仍要建出来，
    // 否则「关到托盘」会让窗口彻底找不回来。
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(ui_text::tray_tooltip())
        .menu(&menu)
        // 默认为 true；不关掉的话左键会弹菜单，而不是恢复窗口
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            if event.id() == MENU_ID_QUIT {
                dispatch(app, TrayAction::Quit);
            }
        })
        .on_tray_icon_event(|tray, event| {
            // 只认左键抬起，避免按下 + 抬起各触发一次
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                dispatch(tray.app_handle(), TrayAction::RestoreWindow);
            }
        });

    match app.default_window_icon().cloned() {
        Some(icon) => builder = builder.icon(icon),
        None => crate::log("tray: no default window icon available (bundle.icon 为空?)"),
    }

    builder.build(app)?;

    Ok(())
}

/// 唯一的动作出口：托盘不自己干副作用，统一交给 `lifecycle`。
fn dispatch<R: Runtime>(app: &AppHandle<R>, action: TrayAction) {
    match action {
        TrayAction::RestoreWindow => lifecycle::restore_main_window(app),
        TrayAction::Quit => lifecycle::quit(app),
    }
}
