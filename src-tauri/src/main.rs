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

mod auth_cookie;
mod close_dialog;
mod config;
mod doctor;
mod installer;
mod lifecycle;
mod picker;
mod plugin;
mod proc;
mod settings;
mod tray;
mod ui_text;
mod update;
mod win32;

/// 主窗口 label —— 全项目唯一来源，避免 "main" 字符串散落各处。
pub const MAIN_WINDOW: &str = "main";

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
/// 重启时等旧 dsh 真正退出的上限。
///
/// 为什么不无限等：卡死的 dsh 会让「重启」永远不返回，而**不重启**比
/// 冒险起第二个后端更安全 —— 两个后端会抢同一份会话锁。所以超时后放弃并说明。
const DSH_EXIT_TIMEOUT: Duration = Duration::from_secs(15);
/// 导航回启动页后，等它加载完并注册好事件监听的时间。
const SPLASH_SETTLE: Duration = Duration::from_millis(700);
/// 第二个实例找已有窗口的等待上限（覆盖"用户手快双击"的窗口创建期）。
const FOCUS_TIMEOUT: Duration = Duration::from_secs(5);
/// 等 auth cookie 清理收尾的上限。
///
/// `WebviewWindow::cookies()` 是「发消息给 webview 线程 + 等回调」的同步调用，
/// 卡住的话界面就永远到不了 dsh —— 那比多攒一条 cookie 严重得多。超时就照常导航。
const PURGE_TIMEOUT: Duration = Duration::from_secs(2);

pub struct AppState {
    /// dsh 后端进程树；只记 pid —— `taskkill /PID <pid> /T /F` 不需要 Child 句柄，
    /// 这样退出清理不必和正在等待 URL 的线程抢锁。
    dsh_pid: AtomicU32,
    /// 正在跑的安装进程
    install_pid: AtomicU32,
    cancel: proc::Cancel,
    /// 「已经在起 dsh 了」的闩。**重启时必须复位**，否则第二次 start_handoff 直接早退。
    handoff_started: AtomicBool,
    /// dsh 的代数，每起一次 +1。
    ///
    /// 重启会让上一代的等待线程"过期"：它等的那个进程被杀掉后，`wait_for_url`
    /// 会以"dsh 还没打印地址就退出了"收场 —— 那是**上一代的正常死亡**，不是故障。
    /// 没有这个代号，重启时就会往刚回到启动页的窗口上画一张假的失败卡片。
    dsh_generation: AtomicU32,
    /// 重启正在进行的闩：托盘连点两下不该起两条重启线程。
    restarting: AtomicBool,
    /// 重启期间抑制启动页自己发起的体检。
    ///
    /// 重启会把窗口导航回启动页，而启动页**加载完就自动 invoke `doctor_run`**
    /// （那是首启的设计：监听就绪后再叫，事件不会丢）。如果在旧 dsh 还没停干净时
    /// 放它跑，它会一路 handoff 出**第二个 dsh 后端** —— 正是重启最不能出的结果。
    /// 所以这期间让 `doctor_run` 直接返回，由重启流程自己在停干净之后叫一次。
    restart_guard: AtomicBool,
    doctor_requested: AtomicBool,
    /// DSHELL_SPLASH_HOLD：停在报告页不 handoff，方便截图/调动画
    hold: bool,
    /// dsh 的入口地址（含 token）。handoff 成功后写入，托盘「在浏览器中打开」读它。
    launch_url: Mutex<String>,
    /// 启动页自己的 URL。重启时要导航回来，而它的形状由 Tauri 决定
    /// （Windows 上是 `http://tauri.localhost/index.html`），所以建窗后实测一次存下来，
    /// 不硬编码。
    splash_url: Mutex<String>,
}

impl AppState {
    fn new(hold: bool) -> Self {
        Self {
            dsh_pid: AtomicU32::new(0),
            install_pid: AtomicU32::new(0),
            cancel: proc::Cancel::new(),
            handoff_started: AtomicBool::new(false),
            dsh_generation: AtomicU32::new(0),
            restarting: AtomicBool::new(false),
            restart_guard: AtomicBool::new(false),
            doctor_requested: AtomicBool::new(false),
            hold,
            launch_url: Mutex::new(String::new()),
            splash_url: Mutex::new(String::new()),
        }
    }

    pub fn set_launch_url(&self, url: &str) {
        if let Ok(mut u) = self.launch_url.lock() {
            *u = url.to_string();
        }
    }

    /// 还没 handoff 成功时返回 None（启动页阶段）。
    pub fn launch_url(&self) -> Option<String> {
        let u = self.launch_url.lock().ok()?;
        (!u.is_empty()).then(|| u.clone())
    }

    pub fn set_splash_url(&self, url: &str) {
        if let Ok(mut u) = self.splash_url.lock() {
            *u = url.to_string();
        }
    }

    pub fn splash_url(&self) -> Option<String> {
        let u = self.splash_url.lock().ok()?;
        (!u.is_empty()).then(|| u.clone())
    }

    /// 领一个新代号，表示"这一代 dsh 由我负责"。
    fn begin_dsh_generation(&self) -> u32 {
        self.dsh_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// 这个代号还是当前代吗？不是就说明它等的那次 handoff 已经被重启取代。
    fn is_current_generation(&self, generation: u32) -> bool {
        self.dsh_generation.load(Ordering::SeqCst) == generation
    }

    /// 当前 dsh 后端的 pid（0 = 没有）。
    pub fn dsh_pid(&self) -> u32 {
        self.dsh_pid.load(Ordering::SeqCst)
    }

    fn set_dsh_pid(&self, pid: u32) {
        self.dsh_pid.store(pid, Ordering::SeqCst);
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
///
/// `pub(crate)`：`settings.rs` 注入 shim 与推数据时用的是同一条约定。
pub(crate) fn b64(s: &str) -> String {
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

/// 起 dsh 并把窗口导航到它。
///
/// **可重入**：重启走的也是这条路径（先 `stop_dsh`，再调它）。所以这里不再
/// 依赖"只跑一次"的假设，改由 `dsh_generation` 把并发的、过期的等待线程分流掉。
fn start_handoff(app: AppHandle, window: WebviewWindow) {
    let generation = {
        let state = app.state::<AppState>();
        if state.handoff_started.swap(true, Ordering::SeqCst) {
            // 已经在起 dsh：不重复 spawn。
            return;
        }
        state.begin_dsh_generation()
    };

    let cfg = Config::load();
    let child = match spawn_dsh(&cfg) {
        Ok(c) => c,
        Err(e) => {
            app.state::<AppState>()
                .handoff_started
                .store(false, Ordering::SeqCst);
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
    app.state::<AppState>().set_dsh_pid(pid);
    log(&format!("spawned dsh web (pid {pid}, generation {generation})"));

    let _ = app.emit(
        "doctor://step",
        doctor::launch_step(StepState::Checking, format!("已启动 dsh web（pid {pid}），等待地址")),
    );

    let err_tail: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));

    // 清陈旧 auth cookie 坐在哪一步，是权衡过的：
    //
    // - **不是**接在拿到 URL 之后。那种写法里，清理由 `wait_for_url` 的内部
    //   `w.show()` 兜底，而两者都是「同步调 webview 线程」的调用；万一 show 卡住，
    //   后面整段都到不了，超时守卫也守不住它前头的东西。
    // - 放在 spawn 之后、等 URL 之前：此刻新端口刚定，上一代 cookie 已经确定作废，
    //   而窗口还停在本地启动页 —— 没有任何请求会撞上 431，可以安心慢等。
    //   清理与 dsh 启动**并行**，不占启动时间。
    let purge_rx = {
        let (tx, rx) = mpsc::channel();
        let w = window.clone();
        std::thread::spawn(move || {
            match auth_cookie::purge_stale(&w) {
                // 没什么可清是常态（刚清过整个目录后），别刷日志。
                Ok(0) => {}
                Ok(n) => log(&format!("auth cookie: purged {n} stale")),
                Err(e) => log(&format!("auth cookie: purge skipped ({e})")),
            }
            let _ = tx.send(());
        });
        rx
    };

    std::thread::spawn(move || {
        let mut child = child;
        match wait_for_url(&mut child, &window, err_tail) {
            Ok(url) => {
                // 等到了地址，但这一代可能已经被重启取代（重启时旧进程会被杀，
                // 而旧进程被杀正好会让 wait_for_url 返回错误而不是成功 —— 这里
                // 只是把"恰好成功但已过期"这条窄缝也堵上）。
                if !app.state::<AppState>().is_current_generation(generation) {
                    log(&format!(
                        "handoff: generation {generation} superseded - ignoring its launch url"
                    ));
                    return;
                }
                log(&format!("launch url: {url}"));
                match url.parse::<tauri::Url>() {
                    Ok(parsed) => {
                        app.state::<AppState>().set_launch_url(parsed.as_str());
                        let _ = app.emit(
                            "doctor://step",
                            doctor::launch_step(StepState::Checking, "准备就绪，正在进入"),
                        );
                        splash_status(&window, "准备就绪，正在进入");
                        std::thread::sleep(Duration::from_millis(120));
                        let _ = window.eval("window.__dshell&&window.__dshell.leave()");
                        std::thread::sleep(Duration::from_millis(470));
                        // 清理是并行做的，这里只等它收尾。**带超时**：cookies() 是同步
                        // 阻塞调用，卡住不该让界面永远到不了 dsh —— 那样比多攒一条 cookie
                        // 严重得多。正常几毫秒，超时就照常导航。
                        if purge_rx.recv_timeout(PURGE_TIMEOUT).is_err() {
                            log(&format!(
                                "auth cookie: purge exceeded {}s - navigating anyway",
                                PURGE_TIMEOUT.as_secs()
                            ));
                        }
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
                // 重启时旧进程被杀 → 这里必然报错。那不是故障，是上一代的正常收场。
                if !app.state::<AppState>().is_current_generation(generation) {
                    log(&format!(
                        "handoff: generation {generation} superseded - its dsh exited as expected"
                    ));
                    return;
                }
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

/// 把窗口导航回启动页，并清掉上一轮的残留（失败卡片 / 步骤状态 / 安装面板）。
///
/// 返回值**只表示"知不知道启动页地址"**，不表示导航成功与否：
///
///   * `false` = 不知道启动页地址。这是**必须放弃**的情况 —— 后续的进度与失败
///     卡片都会发给一个没有监听者的页面，用户只会看到界面莫名其妙地坏掉
///     （08 记的 `about:blank` 就是这一类）。
///   * 导航本身失败（parse 不了 / navigate 报错）只记日志并返回 `true` 继续 ——
///     与拆分前一致：那时也是"日志一下，接着停 dsh"。
///
/// 拆出来的原因见 `stop_dsh` 的说明：重启与更新前半段完全相同，只有中段不同。
fn show_splash(app: &AppHandle, window: &WebviewWindow) -> bool {
    let Some(splash) = app.state::<AppState>().splash_url() else {
        log("splash: url unknown - cannot show the splash page");
        return false;
    };
    match splash.parse::<tauri::Url>() {
        Ok(parsed) => match window.navigate(parsed) {
            Ok(()) => {
                log("splash: navigated back to the splash page");
                // 让启动页完成加载与事件注册：dsh 启动失败时那条 `doctor://step`
                // 不能丢在没人听的窗口上。
                std::thread::sleep(SPLASH_SETTLE);
                let _ = window.eval("window.__dshell&&window.__dshell.restarting()");
            }
            Err(e) => log(&format!("splash: navigate failed: {e}")),
        },
        Err(e) => log(&format!("splash: url unparsable ({e}): {splash}")),
    }
    true
}

/// 停掉当前 dsh，并把 handoff 状态复位到「可以再来一次」。
///
/// **这是"停"这一半的独立原语**，重启与更新共用：
///
/// ```text
/// restart_dsh = show_splash + stop_dsh + start_dsh
/// update_dsh  = show_splash + stop_dsh + install(dsh) + start_dsh
/// ```
///
/// 顺序是有讲究的，每一步都在挡一个具体的失败：
///
///   1. **先作废当前代号** —— 通知还在等地址的那个线程"你已经是上一代了"，
///      它会安静退出，而不是往重新显示的启动页上画一张假的失败卡片。
///   2. `kill_tree` —— cmd → npm → node，只杀直接子进程会留下占着端口的 node。
///   3. **`wait_process_exit`** —— `taskkill` 返回 0 只证明"信号发出去了"。
///      必须等它真的没了，否则新起的 dsh 会撞上旧后端持有的会话锁
///      （`SessionAlreadyOwnedError`，见 roadmap #1）；更新时更要等，因为
///      `npm i -g` 要覆盖的文件正被那个进程锁着。
///   4. 复位 `handoff_started`，下一个 `start_handoff` 才不会被闩挡住。
fn stop_dsh(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();

    // 1) 作废当前代号：旧等待线程从这一刻起只是"过期的旁观者"。
    state.dsh_generation.fetch_add(1, Ordering::SeqCst);

    let pid = state.dsh_pid();
    state.set_dsh_pid(0);
    // URL 里的 token 绑在**那一个** dsh 进程上，它死了这个地址就没意义了。
    // 不清掉的话，托盘「在浏览器中打开」会打开一个 401 的死页面。
    if let Ok(mut u) = state.launch_url.lock() {
        u.clear();
    }

    // 4 的铺垫：先允许下一次 handoff，再去做可能耗时的收尾。
    state.handoff_started.store(false, Ordering::SeqCst);

    if pid == 0 {
        log("stop dsh: no dsh process to stop");
        return true;
    }

    // 2) + 3)
    log(&format!("stop dsh: killing dsh process tree (pid {pid})"));
    proc::kill_tree(pid);
    let exited = win32::wait_process_exit(pid, DSH_EXIT_TIMEOUT);
    if exited {
        log(&format!("stop dsh: dsh (pid {pid}) exited"));
    } else {
        log(&format!(
            "stop dsh: dsh (pid {pid}) still alive after {}s",
            DSH_EXIT_TIMEOUT.as_secs()
        ));
    }
    exited
}

/// 起 dsh：跑一遍体检，全绿则 handoff。**"起"这一半的独立原语。**
///
/// 为什么走完整体检而不是直接 `start_handoff`：用户点「重启」最常见的动机就是
/// "我改了什么，让它按新的来"（改了 dsh 路径、装了插件、更新了版本）。只重起进程
/// 会跳过体检，把"改了路径却没生效"这类问题留到后面才以更难懂的形式暴露。
/// 体检本身是秒级的。
fn start_dsh(app: &AppHandle, window: &WebviewWindow) {
    run_doctor(app, window, !app.state::<AppState>().hold);
}

/// 重启 dsh = `show_splash` + `stop_dsh` + `start_dsh`。
///
/// 三个原语是分开的，因为**更新只需要换掉中间那一段**：
///
/// ```text
/// restart_dsh = show_splash + stop_dsh + start_dsh
/// update_dsh  = show_splash + stop_dsh + install(dsh) + start_dsh
/// ```
///
/// 所以这里刻意不把"停"和"起"再揉在一起 —— 揉在一起就会变成
/// "重启 / 停 / 起"三层，而那正是要避免的重复（见 docs/plan/09）。
///
/// 为什么走完整流程（体检 + handoff）而不是只重起进程：用户点「重启」最常见的
/// 动机就是"我改了什么，让它按新的来"（改了 dsh 路径、装了插件、更新了版本）。
/// 只重起进程会跳过体检，把"改了路径却没生效"这类问题留到后面才以更难懂的形式
/// 暴露。体检本身是秒级的。
fn restart_dsh(app: &AppHandle) {
    {
        let state = app.state::<AppState>();
        // 托盘连点两下不该起两条重启线程互相杀
        if state.restarting.swap(true, Ordering::SeqCst) {
            log("restart: already in progress - ignoring");
            return;
        }
    }

    let app = app.clone();
    std::thread::spawn(move || {
        // 闩的清除放在 `catch_unwind` **之后**：`do_restart_dsh` 里一次 panic
        // 不该让 `restarting` 永远为真（那会让重启从此静默失效）。
        //
        // 注意 `[profile.release] panic = "abort"`：发布版里 panic 直接终止进程，
        // 这个兜底其实只在 debug（unwind）下生效。发布版里进程都没了，闩自然无所谓。
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            do_restart_dsh(&app);
        }));
        if result.is_err() {
            log("restart: panicked - the restarting latch is being cleared");
        }
        app.state::<AppState>().restarting.store(false, Ordering::SeqCst);
    });
}

fn do_restart_dsh(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        log("restart: main window is gone - aborting");
        return;
    };

    // 从现在到"旧 dsh 停干净"之间，不许启动页自己发起体检（见 restart_guard 的说明）。
    app.state::<AppState>()
        .restart_guard
        .store(true, Ordering::SeqCst);

    // 先把窗口从 dsh 页面弄回启动页，用户才有地方看进度。
    //
    // 拿不到启动页 URL 就**放弃重启**，而不是"留在当前页继续"。因为当前页要么是
    // dsh（它马上会被杀掉 → 白屏或错误页），要么是空白页；两种情况下进度和失败卡片
    // 都没有监听者，用户只会看到界面莫名其妙地坏掉。
    if !show_splash(app, &window) {
        log("restart: aborting (the window would have nowhere to show progress)");
        app.state::<AppState>()
            .restart_guard
            .store(false, Ordering::SeqCst);
        splash_fail(
            &window,
            concat!(
                "<b>无法重启</b>\n\n",
                "没有记下启动页地址，重启后界面会无处显示进度。\n\n",
                "请「完全退出」DShell 再重新打开。"
            ),
        );
        return;
    }

    // 窗口可能缩在托盘里：把界面亮出来，否则用户看不到任何反应。
    lifecycle::restore_main_window(app);

    let stopped = stop_dsh(app);

    // 无论成败都要放行体检通道，否则失败卡片上的「重新检查」会变成死按钮。
    app.state::<AppState>()
        .restart_guard
        .store(false, Ordering::SeqCst);

    if !stopped {
        let _ = app.emit(
            "doctor://step",
            doctor::launch_step(
                StepState::Warn,
                "旧 dsh 没有及时退出，重启已放弃（避免两个后端抢同一份会话）",
            ),
        );
        splash_fail(
            &window,
            concat!(
                "<b>旧 dsh 没有退出，重启已放弃</b>\n\n",
                "它的进程可能卡住了。请「完全退出」DShell，再重新打开。\n\n",
                "这样做的原因：两个 dsh 后端会抢同一份会话锁"
            ),
        );
        return;
    }

    start_dsh(app, &window);
}

/// 切换到指定 dsh 版本 = `show_splash` + `stop_dsh` + `install(dsh@target)` + `start_dsh`。
///
/// 与 `restart_dsh` 的唯一差别就是中间那一段"装"，所以两者共用同一组原语，
/// 而不是各写一条链（见 docs/plan/09 的推导）。顺序不能动，每一步都在挡一个具体的失败：
///
///   1. 先关设置面板 —— 它马上要随文档一起没了，显式关掉日志才说得清
///   2. `show_splash` —— 进度与失败卡片必须有监听者（08 的 `about:blank` 类缺陷）
///   3. `stop_dsh` —— `npm i -g` 要覆盖的文件正被活着的 dsh 锁着（node 加载中的 native addon）
///   4. 装（可取消）
///   5. `start_dsh` —— 体检 + handoff
///
/// `restart_guard` 必须跨 3+4 **全程**：启动页一加载完就会自己叫体检，放它过去就会在
/// **文件被覆盖的中途**拉起第二个 dsh 后端。
fn do_switch_dsh(app: &AppHandle, target: &str) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        log("switch: main window is gone - aborting");
        return;
    };

    settings::hide(app);
    app.state::<AppState>()
        .restart_guard
        .store(true, Ordering::SeqCst);

    if !show_splash(app, &window) {
        log("switch: aborting (the window would have nowhere to show progress)");
        app.state::<AppState>()
            .restart_guard
            .store(false, Ordering::SeqCst);
        splash_fail(
            &window,
            concat!(
                "<b>无法切换版本</b>\n\n",
                "没有记下启动页地址，切换后界面会无处显示进度。\n\n",
                "请「完全退出」DShell 再重新打开。"
            ),
        );
        return;
    }

    // 窗口可能缩在托盘里：把界面亮出来，否则用户看不到任何反应。
    lifecycle::restore_main_window(app);
    // 更新期间步骤时间线语义不对（六行"等待检查"杵在安装面板上方），让启动页收起来。
    let _ = window.eval("window.__dshell&&window.__dshell.updating(true)");

    if !stop_dsh(app) {
        app.state::<AppState>()
            .restart_guard
            .store(false, Ordering::SeqCst);
        let _ = window.eval("window.__dshell&&window.__dshell.updating(false)");
        splash_fail(
            &window,
            concat!(
                "<b>旧 dsh 没有退出，切换已放弃</b>\n\n",
                "它的进程可能卡住了。请「完全退出」DShell，再重新打开。\n\n",
                "这样做的原因：两个 dsh 后端会抢同一份会话锁，而且 `npm i -g` 要覆盖的\n",
                "文件正被它锁着"
            ),
        );
        return;
    }

    let outcome = {
        let state = app.state::<AppState>();
        installer::run(app, installer::Kind::Dsh, &state, Some(target))
    };

    // 无论成败都要放行体检通道，否则失败卡片上的出口会变成死按钮。
    app.state::<AppState>()
        .restart_guard
        .store(false, Ordering::SeqCst);

    if !outcome.ok {
        // dsh 已经被停了，而这里**不自动起回**：自动起回会掩盖"更新没成功"这个事实，
        // 而用户此刻最需要知道的就是它。出口交给用户自己点（语义见 09 ③ 的决策 1）。
        let why = outcome.error.unwrap_or_else(|| "未知原因".to_string());
        log(&format!("switch: install failed: {why}"));
        splash_fail(
            &window,
            &format!(
                concat!(
                    "<b>更新 dsh 失败</b>\n\n",
                    "{}\n\n",
                    "dsh 后端已停止。要恢复使用，点下面的「跳过检查，直接启动」——",
                    "它会启动<b>当前已装</b>的那个版本。"
                ),
                esc(&why)
            ),
        );
        let _ = window.eval("window.__dshell&&window.__dshell.updateFailed()");
        return;
    }

    settings::clear_selection();
    let _ = window.eval("window.__dshell&&window.__dshell.updating(false)");
    start_dsh(app, &window);
}

/// 切换版本的入口（由设置面板的回传端点调用）。
///
/// 与「重启 dsh 后端」共用 `restarting` 闩：两者都是"停旧 → 起新"的链，
/// 同时跑会互相杀，也会撞上会话锁。
pub(crate) fn start_switch_dsh(app: &AppHandle, target: String) {
    {
        let state = app.state::<AppState>();
        if state.restarting.swap(true, Ordering::SeqCst) {
            log("switch: another restart/switch is in progress - ignoring");
            return;
        }
    }

    let app = app.clone();
    std::thread::spawn(move || {
        // 闩的清除放在 `catch_unwind` 之后（与 `restart_dsh` 同因）：
        // 一次 panic 不该让切换从此静默失效。
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            do_switch_dsh(&app, &target);
        }));
        if result.is_err() {
            log("switch: panicked - the restarting latch is being cleared");
        }
        app.state::<AppState>()
            .restarting
            .store(false, Ordering::SeqCst);
    });
}

/// 托盘「设置」：先把窗口亮出来（可能缩在托盘里），再注入面板。
pub(crate) fn open_settings(app: &AppHandle) {
    lifecycle::restore_main_window(app);
    settings::open(app);
}

/// 起 `dsh web`：隐藏窗口、stdout/stderr 都接管、PATH 里前置用户指定的目录
/// （否则"指定了 node 路径"这件事对 dsh 不起作用）。
///
/// 另外注入 `DSHELL_PICKER_PORT`：dsh 里的 `dshell-directory-picker` 插件靠它
/// 找到 DShell 的原生对话框服务。**没有这个变量时插件不注册**，所以用户自己在
/// 终端跑 `dsh web` 时内置 picker 照常工作——环境变量同时承担了"是否在 DShell
/// 里"和"服务在哪个端口"两个信息，不需要额外的开关。
fn spawn_dsh(cfg: &Config) -> std::io::Result<Child> {
    let mut cmd = Command::new("cmd");
    cmd.args(["/c", "dsh", "web", "--port", "0", "--no-open"])
        .env("PATH", cfg.child_path_env())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let port = picker::port();
    if port != 0 {
        cmd.env("DSHELL_PICKER_PORT", port.to_string());
        if let Some(token) = picker::auth_token() {
            cmd.env("DSHELL_PICKER_TOKEN", token);
        }
    }
    // 后端一律隐藏控制台：DShell 是 GUI 应用，弹黑框是缺陷。
    // （立项 05 的 V1 实验曾用环境变量放开它来验证"前台权"假设，已被 §3 否证。）
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
    // 重启期间不放行：启动页刚导航回来会自动叫这一下，而那时旧 dsh 可能还没停
    // 干净。放它过去就会 handoff 出第二个后端。重启流程自己会在停干净后再叫。
    if app.state::<AppState>().restart_guard.load(Ordering::SeqCst) {
        log("doctor: suppressed (a restart is in progress)");
        return Ok(());
    }
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
                // `None` = 装默认 dist-tag（"补齐"语义）。带版本号的更新走 `do_switch_dsh`。
                installer::run(&app, kind, &state, None)
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

    // 单实例：必须在建窗**之前**判定。
    //
    // 双击第二次时如果照常建窗，就会出现两个壳、两条 `dsh web`（内存成倍，而且
    // 每个后端都持有 DSH 的会话锁，会干扰 session resume）。这里直接让第二个进程
    // 把已有窗口叫到前面，然后退出。
    //
    // 注意 DSHELL_SPLASH_HOLD：那是调试开关，要能同时开两个（边看旧版边调新版），
    // 所以它绕过单实例判定。
    if !hold {
        match win32::acquire_single_instance() {
            win32::Instance::Primary => {}
            win32::Instance::Duplicate => {
                // 已有实例的窗口可能还没建出来（用户手快双击），给它一点时间。
                let focused = win32::focus_existing_instance(FOCUS_TIMEOUT);
                log(&format!(
                    "single instance: another DShell is already running (focused={focused}) - exiting"
                ));
                std::process::exit(0);
            }
        }
    }

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
            // 立项 04 · R19：先起「原生目录选择框」服务，再建窗口。
            //
            // 这是 DSH Desktop 套路的 Tauri 版：**壳层自己进程内**弹原生对话框，
            // 而不是让 dsh 的 `-native` 后端去起一个独立子进程、弹一个**无 owner**
            // 的 IFileOpenDialog。有主的模态框不会让主窗口失焦 → WebView2 不挂起
            // → 不需要用户补点。
            //
            // 端口由内核分配，dsh 插件包通过配置文件读到它（见 picker.rs 的
            // 端点设计说明）。
            match picker::start(app.handle().clone()) {
                Some(p) => log(&format!("picker: bridge listening on 127.0.0.1:{p}")),
                None => log("picker: bridge unavailable - the workspace picker will fall back"),
            }

            // 设置面板的回传端点（面板 → 壳层）：与 picker 一样挂在辅助端口上，
            // 失败了只是"设置面板打不开"，不该拦住启动。
            match settings::start(app.handle().clone()) {
                Some(p) => log(&format!("settings: bridge listening on 127.0.0.1:{p}")),
                None => log("settings: bridge unavailable - the settings panel will not open"),
            }

            // 先把启动页弹出来：它是本地页面，瞬时绘制、不会 401，
            // 体检期间屏幕上一直是它（而不是空白）。
            let window = WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::App("index.html".into()))
                .title("DShell — DeepSeek Harness")
                .inner_size(1280.0, 860.0)
                .min_inner_size(760.0, 560.0)
                .center()
                // 立项 05 · R6 修复：阻止 WebView2 冻结页面。
                //
                // 机制（已由 R6 探针实测证实，见 docs/closed/05 §3.5）：
                // 点「添加工作区」会弹原生 IFileOpenDialog，它抢走前台焦点后，
                // WebView2 把本窗口判定为「被遮挡的后台窗口」，进而**冻结渲染进程**
                // （Page Lifecycle 的 frozen 态）：事件分发与定时器全部停摆。
                // 对话框关闭后没有任何东西触发 resume，页面就一直是僵尸态——
                // hover 不变色、tooltip 卡住、工作区加了也不显示，
                // 直到用户产生**任意一次输入**把它唤醒。
                //
                // 为什么之前的两个开关不够：那两个治的是「节流」（降频），
                // 而这里是「冻结」（完全停止），且源头是**遮挡判定本身**。
                // `CalculateNativeWinOcclusion` 正是 Chromium 计算窗口遮挡的特性，
                // 关掉它，窗口就不会被判定为 occluded，冻结的触发条件随之消失。
                //
                // 注意：**必须原样保留 wry 的默认三个特性**。wry 用 `unwrap_or_else`
                // （wry-0.55.1/src/webview2/mod.rs:294），一旦调用本方法，
                // 它的默认值会被整体替换而非追加；漏掉会让 mini menu /
                // SmartScreen 回来。
                .additional_browser_args(concat!(
                    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,",
                    "CalculateNativeWinOcclusion ",
                    "--disable-backgrounding-occluded-windows ",
                    "--disable-renderer-backgrounding ",
                    "--disable-background-timer-throttling"
                ))
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
                // 记下启动页自己的 URL —— **必须在页面真正加载时记**。
                //
                // 不能在建窗后立刻 `window.url()`：那时 WebView2 还没提交首次导航，
                // 拿到的是 `about:blank`（实测踩过）。重启要导航回启动页，记错就会
                // 把窗口导到一片白屏，而进度和失败卡片全发给没有监听者的空白页。
                .on_page_load({
                    let handle = app.handle().clone();
                    move |_window, payload| {
                        let url = payload.url().clone();
                        // 换文档 = 覆盖层连同里面的 iframe 一起没了（设置面板是主窗口里的
                        // 覆盖层，不是独立窗口）。面板自己那次加载正常不会走到这里
                        // （那是子框架），挡住只为万一。
                        if !url.path().ends_with("settings.html") {
                            crate::settings::reset_for_document();
                        }
                        // 只认我们自己的页面。刻意排除 `about:blank`：
                        // 它同样以 `about` 开头，但它不是启动页。
                        let is_splash = matches!(url.scheme(), "tauri" | "asset")
                            || matches!(url.host_str(), Some("tauri.localhost"));
                        if !is_splash {
                            return;
                        }
                        // 建窗流程里 `manage` 就在 `build()` 之后，而此时事件循环还没
                        // 转过，所以正常情况下这里一定拿得到状态；`try_state` 只是兜底，
                        // 不能让"记 URL"这件事把启动搞崩。
                        if let Some(state) = handle.try_state::<AppState>() {
                            if state.splash_url().as_deref() != Some(url.as_str()) {
                                log(&format!("splash url: {url}"));
                                state.set_splash_url(url.as_str());
                            }
                        }
                    }
                })
                .build()?;

            log("splash window built");
            app.manage(AppState::new(hold));

            // 托盘：图标复用 exe 图标，左键恢复、右键「完全退出」
            if let Err(e) = tray::init(app) {
                // 托盘建不起来不该拦住主流程，记日志继续
                log(&format!("tray: failed to init: {e}"));
            }

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
        // 拦截主窗口的 ×：不直接关，改为弹窗让用户选「完全退出 / 关到托盘 / 取消」。
        //
        // 前提：本轮只有主窗口一个窗口。将来做设置窗口时，必须在此按 `window.label()`
        // 过滤，否则设置窗口的 × 也会弹出这个框。
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                close_dialog::ask_close_choice(window.app_handle());
            }
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

/// 托盘「在浏览器中打开」：把 dsh 的入口地址丢给系统浏览器。
///
/// 地址里带的进程 token 在整个进程存活期间有效（dsh 只要 token 或 cookie 之一），
/// 所以同一个地址可以反复打开，外部浏览器换来的是它自己的 cookie。
/// 启动页阶段还没 handoff，地址为空 —— 记一行日志，不做静默失败。
fn open_dsh_in_browser<R: tauri::Runtime>(app: &AppHandle<R>) {
    let Some(url) = app.state::<AppState>().launch_url() else {
        log("tray: open in browser skipped (dsh 地址还没就绪)");
        return;
    };
    match open_in_browser(&url) {
        Ok(()) => log("tray: handed dsh url to system browser"),
        Err(e) => log(&format!("tray: open in browser failed: {e}")),
    }
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
