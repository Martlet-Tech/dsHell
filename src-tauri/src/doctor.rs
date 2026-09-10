//! 启动前环境体检：五项探测，一项一条结论。
//!
//! 设计约束（见 docs/closed/01-env-doctor.md 的 D4）：
//! **全部跑完再汇报，缺失只是状态、不是异常**。只有第 ⑥ 步「启动 dsh web」
//! 才真正需要前置条件满足。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::config::Config;
use crate::proc;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const NPM_LS_TIMEOUT: Duration = Duration::from_secs(20);
/// 保守下限；@deepseek-ai/dsh 包内**没有** engines 声明（实测），先按 18 卡。
const MIN_NODE: (u32, u32, u32) = (18, 0, 0);

/// WebView2 Runtime 的固定 GUID（per-machine / per-user 各有一个位置）。
const WEBVIEW2_GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

/// 固定顺序（页面的时间线也按这个顺序排）。
pub const IDS: [&str; 5] = ["webview", "node", "npm", "dsh", "profile"];

pub const WEBVIEW_URL: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";
pub const NODE_URL: &str = "https://nodejs.org/en/download";
pub const DSH_URL: &str = "https://github.com/deepseek-ai/deepseek-harness";

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    /// 页面初始骨架用它；Rust 侧不会主动构造，保留是为了状态机完整可读
    #[allow(dead_code)]
    Pending,
    Checking,
    Ok,
    Warn,
    Missing,
    Failed,
}

#[derive(Clone, Serialize)]
pub struct Hint {
    /// 面板上可一键复制的官方命令
    pub command: String,
    pub url: String,
    /// 能否参与「一键安装」
    pub auto: bool,
}

#[derive(Clone, Serialize)]
pub struct Step {
    pub id: String,
    pub label: String,
    pub state: StepState,
    /// 一句话结论，永远非空
    pub detail: String,
    /// 已生效的自定义路径
    pub source: Option<String>,
    pub hint: Option<Hint>,
}

pub fn label_of(id: &str) -> &'static str {
    match id {
        "webview" => "WebView2 运行环境",
        "node" => "Node.js 运行时",
        "npm" => "npm 包管理器",
        "dsh" => "dsh 命令行（DeepSeek Harness）",
        "profile" => "dsh 配置目录可写",
        "launch" => "启动 DeepSeek Harness",
        _ => "检查项",
    }
}

pub fn mk(
    id: &str,
    state: StepState,
    detail: impl Into<String>,
    hint: Option<Hint>,
    source: Option<String>,
) -> Step {
    Step {
        id: id.to_string(),
        label: label_of(id).to_string(),
        state,
        detail: detail.into(),
        source,
        hint,
    }
}

/// ⑤ 之后的第 ⑥ 步：交给 main 在 handoff 时推给页面。
pub fn launch_step(state: StepState, detail: impl Into<String>) -> Step {
    mk("launch", state, detail, None, None)
}

pub fn hint_node() -> Hint {
    Hint {
        command: "winget install --id OpenJS.NodeJS.LTS --exact".into(),
        url: NODE_URL.into(),
        auto: true,
    }
}

pub fn hint_dsh() -> Hint {
    Hint {
        command: "npm i -g @deepseek-ai/dsh".into(),
        url: DSH_URL.into(),
        auto: true,
    }
}

/// 跑完全部五项，每项完成即推送（页面渐进填充，但**不因缺失中断**）。
pub fn run_all(app: &AppHandle, cfg: &Config) -> Vec<Step> {
    IDS.iter().map(|id| reprobe(app, cfg, id)).collect()
}

/// 只复检一项（安装完一项后立刻看效果，不用等整轮）。
pub fn reprobe(app: &AppHandle, cfg: &Config, id: &str) -> Step {
    let _ = app.emit(
        "doctor://step",
        &mk(id, StepState::Checking, "检测中…", None, None),
    );
    let s = match id {
        "webview" => probe_webview(),
        "node" => probe_node(cfg),
        "npm" => probe_npm(cfg),
        "dsh" => probe_dsh(cfg),
        "profile" => probe_profile(),
        other => mk(other, StepState::Warn, "未知检查项", None, None),
    };
    let _ = app.emit("doctor://step", &s);
    s
}

/// 缺什么、能否自动装（前端不参与决策，只做展示）。
pub fn auto_ids(steps: &[Step]) -> Vec<&'static str> {
    let mut v = Vec::new();
    let missing = |id: &str| {
        steps
            .iter()
            .any(|s| s.id == id && (s.state == StepState::Missing || s.state == StepState::Failed))
    };
    // node 与 npm 是同一件事的两面：npm 丢了就该重装 Node
    if missing("node") || missing("npm") {
        v.push("node");
    }
    if missing("dsh") {
        v.push("dsh");
    }
    v
}

pub fn blocking_ids(steps: &[Step]) -> Vec<String> {
    steps
        .iter()
        .filter(|s| {
            s.id != "launch"
                && s.id != "profile"
                && matches!(s.state, StepState::Missing | StepState::Failed)
        })
        .map(|s| s.id.clone())
        .collect()
}

// ───────────────────────── ① WebView2 ─────────────────────────

/// 读注册表拿版本号。
///
/// 不用 Tauri 的 `webview_version()` 是为了不赌 API 形状；也不引 winreg：
/// `reg.exe` 本来就是系统自带。三条路径依次试（per-machine 64/32、per-user）。
fn probe_webview() -> Step {
    let keys = [
        format!(r"HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{WEBVIEW2_GUID}"),
        format!(r"HKLM\SOFTWARE\Microsoft\EdgeUpdate\Clients\{WEBVIEW2_GUID}"),
        format!(r"HKCU\SOFTWARE\Microsoft\EdgeUpdate\Clients\{WEBVIEW2_GUID}"),
    ];
    for key in keys {
        let Ok(out) = proc::run_capture("reg", &["query", &key, "/v", "pv"], PROBE_TIMEOUT) else {
            continue;
        };
        if out.code != Some(0) {
            continue;
        }
        if let Some(v) = out.stdout.split_whitespace().last() {
            if v.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                let detail = match proc::parse_version(v) {
                    Some(ver) if !proc::ver_ge(ver, (93, 0, 0)) => {
                        format!("{v}（偏低，建议更新）")
                    }
                    _ => v.to_string(),
                };
                return mk("webview", StepState::Ok, detail, None, None);
            }
        }
    }
    // 读不到也别判死：窗口已经在跑了，运行时必然是存在的
    mk(
        "webview",
        StepState::Warn,
        "正在运行，但读不到版本号",
        Some(Hint {
            command: String::new(),
            url: WEBVIEW_URL.into(),
            auto: false,
        }),
        None,
    )
}

// ───────────────────────── ② Node.js ─────────────────────────

fn probe_node(cfg: &Config) -> Step {
    let found = cfg.get("node").or_else(|| proc::which("node"));
    probe_node_at(&found)
}

fn probe_node_at(found: &Option<PathBuf>) -> Step {
    let Some(path) = found.clone() else {
        return mk(
            "node",
            StepState::Missing,
            "未检测到 node。dsh 需要 Node.js 才能运行。",
            Some(hint_node()),
            None,
        );
    };
    // 命中路径 ≠ 能用：可能是坏的 shim，必须就地跑一次
    match proc::run_path(&path, &["--version"], PROBE_TIMEOUT) {
        Ok(out) if out.ok() => match proc::parse_version(&out.stdout) {
            Some(v) if proc::ver_ge(v, MIN_NODE) => mk(
                "node",
                StepState::Ok,
                format!("v{}.{}.{} · {}", v.0, v.1, v.2, path.display()),
                None,
                Some(path.display().to_string()),
            ),
            Some(v) => mk(
                "node",
                StepState::Warn,
                format!(
                    "v{}.{}.{} 过低（需要 ≥ {}.{}） · {}",
                    v.0, v.1, v.2, MIN_NODE.0, MIN_NODE.1, path.display()
                ),
                None,
                Some(path.display().to_string()),
            ),
            None => mk(
                "node",
                StepState::Warn,
                format!("已安装，版本未知 · {}", path.display()),
                None,
                Some(path.display().to_string()),
            ),
        },
        Ok(out) => mk(
            "node",
            StepState::Failed,
            format!("无法执行：{} · {}", path.display(), out.diagnose()),
            Some(hint_node()),
            Some(path.display().to_string()),
        ),
        Err(e) => mk(
            "node",
            StepState::Failed,
            format!("无法执行：{} · {e}", path.display()),
            Some(hint_node()),
            Some(path.display().to_string()),
        ),
    }
}

// ───────────────────────── ③ npm ─────────────────────────

fn probe_npm(cfg: &Config) -> Step {
    // npm 在 Windows 上是 npm.cmd，且与同名的无扩展名 shell 脚本共存，
    // 所以一定要经由 cmd /c（proc::run_path 对 .cmd 自动套壳）。
    let found = cfg.get("npm").or_else(|| proc::which("npm"));
    match found {
        Some(path) => probe_npm_at(&path),
        None => match proc::run_shell(&["npm", "--version"], PROBE_TIMEOUT) {
            Ok(out) if out.ok() => {
                let v = out.stdout.trim().to_string();
                mk(
                    "npm",
                    StepState::Ok,
                    format!("{v} · 由 PATH 解析（路径未知）"),
                    None,
                    None,
                )
            }
            Ok(out) => mk(
                "npm",
                StepState::Missing,
                format!("未检测到 npm。{}", out.diagnose()),
                Some(hint_node()),
                None,
            ),
            Err(e) => mk(
                "npm",
                StepState::Missing,
                format!("未检测到 npm。{e}"),
                Some(hint_node()),
                None,
            ),
        },
    }
}

fn probe_npm_at(path: &Path) -> Step {
    match proc::run_path(path, &["--version"], PROBE_TIMEOUT) {
        Ok(out) if out.ok() => match proc::find_semver(&out.stdout) {
            Some(v) => mk(
                "npm",
                StepState::Ok,
                format!("{v} · {}", path.display()),
                None,
                Some(path.display().to_string()),
            ),
            None => mk(
                "npm",
                StepState::Ok,
                format!("已安装 · {}", path.display()),
                None,
                Some(path.display().to_string()),
            ),
        },
        Ok(out) => mk(
            "npm",
            StepState::Failed,
            format!(
                "这个文件跑不出 npm 版本：{} · {}",
                path.display(),
                out.diagnose()
            ),
            Some(hint_node()),
            Some(path.display().to_string()),
        ),
        Err(e) => mk(
            "npm",
            StepState::Failed,
            format!("无法执行：{} · {e}", path.display()),
            Some(hint_node()),
            Some(path.display().to_string()),
        ),
    }
}

// ───────────────────────── ④ dsh ─────────────────────────

fn probe_dsh(cfg: &Config) -> Step {
    let found = cfg.get("dsh").or_else(|| proc::which("dsh"));
    match found {
        Some(path) => probe_dsh_at(&path),
        None => mk(
            "dsh",
            StepState::Missing,
            "未检测到 dsh 命令。这是 DShell 要托管的后端。",
            Some(hint_dsh()),
            None,
        ),
    }
}

fn probe_dsh_at(path: &Path) -> Step {
    let mut version = None;
    // 降级链第 1 环：`dsh --version`（该 CLI 是否支持 --version 不做假设，
    // 只在输出里捞一个 x.y.z 形状的 token）
    if let Ok(out) = proc::run_path(path, &["--version"], PROBE_TIMEOUT) {
        version = proc::find_semver(&out.stdout)
            .or_else(|| proc::find_semver(&out.stderr))
            .filter(|v| v.contains('.'));
    }
    // 第 2 环：npm 全局清单（权威，同时证明它确实由 npm 装着）
    if version.is_none() {
        version = npm_global_version("@deepseek-ai/dsh");
    }
    match version {
        Some(v) => mk(
            "dsh",
            StepState::Ok,
            format!("{v} · {}", path.display()),
            None,
            Some(path.display().to_string()),
        ),
        // 第 3 环：装了但问不出版本 —— 只报"未知"，绝不因此判失败
        None => mk(
            "dsh",
            StepState::Ok,
            format!("已安装（版本未知） · {}", path.display()),
            None,
            Some(path.display().to_string()),
        ),
    }
}

fn npm_global_version(pkg: &str) -> Option<String> {
    let out = proc::run_shell(
        &["npm", "ls", "-g", pkg, "--depth=0", "--json"],
        NPM_LS_TIMEOUT,
    )
    .ok()?;
    let v: serde_json::Value = serde_json::from_str(&out.stdout).ok()?;
    Some(
        v.get("dependencies")?
            .get(pkg)?
            .get("version")?
            .as_str()?
            .to_string(),
    )
}

// ───────────────────────── ⑤ profile 目录 ─────────────────────────

fn probe_profile() -> Step {
    let home = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
    let dsh_dir = PathBuf::from(home).join(".dsh");
    if let Err(e) = std::fs::create_dir_all(&dsh_dir) {
        return mk(
            "profile",
            StepState::Warn,
            format!("无法创建 {} ：{e}", dsh_dir.display()),
            None,
            None,
        );
    }
    let probe = dsh_dir.join(".dshell-writetest");
    match std::fs::write(&probe, b"ok") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            mk(
                "profile",
                StepState::Ok,
                format!("可写 · {}", dsh_dir.display()),
                None,
                None,
            )
        }
        Err(e) => mk(
            "profile",
            StepState::Warn,
            format!("不可写 · {} ：{e}", dsh_dir.display()),
            None,
            None,
        ),
    }
}

// ───────────────────────── 「指定…」入口 ─────────────────────────

/// 就地把用户指定的路径验证一次：跑不出东西就不接受，绝不留到后面才炸。
pub fn probe_with_path(id: &str, path: &Path) -> Result<Step, String> {
    match id {
        "node" => Ok(probe_node_at(&Some(path.to_path_buf()))),
        "npm" => Ok(probe_npm_at(path)),
        "dsh" => Ok(probe_dsh_at(path)),
        other => Err(format!("{other} 不支持指定路径")),
    }
}
