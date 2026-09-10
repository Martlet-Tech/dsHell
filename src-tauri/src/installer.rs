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

fn command_for(kind: Kind) -> (&'static str, Vec<&'static str>) {
    match kind {
        Kind::Node => (
            "winget",
            vec![
                "install",
                "--id",
                "OpenJS.NodeJS.LTS",
                "--exact",
                "--accept-package-agreements",
                "--accept-source-agreements",
            ],
        ),
        Kind::Dsh => ("cmd", vec!["/c", "npm", "i", "-g", "@deepseek-ai/dsh"]),
    }
}

/// 安装命令是长跑，必须能取消（npm 会拉起一堆子进程，`/T` 一起收）。
pub fn run(app: &AppHandle, kind: Kind, state: &AppState) -> Outcome {
    let cfg = Config::load();
    let (program, args) = command_for(kind);
    let path_env = cfg.child_path_env();

    let log: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::with_capacity(220)));
    let percent: Arc<Mutex<Option<f64>>> = Arc::new(Mutex::new(None));
    let phase: Arc<Mutex<String>> = Arc::new(Mutex::new(format!("正在安装 {}", kind.title())));
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
                while l.len() > 200 {
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

    let mut child = match proc::spawn_streaming(program, &args, Some(path_env), cb) {
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
        Some(format!("安装失败（退出码 {:?}）\n{tail}", code))
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
