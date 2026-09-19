# 07 · 单实例

> 状态：**已收口**（2026-09-20）。实测通过，见 ③。

对应 `roadmap.md` 原第 1 条（已随收口删除）。

## ① 方案

### 要解决的问题

双击两次图标会开出**两个 DShell 壳、两条 dsh 后端**。两个后果，第二个才严重：

| 后果 | 说明 |
| --- | --- |
| 内存成倍 | 每条 `dsh web` 是一整棵 node 进程树 |
| **抢会话锁** | 每个后端都持有 DSH 的会话锁，会干扰 DSH 自身的 session resume（实测报过 `SessionAlreadyOwnedError`） |

单实例同时是**版本更新的前置**：更新要把 `npm i -g @deepseek-ai/dsh` 的新文件覆盖到
已安装的目录上，而只要还有**任意一个** dsh 后端活着，Windows 上被 node 加载的 native
addon 就锁着那些文件，覆盖会失败或者留下半个安装。所以不能靠"关掉当前这个"来保证，
必须保证**整机只有一个**。

### 判定手段：命名互斥体

用 `CreateMutexW` 建一个 `Local\DShell.SingleInstance.v1`：

- 建成功且 `GetLastError() != ERROR_ALREADY_EXISTS` → 第一个实例，继续启动；
- 返回 `ERROR_ALREADY_EXISTS` → 已有实例，**聚焦它的窗口然后自己退出**。

两个细节是有意为之的：

| 选择 | 原因 |
| --- | --- |
| `Local\` 而不是 `Global\` | `Local\` 是"每个登录会话一个"。快速用户切换后两个会话各跑一个是**对的**（各有自己的 dsh 与窗口）；用 `Global\` 反而会让两个会话互相挡住 |
| **句柄故意不关** | 持有到进程结束，由内核在退出时释放。这样**崩溃、任务管理器强杀都不会留下一个永久锁死的单实例** —— 否则一次崩溃就能让用户再也打不开应用 |

### 找回已有窗口：按 exe 路径匹配，不按窗口标题

第二个实例要把已有窗口叫到前面，就得先找到它。候选方案与取舍：

| 方案 | 结论 |
| --- | --- |
| 按窗口标题找 | ❌ 标题会被 WebView2 用页面 `document.title` 覆盖（进到 dsh 后就不是我们的标题了），也随产品文案变化 |
| 按窗口类名找 | ❌ 那是 tao 的实现细节，跨版本会变 |
| **按可执行文件路径匹配进程** | ✅ 用 `EnumWindows` 枚举顶层窗口 → 取每个窗口的 pid → `QueryFullProcessImageNameW` 读该进程的 exe 全路径 → 与本进程 `current_exe()` 比对 |

按 exe 路径匹配是唯一稳的：它对"页面把标题改了"和"我们改了文案"都免疫，也不依赖任何第三方实现细节。

窗口本身优先取**可见**的，没有可见窗口再取隐藏的 —— 「关到托盘」正是 `hide()`，
那时窗口不可见。恢复时用 `ShowWindow(SW_SHOW)` 而不是 `SW_RESTORE`：隐藏的窗口
**不是最小化**，`SW_RESTORE` 对隐藏窗口不生效。

### 边界与失败姿势

- **`DSHELL_SPLASH_HOLD` 绕过单实例判定。** 那是调试开关，调试时经常要"边看旧版边调新版"，挡掉会让它没法用。
- **`CreateMutexW` 失败时不拦启动。** 保护措施本身不能变成新的启动故障点（与 `config.rs`「任何读取失败都静默回落到默认值」同一个原则）。
- **不实现"第二个实例把命令行参数交给第一个再退出"**：DShell 没有命令行参数，这个复杂度没有消费方。

### 改完是什么样

双击第二个图标 → 已有窗口被叫到前台 → 第二个进程静默退出，日志留一行
`single instance: another DShell is already running (focused=true) - exiting`。

## ② 关键代码

### 新增 `src-tauri/src/win32.rs`：手写的最小 Win32 绑定

不引 `windows-sys`。本应用只要三件事（互斥体、按 exe 找窗口、等进程退出），
加起来不到十个调用，而 `windows-sys` 会往依赖树里再塞一份 feature-gated 的大包。
项目已有惯例是宁可手写（`main.rs` 的 base64、`open_in_browser` 用 `cmd start`
而不是引 crate），手写还省掉一次联网取 crate —— **`Cargo.lock` 一行都不用动**。

对外只有三个函数，非 Windows 平台各有一个"永远当作第一个实例 / 找不到 / 已退出"
的退化实现：

```rust
/// 单实例判定的结果。
pub enum Instance {
    /// 本进程是第一个实例；互斥体已持有到进程结束。
    Primary,
    /// 已经有一个实例在跑（互斥体已存在）。
    Duplicate,
}

/// 建命名互斥体来判定是不是第一个实例；建不出来时返回 `Primary`。
pub fn acquire_single_instance() -> Instance;

/// 找回另一个 DShell 实例的主窗口并置顶；在 `timeout` 内没找到返回 `false`。
pub fn focus_existing_instance(timeout: Duration) -> bool;

/// 等一个进程退出；已退出或打不开（当作已退出）都返回 `true`。
pub fn wait_process_exit(pid: u32, timeout: Duration) -> bool;
```

声明的 Win32 面（`kernel32` / `user32` 各一组 `extern "system"`）：

| 库 | 函数 |
| --- | --- |
| kernel32 | `CreateMutexW`、`GetLastError`、`GetCurrentProcessId`、`OpenProcess`、`CloseHandle`、`WaitForSingleObject`、`QueryFullProcessImageNameW` |
| user32 | `EnumWindows`、`GetWindowThreadProcessId`、`GetWindowTextLengthW`、`IsWindowVisible`、`IsIconic`、`ShowWindow`、`SetForegroundWindow` |

`wait_process_exit` 是 08（重启）共用的 —— 两个升级项共享它，这也是把它放进
`win32.rs` 而不是塞进 `main.rs` 的原因。

### `main.rs`：在建窗之前判定

```rust
fn main() {
    let hold = std::env::var("DSHELL_SPLASH_HOLD").is_ok();

    // 单实例：必须在建窗**之前**判定。
    if !hold {
        match win32::acquire_single_instance() {
            win32::Instance::Primary => {}
            win32::Instance::Duplicate => {
                // 已有实例的窗口可能还没建出来（用户手快双击），给它一点时间。
                let focused = win32::focus_existing_instance(FOCUS_TIMEOUT);
                // 此刻还没走到 setup，日志目录要自己建。
                if let Some(dir) = log_path().parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                log(&format!(
                    "single instance: another DShell is already running (focused={focused}) - exiting"
                ));
                std::process::exit(0);
            }
        }
    }

    tauri::Builder::default()
    // …
}
```

新增常量：

| 常量 | 值 | 用途 |
| --- | --- | --- |
| `FOCUS_TIMEOUT` | 5 秒 | 第二个实例找已有窗口的等待上限，覆盖"用户手快双击"的窗口创建期 |

## ③ 结案总结

### 实测结果（2026-09-20）

日志 `%USERPROFILE%\.dshell\dshell-poc.log` 第 2370–2371 行（用户双击 exe 两次）：

```
2370: single instance: another DShell is already running (focused=true) - exiting
2371: single instance: another DShell is already running (focused=true) - exiting
```

| # | 用例 | 结果 |
| --- | --- | --- |
| 1 | 已有实例在跑时再双击 exe | ✅ 命中 `another DShell is already running`，`focused=true` 说明**找到并聚焦了已有窗口** |
| 3 | 连点 | ✅ 每轮双击各拦下一次（全日志共 3 次命中），没有多出壳 |
| 4 | **强杀 `dshell.exe` 后立即双击** | ✅ **能正常启动**（见下） |
| 5 | `DSHELL_SPLASH_HOLD=1` 启动两个 | 待复测 |
| 6 | 缩到托盘后双击 exe | ✅ 见「重启」那条（`tray: restored main window`），但当时是被重启流程唤出的，非纯双击路径 |

`focused=true` 是关键证据：它同时证明了"按 exe 路径匹配进程"这条路走得通
—— 找窗口这一步没有退化成"没找到"。这本来是最没把握的一环（要让 `EnumWindows`
+ `QueryFullProcessImageNameW` 在真实窗口树上正确匹配）。

### 强杀用例（#4）的证据链

**这条不能靠日志判断**：强杀的定义就是 `ExitRequested` 不触发 → 清理代码不跑 →
**日志里什么都不留**。所以证据必须从进程与端口状态取。实测（02:59）：

| 证据 | 值 |
| --- | --- |
| 强杀后立即双击 | 日志出现 `single instance: another DShell is already running (focused=true) - exiting`（第 2428 行），说明**新进程正常启动并被拦住**，而非卡在互斥体上 |
| 存活的 dshell | 只有 1 个（pid 6356，02:55:55 起） |
| 历史后端端口 | 日志累计启动过 **47** 个后端，当前仍在监听的**只有 1 个**（当前会话的 52328）→ 无孤儿 node 占端口 |
| 当前会话后端 | 端口 52328 归 pid 15704，连接正常 |

**这正是设计时的赌注所在**：互斥体句柄故意不关，交给内核在进程退出时回收。
如果当时用"手动 `CloseHandle`"或写文件锁，强杀就会留下永久锁死的单实例——
那才是"崩溃一次用户就再也打不开应用"的致命失效模式。本用例排除了它。

### 遗留

| 项 | 说明 |
| --- | --- |
| `DSHELL_SPLASH_HOLD=1` 启动两个 | 未复测（代码路径明确：`hold` 为真时整块单实例判定被跳过） |
| 快速用户切换 | 两个登录会话各能开一个 —— `Local\` 设计上就该成立，未实测 |
