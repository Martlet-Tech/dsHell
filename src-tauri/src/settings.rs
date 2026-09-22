//! 设置页：**主窗口内的覆盖层**，以及面板回传给壳层的那个本地端点。
//!
//! ## 为什么是覆盖层，不是新窗口、也不是导航
//!
//! 用户要求"设置页与 dsh 的 web 共享主窗口"。三条路里只有覆盖层同时满足两点：
//!
//! | 机制 | 当前文档 | dsh 页面状态 |
//! | --- | --- | --- |
//! | **注入覆盖层** | **不变** | **保全** |
//! | 导航到另一个本地页 | 换掉 | 全部丢失（整页重载） |
//! | 独立窗口 | 不变 | 保全，但不是"共享主窗口" |
//!
//! 覆盖层的形态是**一个全屏 iframe**，而不是往 dsh 文档里插裸 DOM。裸 DOM 有两笔代价
//! （样式互相影响、dsh 的全局快捷键仍会收到事件），iframe 里自然都没有：里面是独立文档。
//! 代价是 iframe **不能自己关闭自己**（跨域父文档碰不到），这件事由宿主 shim 代劳。
//!
//! ## 两条通道，都不依赖 Tauri IPC
//!
//! ```text
//! Rust ──eval(注入 shim)──▶ 当前文档（shim + 全屏 iframe）
//!      ──eval → shim.push() → postMessage──▶ 面板
//! 面板 ──sendBeacon(POST，路径含 nonce)──▶ Rust 的回传端点
//! ```
//!
//! 刻意**不给面板开 Tauri IPC**：面板活在 dsh 的文档里，给它开 IPC 等于给 dsh 页面里的
// 一切内容开 `app_quit` / `install_missing` 这类命令。所以走"`eval` + `sendBeacon`"：
// 两个方向都是本项目既有或已实测的机制（`eval` 见 `main.rs` 的 `splash_*`；
// `sendBeacon` 是简单请求，没有 CORS 预检、不需要自定义头）。
//!
//! ## 回传端点的安全性
//!
//! 只监听 `127.0.0.1`，且在路径里放一次性 nonce（每次启动重新生成 32 位十六进制），
//! 并要求 `Origin` 是我们认得的两个来源之一：
//!
//! | 检查 | 拒绝什么 |
//! | --- | --- |
//! | 路径里的 nonce 必须等于本次启动生成的值 | 本机别的进程扫到这个端口的误打误撞 |
//! | `Origin` 必须是 loopback 或 `tauri.localhost` | 地址栏直达、`<img src>`、无来源的脚本请求 |
//! | 只认 POST，只认那 5 个动作 | 这个服务不代理任何别的东西 |
//!
//! **诚实说明**：nonce 挡不住能读我们内存/日志的同权限进程，与 `picker` 那边的定位一致
//! ——它防的是**误触与降噪**，不是对抗本机同权限攻击者。
//!
//! ## 版本号穿进工具链的地方
//!
//! 面板传来的 `version` 会拼进 `npm i -g @deepseek-ai/dsh@<v>`，所以这里是**边界**：
//! 字符集校验在 `installer::valid_version`（唯一的命令拼装处），动作分发这里先挡一次。

use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::json;
use tauri::{AppHandle, Manager};

use crate::picker;
use crate::update;

/// 面板文件（与启动页同目录，由 Tauri 的 asset 协议提供）。
const PANEL_FILE: &str = "settings.html";

/// 回传端点监听的端口（内核分配）；0 = 还没起。
static BRIDGE_PORT: AtomicU16 = AtomicU16::new(0);
/// 本次启动的一次性 nonce。
static NONCE: Mutex<Option<String>> = Mutex::new(None);
/// 覆盖层当前是否开着（只为日志与幂等，真实状态在页面里）。
static OPEN: AtomicBool = AtomicBool::new(false);
/// 面板是否回报过 `ready`（看门狗用：区分"面板没加载出来"与"加载了但没说话"）。
static READY: AtomicBool = AtomicBool::new(false);
/// 用户在版本页选中的目标版本。
///
/// 存在的意义是让底部那两个按钮**有真实差别**：「确定」提交它（下次打开仍选中），
/// 「取消」清掉它。本轮两个标签都没有别的可落盘字段，不为它发明假功能。
static SELECTED: Mutex<Option<String>> = Mutex::new(None);

/// 等面板回报 `ready` 的上限。超时只记日志 + 在页面上写一行可见的线索，
/// 不改变任何流程（面板起不来是"设置用不了"，不该让别的东西受影响）。
const READY_TIMEOUT: Duration = Duration::from_millis(2500);

/// 宿主 shim。**契约**：`window.__dshellSettings` 的三个方法不许改名
/// （Rust 侧一律用 `window.__dshellSettings&&…` 单向调用）。
///
/// 幂等：文档里已经有 shim 就复用它（重复点托盘「设置」不该叠第二个 iframe）。
/// 整体是自包含的，因为它是被 `eval` 注入到一个**陌生文档**里的：
/// 不能依赖任何外部文件与模块。
///
/// （用 `r##"…"##`：里面含 CSS 颜色，`"#` 会提前结束 `r#"…"#`。）
const SHIM: &str = r##"
(function () {
  // base64 → **UTF-8** 文本。
  //
  // 不能写成裸的 `atob(b64)`：`atob` 给的是 Latin-1（一个字符一个字节），
  // 于是 Rust 侧按 UTF-8 编出去的"体验优化"（E4 BD 93 E9 AA 8C …）到这儿就变成
  // `ä½?éª?ä¼?å?` 上屏 —— 用户 2026-09-23 报的"乱码"就是它。
  //
  // 这个缺陷一直在（`source_note`、错误文案里本来就有中文），只是以前推过去的字段
  // 几乎全是 ASCII（版本号、日期、URL），所以没露出来；更新内容是第一批成段中文。
  // 启动页的 `ui/js/transport.js` 从一开始就是对的解码，这里是把它抄一份过来：
  // shim 必须自包含（被 `eval` 注入陌生文档，不能依赖外部文件）。
  function dec(b64) {
    var bin = atob(b64);
    var bytes = new Uint8Array(bin.length);
    for (var i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return new TextDecoder("utf-8").decode(bytes);
  }
  var url = dec("__URL__");
  var port = __PORT__;
  var nonce = dec("__NONCE__");
  var base = "http://127.0.0.1:" + port + "/_dshell/" + nonce + "/settings/";
  function beacon(action) {
    try { navigator.sendBeacon(base + action); } catch (e) {}
  }
  var S = window.__dshellSettings;
  if (!S) {
    S = window.__dshellSettings = {
      host: null,
      frame: null,
      open: function () {
        if (this.host) { this.host.style.display = "block"; return; }
        var d = document.createElement("div");
        d.id = "__dshell_settings_host";
        d.setAttribute("style", [
          "position:fixed", "inset:0", "z-index:2147483000",
          "background:#0b0f17", "color-scheme:dark", "margin:0", "padding:0"
        ].join(";") + ";");
        var f = document.createElement("iframe");
        f.id = "__dshell_settings_frame";
        f.setAttribute("title", "DShell 设置");
        f.setAttribute("style", [
          "position:absolute", "inset:0", "width:100%", "height:100%",
          "border:0", "margin:0", "padding:0", "display:block", "background:#0b0f17"
        ].join(";") + ";");
        f.src = url + (url.indexOf("?") < 0 ? "?" : "&") + "b=" + port + "&n=" + nonce;
        d.appendChild(f);
        (document.body || document.documentElement).appendChild(d);
        this.host = d;
        this.frame = f;
      },
      hide: function () {
        if (this.host && this.host.parentNode) this.host.parentNode.removeChild(this.host);
        this.host = null;
        this.frame = null;
      },
      push: function (b64) {
        if (!this.frame || !this.frame.contentWindow) return;
        var json = dec(b64);
        this.frame.contentWindow.postMessage({ __dshell: 1, payload: json }, "*");
      },
      status: function (b64) {
        if (!this.host) return;
        var el = this.host.querySelector("#__dshell_settings_status");
        if (!el) {
          el = document.createElement("div");
          el.id = "__dshell_settings_status";
          el.setAttribute("style", [
            "position:absolute", "left:50%", "top:50%", "transform:translate(-50%,-50%)",
            "max-width:520px", "padding:20px 24px", "border-radius:12px",
            "background:#111827", "color:#e5e7eb", "font:14px/1.7 system-ui,Segoe UI,sans-serif",
            "border:1px solid #374151", "z-index:2"
          ].join(";") + ";");
          this.host.appendChild(el);
        }
        el.textContent = dec(b64);
      }
    };
    // Esc 关闭：焦点不在 iframe 里时（事件落在 dsh 文档上）由这里兜底。
    // iframe 里的按键不会冒泡出来，所以面板自己处理时不会重复。
    document.addEventListener("keydown", function (e) {
      if (e.key !== "Escape" || !window.__dshellSettings.host) return;
      e.preventDefault();
      e.stopPropagation();
      beacon("cancel");
    }, true);
  }
  S.open();
})();
"##;

/// 起回传端点。失败返回 `None`：**设置面板用不了不该拦住启动**，与 `picker` 同一处置。
pub fn start(app: AppHandle) -> Option<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    BRIDGE_PORT.store(port, Ordering::SeqCst);
    let nonce = fresh_nonce(port);
    *NONCE.lock().unwrap() = Some(nonce.clone());

    // nonce 进日志：它只是本次运行的 loopback 临时口令，而**没有它就没法用 curl
    // 手工验证这个端点**（面板那条路要真的点击才走得通）。与主流程里
    // `launch url: …?token=…` 记日志是同一处置。
    crate::log(&format!(
        "settings: bridge path /_dshell/{nonce}/settings/ (loopback only)"
    ));

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let app = app.clone();
            std::thread::spawn(move || {
                let _ = handle(stream, &app);
            });
        }
    });

    Some(port)
}

/// 页面每次**换文档**都要复位覆盖层状态：旧文档连同里面的 iframe 一起没了，
/// 而 shim 也随之消失（下次 `open` 会重新注入）。
pub fn reset_for_document() {
    if OPEN.swap(false, Ordering::SeqCst) {
        crate::log("settings: overlay gone with the previous document");
    }
    READY.store(false, Ordering::SeqCst);
}

/// 打开设置面板：把 shim 注入当前文档，由它建出 iframe。
pub fn open(app: &AppHandle) {
    let Some(window) = app.get_webview_window(crate::MAIN_WINDOW) else {
        crate::log("settings: main window is gone - cannot open the panel");
        return;
    };
    let Some(url) = panel_url(app) else {
        crate::log("settings: panel url unknown (启动页地址还没记下) - cannot open the panel");
        return;
    };
    let Some(nonce) = nonce() else {
        crate::log("settings: bridge is not listening - cannot open the panel");
        return;
    };
    let port = BRIDGE_PORT.load(Ordering::SeqCst);

    let js = SHIM
        .replace("__URL__", &crate::b64(&url))
        .replace("__PORT__", &port.to_string())
        .replace("__NONCE__", &crate::b64(&nonce));

    OPEN.store(true, Ordering::SeqCst);
    READY.store(false, Ordering::SeqCst);

    // 先假设"能开"，失败与否由面板回报的 `ready` 说了算（`eval` 的 Ok 只表示入队，
    // 这点是本项目 R17–R19 的教训 M13）。
    match window.eval(&js) {
        Ok(()) => crate::log(&format!("settings: shim injected (panel={url})")),
        Err(e) => crate::log(&format!("settings: eval failed: {e}")),
    }

    // 看门狗：面板没起来时，日志与页面各留一条线索，而不是让用户对着空白面板发呆。
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(READY_TIMEOUT);
        if OPEN.load(Ordering::SeqCst) && !READY.load(Ordering::SeqCst) {
            crate::log("settings: panel did not report ready - see the log for details");
            if let Some(w) = app.get_webview_window(crate::MAIN_WINDOW) {
                let hint = "设置面板没有加载出来。\n\n这通常是打包不完整或文件损坏。\n按 Esc 关闭；完整日志：%USERPROFILE%\\.dshell\\dshell-poc.log";
                let _ = w.eval(&format!(
                    "window.__dshellSettings&&window.__dshellSettings.status('{}')",
                    crate::b64(hint)
                ));
            }
        }
    });
}

/// 关掉面板并把 dsh 页面切回前台（用户点「取消 / 确定」都要走这里）。
pub fn hide(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(crate::MAIN_WINDOW) {
        let _ = w.eval("window.__dshellSettings&&window.__dshellSettings.hide()");
    }
    OPEN.store(false, Ordering::SeqCst);
    crate::lifecycle::restore_main_window(app);
}

/// 面板地址：由**启动页自己的 URL** 推出（Tauri 决定它的形状，不硬编码）。
fn panel_url(app: &AppHandle) -> Option<String> {
    let splash = app.state::<crate::AppState>().splash_url()?;
    let cut = splash.rfind('/')?;
    Some(format!("{}/{}", &splash[..cut], PANEL_FILE))
}

fn nonce() -> Option<String> {
    NONCE.lock().ok()?.clone()
}

/// 每次启动重新生成的路径 nonce（32 位十六进制）。
///
/// 用 `RandomState` 的哈希做熵源：标准库已经把它接在系统随机数上，
/// 不必为一个临时令牌引依赖（与 `picker` 同一做法）。
fn fresh_nonce(port: u16) -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::with_capacity(32);
    for i in 0..2u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(((port as u64) << 8) | i);
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

// ───────────────────────── 回传端点 ─────────────────────────

fn handle(mut stream: TcpStream, app: &AppHandle) -> std::io::Result<()> {
    let req = match picker::read_request(&mut stream) {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };

    if req.method != "POST" {
        return picker::write_json(
            &mut stream,
            405,
            "Method Not Allowed",
            r#"{"ok":false,"error":"method not allowed"}"#,
        );
    }

    let Some(nonce) = nonce() else {
        return picker::write_json(
            &mut stream,
            503,
            "Service Unavailable",
            r#"{"ok":false,"error":"bridge not ready"}"#,
        );
    };

    let Some((action, query)) = split_action(&req.path, &nonce) else {
        return picker::write_json(
            &mut stream,
            404,
            "Not Found",
            r#"{"ok":false,"error":"not found"}"#,
        );
    };

    // 面板是我们自己注入的，所以一定带 `Origin`；没有来源的请求一律不认。
    if !origin_allowed(req.origin.as_deref()) {
        crate::log(&format!(
            "settings: rejected {:?} from origin {:?}",
            action, req.origin
        ));
        return picker::write_json(
            &mut stream,
            403,
            "Forbidden",
            r#"{"ok":false,"error":"forbidden"}"#,
        );
    }

    match action {
        "ready" => {
            READY.store(true, Ordering::SeqCst);
            crate::log("settings: panel reported ready");
            push_data_async(app);
        }
        "refresh" => {
            crate::log("settings: panel asked to refresh the catalog");
            push_data_forced(app);
        }
        "notes" => {
            let Some(version) = query_param(query, "version") else {
                return picker::write_json(
                    &mut stream,
                    400,
                    "Bad Request",
                    r#"{"ok":false,"error":"version required"}"#,
                );
            };
            // 回传端点**必须立刻返回**：这是面板发的一次 beacon，拿不到响应，
            // 结果走 push。拉取要联网 ~1s，不能占住这个连接线程。
            push_notes_async(app, version);
        }
        "cancel" => {
            set_selected(None);
            crate::log("settings: cancelled (selection discarded)");
            hide(app);
        }
        "apply" => {
            let sel = query_param(query, "version").filter(|v| !v.is_empty());
            crate::log(&format!("settings: applied (selection={sel:?})"));
            set_selected(sel);
            hide(app);
        }
        "switch" => {
            let Some(version) = query_param(query, "version") else {
                return picker::write_json(
                    &mut stream,
                    400,
                    "Bad Request",
                    r#"{"ok":false,"error":"version required"}"#,
                );
            };
            crate::log(&format!("settings: switch requested -> {version}"));
            crate::start_switch_dsh(app, version);
        }
        _ => {
            return picker::write_json(
                &mut stream,
                404,
                "Not Found",
                r#"{"ok":false,"error":"unknown action"}"#,
            );
        }
    }

    picker::write_json(&mut stream, 200, "OK", r#"{"ok":true}"#)
}

/// `/_dshell/<nonce>/settings/<action>?<query>` → `("<action>", Some("<query>"))`。
fn split_action<'a>(path: &'a str, nonce: &str) -> Option<(&'a str, Option<&'a str>)> {
    let (head, query) = match path.split_once('?') {
        Some((h, q)) => (h, Some(q)),
        None => (path, None),
    };
    let prefix = format!("/_dshell/{nonce}/settings/");
    let action = head.strip_prefix(&prefix)?;
    (!action.is_empty()).then_some((action, query))
}

/// 只取一个参数就够（`version`）。值里允许 `+`（build metadata），所以不解码 `+`。
fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    let q = query?;
    for pair in q.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == key {
            return Some(percent_decode(v));
        }
    }
    None
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 页面发来的请求只可能来自这两个来源：面板（启动页阶段是 `tauri.localhost`）
/// 或 dsh 页面自己的 origin（`127.0.0.1:<port>`）。
fn origin_allowed(origin: Option<&str>) -> bool {
    let Some(o) = origin else { return false };
    let o = o.trim().to_ascii_lowercase();
    o.starts_with("http://127.0.0.1:") || o.starts_with("http://localhost:") || o == "http://tauri.localhost"
}

// ───────────────────────── 推数据给面板 ─────────────────────────

/// 查一次台账并推给面板。
///
/// **放后台线程**：`npm view` 要联网，不能占住这个连接线程。
/// 也是"先用上次结果渲染、过期再后台重查"的落点：registry 那半边有两小时缓存
/// （见 `update::TTL_MS`），所以 TTL 内开面板是瞬开的、一条 npm 都不跑。
fn push_data_async(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let stale = push_data(&app);
        if !stale {
            return;
        }
        crate::log("settings: cached catalog is stale - re-querying in the background");
        if let Err(e) = update::refresh_registry() {
            // 后台重查失败不打扰用户：面板上那份旧数据仍然可用，而且带时间戳。
            crate::log(&format!("settings: background refresh failed: {e}"));
            return;
        }
        push_data(&app);
    });
}

/// 手动「刷新」：无视 TTL 重查，失败时把原因告诉面板（**不**清掉它已有的列表）。
fn push_data_forced(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = update::refresh_registry() {
            crate::log(&format!("settings: refresh failed: {e}"));
            push(&app, json!({ "type": "refresh_error", "text": e }));
            return;
        }
        push_data(&app);
    });
}

/// 组装并推一份台账。返回 registry 数据是否**已过期**（调用方据此决定要不要后台重查）。
fn push_data(app: &AppHandle) -> bool {
    let cfg = crate::config::Config::load();
    let about = json!({
        "name": "DShell",
        "version": app.package_info().version.to_string(),
        "dsh_path": cfg.dsh.clone(),
        "selected": selected(),
        "disk": disk_info(),
    });
    match update::catalog(false) {
        Ok(catalog) => {
            crate::log(&format!(
                "settings: catalog ok (current={:?}, latest={:?}, {} 个版本, npm_global={}, age={}s, stale={}, disk_free={}GB)",
                catalog.current,
                catalog.latest,
                catalog.versions.len(),
                catalog.npm_global,
                catalog.registry_age_secs.unwrap_or(0),
                catalog.registry_stale,
                about
                    .get("disk")
                    .and_then(|d| d.get("free_gb"))
                    .and_then(|v| v.as_f64())
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".to_string())
            ));
            let stale = catalog.registry_stale;
            push(app, json!({ "type": "data", "about": about, "catalog": catalog }));
            stale
        }
        Err(e) => {
            crate::log(&format!("settings: catalog failed: {e}"));
            push(app, json!({ "type": "error", "about": about, "text": e }));
            false
        }
    }
}

fn push(app: &AppHandle, msg: serde_json::Value) {
    let Some(w) = app.get_webview_window(crate::MAIN_WINDOW) else {
        return;
    };
    let _ = w.eval(&format!(
        "window.__dshellSettings&&window.__dshellSettings.push('{}')",
        crate::b64(&msg.to_string())
    ));
}

/// 面板点了某一版的「更新内容」→ 后台取一次发布说明再推回去。
///
/// 三条口径：
///
/// * **放后台线程**：拉取要联网（实测 ~1.1 s），不能占住回传端点那个连接线程。
/// * **失败只推 `notes_error`**：面板把已展开的窗口留成"拉取失败 + 可重试"，
///   列表本身一点都不受影响（与 `refresh_error` 同一处置）。
/// * **`Ok(None)` 不是错误**：registry 里有这一版、GitHub 上没有对应 release
///   （实测 7 个），面板显示"这一版没有发布说明"。
fn push_notes_async(app: &AppHandle, version: String) {
    let app = app.clone();
    std::thread::spawn(move || {
        match crate::release_notes::notes(&version) {
            Ok(Some(note)) => {
                crate::log(&format!(
                    "notes: {} ({} 字符, url={}, cached={})",
                    note.version,
                    note.body.chars().count(),
                    note.url,
                    note.cached
                ));
                push(&app, json!({ "type": "notes", "note": note }));
            }
            Ok(None) => {
                crate::log(&format!("notes: {version} has no GitHub release"));
                push(&app, json!({ "type": "notes_missing", "version": version }));
            }
            Err(e) => {
                crate::log(&format!("notes: {version} failed: {e}"));
                push(&app, json!({ "type": "notes_error", "version": version, "text": e }));
            }
        }
    });
}

fn selected() -> Option<String> {
    SELECTED.lock().ok()?.clone()
}

/// 系统盘（npm 缓存所在卷）的空间，给面板在切换前给一句警告用。
///
/// 2026-09-21 实测踩过一次：换版本要一次性重装 400+ 个包，而系统盘只剩 12G（已用 95%）、
/// npm 缓存自己就 15G —— 那次降级的写入高峰把整台机器拖死（Kernel-Power 41 异常关机）。
/// 面板据此提醒"先清理"，比事后解释便宜得多。
fn disk_info() -> serde_json::Value {
    // npm 缓存默认在 %LOCALAPPDATA%\npm-cache，所以问这个盘
    let probe = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| "C:\\".to_string());
    match crate::win32::free_space_bytes(&probe) {
        Some(free) => json!({ "free_gb": (free as f64 / 1073741824.0 * 10.0).round() / 10.0 }),
        None => json!({}),
    }
}

fn set_selected(v: Option<String>) {
    if let Ok(mut s) = SELECTED.lock() {
        *s = v;
    }
}

/// 供 `main.rs` 在切换流程里清选中项（切换成功后旧选中就没意义了）。
pub fn clear_selection() {
    set_selected(None);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 注入的 shim 里**不许有裸的 `atob(...)`** —— 一律走 `dec()`。
    ///
    /// 这条守的是用户 2026-09-23 报的"乱码"：`atob` 给 Latin-1，Rust 侧按 UTF-8
    /// 编出去的中文到面板就成了 `ä½?éª?ä¼?å?`。缺陷很隐蔽 —— 推过去的字段以前几乎
    /// 全是 ASCII（版本号 / 日期 / URL），所以这个 shim 写了很久都没暴露；
    /// 更新内容是第一批成段中文，一下就炸了。
    ///
    /// 用"扫源码文本"这种粗办法是因为真正的失效点在浏览器里（`eval` 注入的字符串），
    /// Rust 这边没有可断言的返回值 —— 这条测试至少能挡住"又写回 atob"。
    #[test]
    fn shim_decodes_base64_as_utf8() {
        assert!(SHIM.contains("function dec(b64)"), "shim 里应有 UTF-8 解码器");
        assert!(SHIM.contains("new TextDecoder(\"utf-8\")"), "解码器必须显式指定 utf-8");

        // 逐处：`atob(` 只允许出现在 dec() 自己内部。
        // 注释行（`//`）跳过 —— 解释这段历史时免不了要写出 `atob` 这个词。
        let bad: Vec<&str> = SHIM
            .lines()
            .map(str::trim)
            .filter(|l| l.contains("atob("))
            .filter(|l| !l.starts_with("//"))
            .filter(|l| !l.starts_with("var bin = atob(b64);"))
            .collect();
        assert!(bad.is_empty(), "shim 里有裸 atob（会出乱码）：{bad:?}");

        // 三处调用点都要用 dec
        for call in ["dec(\"__URL__\")", "dec(\"__NONCE__\")", "var json = dec(b64);", "el.textContent = dec(b64);"] {
            assert!(SHIM.contains(call), "缺少 UTF-8 解码调用：{call}");
        }
    }

    /// 反证：这条路走错会是什么样。把中文按 UTF-8 编出去、再按 Latin-1 读回来，
    /// 就该得到用户截图里那种 `ä½?…`；而按 UTF-8 读回来必须是原文。
    #[test]
    fn utf8_vs_latin1_decoding_differ_on_chinese() {
        let text = "体验优化";
        let bytes = text.as_bytes();
        // Latin-1（等价于裸 atob 的结果）：每个字节一个字符
        let latin1: String = bytes.iter().map(|&b| b as char).collect();
        assert_ne!(latin1, text, "按 Latin-1 读必须是乱的（这正是缺陷现场）");
        assert!(latin1.starts_with('ä'), "乱码形状：{latin1}");
        // UTF-8：还原
        assert_eq!(String::from_utf8(bytes.to_vec()).unwrap(), text);
    }
}
