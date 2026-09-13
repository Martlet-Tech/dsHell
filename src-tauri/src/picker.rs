//! 立项 04 · R19：DShell 自己弹原生目录选择框，替代 dsh 的 `-native` 后端。
//!
//! ## 要解决的问题
//!
//! 点「添加工作区」→ dsh 的 `-native` 后端起一个**独立 node 子进程**去弹 Win32
//! `IFileOpenDialog`，而且是 `Show(null)` —— **没有 owner**。无主窗口必然抢前台，
//! DShell 主窗口随即失焦；WebView2 把失焦窗口判定为「被遮挡的后台窗口」，
//! **挂起渲染进程**（实测 11.7 秒，期间页面 `Date.now()` 只走 261ms）。
//! 挂起期间事件分发与定时器全停，解冻后 React 也不提交那次更新，
//! 于是用户必须再点一下（实测 DOM 更新距那次点击仅 `1ms`）。
//!
//! dsh 自己的源码把这个因果写得很直白（`win32-dialog-host.js`）：
//!
//! > 子进程自己把对话框打开成前台：`runFolderDialog` 在 `Show` 之前**合成一次
//! > Alt 按下**，这在「后台宿主 spawn 子进程」时是必要的。
//!
//! **正因为没有 owner，才需要合成按键去抢前台。**
//!
//! ## 参考实现给出的答案
//!
//! 官方 DSH Desktop 没有这个问题，差别**不在 dsh 的后端，而在「谁弹这个框」**：
//!
//! ```js
//! // electron-shell-generation.ts
//! await dialog.showOpenDialog(window, options)   // ← window 作为 parent
//! ```
//!
//! Electron 是**自己进程内**弹框、并且**把主窗口设为 parent**。**本模块就是它的
//! Tauri 版**，逐项对齐：
//!
//! | | DSH Desktop（Electron） | DShell（本模块） |
//! | --- | --- | --- |
//! | 谁弹框 | 主进程自己 | Rust 侧（Tauri 进程自己） |
//! | parent | `showOpenDialog(window, …)` | `set_parent(&window)` → rfd `Show(hwnd)` |
//! | 通道 | HTTP 端点 | HTTP 端点（同一路径与响应体） |
//! | dsh 侧 | 打补丁让 browse 面板调端点 | 一个 dsh 插件替换 `ctx.directoryPicker` |
//!
//! ## dsh 侧是怎么接上的：一个普通插件
//!
//! 这一环是 R17 失败后最大的修正。之前想靠**页面注入**去改 dsh 的行为，
//! 但实测证明 `initialization_script` 与 `eval` **两条注入通道都是死的**
//! （脚本第一行的 `sendBeacon` 一次都没到）——页面里从来没有我们的代码。
//!
//! 正确的位置**不在页面里，而在 dsh 的 host 进程里**。dsh 把「怎么选目录」做成了
//! 一个 capability seam（`@deepseek-ai/dsh-host-directory-picker`），
//! 后端只是一个实现了 `capability()` 的类。于是：
//!
//! ```text
//! dsh host 进程                           DShell 进程（本模块）
//! ─────────────                           ────────────────────
//! dshell-directory-picker（本仓库的包）
//!   pick(signal)
//!     └── fetch POST /_dsh/desktop/pick-directory ──► 原生对话框（有 owner）
//!     ◄──────────── {"path": "..." | null} ─────────┘
//! ```
//!
//! 好处是**完全走在 dsh 官方的插件机制里**：不注入、不 hack、不改 dsh 源码，
//! dsh 自己会加载它。这也正是 DSH Desktop 的位置（它是 Electron 主进程，
//! 我们这里是 dsh host 进程 + Tauri 进程两个进程协作）。
//!
//! ## 为什么用 HTTP 而不是别的进程间通道
//!
//! 两端是**两个独立进程**。loopback HTTP 是这里最省事、最少权限的通道：
//! 不需要命名管道权限、不需要共享内存、不需要额外端口协商（端口由 DShell
//! 分配后经环境变量交给 dsh）。DSH Desktop 在"页面 → 主进程"那一跳用的也是
//! HTTP 端点。
//!
//! ## 安全性：这个端点谁能调用
//!
//! 端点只监听 `127.0.0.1`，且要求**共享令牌**（`x-dshell-token` 头）：
//!
//! | 检查 | 拒绝什么 |
//! | --- | --- |
//! | 令牌必须匹配本次启动生成的值 | 本机上别的进程扫到这个端口的误打误撞 |
//! | 只认 POST | 地址栏直达 / `<img src>` 之类 |
//! | 只认那两个路径 | 这个服务不代理任何别的东西 |
//!
//! **诚实说明它挡不住什么**：任何能读 dsh 进程环境的同用户进程都能拿到令牌。
//! 但那种进程本来就能读写这个用户的一切，这个端点不是它的瓶颈。令牌的定位是
//! **把端点认给启动它的那次 dsh、并挡掉误触**，不是对抗本机同权限攻击者。
//!
//! （R17 曾用 `Origin` 同源校验，那对"页面直连"是对的，但本模块的调用方是
//! **dsh 的 host 进程**——Node 的 `fetch` 不发 `Origin`，那个判据不成立。）

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Manager};

/// 与 DSH Desktop 完全一致的端点路径。保持一致是有意的：将来若 dsh 上游把
/// 这条约定收编，壳层不用改。
pub const PICK_PATH: &str = "/_dsh/desktop/pick-directory";

/// 本次运行的共享令牌（`DSHELL_PICKER_TOKEN` 交给 dsh，请求要带回来）。
static BRIDGE_TOKEN: Mutex<Option<String>> = Mutex::new(None);/// 桥接服务实际监听的端口（内核分配）。注入脚本要写死它，所以存起来。
static BRIDGE_PORT: AtomicU16 = AtomicU16::new(0);
/// 同一时刻只允许一个原生对话框：重复点击不应该叠出两个框。
static DIALOG_OPEN: AtomicU32 = AtomicU32::new(0);

/// 桥接端口；0 = 还没起。
pub fn port() -> u16 {
    BRIDGE_PORT.load(Ordering::SeqCst)
}

/// 本次运行的共享令牌（`DSHELL_PICKER_TOKEN` 交给 dsh，请求要带回来）。
pub fn auth_token() -> Option<String> {
    BRIDGE_TOKEN.lock().unwrap().clone()
}

/// 生成一个随机令牌。
///
/// 用途是**把端点认给启动它的那次 dsh**，不是做强认证（见 `handle` 里的说明）。
/// 用 `RandomState` 的哈希做熵源：标准库已经把它接在系统随机数上，
/// 不必为一个临时令牌引依赖。
fn fresh_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::with_capacity(32);
    for _ in 0..2 {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(BRIDGE_PORT.load(Ordering::SeqCst) as u64);
        out.push_str(&format!("{:016x}", hasher.finish()));
    }
    out
}

/// 起反代服务并返回它监听的端口。失败返回 `None`——**这个功能坏掉不该拦住启动**，
/// 只是退回 dsh 原本的挑框行为（能选，但要补点一下）。
pub fn start(app: AppHandle) -> Option<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    BRIDGE_PORT.store(port, Ordering::SeqCst);
    // 令牌在端口之后就位：`fresh_token` 把端口搅进熵里，两次启动不会撞。
    *BRIDGE_TOKEN.lock().unwrap() = Some(fresh_token());

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let app = app.clone();
            // 每个连接一个线程：原生对话框会阻塞住这个线程直到用户选完，
            // 不能占用 accept 循环（否则对话框开着的时候别的请求全堵住）。
            std::thread::spawn(move || {
                let _ = handle(stream, &app);
            });
        }
    });

    Some(port)
}

/// 读一个 HTTP 请求。只支持我们需要的形状：方法 + 路径 + 头部。
///
/// 本端点**不看请求体**：调用方（dsh 插件）只发一个带令牌的空 POST。
/// 所以这里读完头部就够了，不需要按 `Content-Length` 再收一段——
/// 少一段就少一处能出错的地方。
struct Request {
    method: String,
    path: String,
    /// 共享令牌（`x-dshell-token` 头）。
    token: Option<String>,
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Request> {
    // 原生对话框可能开着几十秒，读超时给宽一点，但不无限等。
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;

    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 2048];
    // 先读到头部结束（\r\n\r\n）
    let head_end = loop {
        if let Some(i) = find_double_crlf(&buf) {
            break i;
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection closed before headers",
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 64 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "headers too large",
            ));
        }
    };

    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();

    let mut token = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("x-dshell-token") {
            token = Some(value.trim().to_string());
        }
    }

    Ok(Request {
        method,
        path,
        token,
    })
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn write_json(stream: &mut TcpStream, status: u16, reason: &str, body: &str) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: application/json; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\
         \r\n{body}",
        body.as_bytes().len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn handle(mut stream: TcpStream, app: &AppHandle) -> std::io::Result<()> {
    let req = match read_request(&mut stream) {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };

    // 只接我们自己的路径；其它一律 404（这个服务不代理别的东西）。
    if req.path != PICK_PATH {
        return write_json(&mut stream, 404, "Not Found", r#"{"error":"not found"}"#);
    }

    if req.method != "POST" {
        return write_json(
            &mut stream,
            405,
            "Method Not Allowed",
            r#"{"error":"method not allowed"}"#,
        );
    }

    // ── 令牌校验 ─────────────────────────────────────────────────────
    //
    // 调用方是 **dsh 的 host 进程**（Node 里的 `fetch`），不是浏览器页面，
    // 所以这里**没有 `Origin` 头可用**——那个判据适用于页面直连的场景，
    // 对进程间调用不成立。
    //
    // 改用**共享令牌**：DShell 启动时随机生成一个，同时写进环境变量交给 dsh。
    // 这挡住的是"同一台机器上别的进程恰好发现了这个端口"——它们猜不到令牌。
    //
    // 诚实说明它挡不住什么：任何能读 dsh 进程环境的同用户进程都能拿到令牌。
    // 但那种进程本来就能直接读写用户的一切，这个端点不是它的瓶颈。
    // 令牌的定位是**防误触与降噪**，不是对抗本机上的同权限攻击者。
    let expected = auth_token();
    match (expected.as_deref(), req.token.as_deref()) {
        (Some(want), Some(got)) if want == got => {}
        _ => {
            crate::log(&format!(
                "picker: rejected request with a missing or wrong token (path={})",
                req.path
            ));
            return write_json(&mut stream, 403, "Forbidden", r#"{"error":"forbidden"}"#);
        }
    }

    pick_directory(&mut stream, app)
}

/// 弹原生目录选择框。**这是整个修复的核心**：
///
/// * `set_parent(&window)` —— 把 DShell 主窗口设为 owner，对应 Electron 的
///   `showOpenDialog(window, options)`。Windows 上它最终落到
///   `IFileDialog::Show(owner)`（rfd `dialog_ffi.rs:117`），有主的模态框
///   **不会让父窗口失去前台**，WebView2 也就不会挂起渲染进程。
/// * `blocking_pick_folder` —— 在**当前这个连接线程**上阻塞等待，主线程与
///   WebView2 UI 线程都不受影响（rfd 的 async 路径 `init_com` 会在自己起的
///   worker 线程上做 STA COM 初始化，见 `win_cid/file_dialog.rs`）。
fn pick_directory(stream: &mut TcpStream, app: &AppHandle) -> std::io::Result<()> {
    use tauri_plugin_dialog::DialogExt;

    // 防重入：用户连点两下不该叠出两个模态框。
    if DIALOG_OPEN.swap(1, Ordering::SeqCst) != 0 {
        crate::log("picker: ignored (a chooser is already open)");
        return write_json(stream, 200, "OK", r#"{"path":null}"#);
    }

    let started = std::time::Instant::now();
    crate::log("picker: opening native folder chooser (parented to the main window)");

    let picked = {
        // 主窗口必须先取出来：`set_parent` 借用它，builder 的链式调用要活到这个
        // 借用结束（窗口没了就没法设 owner，直接报错比弹一个无主框好）。
        let Some(window) = app.get_webview_window(crate::MAIN_WINDOW) else {
            DIALOG_OPEN.store(0, Ordering::SeqCst);
            crate::log("picker: main window is gone; cannot parent the chooser");
            return write_json(
                stream,
                500,
                "Internal Server Error",
                r#"{"error":"native directory picker failed"}"#,
            );
        };
        app.dialog()
            .file()
            .set_title("选择工作区目录")
            .set_parent(&window)
            .blocking_pick_folder()
    };

    DIALOG_OPEN.store(0, Ordering::SeqCst);

    let path = picked.and_then(|p| p.into_path().ok());
    let value = match &path {
        Some(p) => {
            crate::log(&format!(
                "picker: chose {} ({} ms)",
                p.display(),
                started.elapsed().as_millis()
            ));
            p.display().to_string()
        }
        None => {
            crate::log(&format!(
                "picker: cancelled ({} ms)",
                started.elapsed().as_millis()
            ));
            String::new()
        }
    };

    // 响应体与 DSH Desktop 的 `DesktopDirectoryPickerResponse` 逐字一致。
    let body = if path.is_some() {
        format!(r#"{{"path":{}}}"#, json_string(&value))
    } else {
        r#"{"path":null}"#.to_string()
    };
    write_json(stream, 200, "OK", &body)
}

/// 最小 JSON 字符串转义（路径里可能有反斜杠和引号）。
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
