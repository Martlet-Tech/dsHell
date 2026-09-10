# 02 · 关键代码（第二轮）

> 依据 `01-env-doctor.md` §12 已冻结的四条决策（D1 原生 IPC / D2 可加依赖 / D3 只装 Node+dsh / D4 全面体检统一汇报）。
> 本文只给**关键代码与契约**，不动仓库里的实现代码。第三轮按 §9 顺序落地。

---

## 1. D4 把方案改成了什么样

```
                 ┌──────────── 一次跑完，不停机 ────────────┐
双击 exe → 建窗 → ① webview ② node ③ npm ④ dsh ⑤ profile → 统一汇报
                                                             │
                       ┌─────────────────────────────────────┴──────────────────┐
                       │  全绿 → ⑥ 启动 dsh web → 抓 token → navigate → DSH UI   │
                       │  有缺失 → 停在报告页：每行 「指定…」+ 底部「一键安装 / 退出」│
                       └────────────────────────────────────────────────────────┘
                                          │ 一键安装
                                          ▼
                       node(winget) → 重新探测 → dsh(npm -g) → 重新探测
                                          │ 装完且全绿
                                          └──────► 自动进入 ⑥（不用再点一次）
```

关键差异：

- 探测**不因缺失而中断**，5 项一路跑完（每项自带 5s 超时，最坏 ~15s，实测毫秒级）。
- "缺失"是**状态**，不是**异常**。只有 ⑥（启动 dsh）才真正需要前置条件满足。
- 一键安装按依赖序执行，每装完一项**就地重探**该项；全部装完自动重跑整轮体检，全绿才继续。

---

## 2. 数据契约（先定接口，再写实现）

### 2.1 新增依赖（`src-tauri/Cargo.toml`）

```toml
[dependencies]
tauri = { version = "2", features = [] }
tauri-plugin-dialog = "2"        # D2：原生文件选择器
serde = { version = "1", features = ["derive"] }   # tauri 已间接依赖，这里显式化
serde_json = "1"                 # D2：配置文件
```

### 2.2 事件与命令清单

| 方向 | 名称 | 载荷 | 说明 |
| --- | --- | --- | --- |
| R→页面 | `doctor://step` | `Step` | 单项探测完成即推（渐进填充，但**不阻塞**） |
| R→页面 | `doctor://done` | `{ allOk: bool, missing: string[] }` | 统一汇报结束，此时才启用底部按钮 |
| R→页面 | `install://progress` | `InstallProgress` | 安装中的节流进度（含日志尾部） |
| R→页面 | `install://done` | `{ kind: string, ok: bool, error?: string }` | 单项安装结束 |
| 页面→R | `doctor_run` | — | 重跑整轮体检 |
| 页面→R | `doctor_set_path` | `{ id, path }` | 独立指定，**就地验证**后返回新 `Step` |
| 页面→R | `install_missing` | — | 一键安装（队列由 Rust 侧决定，前端不传参数，避免前端越权） |
| 页面→R | `install_cancel` | — | 取消当前安装 |
| 页面→R | `app_quit` | — | 退出（走既有清理链） |

> 各 `id` 取值固定为：`webview` / `node` / `npm` / `dsh` / `profile`。

### 2.3 Rust 侧类型

```rust
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState { Pending, Checking, Ok, Warn, Missing, Failed, Installing }

#[derive(Clone, serde::Serialize)]
pub struct Hint {
    /// 面板上展示、可一键复制的官方命令
    pub command: String,
    /// 官网 / 文档链接
    pub url: String,
    /// 该缺失项能否参与「一键安装」
    pub auto: bool,
}

#[derive(Clone, serde::Serialize)]
pub struct Step {
    pub id: &'static str,
    pub label: String,
    pub state: StepState,
    /// 一句话结论：版本号 / 路径 / 缺失原因。永远非空。
    pub detail: String,
    /// 已生效的自定义路径（用于回显在「指定…」旁）
    pub source: Option<String>,
    pub hint: Option<Hint>,
}

#[derive(Clone, serde::Serialize)]
pub struct InstallProgress {
    pub kind: &'static str,          // "node" | "dsh"
    pub phase: String,               // "下载中" / "安装中" / "初始化"
    pub percent: Option<f64>,        // winget 可给真值；npm 恒为 None
    pub elapsed_ms: u64,
    pub lines: Vec<String>,          // 日志尾部，最多 N 行
}
```

---

## 3. `proc.rs`：统一命令助手（修掉 C1 的既有缺陷）

现有代码 `stderr(Stdio::piped())` 却从不读取——要么丢线索，要么把子进程堵死。所有外部命令都走这里。

### 3.1 关键：Windows 上怎么调 `.cmd`

```rust
// Windows 上 node 是 node.exe，但 npm / dsh 是 .cmd（design.md 第 3 条已确认）。
// 两种调用形态不能混：
//   node.exe  →  Command::new(node_path)              直接用真实 exe，路径含空格也没事
//   npm.cmd   →  Command::new("cmd").args(["/c", path, ...args])
//
// 后者能成立是因为 cmd 的 /c 引号剥离规则：命令行中"恰好两个引号、两个引号之间
// 是带空格的合法可执行文件名、且无 & < > ( ) @ ^ | 等特殊字符"时，引号被保留。
//   cmd /c "C:\Program Files\nodejs\npm.cmd" --version   ← 正确工作
// 若用户指定的路径里含 & 等元字符，这个规则失效 → 因此下面 provided_path()
// 会先做字符白名单校验，再允许写入配置。
```

```rust
/// 校验用户提供的路径：必须是存在的文件，且不含 cmd 元字符。
/// 这是「指定安装位置」唯一的注入面，必须在入口挡住。
pub fn validate_user_path(raw: &str) -> Result<std::path::PathBuf, String> {
    let p = std::path::PathBuf::from(raw.trim().trim_matches('"'));
    if !p.is_file() {
        return Err(format!("不是一个文件：{}", p.display()));
    }
    const BAD: &[char] = &['&', '|', '<', '>', '^', '"', '\n', '\r', '%', '!'];
    if raw.chars().any(|c| BAD.contains(&c)) {
        return Err("路径里含有命令行特殊字符，请把 dsh / node 装到普通路径下（如 C:\\nodejs）".into());
    }
    Ok(p)
}
```

### 3.2 一次性捕获（探测用）

```rust
pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// 跑一条命令拿全部输出。stdout / stderr 各起一个线程读干，**绝不**留未消费的管道。
/// 超时 → taskkill /T /F（node/npm 都是会分叉的父进程）+ 标记 timed_out。
pub fn run_capture(program: &str, args: &[&str], timeout: Duration) -> std::io::Result<Output> {
    let mut child = spawn_hidden(program, args)?;
    let out_rx = drain(child.stdout.take());
    let err_rx = drain(child.stderr.take());

    let deadline = Instant::now() + timeout;
    let code = loop {
        if let Some(st) = child.try_wait()? { break st.code(); }
        if Instant::now() >= deadline {
            kill_tree(child.id());
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(40)); // 轮询而非 wait_timeout：不引依赖
    };

    Ok(Output {
        code,
        stdout: out_rx.join().unwrap_or_default(),
        stderr: err_rx.join().unwrap_or_default(),
        timed_out: code.is_none(),
    })
}

fn spawn_hidden(program: &str, args: &[&str]) -> std::io::Result<Child> {
    let mut cmd = Command::new(program);
    cmd.args(args)
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
```

### 3.3 流式执行（安装用，带取消）

```rust
/// 安装命令专用：逐行回调 + 可取消。
/// 与 run_capture 的差别是输出量级——npm 装依赖能刷出上千行，
/// 所以这里只把行丢进 mpsc，由调用方**节流**（见 §6.3），不做 per-line emit。
pub fn spawn_streaming(
    program: &str,
    args: &[&str],
    env_path: Option<String>,
    on_line: impl Fn(Line) + Send + 'static,
    cancel: Arc<AtomicBool>,
) -> std::io::Result<u32> { /* 返回 pid，供取消时 taskkill /T /F */ }
```

---

## 4. `doctor.rs`：五项探测

```rust
pub fn run_all(app: &AppHandle, cfg: &Config) -> Vec<Step> {
    // 顺序执行即可：5 项合计通常 < 300ms。逐项 emit，页面渐进填充，
    // 但**不因 Missing 中断**（D4）。
    let mut out = vec![probe_webview(app), probe_node(cfg), probe_npm(cfg), probe_dsh(cfg), probe_profile()];
    for s in &out { let _ = app.emit("doctor://step", s); }
    out
}
```

### 4.1 ② Node —— "配置优先，PATH 兜底"

```rust
fn probe_node(cfg: &Config) -> Step {
    // 1) 配置里指定过、且文件还在 → 直接用它
    // 2) 否则 where.exe node
    let found = cfg.node.clone().filter(|p| Path::new(p).is_file())
        .or_else(|| which("node"));
    let Some(path) = found else {
        return Step::missing("node", "Node.js 运行时",
            "未检测到 node。dsh 需要 Node.js 才能运行。",
            Hint { command: "winget install --id OpenJS.NodeJS.LTS --exact".into(),
                   url: "https://nodejs.org/en/download".into(), auto: true });
    };
    // 就地验证版本：命中路径 ≠ 能用（可能是个坏 shim）
    match version_of(&path, &["--version"]) {
        Some(v) if ver_ge(&v, (18, 0, 0)) => Step::ok("node", "Node.js 运行时",
            format!("v{v} · {path}")),
        Some(v) => Step::warn("node", "Node.js 运行时",
            format!("v{v} 过低（需要 ≥ 18）· {path}")),
        None => Step::failed("node", "Node.js 运行时", format!("无法执行：{path}")),
    }
}
```

### 4.2 各步探测手段一览（含踩坑点）

| 步骤 | 手段 | 关键点 |
| --- | --- | --- |
| ① `webview` | `window.webview_version()` | 本机 `152.0.4191.66`。**D3 后只展示 + 过低告警**，不提供安装 |
| ② `node` | 配置路径 → `where.exe node` → `node --version` | `where` 未命中时退出码 1 + stderr 有 `INFO:`，**按退出码判**，别 grep 文案 |
| ③ `npm` | 配置路径 → `where.exe npm` → `cmd /c npm --version` | **必须 `cmd /c`**：PowerShell 里 `Get-Command npm` 命中的是 `npm.ps1`（实测），直连 `.ps1` 会踩执行策略 |
| ④ `dsh` | 配置 → `where.exe dsh` → 版本降级链（§4.3） | 缺失时 hint 的 `auto: true`，命令 `npm i -g @deepseek-ai/dsh` |
| ⑤ `profile` | 建 `%USERPROFILE%\.dsh` + 写读删 `.dshell-writetest` | 只 Warn 不阻断；提前暴露"目录不可写"这类后面才炸的问题 |

### 4.3 ④ 的版本降级链（P1 未决的兜底）

```rust
fn dsh_version(path: &Path) -> Option<String> {
    // 1) dsh --version —— 该 CLI 是否支持**待实测**（01 文档 P1）
    if let Some(v) = version_of(path, &["--version"]) { return Some(v); }
    // 2) npm ls -g @deepseek-ai/dsh --depth=0 --json → dependencies 里的 version（权威）
    // 3) 都没有 → 只报"已安装，版本未知"，绝不因此阻断
    npm_global_version("@deepseek-ai/dsh")
}
```

另：`npm ls -g` 顺带证明"它确实由 npm 全局安装"，这决定一键安装时该往哪写。

---

## 5. 配置持久化（C9）

```rust
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Config {
    #[serde(default)] pub node: Option<String>,
    #[serde(default)] pub npm:  Option<String>,
    #[serde(default)] pub dsh:  Option<String>,
    #[serde(default)] pub npm_prefix: Option<String>,  // 装完 dsh 后用来定位 dsh.cmd
}

impl Config {
    /// 读失败 / 解析失败 / 字段类型不对 → 一律回落到 Default，**绝不阻断启动**。
    /// 路径失效在 probe 阶段逐项判 is_file()，不在这里做全量校验。
    pub fn load() -> Self { /* %USERPROFILE%\.dshell\config.json */ }
    /// 先写临时文件再 rename，避免写一半断电留下半个 JSON。
    pub fn save(&self) -> std::io::Result<()> { /* … */ }
}
```

`%USERPROFILE%\.dshell\` 沿用现有目录（日志已在用），不新增目录。

---

## 6. `installer.rs`：一键安装

### 6.1 命令链（D3：只这两项）

| kind | 命令 | 装到哪 | 权限 |
| --- | --- | --- | --- |
| `node` | `winget install --id OpenJS.NodeJS.LTS --exact --accept-package-agreements --accept-source-agreements` | `C:\Program Files\nodejs` | **会弹 UAC**，UI 必须提前说明 |
| `dsh` | `cmd /c npm i -g @deepseek-ai/dsh` | `%APPDATA%\npm`（实测 prefix） | 不需要管理员 |

降级链（失败时展示给用户复制，不自动做）：

- node：winget 失败 / 被策略禁用 → 打开 `https://nodejs.org/en/download`。
- dsh：npm 失败 → 展示原始 `stderr` + `https://github.com/deepseek-ai/deepseek-harness`；可选追加 `--registry=https://registry.npmmirror.com`（**默认不加**，避免镜像版本偏差）。

### 6.2 安装后的两条硬约束

```rust
// ① PATH 不会自动更新：winget 装完 node，本进程的 PATH 里仍然没有 nodejs 目录，
//    所以安装成功后**不能靠 where node 复查**。按已知安装位置直接找：
const NODE_CANDIDATES: &[&str] = &[
    r"C:\Program Files\nodejs\node.exe",
    r"%LOCALAPPDATA%\Programs\nodejs\node.exe",   // winget 部分版本的落点
];
// 找到后回填 Config::node，并在启动 dsh 时把这个目录前置进子进程 PATH（C10）。

// ② dsh 装完落点是 npm prefix，不是 PATH 里的旧值：
//    let prefix = npm prefix -g;  →  {prefix}\dsh.cmd
//    同样回填 Config::dsh，后续直接用它，不再依赖 where。
```

### 6.3 进度：能给的和给不了的（§7 的现实边界落地）

```rust
// winget：输出里有带百分比的进度行 → 摘出 percent = Some(…)
// 但！stdout 被重定向（非 tty）时 winget 可能不渲染进度条 → percent 退化为 None，
// 自动回落到不确定进度条。这条列为第三轮第一个要实测的点（P9）。

// npm：没有稳定百分比。确定方案 = 不确定进度条 + 日志尾部 + 已用时。
// 节流：行只入环形缓冲（最近 200 行），每 120ms 才 emit 一次，
// 否则上千行会把 webview 打爆。
struct Throttle { ring: VecDeque<String>, last: Instant }
```

---

## 7. Tauri 2 接线（D1）

### 7.1 配置与权限

```jsonc
// src-tauri/tauri.conf.json
{
  "app": {
    "withGlobalTauri": true,          // D1：把 __TAURI__ 注入页面，无需打包器
    "windows": [],                    // 窗口仍在 main.rs 里代码创建（现状不变）
    "security": { "csp": null }
  }
}
```

```jsonc
// src-tauri/capabilities/default.json（新增）
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "DShell 启动页：事件监听 + 原生文件选择",
  "windows": ["main"],                                  // 匹配代码创建时的 label
  "permissions": ["core:default", "dialog:default"]     // core:default 含 event 的 listen/emit
}
```

> ⚠️ 待验证（01 文档 P3）：Tauri 2 的 ACL 主要约束**插件**命令；应用自定义的 `#[tauri::command]` 按文档应直接可调用，但 `core:event` 的 `listen` **必须**授权。第三轮第一件事就是拿一个 `ping` 命令打通这条链路，再往下铺。

### 7.2 命令注册与"别阻塞主线程"

```rust
// 【重要】Tauri 2 里非 async 的 command 跑在**主线程**上，会卡住窗口。
// 我们全是阻塞式子进程调用 → 必须 async + spawn_blocking。
#[tauri::command]
async fn doctor_run(app: AppHandle) -> Result<Vec<Step>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let cfg = Config::load();
        Ok(doctor::run_all(&app, &cfg))
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn install_cancel(app: AppHandle) {
    // 记账的 Child → taskkill /PID <pid> /T /F
    // 安装中的状态放在 app.manage(InstallState(Mutex::new(None)))
}

#[tauri::command]
async fn doctor_set_path(app: AppHandle, id: String, path: String) -> Result<Step, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let p = proc::validate_user_path(&path)?;          // §3.1 唯一注入面
        let step = doctor::probe_one(&id, &p)?;            // 就地验证：真跑一次 --version
        if matches!(step.state, StepState::Ok) {
            Config::load().with_path(&id, &p).save().ok(); // 验证通过才落盘
        }
        let _ = app.emit("doctor://step", &step);
        Ok(step)
    }).await.map_err(|e| e.to_string())?
}
```

```rust
// main.rs：注册插件与命令
tauri::Builder::default()
    .plugin(tauri_plugin_dialog::init())        // D2
    .invoke_handler(tauri::generate_handler![
        doctor_run, doctor_set_path, install_missing, install_cancel, app_quit
    ])
```

### 7.3 主流程编排（改 `main.rs` 的 `setup` 分支）

```rust
// 现状：setup → 建窗 → 起 worker → 抓 URL → navigate
// 改造：setup → 建窗 → 起 worker →
//          ① 跑 doctor::run_all（逐项 emit）
//          ② emit doctor://done { allOk, missing }
//          ③ allOk ? 原「抓 URL → navigate」原样续用
//                  : 停在报告页，等页面 invoke（一键安装 / 指定 / 退出）
//
// 关键：**原 handoff 代码一行不改**，只是它的调用前提从"立刻"变成"体检全绿"。
// 这样即使体检模块整体出问题，也不会破坏已验证过的核心链路。
```

体检结果与安装日志分别写 `%USERPROFILE%\.dshell\dshell-poc.log`（已有）与新增的 `dshell-install.log`（npm 输出量大，不与主日志混）。

---

## 8. 启动页改造（C4/C5/C7）

保留现有品牌动效（流光/脉冲/网格）但缩小到顶部装饰，主体让给步骤列表。

### 8.1 结构

```html
<ol id="steps">
  <li class="step" data-id="node">
    <span class="ico"></span>
    <div class="body">
      <b>Node.js 运行时</b>
      <span class="detail">v22.22.2 · C:\Program Files\nodejs\node.exe</span>
    </div>
    <div class="acts"><button class="pick">指定…</button></div>
  </li>
  <!-- webview / npm / dsh / profile 同构 -->
</ol>

<section id="install-panel" hidden>
  <div class="bar"><i style="width:0%"></i></div>   <!-- percent=None 时切到 .indeterminate -->
  <pre id="install-log"></pre>
  <div class="meta">已用 12s</div>
</section>

<div id="actions">
  <button id="install" disabled>一键安装</button>
  <button id="quit">退出</button>
</div>
<a class="skip" href="#" hidden>跳过检查直接启动</a>   <!-- 次级逃生口，见 01 文档 D4 -->
```

### 8.2 接线（关键片段）

```js
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// 状态放本地 Map，事件只推"增量"，重绘由 render() 统一负责
const steps = new Map();

listen('doctor://step', e => { steps.set(e.payload.id, e.payload); render(); });
listen('doctor://done', e => {
  document.getElementById('install').disabled = e.payload.missing.length === 0;
  document.getElementById('quit').textContent =
    e.payload.allOk ? '退出' : '退出程序';
});
listen('install://progress', e => paintProgress(e.payload));   // 已节流，可直接画

// 「指定…」= 原生选择器 + 就地验证（失败时把 Rust 的报错原样显示在该行）
async function pick(id) {
  const p = await window.__TAURI__.dialog.open({
    multiple: false, directory: false,
    filters: [{ name: '可执行文件', extensions: ['exe', 'cmd', 'bat'] }],
  });
  if (!p) return;
  try { const step = await invoke('doctor_set_path', { id, path: p });
        steps.set(step.id, step); render(); }
  catch (err) { toast(String(err)); }
}

document.getElementById('install').onclick = () => invoke('install_missing');
document.getElementById('quit').onclick    = () => invoke('app_quit');
```

### 8.3 与现有通道的关系（降低回归风险）

现有 `window.__dshell.statusB64 / failB64 / leave` 这套 eval 通道**保持原样不动**：`leave()` 仍是 handoff 前淡出的唯一手段（已验证），`failB64()` 仍负责 ⑥ 步失败的红卡片。新增的事件通道只服务体检与安装。两套机制职责不重叠，出问题时能独立定位。

---

## 9. 第三轮实施顺序

| 序 | 动作 | 完成标志 |
| --- | --- | --- |
| 0 | **先打通 IPC**：`withGlobalTauri` + `capabilities/default.json` + 一个 `ping` 命令 | 启动页能 `invoke('ping')` 并收到事件（验证 P3） |
| 1 | `proc.rs`：`run_capture` / `validate_user_path` / `kill_tree` | 缺 dsh 时能拿到 cmd 的真实 stderr，不再只有一行"没打印地址就退出" |
| 2 | `doctor.rs` 五项探测 + `doctor://step` | 页面能顺序打出 5 行结论（含本机 `152.0.4191.66` / `v22.22.2` / dsh 缺失） |
| 3 | 启动页改版：步骤列表 + 两个按钮（先不接安装） | 统一汇报能看，按钮状态正确 |
| 4 | `doctor_set_path` + 原生选择器 + 配置落盘 | 指向 `notepad.exe` 被拒；指向 `node.exe` 转绿并记住 |
| 5 | `installer.rs` + `install://progress` | 一键装 dsh 出日志流；装完自动重探并转绿 |
| 6 | winget 进度实测（P9）+ node 安装 | 能给出真百分比则用，否则确定进度条 |
| 7 | 取消 / 超时 / 清理 + `DSHELL_DOCTOR_MOCK` | 取消后无残留 node；mock 可复现三个失败分支 |
| 8 | 按 `01-env-doctor.md` §10 跑 12 条验证 + 更新 README/roadmap | 全部通过，本节写回实测结果 |

**新增文件**：`src-tauri/src/{proc,doctor,installer,config}.rs`、`src-tauri/capabilities/default.json`
**修改文件**：`src-tauri/src/main.rs`（编排 + 命令注册）、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`、`ui/index.html`、`README.md`、`docs/roadmap.md`

---

## 10. 需要在第三轮实测确认的小项（都不阻塞开工）

| # | 项 | 兜底 |
| --- | --- | --- |
| P1 | ~~`dsh --version` 是否支持~~ | **✅ 已实测支持**（`0.1.5-rc.1`），降级链第一环可用 |
| P2 | Node 最低版本（包内无 `engines`） | 暂定 ≥ 18，实测校准 |
| P3 | ~~应用命令是否需 capability~~ | **✅ 已实测不需要**；页面成功 `invoke('doctor_run')`，IPC 落地无阻 |
| P9 | winget 在重定向 stdout 下是否输出百分比 | 退化为不确定进度条，功能不受影响（仍未实测：本机已装 Node） |
| P10 | `webview_version()` 在 Tauri 2 的确切签名 | 实现改为 `reg query` 读注册表 `EdgeUpdate\Clients\{F3017226-…}` 的 `pv`，绕开 API 形状问题（本机读到 152.0.4191.66） |
