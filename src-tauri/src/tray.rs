//! 托盘图标：构建、菜单、点击。
//!
//! 本模块**只把托盘事件翻译成语义动作**（`TrayAction`），再由 `lifecycle` 执行。
//! 托盘不知道窗口怎么显示、也不知道程序怎么退出 —— 两边可独立修改。

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle,
};

use crate::{lifecycle, ui_text};

/// 托盘菜单项 id（集中一处，避免字符串散落）
const MENU_ID_QUIT: &str = "tray-quit";
const MENU_ID_OPEN_BROWSER: &str = "tray-open-browser";
const MENU_ID_RESTART_DSH: &str = "tray-restart-dsh";
const MENU_ID_SETTINGS: &str = "tray-settings";
/// 托盘图标 id
const TRAY_ID: &str = "main-tray";

/// 托盘能产生的动作。新增动作时在这里加一项，编译器会提示所有该处理的地方。
enum TrayAction {
    /// 左键单击 → 重新展开主界面
    RestoreWindow,
    /// 菜单「在浏览器中打开」
    OpenInBrowser,
    /// 菜单「重启 dsh 后端」
    RestartDsh,
    /// 菜单「设置」（主窗口内的覆盖层）
    OpenSettings,
    /// 菜单「完全退出」
    Quit,
}

/// 建托盘。在 `setup` 里调用一次。
///
/// 不泛型化 `Runtime`（与 `dispatch` 同因）：本应用只有 wry 一个运行时，而
/// 「重启 dsh」那条链上的 `run_doctor` / `start_handoff` / `splash_*` 都是具体的。
pub fn init(app: &App) -> tauri::Result<()> {
    let open_browser_item = MenuItem::with_id(
        app,
        MENU_ID_OPEN_BROWSER,
        ui_text::menu_open_in_browser(),
        true,
        None::<&str>,
    )?;
    let restart_item = MenuItem::with_id(
        app,
        MENU_ID_RESTART_DSH,
        ui_text::menu_restart_dsh(),
        true,
        None::<&str>,
    )?;
    let settings_item = MenuItem::with_id(
        app,
        MENU_ID_SETTINGS,
        ui_text::menu_settings(),
        true,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(app, MENU_ID_QUIT, ui_text::menu_quit(), true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&settings_item, &open_browser_item, &restart_item, &quit_item],
    )?;

    // 复用 exe 图标：不新增任何资源文件。
    // 拿不到时（tauri.conf.json 的 bundle.icon 为空）托盘无图标，但仍要建出来，
    // 否则「关到托盘」会让窗口彻底找不回来。
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(ui_text::tray_tooltip())
        .menu(&menu)
        // 默认为 true；不关掉的话左键会弹菜单，而不是恢复窗口
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let id = event.id();
            if id == MENU_ID_QUIT {
                dispatch(app, TrayAction::Quit);
            } else if id == MENU_ID_OPEN_BROWSER {
                dispatch(app, TrayAction::OpenInBrowser);
            } else if id == MENU_ID_RESTART_DSH {
                dispatch(app, TrayAction::RestartDsh);
            } else if id == MENU_ID_SETTINGS {
                dispatch(app, TrayAction::OpenSettings);
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

/// 唯一的动作出口：托盘不自己干副作用，一律交给 `lifecycle` 或壳层的 `open_dsh_in_browser`。
///
/// 故意**不泛型化** `Runtime`：`restart_dsh` 要落进一个后台线程，而它依赖的
/// `run_doctor` / `start_handoff` / `splash_*` 都写死了具体运行时。为了托盘这一处
/// 把整条调用链都泛型化不值得 —— 本应用只有 wry 一个运行时。
fn dispatch(app: &AppHandle, action: TrayAction) {
    match action {
        TrayAction::RestoreWindow => lifecycle::restore_main_window(app),
        TrayAction::OpenInBrowser => crate::open_dsh_in_browser(app),
        TrayAction::RestartDsh => crate::restart_dsh(app),
        TrayAction::OpenSettings => crate::open_settings(app),
        TrayAction::Quit => lifecycle::quit(app),
    }
}
