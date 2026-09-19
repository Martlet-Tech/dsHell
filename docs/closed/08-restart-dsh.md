# 08 · 重启 dsh 后端

> 状态：**已收口**（2026-09-20）。实测通过（含一次实测暴露并修复的 `about:blank` bug），见 ③。

对应 `roadmap.md` 第 4 条（版本更新）的**前置**，该条仍在办。本项**不含**版本检查与更新执行本身 ——
它与更新解耦，独立就有价值（见下）。

## ① 方案

### 为什么要单独做「重启」

用户点「重启」的常见动机都不是"更新"，而是"我改了什么，让它按新的来"：

- 改了 dsh / node 的路径（`config.json` 里的指定）；
- 手工装/卸了 dsh 插件（dsh 只在启动时读一次 profile —— `install-picker-plugin.ps1` 的注释里
  专门警告过"装了却没生效"这种最难查的情况）；
- dsh 卡住了，想原地重来；
- 将来的版本更新。

所以它**不绑在更新上**单独交付：无论更新做不做，它都有用；而且它是更新的技术底座 ——
更新执行完必须重起 dsh 才生效。

### 关键事实：只需重启 dsh 子进程，不用重启 exe

dsh 是 exe **spawn 出来的独立进程**（`spawn_dsh`），exe 自己不加载 dsh 的代码。
所以更新完不需要让用户重新双击 exe，重起子进程就够了。代价是几秒 + 页面重新导航 + 重新握手。

### 核心障碍：`start_handoff` 原本是"一次性"的

```rust
if state.handoff_started.swap(true, Ordering::SeqCst) { return; }   // 原第 193 行
```

它假定「起 dsh → 导航 → 结束」只发生一次。要重启，需要处理四件事：

| 要处理的 | 为什么 |
| --- | --- |
| `handoff_started` 复位 | 否则第二次直接早退，什么都不发生 |
| `dsh_pid` 换新 | 退出清理链只认这一个 pid |
| **确认旧进程真的死了** | 不能 `taskkill` 完立刻起新的 —— dsh 持有会话锁，撞上就是 `SessionAlreadyOwnedError` |
| `launch_url` 清空 | 托盘「在浏览器中打开」读它；重启后 token 变了，旧地址会打开一个 401 死页面 |

### 设计：用"代数"分流过期的等待线程

难点在于**旧进程被杀会让旧等待线程报错**。`wait_for_url` 等的是那个进程的输出，
进程一死它就返回 `Err("dsh 还没打印地址就退出了")` —— 那是**上一代的正常死亡**，
不是故障。没有机制区分的话，重启时就会往刚回到启动页的窗口上画一张假的失败卡片。

解法是给每次 handoff 一个**代数**（`dsh_generation`）：等待线程记住自己那一代的号，
上报前先问一句"我还是当前代吗"，不是就安静退出。这个机制换掉了原来"只跑一次"的
假设，让 `start_handoff` 变成可重入的。

顺带堵上另一条窄缝：重启时旧进程若**恰好**在被杀前打印了地址（`Ok` 分支），
同样按代数判定忽略掉。

### 第二个陷阱：启动页会自己叫体检

重启把窗口导航回启动页，而启动页**加载完就自动 `invoke('doctor_run')`**
（那是首启的设计：监听就绪后再叫，事件不会丢）。如果在旧 dsh 还没停干净时放它跑，
它会一路 handoff 出**第二个 dsh 后端** —— 正是重启最不能出的结果。

所以加一个 `restart_guard`：从"开始重启"到"旧 dsh 停干净"之间，`doctor_run` 直接返回；
停干净之后由重启流程自己叫一次。**成败都要放行**，否则失败卡片上的「重新检查」
会变成死按钮。

### 为什么重置走完整流程（体检 + handoff）

用户点重启最常见的动机就是"我改了路径/装了插件"。只重起进程会**跳过体检**，
把"改了路径却没生效"这类问题留到后面以更难懂的形式暴露。体检本身是秒级的。

### 顺序是被设计过的

```
① 作废当前代数   →  旧等待线程从此只是旁观者
② kill_tree      →  cmd → npm → node 整棵树（只杀直接子进程会留下占着端口的 node）
③ wait_process_exit → 真的等它没了，才允许起新的（会话锁）
④ 复位 handoff_started → 下一个 start_handoff 才不会被闩挡住
```

### 卡死的 dsh 怎么办：放弃，而不是硬上

`wait_process_exit` 给 15 秒上限。超时**不重试、不强起**，而是发一条 `Warn` 步骤 +
失败卡片，让用户「完全退出」DShell 再重开。

理由：卡死的 dsh 会让「重启」永远不返回，而**不重启**比冒险起第二个后端安全得多
—— 两个后端会抢同一份会话锁（roadmap #1 记的 `SessionAlreadyOwnedError`）。

### UI 落点：托盘菜单项 + 复用启动页

硬约束：壳自有 UI 只有四处（启动页、托盘、原生对话框、新建窗口）。往 dsh 页面里注入
UI 是**死的** —— `docs/closed/05` §19.1 实测 `initialization_script` 与 `eval`
两条通道都不可用。

选托盘菜单项（`重启 dsh 后端`），因为：
- 与已有两项（`在浏览器中打开` / `完全退出`）同级，零新增窗口、零新增前端代码；
- 重启时窗口本来就要离开 dsh 页面，托盘是唯一在"dsh 已经死了"时还活着的入口。

进度显示**复用启动页**：`ui/index.html` 已经有整套步骤时间线与失败卡片，重启导航回去
就是顺路的。启动页因此新增一个 `window.__dshell.restarting()` 入口，把上次留下的
失败卡片、步骤状态和安装面板清干净 —— 否则用户会看到上一次会话的残留结论。

页面导航由 `WebviewUrl::App("index.html")` 决定，Windows 上实际是
`http://tauri.localhost/index.html`。**不硬编码**这个形状，也**不能在建窗后立刻读**
—— 用 `.on_page_load` 在页面真正加载时捕获（见 ③ 里实测暴露的 `about:blank` bug）。

### 改完是什么样

托盘右键 → 「重启 dsh 后端」→ 窗口回到启动页并显示体检步骤 → 旧 dsh 被杀、新 dsh 起来
→ 自动进入 dsh。全程不需要用户离开托盘菜单，也不需要重新双击 exe。

## ② 关键代码

### `AppState` 新增字段

| 字段 | 用途 |
| --- | --- |
| `dsh_generation: AtomicU32` | 代数。每起一次 +1；过期的等待线程据此安静退出 |
| `restarting: AtomicBool` | 重启闩。托盘连点两下不该起两条重启线程互相杀 |
| `restart_guard: AtomicBool` | 重启期间抑制启动页自发的 `doctor_run`（否则会多起一个后端） |
| `splash_url: Mutex<String>` | 启动页自己的 URL，建窗后实测一次存下来 |

新增方法：`begin_dsh_generation()` / `is_current_generation(g)` / `dsh_pid()` /
`set_dsh_pid()` / `set_splash_url()` / `splash_url()`。

新增常量：

| 常量 | 值 | 用途 |
| --- | --- | --- |
| `DSH_EXIT_TIMEOUT` | 15 秒 | 等旧 dsh 真正退出的上限 |
| `SPLASH_SETTLE` | 700 毫秒 | 导航回启动页后等它注册好事件监听 |

### `start_handoff` 可重入化

```rust
fn start_handoff(app: AppHandle, window: WebviewWindow) {
    let generation = {
        let state = app.state::<AppState>();
        if state.handoff_started.swap(true, Ordering::SeqCst) {
            return;   // 已经在起 dsh：不重复 spawn
        }
        state.begin_dsh_generation()
    };
    // … spawn_dsh 失败时复位 handoff_started，否则那一代卡死后再也无法 handoff
    // …
    std::thread::spawn(move || {
        match wait_for_url(&mut child, &window, err_tail) {
            Ok(url) => {
                if !app.state::<AppState>().is_current_generation(generation) {
                    log(/* superseded */);
                    return;                        // 恰好成功但已过期
                }
                // … navigate …
            }
            Err(e) => {
                // 重启时旧进程被杀 → 这里必然报错。那不是故障，是上一代的正常收场。
                if !app.state::<AppState>().is_current_generation(generation) {
                    log(/* superseded */);
                    return;
                }
                // … 真的失败：失败卡片 …
            }
        }
    });
}
```

### `reset_handoff`：四步顺序

```rust
fn reset_handoff(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();

    // 1) 作废当前代号
    state.dsh_generation.fetch_add(1, Ordering::SeqCst);

    let pid = state.dsh_pid();
    state.set_dsh_pid(0);
    if let Ok(mut u) = state.launch_url.lock() { u.clear(); }

    // 4 的铺垫：先允许下一次 handoff
    state.handoff_started.store(false, Ordering::SeqCst);

    if pid == 0 { return true; }

    // 2) + 3)
    proc::kill_tree(pid);
    win32::wait_process_exit(pid, DSH_EXIT_TIMEOUT)
}
```

### `restart_dsh` / `do_restart_dsh`

`restart_dsh` 只做闩与开线程，并把闩的清除放进 `catch_unwind` 的**后面** ——
否则 `do_restart_dsh` 里一次 panic 会让 `restarting` 永远为真，重启功能从此静默失效。
（`[profile.release] panic = "abort"`，所以发布版里这个兜底实际不生效 —— 但发布版
panic 会直接终止进程，闩也随之消失，两种 profile 都安全。）

```rust
fn restart_dsh(app: &AppHandle) {
    if app.state::<AppState>().restarting.swap(true, Ordering::SeqCst) {
        log("restart: already in progress - ignoring");
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            do_restart_dsh(&app);
        }));
        if result.is_err() { log("restart: panicked - the restarting latch is being cleared"); }
        app.state::<AppState>().restarting.store(false, Ordering::SeqCst);
    });
}
```

`do_restart_dsh` 的次序：**置 guard → 导航回启动页 → 唤醒窗口 → `reset_handoff` →
清 guard → 超时则失败卡片，否则 `run_doctor`**。

```rust
app.state::<AppState>().restart_guard.store(true, Ordering::SeqCst);
// 导航回启动页 + SPLASH_SETTLE + window.__dshell.restarting()
lifecycle::restore_main_window(app);
let stopped = reset_handoff(app);
// 无论成败都要放行，否则失败卡片上的「重新检查」会变成死按钮
app.state::<AppState>().restart_guard.store(false, Ordering::SeqCst);
if !stopped { /* Warn 步骤 + 失败卡片：请完全退出 DShell 再重开 */ return; }
run_doctor(app, &window, !app.state::<AppState>().hold);
```

### `doctor_run` 加 guard

```rust
if app.state::<AppState>().restart_guard.load(Ordering::SeqCst) {
    log("doctor: suppressed (a restart is in progress)");
    return Ok(());
}
```

### 托盘与文案

- `tray.rs`：新增菜单项 `tray-restart-dsh`、`TrayAction::RestartDsh`，
  `dispatch` 转到 `crate::restart_dsh(app)`；
- `ui_text.rs`：新增 `menu_restart_dsh()` → 「重启 dsh 后端」（保持"文案只在这一处"的约定）。

**顺带去掉了一个泛型**：`tray::init` / `dispatch` 原本写 `R: Runtime`，而重启这条链
（`run_doctor` / `start_handoff` / `splash_*`）都是具体的 `Wry` 运行时。为了托盘一处
把整条链泛型化不值得 —— 本应用只有 wry 一个运行时，改成具体类型后调用点更简单。
这不是为重启而做的妥协，是把原来多余的泛型收掉了。

### 启动页新增入口

```js
// 「重启 dsh 后端」把窗口导航回本页后调用
restarting() {
  done = false;
  stage.classList.remove("leaving");
  failEl.className = "fail";
  failEl.innerHTML = "";
  panel.hidden = true;
  logEl.textContent = "";
  // …复位安装面板文案…
  stepsEl.innerHTML = "";
  ["webview", "node", "npm", "dsh", "profile", "picker"].forEach((id) =>
    renderStep({ id: id, state: "pending", detail: "等待检查" })
  );
},
```

## ③ 结案总结

### 第一轮：暴露 bug（第 2353–2371 行，修复前）

用户实测：首启 → 进 dsh → 托盘「重启 dsh 后端」→ 再次进入 dsh → 再双击 exe。

```
2353: picker: bridge listening on 127.0.0.1:50633
2354: splash window built
2355: splash url: about:blank                     ← ❌ 记错了
2356: doctor: allOk=true missing=[] auto=[]
2357: spawned dsh web (pid 15700, generation 1)
2358: dsh> dsh web: http://127.0.0.1:50698/?token=jCBO…
2359: launch url: http://127.0.0.1:50698/?token=jCBO…
2360: window navigated to dsh
2361: restart: navigated back to the splash page   ← 实际导到了 about:blank
2362: tray: restored main window
2363: restart: killing dsh process tree (pid 15700)
2364: restart: dsh (pid 15700) exited
2365: doctor: allOk=true missing=[] auto=[]
2366: spawned dsh web (pid 11436, generation 3)
2367: dsh> dsh web: http://127.0.0.1:50728/?token=kJTW…
2368: launch url: http://127.0.0.1:50728/?token=kJTW…
2369: window navigated to dsh
2370: single instance: another DShell is already running (focused=true) - exiting
2371: single instance: another DShell is already running (focused=true) - exiting
```

| # | 用例 | 第一轮 | 第二轮（修复后） |
| --- | --- | --- | --- |
| 1 | 托盘 →「重启 dsh 后端」 | ⚠️ 功能通但**白屏** | ✅ **通过** |
| 2 | 任意时刻最多一个后端 | ✅ 15700 退出后才出现 11436 | ✅ 21644→24148、24148→8952 |
| 3 | 日志无假的 FATAL | ✅ 零命中 | ✅ 全日志 `FATAL` **0** |
| 4 | 重启后新 token | ✅ | ✅ 每次都刷新 |
| 5 | 「关到托盘」后重启 | 未覆盖 | ✅ 见下 |
| 6 | 连点两次 | 未覆盖 | 未复测（闩的代码路径明确） |
| 7 | 坏路径体检拦住 | 未覆盖 | 未复测 |

### 第二轮（修复后，第 2385–2403 行）

```
2385: picker: bridge listening on 127.0.0.1:51739
2386: splash window built
2387: splash url: http://tauri.localhost/            ← ✅ 修复生效（原来是 about:blank）
2388: doctor: allOk=true missing=[] auto=[]
2389: spawned dsh web (pid 21644, generation 1)
2392: window navigated to dsh
2393: single instance: another DShell is already running (focused=true) - exiting
2394: restart: navigated back to the splash page
2395: doctor: suppressed (a restart is in progress)   ← ✅ guard 真的被触发了
2396: tray: restored main window
2397: restart: killing dsh process tree (pid 21644)
2398: restart: dsh (pid 21644) exited
2399: doctor: allOk=true missing=[] auto=[]
2400: spawned dsh web (pid 24148, generation 3)       ← ✅ 1 → 3
2403: window navigated to dsh                         ← ✅ 自动进入
```

### 「关到托盘」后重启（第 2404–2416 行）

```
2406: close behavior: hidden to tray (dsh kept alive)   ← 窗口被关到托盘
2407: restart: navigated back to the splash page
2408: doctor: suppressed (a restart is in progress)
2409: tray: restored main window                        ← ✅ 隐藏的窗口被唤出
2410: restart: killing dsh process tree (pid 24148)
2411: restart: dsh (pid 24148) exited
2413: spawned dsh web (pid 8952, generation 5)
2416: window navigated to dsh
```

`restored main window` 出现在重启流程**内部**（杀旧进程之前），且用户在隐藏窗口后仍能
继续操作 → 证明 `SW_SHOW` 那条分支（隐藏窗口**不是最小化**，`SW_RESTORE` 对它无效）
走通了。

**`doctor: suppressed` 是意外收获。** 加 `restart_guard` 时只是按逻辑推断"启动页导航
回来会自己调 `doctor_run`，必须挡住"，没实测过它是否真会触发。日志证明**它确实触发了**
—— 若没有这道 guard，那次调用会在旧 dsh 还活着时一路 handoff 出**第二个后端**。
且它出现的位置（2394 导航之后、2397 杀进程之前）与设计的时间窗完全吻合。

### 代数机制被正面验证

`dsh_generation` 逐次递增（1 → 3 → 5），而**假 FATAL 全日志零命中** —— 这正是加代数
要挡住的东西（旧进程被杀会让旧等待线程报错，那本是"上一代的正常死亡"，没有代数就会
往窗口上画一张假的失败卡片）。

### 实测暴露的 bug：`window.url()` 记到 `about:blank`

**根因**：建窗后**立刻**调 `window.url()`，此时 WebView2 还没提交首次导航，返回
`about:blank`。

**后果**：重启把窗口导航到 `about:blank` → 白屏；`restarting()` 和所有
`doctor://step` 都发给了**没有监听者**的空白页，用户看不到任何进度。

**为什么功能还是通了**：`run_doctor` 在 Rust 侧跑完照样 emit、照样 `start_handoff`
（`then_handoff` 传 `!hold`，不依赖页面），所以 dsh 还是起来了。属于"结果对、过程全错"
—— 页面全程白屏，任何体检失败都会变成无声失败。

**修复**：改用 `.on_page_load` 在页面**真正加载**时捕获 URL，并只认
`tauri` / `asset` scheme 或 `tauri.localhost` 主机（刻意排除 `about:blank`：
它同样以 `about` 开头，但不是启动页）。

```rust
.on_page_load({
    let handle = app.handle().clone();
    move |_window, payload| {
        let url = payload.url().clone();
        let is_splash = matches!(url.scheme(), "tauri" | "asset")
            || matches!(url.host_str(), Some("tauri.localhost"));
        if !is_splash { return; }
        if let Some(state) = handle.try_state::<AppState>() {
            if state.splash_url().as_deref() != Some(url.as_str()) {
                log(&format!("splash url: {url}"));
                state.set_splash_url(url.as_str());
            }
        }
    }
})
```

**同时收紧了"拿不到 URL"的处理**：原来是"留在当前页继续"（错的选择 —— 当前页要么是
马上要被杀的 dsh，要么是空白页），改成**放弃重启 + 明确失败卡片**。宁可不动，
也不要把界面导到一个没监听者的页面上。

### 遗留

| 项 | 说明 |
| --- | --- |
| **为什么这个 bug 漏到实测** | 原注释断言"Windows 上是 `http://tauri.localhost/index.html`"，但代码从没验证过这个形状。教训：**读运行时状态再用它做后续决策的地方，第一次落地就要把读到的值记进日志** —— 这次正因为记了 `splash url: about:blank` 才一眼抓到 |
| 判读强杀类用例的方法 | 强杀的特征是**日志里什么都不留**（`ExitRequested` 不触发）。查这类场景要看进程与端口状态，不能数新增日志行 —— 本次读日志时先走错了这条路 |
| 用例 6/7 | 未复测（闩与体检的代码路径明确） |
| 体检耗时 | 重启会完整跑一遍体检（含网络调用），实际体感待测 |

### 与后续（版本更新）的接口

更新功能只剩三件事：查版本、执行 `npm i -g`、调 `restart_dsh`。
`installer.rs` 目前硬编码 `npm i -g @deepseek-ai/dsh`，而用户可能用 pnpm / npx /
本地 checkout 装 dsh —— **更新前必须校验当前 dsh 确实来自 npm 全局**（`config.rs`
缓存了 `dsh` 路径，与 `npm prefix -g` 比对），否则会装出一份 DShell 根本不启动的
第二份。这条留给 09。

---

## 附注（2026-09-20，立项 [09](../plan/09-dsh-update.md) 时补记）

三处需要更正或补充，**原文保持不动**：

1. **上面「更新功能只剩三件事…调 `restart_dsh`」这句不准确。** `restart_dsh` 是
   "停完立刻起"（`reset_handoff` → `run_doctor` → `start_handoff`），中间**没有**插
   `npm i -g` 的位置，整条不能复用。真正可复用的是 `reset_handoff` 这个"停"原语
   （已含 `kill_tree` + `wait_process_exit` 超时确认）。故 09 的第一步是把它拆成
   `stop_dsh` / `start_dsh` / `show_splash` 三个原语。
2. **① 里「手工装/卸了 dsh 插件」这条动机，对"更新插件本身"不成立。**
   `plugin::ensure_installed` 的第一条不变量是"目标存在且完整就**永不覆盖**"
   （为保护开发者的 junction），所以它只在**首次**安装时生效。DShell 升级后，
   已装用户手里仍是**旧副本**（开发机是 junction，不受影响）。本项的 `restart_dsh`
   会跑到 `probe_picker` → `ensure_installed`，所以**拆分完成后，"重启"天然就是
   重装入口**，不需要为此另加托盘按钮；前提是 `ensure_installed` 先做到版本感知。
3. **`roadmap.md` 已重编号**：单实例条目收口后删除，后续条目上移，所以本文开头
   "对应 `roadmap.md` 第 4 条（版本更新）"里的**第 4 条现为第 3 条**。本文并非那条
   本身，只是它的前置。

## 附注二（2026-09-20，拆分落地时补记）

09 的第 1 步（拆 `stop_dsh` / `start_dsh` / `show_splash`）已实施。**本文 ③ 里引用的
日志行因此有两处改名**，按日志排查时要注意：

| 本文 ③ 引用的旧日志 | 现在的日志 |
| --- | --- |
| `restart: navigated back to the splash page` | `splash: navigated back to the splash page` |
| `restart: killing dsh process tree (pid N)` | `stop dsh: killing dsh process tree (pid N)` |
| `restart: dsh (pid N) exited` | `stop dsh: dsh (pid N) exited` |
| `restart: no dsh process to stop` | `stop dsh: no dsh process to stop` |
| `restart: dsh (pid N) still alive after Ns` | `stop dsh: dsh (pid N) still alive after Ns` |

**行为没有变化**，只是把"停"与"回到启动页"这两件事从 `restart:` 前缀里分出来 ——
它们现在是独立原语，更新也要用同一条路径。

其余 `restart:` 前缀的日志（`already in progress` / `panicked` / `main window is gone` /
`aborting`）仍在 `restart_dsh` 自己的编排里，**未改**。
`doctor: suppressed (a restart is in progress)` 亦未改。
