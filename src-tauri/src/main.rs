// DShell POC — a minimal desktop container for the DeepSeek Harness Web GUI.
//
// The whole trick, in four steps:
//   1. spawn `dsh web --port 0 --no-open` with no console window
//   2. read its stdout until it announces `dsh web: http://127.0.0.1:PORT/?token=...`
//   3. open a WebView pointed at THAT url (the 303 mints the session cookie that
//      a cold request to a bare `/` would be denied with a 401)
//   4. kill the whole process tree on exit, so no orphan node keeps the port
//
// Deliberately no Tauri IPC, no capabilities, no frontend: the DSH UI talks only
// to its own backend over HTTP/WebSocket, so the shell stays a shell.

// Hide the console window in release builds; keep it in dev for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// How long we wait for dsh to announce its URL before giving up.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

/// The dsh child process, kept so we can tear it down with the window.
struct DshProcess(Mutex<Option<Child>>);

fn log_path() -> std::path::PathBuf {
    let home = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home).join(".dshell").join("dshell-poc.log")
}

fn log(msg: &str) {
    eprintln!("{msg}");
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let exists = path.exists();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write;
        // Prepending a UTF-8 BOM on first write keeps Notepad from decoding the
        // Chinese failure text as GBK (which renders as mojibake).
        if !exists {
            let _ = f.write_all(&[0xEF, 0xBB, 0xBF]);
        }
        let _ = writeln!(f, "{msg}");
    }
}

/// Start `dsh web` hidden, with stdout piped so we can read the launch URL.
fn spawn_dsh() -> std::io::Result<Child> {
    let mut cmd = Command::new("cmd");
    cmd.args(["/c", "dsh", "web", "--port", "0", "--no-open"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn()
}

/// Block until dsh prints `dsh web: <url>`, and return that url.
///
/// `--port 0` means the OS picks the port, and dsh prints the real one, so we
/// never have to negotiate ports ourselves. Progress is mirrored onto the splash
/// so the wait is never a blank screen, and failures come back as HTML the splash
/// can render (release builds have no console to print to).
fn wait_for_url(child: &mut Child, window: &tauri::WebviewWindow) -> Result<String, String> {
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => return Err("<b>无法读取 dsh 输出</b>\n\nstdout 没有被重定向。".into()),
    };

    // Drain stdout on a worker thread; the main thread stays responsive to the deadline.
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + STARTUP_TIMEOUT;
    let mut saw_output = false;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(remaining) {
            Ok(line) => {
                log(&format!("dsh> {line}"));
                let Some(rest) = line.strip_prefix("dsh web: ") else {
                    // dsh chatters while booting; note that it is alive so a slow
                    // start reads as "working", not "hung".
                    if !saw_output && line.trim().len() > 2 {
                        saw_output = true;
                        splash_status(window, "正在启动 DeepSeek Harness");
                    }
                    continue;
                };
                // Guard against the informational "dsh web: opening the default browser..." line.
                let Some(url) = rest.split_whitespace().next() else {
                    continue;
                };
                if url.starts_with("http") {
                    return Ok(url.to_string());
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(concat!(
                    "<b>dsh 还没打印地址就退出了</b>\n\n",
                    "常见原因：\n",
                    "· 没有安装 Node.js 或 <code>@deepseek-ai/dsh</code>，或不在 PATH 上\n",
                    "· dsh 无法写入自己的 profile 目录 <code>%USERPROFILE%\\.dsh</code>\n\n",
                    "直接在终端里跑 <code>dsh web</code> 可以看到它的原始报错。"
                )
                .into());
            }
        }
    }

    let _ = saw_output;
    Err(format!(
        concat!(
            "<b>等待 dsh 启动超时（{} 秒）</b>\n\n",
            "它在超时前没有打印出监听地址。可以试试：\n",
            "· 在终端里手动跑 <code>dsh web</code>，确认它自己能起来\n",
            "· 检查杀毒软件 / 安全策略是否拦住了 Node.js 监听本地端口\n\n",
            "完整日志：<code>%USERPROFILE%\\.dshell\\dshell-poc.log</code>"
        ),
        STARTUP_TIMEOUT.as_secs()
    ))
}

/// Kill the child *and its descendants*; `cmd` -> `node`, so killing only the
/// direct child would leave node holding the port.
fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
        match cmd.status() {
            Ok(s) => log(&format!("taskkill /PID {pid} /T /F -> {s}")),
            Err(e) => log(&format!("taskkill failed: {e}")),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
    }
}

/// Encode text for transport into the splash page.
///
/// The text crosses two nested contexts -- a JS string literal, then innerHTML --
/// so hand-rolled escaping is bug-prone (a quote or `<` in a dsh error message
/// would break out). Base64 sidesteps both: the payload contains only
/// `[A-Za-z0-9+/=]`, which is inert in a JS string, and the page decodes it to
/// UTF-8 before use. Newlines are preserved exactly, so the failure card keeps
/// its line breaks without any `<br>` juggling.
fn b64(s: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Push a status line into the splash page (plain text, never markup).
fn splash_status(window: &tauri::WebviewWindow, text: &str) {
    let _ = window.eval(&format!(
        "window.__dshell&&window.__dshell.statusB64('{}')",
        b64(text)
    ));
}

/// Turn the splash into a readable failure card (release has no console).
///
/// `html` is authored as HTML in this file, not taken from user input, so it is
/// decoded and used as markup directly.
fn splash_fail(window: &tauri::WebviewWindow, html: &str) {
    let _ = window.eval(&format!(
        "window.__dshell&&window.__dshell.failB64('{}')",
        b64(html)
    ));
}

fn main() {
    let child = match spawn_dsh() {
        Ok(c) => c,
        Err(e) => {
            log(&format!("FATAL: could not start `dsh web`: {e}"));
            std::process::exit(1);
        }
    };
    let pid = child.id();
    log(&format!("spawned dsh web (pid {pid})"));

    tauri::Builder::default()
        .setup(move |app| {
            // Show the animated splash FIRST, so the wait for dsh is never a blank
            // screen. It is a local page, so it paints instantly and never 401s.
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("DShell — DeepSeek Harness")
                .inner_size(1280.0, 860.0)
                .min_inner_size(720.0, 520.0)
                .center()
                // The shell hosts two origins: the local splash (tauri://) and the
                // dsh server on loopback. Anything else is an external link — hand it
                // to the system browser, or clicking a citation would navigate the
                // shell off DSH with no way back.
                .on_navigation(|url| {
                    // Tauri serves the bundled splash from http://tauri.localhost on
                    // Windows, so the internal origin has to be allowed explicitly --
                    // otherwise the splash itself gets punted to the system browser.
                    let internal = matches!(url.host_str(), Some("tauri.localhost"))
                        || matches!(url.scheme(), "tauri" | "asset" | "about" | "data");
                    let dsh = matches!(url.host_str(), Some("127.0.0.1") | Some("localhost"));
                    if !internal && !dsh {
                        log(&format!("external link -> system browser: {url}"));
                        let _ = open_in_browser(url.as_str());
                    }
                    internal || dsh
                })
                .build()?;

            log("splash window built");

            app.manage(DshProcess(Mutex::new(Some(child))));
            let handle = app.handle().clone();

            // Debug hook: DSHELL_SPLASH_HOLD=1 keeps the app parked on the splash
            // and skips the dsh handoff, so the animation can be inspected (or
            // screenshotted) without racing the startup.
            if std::env::var("DSHELL_SPLASH_HOLD").is_ok() {
                log("DEBUG: DSHELL_SPLASH_HOLD set - holding on splash, skipping handoff");
                return Ok(());
            }

            // Everything that blocks lives on a worker thread; setup returns at once
            // so the window is on screen while dsh boots.
            std::thread::spawn(move || {
                let state = handle.state::<DshProcess>();
                let url = {
                    let mut guard = state.0.lock().unwrap();
                    let Some(child) = guard.as_mut() else { return };
                    wait_for_url(child, &window)
                };

                match url {
                    Ok(url) => {
                        log(&format!("launch url: {url}"));
                        match url.parse::<tauri::Url>() {
                            Ok(parsed) => {
                                // Let the splash fade out, then hand the window over.
                                let w = window.clone();
                                let _ = window.eval(
                                    "window.__dshell&&window.__dshell.status('准备就绪，正在进入')",
                                );
                                std::thread::sleep(Duration::from_millis(120));
                                let _ = window.eval("window.__dshell&&window.__dshell.leave()");
                                std::thread::sleep(Duration::from_millis(470));
                                let _ = w.navigate(parsed);
                                log("window navigated to dsh");
                            }
                            Err(e) => {
                                log(&format!("FATAL: unparsable url ({e}): {url}"));
                                splash_fail(
                                    &window,
                                    concat!(
                                        "<b>dsh 打印了无法解析的地址</b>\n\n",
                                        "请检查 dsh 版本，或在日志中查看原始输出。"
                                    ),
                                );
                            }
                        }
                    }
                    Err(e) => {
                        log(&format!("FATAL: {e}"));
                        splash_fail(&window, &e);
                        // Leave the window up: the failure card is the only UI the
                        // user gets in a release build with no console.
                    }
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build the Tauri application")
        .run(move |app, event| {
            // Tear the server down with the UI, on any exit path.
            if let RunEvent::ExitRequested { .. } | RunEvent::Exit = event {
                if let Some(state) = app.try_state::<DshProcess>() {
                    if let Ok(mut guard) = state.0.lock() {
                        if let Some(mut c) = guard.take() {
                            log("shutting down: killing the dsh process tree");
                            kill_tree(c.id());
                            let _ = c.wait();
                        }
                    }
                }
            }
        });
}

/// Hand a url to the default browser via cmd's `start` (no extra crate needed).
fn open_in_browser(url: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        // The empty "" is the window title `start` would otherwise eat as the url.
        cmd.args(["/c", "start", "", url])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd.spawn().map(|_| ())
    }
    #[cfg(not(windows))]
    {
        Command::new("xdg-open").arg(url).spawn().map(|_| ())
    }
}
