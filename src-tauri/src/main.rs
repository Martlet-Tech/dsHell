// DShell — a minimal desktop container for the DeepSeek Harness Web GUI.
//
// 启动顺序（改造后）：
//   建窗 → ①WebView2 ②Node ③npm ④dsh ⑤profile 逐项体检（不因缺失中断）
//        → 统一汇报给启动页
//        → 全绿才 handoff：spawn `dsh web --port 0` → 抓 stdout 里的 token URL →
//          navigate → 换 cookie → 真 UI
//
// 缺失时停在报告页：用户可以对每一项单独「指定…」，或用底部按钮「一键安装 / 退出」。
// handoff 本身仍是原来的那套（design.md 里已验证过的链路），只是前提条件
// 从"立刻"变成了"体检全绿"。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod doctor;
mod installer;
mod proc;

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{
    AppHandle, Emitter, Manager, RunEvent, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

use config::Config;
use doctor::StepState;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 等 dsh 打印地址的上限（体检已经过了，这里只是真正的启动等待）。
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
/// 保留多少行 dsh stderr 用于失败诊断。
const STDERR_TAIL: usize = 12;
/// 页面多久没来叫体检就自己跑（兜底，避免页面出问题时卡在启动页）。
const PAGE_FALLBACK: Duration = Duration::from_millis(2500);

pub struct AppState {
    /// dsh 后端进程树；只记 pid —— `taskkill /PID <pid> /T /F` 不需要 Child 句柄，
    /// 这样退出清理不必和正在等待 URL 的线程抢锁。
    dsh_pid: AtomicU32,
    /// 正在跑的安装进程
    install_pid: AtomicU32,
    cancel: proc::Cancel,
    handoff_started: AtomicBool,
    doctor_requested: AtomicBool,
    /// DSHELL_SPLASH_HOLD：停在报告页不 handoff，方便截图/调动画
    hold: bool,
}

impl AppState {
    fn new(hold: bool) -> Self {
        Self {
            dsh_pid: AtomicU32::new(0),
            install_pid: AtomicU32::new(0),
            cancel: proc::Cancel::new(),
            handoff_started: AtomicBool::new(false),
            doctor_requested: AtomicBool::new(false),
            hold,
        }
    }
}

fn log_path() -> std::path::PathBuf {
    Config::dir().join("dshell-poc.log")
}

pub fn log(msg: &str) {
    eprintln!("{msg}");
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let exists = path.exists();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        // 首次写入先落 BOM，否则记事本会按 GBK 解码，中文日志成乱码。
        if !exists {
            let _ = f.write_all(&[0xEF, 0xBB, 0xBF]);
        }
        let _ = writeln!(f, "{msg}");
    }
}

/// 把文本编成 base64 再送进页面：穿过"JS 字符串字面量 → innerHTML"两层上下文时，
/// 手写转义很脆，base64 载荷在两种上下文里都是惰性的。
fn b64(s: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// 失败卡片里要插入 dsh 的原始输出，必须转义（这些文本不是我们自己写的）。
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn splash_status(window: &WebviewWindow, text: &str) {
    let _ = window.eval(&format!(
        "window.__dshell&&window.__dshell.statusB64('{}')",
        b64(text)
    ));
}

fn splash_fail(window: &WebviewWindow, html: &str) {
    let _ = window.eval(&format!(
        "window.__dshell&&window.__dshell.failB64('{}')",
        b64(html)
    ));
}

// ───────────────────────────── 体检编排 ─────────────────────────────

fn run_doctor(app: &AppHandle, window: &WebviewWindow, then_handoff: bool) {
    let cfg = Config::load();
    let steps = doctor::run_all(app, &cfg);
    let missing = doctor::blocking_ids(&steps);
    let auto = doctor::auto_ids(&steps);
    let all_ok = missing.is_empty();

    log(&format!(
        "doctor: allOk={all_ok} missing={missing:?} auto={auto:?}"
    ));

    let _ = app.emit(
        "doctor://done",
        serde_json::json!({ "allOk": all_ok, "missing": missing, "auto": auto }),
    );

    if all_ok && then_handoff {
        start_handoff(app.clone(), window.clone());
    }
}

// ───────────────────────────── handoff ─────────────────────────────

fn start_handoff(app: AppHandle, window: WebviewWindow) {
    {
        let state = app.state::<AppState>();
        if state.handoff_started.swap(true, Ordering::SeqCst) {
            return;
        }
    }

    let cfg = Config::load();
    let child = match spawn_dsh(&cfg) {
        Ok(c) => c,
        Err(e) => {
            log(&format!("FATAL: could not start `dsh web`: {e}"));
            let _ = app.emit(
                "doctor://step",
                doctor::launch_step(StepState::Failed, format!("无法启动 dsh：{e}")),
            );
            splash_fail(
                &window,
                &format!(
                    "<b>无法启动 dsh</b>\n\n{e}\n\n完整日志：<code>%USERPROFILE%\\.dshell\\dshell-poc.log</code>"
                ),
            );
            return;
        }
    };

    let pid = child.id();
    app.state::<AppState>().dsh_pid.store(pid, Ordering::SeqCst);
    log(&format!("spawned dsh web (pid {pid})"));
    let _ = app.emit(
        "doctor://step",
        doctor::launch_step(StepState::Checking, format!("已启动 dsh web（pid {pid}），等待地址")),
    );

    let err_tail: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));

    std::thread::spawn(move || {
        let mut child = child;
        match wait_for_url(&mut child, &window, err_tail) {
            Ok(url) => {
                log(&format!("launch url: {url}"));
                match url.parse::<tauri::Url>() {
                    Ok(parsed) => {
                        let _ = app.emit(
                            "doctor://step",
                            doctor::launch_step(StepState::Checking, "准备就绪，正在进入"),
                        );
                        splash_status(&window, "准备就绪，正在进入");
                        std::thread::sleep(Duration::from_millis(120));
                        let _ = window.eval("window.__dshell&&window.__dshell.leave()");
                        std::thread::sleep(Duration::from_millis(470));
                        let w = window.clone();
                        let _ = w.navigate(parsed);
                        log("window navigated to dsh");
                    }
                    Err(e) => {
                        log(&format!("FATAL: unparsable url ({e}): {url}"));
                        let _ = app.emit(
                            "doctor://step",
                            doctor::launch_step(StepState::Failed, "dsh 打印了无法解析的地址"),
                        );
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
                let _ = app.emit(
                    "doctor://step",
                    doctor::launch_step(StepState::Failed, "dsh 没有正常启动"),
                );
                splash_fail(&window, &e);
                // 窗口留着：release 没有控制台，失败卡片是用户唯一的线索
            }
        }
    });
}

/// 起 `dsh web`：隐藏窗口、stdout/stderr 都接管、PATH 里前置用户指定的目录
/// （否则"指定了 node 路径"这件事对 dsh 不起作用）。
fn spawn_dsh(cfg: &Config) -> std::io::Result<Child> {
    let mut cmd = Command::new("cmd");
    cmd.args(["/c", "dsh", "web", "--port", "0", "--no-open"])
        .env("PATH", cfg.child_path_env())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn()
}

/// 阻塞等到 dsh 打印 `dsh web: <url>`。
///
/// `--port 0` 让系统选端口、dsh 自己把真实端口打出来，所以不用协商端口。
/// stderr 现在**真的被读取**了：旧实现设成 piped 却从不读，既丢诊断信息，
/// 又可能把输出多的子进程堵死。
fn wait_for_url(
    child: &mut Child,
    window: &WebviewWindow,
    err_tail: Arc<Mutex<VecDeque<String>>>,
) -> Result<String, String> {
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => return Err("<b>无法读取 dsh 输出</b>\n\nstdout 没有被重定向。".into()),
    };

    if let Some(stderr) = child.stderr.take() {
        let tail = err_tail.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                log(&format!("dsh! {line}"));
                let mut t = tail.lock().unwrap();
                t.push_back(line);
                while t.len() > STDERR_TAIL {
                    t.pop_front();
                }
            }
        });
    }

    // stdout 交给 worker 排空；主线程只等截止时间，保持可响应
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
                    if !saw_output && line.trim().len() > 2 {
                        saw_output = true;
                        splash_status(window, "正在启动 DeepSeek Harness");
                    }
                    continue;
                };
                // 挡掉 "dsh web: opening the default browser..." 这种描述性行
                let Some(url) = rest.split_whitespace().next() else {
                    continue;
                };
                if url.starts_with("http") {
                    return Ok(url.to_string());
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(format!(
                    "{}{}",
                    concat!(
                        "<b>dsh 还没打印地址就退出了</b>\n\n",
                        "常见原因：\n",
                        "· dsh 的 profile 目录 <code>%USERPROFILE%\\.dsh</code> 不可写\n",
                        "· 杀毒软件 / 安全策略拦住了 Node.js 监听本地端口\n\n",
                        "直接在终端里跑 <code>dsh web</code> 可以看到原始报错。"
                    ),
                    err_tail_html(&err_tail)
                ));
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

fn err_tail_html(tail: &Arc<Mutex<VecDeque<String>>>) -> String {
    let t = tail.lock().unwrap();
    if t.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n\n<b>dsh 的原始输出：</b>\n");
    for l in t.iter() {
        s.push_str(&format!("· {}\n", esc(l)));
    }
    s
}

// ───────────────────────────── 前端命令 ─────────────────────────────

/// 页面加载完注册好监听后主动叫一次（这样事件不会丢）；也是「重新检查」。
#[tauri::command]
async fn doctor_run(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let hold = app.state::<AppState>().hold;
        app.state::<AppState>()
            .doctor_requested
            .store(true, Ordering::SeqCst);
        if let Some(w) = app.get_webview_window("main") {
            run_doctor(&app, &w, !hold);
        }
    })
    .await
    .map_err(|e| e.to_string())
}

/// 「指定…」：就地把路径验证一次，通过才落盘。
#[tauri::command]
async fn doctor_set_path(app: AppHandle, id: String, path: String) -> Result<doctor::Step, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<doctor::Step, String> {
        let p = proc::validate_user_path(&path)?;
        let step = doctor::probe_with_path(&id, &p)?;
        if step.state == StepState::Ok {
            let mut cfg = Config::load();
            cfg.set(&id, &p);
            if let Err(e) = cfg.save() {
                log(&format!("config save failed: {e}"));
            }
            log(&format!("doctor: {} -> {}", id, p.display()));
        }
        let _ = app.emit("doctor://step", &step);
        Ok(step)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 「一键安装」：队列在 Rust 侧决定（只装官方通道的 node / dsh），前端不参与。
#[tauri::command]
async fn install_missing(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut cfg = Config::load();
        let steps = doctor::run_all(&app, &cfg);
        let mut queue: Vec<installer::Kind> = Vec::new();
        for id in doctor::auto_ids(&steps) {
            match id {
                "node" => queue.push(installer::Kind::Node),
                "dsh" => queue.push(installer::Kind::Dsh),
                _ => {}
            }
        }

        if queue.is_empty() {
            log("install: nothing to do");
            if let Some(w) = app.get_webview_window("main") {
                run_doctor(&app, &w, true);
            }
            return;
        }

        for kind in queue {
            let outcome = {
                let state = app.state::<AppState>();
                installer::run(&app, kind, &state)
            };
            let _ = app.emit(
                "install://done",
                serde_json::json!({
                    "kind": kind.id(),
                    "ok": outcome.ok,
                    "cancelled": outcome.cancelled,
                    "error": outcome.error,
                }),
            );

            cfg = Config::load();
            if !outcome.ok {
                // 失败就停手，并把该步刷新成最新状态；剩余项等用户决定
                doctor::reprobe(&app, &cfg, kind.id());
                return;
            }

            // 装完不能靠 where 复查（PATH 不会自动更新），按已知落点回填配置
            installer::locate_and_remember(kind, &mut cfg);
            cfg = Config::load();
            match kind {
                // Node 装完，npm 通常是跟着来的，一起复检
                installer::Kind::Node => {
                    doctor::reprobe(&app, &cfg, "node");
                    doctor::reprobe(&app, &cfg, "npm");
                }
                installer::Kind::Dsh => {
                    doctor::reprobe(&app, &cfg, "dsh");
                }
            }
        }

        // 队列跑完 → 重跑整轮，全绿则自动进入 handoff（不用用户再点一次）
        if let Some(w) = app.get_webview_window("main") {
            run_doctor(&app, &w, true);
        }
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn install_cancel(app: AppHandle) {
    let state = app.state::<AppState>();
    state.cancel.request();
    let pid = state.install_pid.load(Ordering::SeqCst);
    log(&format!("install: cancel requested (pid {pid})"));
    if pid != 0 {
        proc::kill_tree(pid);
    }
}

/// 逃生口：不体检直接启动（页面角落里那个低调的链接）。
#[tauri::command]
async fn app_skip_doctor(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(w) = app.get_webview_window("main") {
            start_handoff(app.clone(), w);
        }
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn app_quit(app: AppHandle) {
    log("user requested quit");
    app.exit(0);
}

// ───────────────────────────── main ─────────────────────────────

fn main() {
    let hold = std::env::var("DSHELL_SPLASH_HOLD").is_ok();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            doctor_run,
            doctor_set_path,
            install_missing,
            install_cancel,
            app_skip_doctor,
            app_quit
        ])
        .setup(move |app| {
            // 先把启动页弹出来：它是本地页面，瞬时绘制、不会 401，
            // 体检期间屏幕上一直是它（而不是空白）。
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("DShell — DeepSeek Harness")
                .inner_size(1280.0, 860.0)
                .min_inner_size(760.0, 560.0)
                .center()
                // 只放行两个 origin：本地启动页和 loopback 上的 dsh。
                // 其余一律交给系统浏览器，否则点个引用链接就把壳顶掉且回不来。
                .on_navigation(|url| {
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
            app.manage(AppState::new(hold));

            let handle = app.handle().clone();
            let w = window.clone();
            std::thread::spawn(move || {
                // 正常路径：页面注册好监听后自己 invoke('doctor_run')。
                // 这里只兜底，免得页面出问题时永远停在启动页。
                std::thread::sleep(PAGE_FALLBACK);
                if handle
                    .state::<AppState>()
                    .doctor_requested
                    .load(Ordering::SeqCst)
                {
                    return;
                }
                log("doctor: page did not ask within 2.5s - running anyway (fallback)");
                run_doctor(&handle, &w, !hold);
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build the Tauri application")
        .run(|app, event| {
            // 关窗即收掉整棵进程树，任何退出路径都一样
            if let RunEvent::ExitRequested { .. } | RunEvent::Exit = event {
                if let Some(state) = app.try_state::<AppState>() {
                    for (what, pid) in [
                        ("dsh", state.dsh_pid.load(Ordering::SeqCst)),
                        ("install", state.install_pid.load(Ordering::SeqCst)),
                    ] {
                        if pid != 0 {
                            log(&format!("shutting down: killing the {what} process tree (pid {pid})"));
                            proc::kill_tree(pid);
                        }
                    }
                }
            }
        });
}

/// 外链丢给系统浏览器（`cmd` 的 `start`，不引额外 crate）。
fn open_in_browser(url: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        // 那个空 "" 是 `start` 会当成窗口标题吃掉的参数位
        cmd.args(["/c", "start", "", url])
            .stdin(Stdio::null())
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
