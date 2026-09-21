//! 一键安装：只做官方通道的两个动作（见 docs/closed/01-env-doctor.md 的 D3）
//!
//!   node → `winget install --id OpenJS.NodeJS.LTS`（**会弹 UAC**）
//!   dsh  → `npm i -g @deepseek-ai/dsh`（装到 %APPDATA%\npm，不需要管理员）
//!
//! 进度方面不糊弄：winget 能解析出真百分比，npm 给不了，就老老实实
//! 不确定进度条 + 日志尾部 + 已用时。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::config::Config;
use crate::proc;
use crate::AppState;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Node,
    Dsh,
}

impl Kind {
    pub fn id(self) -> &'static str {
        match self {
            Kind::Node => "node",
            Kind::Dsh => "dsh",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Kind::Node => "Node.js",
            Kind::Dsh => "@deepseek-ai/dsh",
        }
    }
}

#[derive(Clone, Serialize)]
pub struct Progress {
    pub kind: &'static str,
    pub phase: String,
    /// `None` = 拿不到真实百分比，前端应切成不确定进度条
    pub percent: Option<f64>,
    pub elapsed_ms: u64,
    pub lines: Vec<String>,
}

pub struct Outcome {
    pub ok: bool,
    pub cancelled: bool,
    pub error: Option<String>,
}

/// `Kind::Dsh` 装的包，也是版本查询用的包名（`update.rs` 共用这一个来源）。
pub const DSH_PACKAGE: &str = "@deepseek-ai/dsh";

fn command_for(kind: Kind, target: Option<&str>, resolve_before: Option<&str>) -> (String, Vec<String>) {
    match kind {
        Kind::Node => (
            "winget".to_string(),
            [
                "install",
                "--id",
                "OpenJS.NodeJS.LTS",
                "--exact",
                "--accept-package-agreements",
                "--accept-source-agreements",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        ),
        Kind::Dsh => {
            // 带 target 时是"装指定版本"：npm 对已装包语义即升级，对同版本即重装，
            // 对更低版本即降级——三条路径都是同一条命令。
            let spec = match target {
                Some(v) => format!("{DSH_PACKAGE}@{v}"),
                None => DSH_PACKAGE.to_string(),
            };
            // `--no-audit --no-fund`：省掉两次与安装无关的网络往返。
            //
            // `--loglevel=http`：**这条是为了让用户看得见动静**。真装的时候 npm 默认
            // 只吐 warnings 和最后那行 "added N packages … in 2m"，中间一两分钟一声不吭
            // （实测 2026-09-21：升级要动 430+ 个包）——面板上只有一个不确定进度条在转，
            // 看着像卡死。http 级会逐条打出 `npm http cache/fetch … 12ms`，
            // 代价是几百行噪声（面板只显示尾部 60 行，且不进日志文件）。
            let mut args: Vec<String> = ["/c", "npm", "i", "-g", "--no-audit", "--no-fund", "--loglevel=http"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            // 装"不是最新那一版"时必须把 npm 的解析视野拉回那个版本发布时，
            // 否则 `^` 范围会配上新版同级包，装出一棵跑不起来的树（见 `update::resolve_before`）。
            if let Some(b) = resolve_before {
                args.push(format!("--before={b}"));
            }
            args.push(spec);
            ("cmd".to_string(), args)
        }
    }
}

/// 给面板日志看的命令行（`cmd /c` 的 `/c` 是纯噪声，去掉）。
fn display_command(program: &str, args: &[String]) -> String {
    let body = if program == "cmd" && args.first().map(String::as_str) == Some("/c") {
        args[1..].join(" ")
    } else {
        args.join(" ")
    };
    format!("{program} {body}")
}

/// 版本号字符集白名单：`0-9 A-Za-z . - +`。
///
/// **这是把外部输入拼进安装命令的唯一入口**（版本号来自固定端点的请求），
/// 所以在这里一次挡住：`npm i -g @deepseek-ai/dsh@<v>` 里 `<v>` 若含 `&` `"` 空格
/// 之类，就不再是一个参数了。与 `proc::validate_user_path` 同一套理由。
fn valid_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 64
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
}

/// 安装命令是长跑，必须能取消（npm 会拉起一堆子进程，`/T` 一起收）。
///
/// `target` = 要装的指定版本（`None` = 装/升级到默认 dist-tag，即「一键安装」的行为）。
/// `resolve_before` = 给 npm 的 `--before` 时刻：装"不是最新那一版"时必须把解析视野
/// 拉回那个版本发布时，否则 `^` 范围会配上新版同级包（见 `update::resolve_before`）。
pub fn run(
    app: &AppHandle,
    kind: Kind,
    state: &AppState,
    target: Option<&str>,
    resolve_before: Option<&str>,
) -> Outcome {
    if let Some(v) = target {
        if !valid_version(v) {
            crate::log(&format!(
                "install {}: rejected target version (unexpected characters): {v}",
                kind.id()
            ));
            return Outcome {
                ok: false,
                cancelled: false,
                error: Some(format!("版本号 `{v}` 含意外字符，已拒绝执行")),
            };
        }
    }

    let cfg = Config::load();
    let (program, args) = command_for(kind, target, resolve_before);
    let path_env = cfg.child_path_env();

    // 带目标版本 = 用户明确指了要装哪个（版本页的切换），此时"更新"比"安装"准。
    let verb = if target.is_some() { "正在更新" } else { "正在安装" };
    let what = match target {
        Some(v) => format!("{}@{v}", kind.title()),
        None => kind.title().to_string(),
    };

    if let Some(b) = resolve_before {
        crate::log(&format!("install {}: resolving as of {b}", kind.id()));
    }

    let log: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::with_capacity(220)));
    let percent: Arc<Mutex<Option<f64>>> = Arc::new(Mutex::new(None));
    let phase: Arc<Mutex<String>> = Arc::new(Mutex::new(format!("{verb} {what}")));

    // t=0 就往日志里放点东西：面板一出现就有内容，而不是一片空白等 npm 开口。
    // 第二行是**预期管理**：npm 解析依赖时会安静一两分钟，不写出来用户会以为卡死。
    {
        let mut l = log.lock().unwrap();
        l.push_back(format!("$ {}", display_command(&program, &args)));
        if matches!(kind, Kind::Dsh) {
            l.push_back(
                "（换版本要重装几百个包；npm 在解析依赖时会安静一会儿，通常 1-3 分钟）".to_string(),
            );
        }
    }
    let started = Instant::now();
    let done = Arc::new(AtomicBool::new(false));

    // 心跳：npm 可能安静好几分钟，没有心跳界面看起来就是死了
    let ticker = {
        let app = app.clone();
        let log = log.clone();
        let percent = percent.clone();
        let phase = phase.clone();
        let done = done.clone();
        std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(500));
                if done.load(Ordering::SeqCst) {
                    break;
                }
                emit(&app, kind, &phase, &percent, started, &log, false);
            }
        })
    };

    let cb: Arc<dyn Fn(proc::Line) + Send + Sync> = {
        let app = app.clone();
        let log = log.clone();
        let percent = percent.clone();
        let phase = phase.clone();
        let last = Arc::new(Mutex::new(Instant::now()));
        Arc::new(move |line: proc::Line| {
            if let Some(p) = parse_percent(&line.text) {
                *percent.lock().unwrap() = Some(p);
            }
            if let Some(ph) = guess_phase(&line.text) {
                *phase.lock().unwrap() = ph;
            }
            {
                let mut l = log.lock().unwrap();
                l.push_back(format!(
                    "{} {}",
                    if line.is_err { "!" } else { " " },
                    line.text
                ));
                // 上限 120：面板只显示尾部 60 行，而 `--loglevel=http` 会刷几百行 ——
                // 上限越大队列序列化进事件的 JSON 越大（每 120ms 一次）。
                while l.len() > 120 {
                    l.pop_front();
                }
            }
            // 节流：npm 能刷上千行，逐行 emit 会把 webview 打爆
            let mut last = last.lock().unwrap();
            if last.elapsed() >= Duration::from_millis(120) {
                *last = Instant::now();
                drop(last);
                emit(&app, kind, &phase, &percent, started, &log, false);
            }
        })
    };

    state.cancel.reset();
    emit(&app, kind, &phase, &percent, started, &log, true);

    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let mut child = match proc::spawn_streaming(&program, &arg_refs, Some(path_env), cb) {
        Ok(c) => c,
        Err(e) => {
            crate::log(&format!("install {}: spawn failed: {e}", kind.id()));
            done.store(true, Ordering::SeqCst);
            let _ = ticker.join();
            return Outcome {
                ok: false,
                cancelled: false,
                error: Some(format!("无法启动安装程序：{e}")),
            };
        }
    };

    state.install_pid.store(child.id(), Ordering::SeqCst);
    crate::log(&format!(
        "install {}: spawned {} (pid {})",
        kind.id(),
        program,
        child.id()
    ));

    let mut cancelled = false;
    let code = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code(),
            Ok(None) => {
                if state.cancel.requested() {
                    cancelled = true;
                    proc::kill_tree(child.id());
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => break None,
        }
    };

    state.install_pid.store(0, Ordering::SeqCst);
    done.store(true, Ordering::SeqCst);
    let _ = ticker.join();

    let ok = !cancelled && code == Some(0);
    let error = if ok {
        None
    } else if cancelled {
        Some("安装已取消".to_string())
    } else {
        let tail = log
            .lock()
            .unwrap()
            .iter()
            .rev()
            .take(6)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        // `ETARGET` 是"按发布时间窗装旧版"最常见也最难懂的失败：npm 只说
        // "No matching version found for … with a date before …"，用户看不出跟版本管理有关。
        // 翻译成一句能懂的话（原始输出仍然附在后面，不藏证据）。
        let head = if tail.contains("ETARGET") {
            concat!(
                "这个版本装不了：它发布那一刻，同一批的其它包还没发全（npm ETARGET）。\n",
                "— 换一个版本，或点下面的「装回 …」。\n"
            )
        } else {
            ""
        };
        Some(format!("{head}安装失败（退出码 {:?}）\n{tail}", code))
    };

    crate::log(&format!(
        "install {}: ok={ok} cancelled={cancelled} code={code:?}",
        kind.id()
    ));
    emit(&app, kind, &phase, &percent, started, &log, true);

    Outcome {
        ok,
        cancelled,
        error,
    }
}

fn emit(
    app: &AppHandle,
    kind: Kind,
    phase: &Arc<Mutex<String>>,
    percent: &Arc<Mutex<Option<f64>>>,
    started: Instant,
    log: &Arc<Mutex<VecDeque<String>>>,
    final_tick: bool,
) {
    let lines: Vec<String> = log.lock().unwrap().iter().cloned().collect();
    let p = Progress {
        kind: kind.id(),
        phase: phase.lock().unwrap().clone(),
        percent: if final_tick { None } else { *percent.lock().unwrap() },
        elapsed_ms: started.elapsed().as_millis() as u64,
        lines,
    };
    let _ = app.emit("install://progress", p);
}

/// 从一行里抠百分比：先找 `NN%`，找不到再数进度条的方块（winget 两种都会用）。
fn parse_percent(s: &str) -> Option<f64> {
    // `--loglevel=http` 的行里全是 URL，而 URL 里可能有百分号转义
    // （`1.2.3%2Bbuild` 那种），被当成百分比会让进度条乱跳。带 `://` 的行直接不看。
    if s.contains("://") {
        return None;
    }
    let b = s.as_bytes();
    let mut best = None;
    for i in 0..b.len() {
        if b[i] != b'%' {
            continue;
        }
        let mut j = i;
        while j > 0 && b[j - 1].is_ascii_digit() {
            j -= 1;
        }
        if j < i {
            if let Ok(v) = s[j..i].parse::<f64>() {
                if (0.0..=100.0).contains(&v) {
                    best = Some(v);
                }
            }
        }
    }
    if best.is_some() {
        return best;
    }
    let filled = s.chars().filter(|c| matches!(c, '█' | '▓' | '■')).count();
    let empty = s.chars().filter(|c| matches!(c, '░' | '▒' | '□')).count();
    let total = filled + empty;
    if total >= 10 {
        return Some(filled as f64 / total as f64 * 100.0);
    }
    None
}

fn guess_phase(line: &str) -> Option<String> {
    let l = line.to_ascii_lowercase();
    // `--loglevel=http` 的流水：fetch 是真的在下载，cache 只是读元数据缓存
    if l.starts_with("npm http") {
        return Some(if l.contains(" fetch ") {
            "正在下载".into()
        } else {
            "正在解析依赖".into()
        });
    }
    if l.contains("downloading") || l.contains("download") {
        Some("正在下载".into())
    } else if l.contains("installing") || l.contains("install") {
        Some("正在安装".into())
    } else if l.contains("verif") {
        Some("正在校验".into())
    } else {
        None
    }
}

// ───────────────────── 装完之后：定位 & 回填 ─────────────────────

/// 装完**不能**靠 `where node` 复查：winget 装完，当前进程的 PATH 里仍然没有
/// `C:\Program Files\nodejs`。按已知落点直接找，并回填配置。
pub fn locate_and_remember(kind: Kind, cfg: &mut Config) -> Option<PathBuf> {
    match kind {
        Kind::Node => {
            let node = node_candidates().into_iter().find(|p| p.is_file())?;
            if let Some(dir) = node.parent() {
                let npm = dir.join("npm.cmd");
                if npm.is_file() {
                    cfg.npm = Some(npm.display().to_string());
                }
            }
            cfg.node = Some(node.display().to_string());
            cfg.npm_prefix = npm_prefix();
            cfg.save().ok();
            Some(node)
        }
        Kind::Dsh => {
            let prefix = npm_prefix()?;
            let dsh = PathBuf::from(&prefix).join("dsh.cmd");
            if !dsh.is_file() {
                return None;
            }
            cfg.dsh = Some(dsh.display().to_string());
            cfg.npm_prefix = Some(prefix);
            cfg.save().ok();
            Some(dsh)
        }
    }
}

fn npm_prefix() -> Option<String> {
    let out = proc::run_shell(&["npm", "prefix", "-g"], Duration::from_secs(20)).ok()?;
    if !out.ok() {
        return None;
    }
    let p = out.stdout.trim();
    (!p.is_empty()).then(|| p.to_string())
}

fn node_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(pf) = std::env::var("ProgramFiles") {
        v.push(PathBuf::from(pf).join("nodejs").join("node.exe"));
    }
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        v.push(PathBuf::from(la).join("Programs").join("nodejs").join("node.exe"));
    }
    v.push(PathBuf::from(r"C:\Program Files\nodejs\node.exe"));
    v
}
