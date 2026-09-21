//! 统一的外部命令执行助手。
//!
//! 三件事：
//!   * `cmd /c` 的引号规则（Windows 上 npm / dsh 都是 .cmd，CreateProcess 不解析 .cmd）
//!   * stdout **和 stderr 都排空**（旧实现把 stderr 设成 piped 却从不读取：丢诊断信息，
//!     量大时还会把子进程堵死）
//!   * 硬超时 + `taskkill /T /F`，不留孤儿进程

#![allow(dead_code)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 一行输出（安装命令流式读取用）。
pub struct Line {
    pub text: String,
    pub is_err: bool,
}

pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl Output {
    pub fn ok(&self) -> bool {
        !self.timed_out && self.code == Some(0)
    }

    /// 给用户看的诊断文本：优先 stderr，空了退回 stdout。
    pub fn diagnose(&self) -> String {
        let raw = if self.stderr.trim().is_empty() {
            self.stdout.clone()
        } else {
            self.stderr.clone()
        };
        let mut s = raw.trim().to_string();
        if self.timed_out {
            s = format!("命令超时无响应。{s}");
        }
        if s.len() > 600 {
            s.truncate(600);
            s.push('…');
        }
        s
    }
}

/// 校验用户在「指定…」里挑的路径。
///
/// 这是全流程**唯一**把外部输入拼进命令行的地方，所以在这里一次挡住：
/// 必须存在、必须是文件、不能含 cmd 元字符（`&` 会在 `cmd /c` 里断句）。
pub fn validate_user_path(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim().trim_matches('"');
    if trimmed.is_empty() {
        return Err("路径为空".into());
    }
    let p = PathBuf::from(trimmed);
    if !p.is_file() {
        return Err(format!("不是一个文件：{}", p.display()));
    }
    const BAD: &[char] = &['&', '|', '<', '>', '^', '"', '\n', '\r', '%', '!'];
    if let Some(c) = trimmed.chars().find(|c| BAD.contains(c)) {
        return Err(format!(
            "路径里含有命令行特殊字符 `{c}`，请把 node / dsh 装在普通路径下（例如 C:\\nodejs）"
        ));
    }
    Ok(p)
}

/// 构造命令。`.cmd` / `.bat` 必须套 `cmd /c`：
/// `cmd /c "C:\Program Files\nodejs\npm.cmd" --version` 能工作，是因为 cmd 的 /c
/// 引号剥离规则（恰好两个引号、引号内是带空格的合法可执行文件名、无特殊字符）会保留引号。
fn build_command(program: &str, args: &[&str]) -> Command {
    let lower = program.to_ascii_lowercase();
    let mut cmd = if lower.ends_with(".cmd") || lower.ends_with(".bat") {
        let mut c = Command::new("cmd");
        c.arg("/c").arg(program);
        c
    } else {
        Command::new(program)
    };
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn spawn_child(program: &str, args: &[&str], path_env: Option<String>) -> std::io::Result<Child> {
    let mut cmd = build_command(program, args);
    if let Some(p) = path_env {
        cmd.env("PATH", p);
    }
    cmd.spawn()
}

/// 通过 `cmd /c` 跑：用于依赖 PATH/PATHEXT 解析的东西（npm、dsh 都是 .cmd，
/// 直接 CreateProcess 会失败）。
pub fn run_shell(args: &[&str], timeout: Duration) -> std::io::Result<Output> {
    run_shell_env(args, timeout, None)
}

/// 同 `run_shell`，但能前置 PATH。
///
/// 用户在「指定…」里改过 node / npm 路径时**必须**用这个：配置里的路径只进
/// `cfg.child_path_env()`，而进程自己的 PATH 不会被改。体检里的 `dsh` 探测就是
/// 这个道理——否则"指定了 npm 路径"对版本查询不起作用。
pub fn run_shell_env(
    args: &[&str],
    timeout: Duration,
    path_env: Option<String>,
) -> std::io::Result<Output> {
    let mut all = Vec::with_capacity(args.len() + 1);
    all.push("/c");
    all.extend_from_slice(args);
    run_capture_env("cmd", &all, timeout, path_env)
}

/// 跑一个已知路径的命令（node.exe 直连；.cmd 自动套 `cmd /c`）。
pub fn run_path(exe: &Path, args: &[&str], timeout: Duration) -> std::io::Result<Output> {
    run_capture(&exe.to_string_lossy(), args, timeout)
}

pub fn run_capture(program: &str, args: &[&str], timeout: Duration) -> std::io::Result<Output> {
    run_capture_env(program, args, timeout, None)
}

pub fn run_capture_env(
    program: &str,
    args: &[&str],
    timeout: Duration,
    path_env: Option<String>,
) -> std::io::Result<Output> {
    let mut child = spawn_child(program, args, path_env)?;
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let code = loop {
        match child.try_wait()? {
            Some(status) => break status.code(),
            None => {
                if Instant::now() >= deadline {
                    timed_out = true;
                    kill_tree(child.id());
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
        }
    };

    Ok(Output {
        code,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
        timed_out,
    })
}

fn drain<R: Read + Send + 'static>(r: Option<R>) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut r) = r {
            let _ = r.read_to_end(&mut buf);
        }
        String::from_utf8_lossy(&buf).into_owned()
    })
}

/// 流式执行：按 `\r` 与 `\n` 切行（winget 的进度条就是用 `\r` 原地刷新的，
/// 只按 `\n` 切会攒成一条巨长的行），逐行回调。
pub fn spawn_streaming(
    program: &str,
    args: &[&str],
    path_env: Option<String>,
    cb: Arc<dyn Fn(Line) + Send + Sync>,
) -> std::io::Result<Child> {
    let mut child = spawn_child(program, args, path_env)?;
    if let Some(o) = child.stdout.take() {
        pump(o, false, cb.clone());
    }
    if let Some(e) = child.stderr.take() {
        pump(e, true, cb);
    }
    Ok(child)
}

fn pump<R: Read + Send + 'static>(mut r: R, is_err: bool, cb: Arc<dyn Fn(Line) + Send + Sync>) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut acc: Vec<u8> = Vec::new();
        let emit = |acc: &mut Vec<u8>| {
            if acc.is_empty() {
                return;
            }
            let text = String::from_utf8_lossy(acc).trim_end().to_string();
            acc.clear();
            if text.trim().is_empty() {
                return;
            }
            // 进度条行可能极长，截断免得把日志缓冲和 webview 撑爆
            let text = if text.chars().count() > 300 {
                text.chars().take(300).collect::<String>() + "…"
            } else {
                text
            };
            cb(Line { text, is_err });
        };
        loop {
            match r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    for &b in &buf[..n] {
                        if b == b'\r' || b == b'\n' {
                            emit(&mut acc);
                        } else {
                            acc.push(b);
                        }
                    }
                }
            }
        }
        emit(&mut acc);
    });
}

/// 杀掉整棵进程树。`cmd` → `npm` → `node`，只杀直接子进程会留下占着端口的 node。
pub fn kill_tree(pid: u32) {
    if pid == 0 {
        return;
    }
    let mut cmd = Command::new("taskkill");
    cmd.args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = cmd.status();
}

/// 后台任务里的取消标志。
pub struct Cancel(AtomicBool);

impl Cancel {
    pub fn new() -> Self {
        Self(AtomicBool::new(false))
    }
    pub fn request(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn reset(&self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn requested(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Default for Cancel {
    fn default() -> Self {
        Self::new()
    }
}

/// `where.exe` 查可执行文件，按扩展名优先级挑：
/// 同名文件里 `npm`（无扩展名的 shell 脚本）、`npm.cmd`、`npm.ps1` 是共存的，
/// 直接取第一行可能拿到跑不了的那个。
pub fn which(name: &str) -> Option<PathBuf> {
    let out = run_capture("where.exe", &[name], Duration::from_secs(5)).ok()?;
    if out.code != Some(0) {
        return None;
    }
    let mut cands: Vec<PathBuf> = out
        .stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .collect();
    cands.sort_by_key(|p| rank(p));
    cands.into_iter().next()
}

fn rank(p: &Path) -> u8 {
    match p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("exe") => 0,
        Some("cmd") => 1,
        Some("bat") => 2,
        Some("ps1") => 4,
        None => 5,
        _ => 3,
    }
}

/// 从任意文本里捞出第一个 `x.y.z` 形状的版本号（dsh 的 `--version` 输出格式未知，
/// 不赌它的排版）。
pub fn find_semver(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() {
            let c = bytes[i] as char;
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+' {
                i += 1;
            } else {
                break;
            }
        }
        let tok = &s[start..i];
        let dots = tok.matches('.').count();
        if dots >= 2 && tok.ends_with(|c: char| c.is_ascii_alphanumeric()) {
            return Some(tok.to_string());
        }
        i += 1;
    }
    None
}

pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let t = find_semver(s)?;
    let mut it = t.split(['.', '-', '+']);
    let a = it.next()?.parse().ok()?;
    let b = it.next().unwrap_or("0").parse().unwrap_or(0);
    let c = it.next().unwrap_or("0").parse().unwrap_or(0);
    Some((a, b, c))
}

pub fn ver_ge(v: (u32, u32, u32), min: (u32, u32, u32)) -> bool {
    v >= min
}
