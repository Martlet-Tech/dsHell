# 09 · dsh 版本更新检查与更新

> **立项日期**：2026-09-20
> **状态**：**在办**（仅立项，未实现）
> **对应 roadmap**：`roadmap.md` 第 3 条
> **前置**：[closed/07 单实例](../closed/07-single-instance.md)、[closed/08 重启 dsh](../closed/08-restart-dsh.md)（均已收口）
> **红线**：不碰 dsh 源码

---

## ① 方案

### 要解决什么

用户不会自己更新 dsh（现状是让智能体代劳 `npm i -g @deepseek-ai/dsh`）。目标：在 DShell 里
看到"有没有新版"、一键更新、且**更新完自动生效**——不用关掉 DShell 再打开。

### 为什么前置是那两项

| 前置 | 为什么不可省 |
| --- | --- |
| 单实例 | `npm i -g` 要覆盖全局安装目录里的文件。只要还有**任意一个** dsh 后端活着，node 加载中的 native addon 就锁着那些文件 → 安装失败或留下半装 |
| 重启 dsh 后端 | dsh 是 exe `spawn` 的独立进程，exe 自己不加载 dsh 代码。装完必须重起**子进程**才生效（不需要重开 exe） |

两项都已收口，所以本项现在才具备开工条件。

### 落点：托盘两项，**不进启动体检**

体检里的每一项都可能拦住启动——`doctor::blocking_ids` 的返回值直接决定 `run_doctor` 的
`all_ok`，而 `all_ok` 决定要不要 handoff。**更新检查要联网，联网就会失败**；放进体检等于让
"今天网络抖动"成为一个能拦住用户打开 DShell 的理由。

这与 `picker` 那条是同一个道理（"增强项绝不因失败而拦住启动"），只是 picker 的处置方式是
从 `blocking_ids` 里排除。更新连排除都不必——它根本不进体检。

托盘加两项：

| 菜单项 | 性质 | 说明 |
| --- | --- | --- |
| 检查 dsh 更新 | 信息 | 查版本并报告，不改动任何东西 |
| 更新 dsh | 动作 | 停 → 装 → 起 |

**「更新 dsh」不要因为"没查到新版"就灰掉**：它对"npm 全局安装坏了 / 文件缺失"同样有效，
是一个修复入口，独立有价值。

### 为什么不能直接复用「一键安装」

`install_missing` 的执行器可以复用，**触发器不能**。队列来自 `doctor::auto_ids`，而它只在
`dsh` 处于 `Missing` / `Failed` 时才入列；`probe_dsh_at` 对**版本过旧的 dsh 返回 `Ok`**
（它只关心"在不在、能不能问出版本"）。所以现有通道是**补齐型**，不是**版本型**——点一百次
「一键安装」也不会把 `0.1.5-rc.1` 升上去。

| | 可复用 | 要新写 |
| --- | --- | --- |
| 执行 | `installer::run(Kind::Dsh)`（跑的就是 `npm i -g`，对已装包语义即升级）<br>`installer::locate_and_remember`（回填 `cfg.dsh`） | — |
| 界面 | `install://progress` / `install://done` 事件 + 启动页现成的安装面板 | 一个"更新模式"的显示切换 |
| 判断 | `doctor::npm_global_version`（证明"确实由 npm 全局安装"） | 版本比较与"要不要升"的决策 |

### 核心设计一：把「重启」拆成停 / 起两个原语

`reset_handoff`（`main.rs`）**已经是一个完整的"停"原语**——它做了作废代数 → 清
`launch_url` → 复位 `handoff_started` → `kill_tree` → **`wait_process_exit` 超时确认**。
只是名字叫 handoff，看不出可以复用。

拆法（**不是**"重启 / 停 / 起"，三层里没有重复动作）：

```
restart_dsh = show_splash + stop_dsh + start_dsh
update_dsh  = show_splash + stop_dsh + install(dsh) + start_dsh
                                        ↑ 唯一的差别
```

先做这一步拆分（**纯重构，行为不变**），它本身就是更新的技术底座；拆完先跑通重启回归，
再往上加更新。

### 核心设计二：抑制体检的闩必须跨"停 + 装"全程

`restart_guard` 现在的生命周期是"设 true → `run_doctor` 之前清掉"。更新会把窗口导航回启动页，
而**启动页加载完就自动 `invoke('doctor_run')`**（那是首启的设计：监听就绪后再叫，事件不丢）。
若在"装"的过程中把它放行，启动页会一路 handoff 出**第二个 dsh 后端**——正是 08 花大力气堵掉的
那个坑，而这里的情况更糟：旧后端已经停了，新后端是在 `npm i -g` **覆盖文件的中途**起来的。

所以更新时这个闩必须覆盖 **stop + install** 全程，只在 `start_dsh` 前放开。它的职责其实不是
"重启守卫"，而是"**抑制启动页自发的体检**"，名字应一并改准。

### 核心设计三：更新进度复用现有安装面板，零新增 UI

启动页已经有全套东西：面板本体、百分比 / 已用时 / 进度条 / 取消 / 日志尾部，以及驱动它的
`paint()`；而 `installer::run` 发的**就是**它监听的那两个事件。所以更新只要走 `installer::run`，
进度条自己就画出来了。

要补的只是用户提出的那个**模式切换**：

```
点「更新 dsh」 → 回启动页 → stop → 显示"正在更新 dsh"+实时进度
    → 装成功 → 正常跑体检（步骤时间线填回来）→ 全绿 → leave() 进 dsh
    → 装失败 → 面板停"更新失败"+失败卡片；此时 dsh 已停，必须给"重新启动"的出口
```

更新期间步骤时间线语义不对（6 行"等待检查"空杵在面板上方），所以在更新模式下把它隐藏，
装完再显示——**这正是用户描述的顺序，且是顺着现有结构走，不是硬塞**。

### 版本检查怎么算

| 要点 | 做法 | 为什么 |
| --- | --- | --- |
| registry | **必须与安装用同一个**（本机 `npm config get registry` = `registry.npmmirror.com`） | 检查用的 registry 与装的不是同一个 → "说有大版本、装完却不是"。用户 `.npmrc` 里已有的配置不要覆盖 |
| 取哪些版本 | `npm view @deepseek-ai/dsh dist-tags --json` 的**全部** tag | 只看 `latest` 会得出错误结论：本机是预发布 `0.1.5-rc.1`，若 `latest` 指向 `0.1.4` 就会**提示降级** |
| 比较 | semver 感知的**预发布比较** | 字符串比较会把 `0.1.5-rc.1` 与 `0.1.5` 判错 |
| 决策 | 查到新版**问用户**，不静默升级 | 用户明确要求 |

### 来源校验：只更新"确实由 npm 全局安装"的那份

若用户的 dsh 来自 pnpm / npx / 别处，`npm i -g` 会装出**第二份** DShell 根本不启动的 dsh，
比不更新更糟。所以执行前必须校验当前 `cfg.dsh` 确实对应 npm 全局那份。

现成能力：`doctor::npm_global_version("@deepseek-ai/dsh")` 能同时给出"安装版本"和"它确实由
npm 全局装着"。本机实测：`where dsh` → `%APPDATA%\npm\dsh.cmd`，`npm prefix -g` →
`%APPDATA%\npm`，**一致**。

### 本轮交付边界

**做**：拆分 stop / start；托盘两项；版本检查（含来源校验）；更新执行（停→装→起）；
更新进度显示与模式切换。

**不做**：自动 / 静默更新；后台轮询检查；DShell 自身（exe）的更新——那是另一件事。

### 前端模块化：按**职责**拆，不按**状态**拆

`ui/index.html` 现在 651 行（CSS ~310 / HTML ~52 / JS ~278），09 还要再加一个"更新模式"。
确实该拆，但**拆的轴不能是"状态"**。

**为什么不能按状态拆成多个文件**：启动页的四种状态（体检中 / 安装中 / 更新中 / 失败）
共用**同一份 DOM**（步骤时间线 + 面板 + 失败卡片 + 动作区）和**同一条事件总线**。
按状态拆成多个文档，状态切换就变成**页面导航** —— 而"导航会丢掉监听者"正是 08 花大力气
堵掉的那个 bug（`about:blank`、`SPLASH_SETTLE`、`restart_guard` 全是为它而设）。
重启一次已经要"导航回启动页 + 等 700ms 让事件注册好"，再把状态切换也变成导航，
等于把一个已修好的缺陷类主动请回来。

所以：**状态是数据，不是文件**。切换状态是同一个 DOM 上的函数调用（现在的
`restarting()` 就是这个形状），不是加载另一个页面。

**按职责拆**（无构建步骤、无依赖，直接 ES module）：

| 文件 | 职责 |
| --- | --- |
| `ui/index.html` | 只有结构骨架 + `<link>` + `<script type="module">` + 一段内联兜底 |
| `ui/app.css` | 样式（含更新态） |
| `ui/js/labels.js` | 步骤文案表（`LABELS` / `PICKABLE`） |
| `ui/js/steps.js` | 步骤时间线：`ensureRow` / `renderStep` |
| `ui/js/install.js` | 安装 / 更新面板：`paint` / `installRunning` / 取消 |
| `ui/js/transport.js` | `window.__TAURI__` 获取、事件订阅、`invoke` 包装、`toast` |
| `ui/js/app.js` | 编排：`wire()`、模式切换、`window.__dshell` 老通道 |

**两个必须守住的东西**：

1. **`window.__dshell` 的契约不变。** Rust 通过 `window.eval("window.__dshell&&…")`
   调用 `leave()` / `restarting()` / `statusB64()` / `failB64()`（`main.rs`）。
   拆成 module 后这些仍必须显式挂在 `window` 上——module 作用域不污染 `window`，
   少挂一个就是**静默无反应**（Rust 侧 `&&` 会安静跳过）。
2. **保留一段内联兜底**。module 是外部文件，多出一种失败方式：文件没取到 → 页面上
   什么都不显示。而启动页**是唯一能显示失败的地方**。所以 `index.html` 里要留几行
   内联脚本，监听 `window.onerror` 与 module script 的 `onerror`，至少画出"启动页加载失败"。
   这正是 06 那类"页面自己加载不出来"的教训。

**风险（首次落地必须实测）**：ES module 走 Tauri 的 asset 协议（`http://tauri.localhost/`），
浏览器对 module 脚本做严格 MIME 校验 —— 若 `.js` 的 Content-Type 不对，module 会被拒绝执行。
`csp: null`，所以 CSP 不是变量。**第一次跑就必须看控制台/日志确认**（同 08 的教训：
读运行时状态的地方，第一次落地就把结果记进日志）。

**时机**：作为 09 的**第一步**先做（纯重构，行为不变，单独一次提交），再加更新模式。
先拆后加，更新模式就是直接生在正确结构里；反过来要做两遍。

**按窗口拆是可以的**（roadmap §4 的设置窗口会是独立的 HTML）——那是**不同窗口**，
不是同一窗口的不同状态。

### 实施顺序

1. 拆分 `stop_dsh` / `start_dsh` / `show_splash`（纯重构）→ **先跑通重启回归**
2. UI 按职责拆成 module（纯重构）→ **实测 module 能被加载**（MIME 校验）
3. 托盘两项 + `ui_text` 文案（此时"更新"可先只做"检查"）
4. 版本检查：registry 感知的 dist-tags 采集 + 预发布 semver 比较 + 来源校验
5. 更新执行：`stop → installer::run(Dsh) → start`，闩跨全程
6. 前端 `updating()` 模式切换 + 失败后的"重新启动"出口

---

## ② 关键代码

### main.rs —— 重启拆成原语

```rust
/// 把窗口导航回启动页并清掉上一轮的残留（失败卡片 / 步骤 / 面板）。
/// 拿不到 splash URL 时返回 false（调用方据此放弃，而不是留在 dsh 页面上）。
fn show_splash(app: &AppHandle, window: &WebviewWindow) -> bool;

/// 停 dsh：作废代数 → 清 launch_url → 复位 handoff_started → kill_tree → wait_process_exit。
/// 返回"是否确认已退出"。**现 `reset_handoff` 改名**，逻辑不动。
fn stop_dsh(app: &AppHandle) -> bool;

/// 起 dsh：跑体检，全绿则 handoff。**现 `run_doctor(app, w, true)` 的调用点**。
fn start_dsh(app: &AppHandle, window: &WebviewWindow);

fn restart_dsh(app: &AppHandle);   // show_splash + stop_dsh + start_dsh
fn update_dsh(app: &AppHandle);    // show_splash + stop_dsh + install(Dsh) + start_dsh
```

`restart_guard` 的持有范围（更新时）：

```rust
guard.store(true);          // 从导航回启动页之前就开始抑制
show_splash(..);
if !stop_dsh(app) { /* 放弃：旧后端没死，装下去会撞会话锁 */ }
let outcome = installer::run(app, Kind::Dsh, &state);   // 期间启动页的 doctor_run 被抑制
guard.store(false);         // 放开，让接下来的体检能跑
start_dsh(app, &window);
```

### 版本检查（新增模块，建议 `update.rs`）

```rust
pub struct DshUpdate {
    /// 当前安装版本（来自 npm 全局清单；None = 问不出来）
    pub current: Option<String>,
    /// 候选最新版本（dist-tags 里 semver 最大的那个，含预发布）
    pub latest: Option<String>,
    /// latest 是否确实比 current 新
    pub newer: bool,
    /// 当前 dsh 是否确实由 npm 全局安装（false → 拒绝执行更新）
    pub npm_global: bool,
}

pub fn check(cfg: &Config) -> Result<DshUpdate, String>;
```

要点：

```rust
// current：复用既有能力（同时证明"确实 npm 全局装着"）
doctor::npm_global_version("@deepseek-ai/dsh")

// latest：**不覆盖用户 registry**，用用户自己配置的那个
proc::run_shell(&["npm", "view", "@deepseek-ai/dsh", "dist-tags", "--json"], T)

// 比较：core 段数值比较，core 相等时按 semver 规则处理预发布
//   无预发布 > 有预发布（1.0.0 > 1.0.0-rc.1）
//   都有预发布 → 逐段比较（数值段按数值）
```

### 托盘与文案

```rust
const MENU_ID_CHECK_UPDATE: &str = "tray-check-update";
const MENU_ID_UPDATE_DSH:   &str = "tray-update-dsh";

enum TrayAction { RestoreWindow, OpenInBrowser, RestartDsh, CheckUpdate, UpdateDsh, Quit }

// ui_text.rs
pub fn menu_check_update() -> &'static str;   // "检查 dsh 更新"
pub fn menu_update_dsh()   -> &'static str;   // "更新 dsh"
```

### 前端（ui/index.html）

```js
// 更新模式：隐藏步骤时间线（更新期间它们语义不对），面板标题改成"正在更新"
window.__dshell.updating();

// 已有的可直接复用：
//   install://progress → paint(p)   进度条 / 百分比 / 日志尾部
//   install://done     → 标题改"安装完成，正在复检…" / "安装失败"
//   restarting()       → 清残留（更新开始时同样要用）
```

### 关键事件与既有契约

| 事件 | 载荷 | 来源 |
| --- | --- | --- |
| `install://progress` | `Progress { kind, phase, percent: Option<f64>, elapsed_ms, lines }` | `installer.rs` |
| `install://done` | `{ kind, ok, cancelled, error }` | `installer.rs` |
| `doctor://step` | `Step { id, state, detail, hint }` | `doctor.rs` |
| `doctor://done` | `{ allOk, missing, auto }` | `main.rs` |

`percent: None` = npm 给不出真实百分比，前端切成不确定进度条（**不编数字**）。

---

## ③ 结案总结

**未收口。** 待实现后补：实测结果、预期与实际的差异、遗留项。

### 开工前需要先定的两个问题

1. **取消的语义**：用户在更新中途点「取消」→ `install_cancel` 会 `kill_tree` 掉 npm。
   但此时 dsh **已经被停了**，取消的后果是"没有后端可用"。需要明确：取消后是自动
   `start_dsh` 起回旧的，还是停在失败卡片上给"重新启动"按钮（倾向后者，语义更诚实）。
2. **要不要引 semver crate**：上一轮（07/08）刻意保持**零新增依赖**（手写 Win32 绑定
   而不引 `windows-sys`）。预发布比较手写约几十行；是否值得为此引 `semver` 需要定夺。
   倾向**手写**，与既有取向一致，但要在文档里写清比较规则并补测试。
