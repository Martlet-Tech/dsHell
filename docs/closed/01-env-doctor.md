# 01 · 首次运行环境体检与分步启动改造（第一轮：方案）

> 本文只讲**改什么、怎么改、为什么、改完是什么样**。不含实现代码。
> 第二轮出关键代码，第三轮动手改代码。
> 对应 roadmap 第 5 条「首次运行环境体检」，并把启动页从「单行状态」升级为「分步流水线」。

---

## 1. 根因：为什么现在双击起不来

### 1.1 实测环境（本机，2026-09-10）

| 项 | 实测值 | 判定 |
| --- | --- | --- |
| WebView2 Runtime | `152.0.4191.66` | ✅ 远超最低要求 |
| Node.js | `v22.22.2` @ `C:\Program Files\nodejs\node.exe` | ✅ 在 PATH 上 |
| npm | `10.9.7`，全局 prefix `C:\Users\zt\AppData\Roaming\npm`（已在 PATH） | ✅ |
| dsh CLI | **不存在**（`where dsh` 无输出） | ❌ **直接根因** |
| winget | `C:\Users\zt\AppData\Local\Microsoft\WindowsApps\winget.exe` | ✅ 可用于自动安装 |
| 构建产物 | `dshell.exe` 3,001,856 B（release, LTO） | ✅ 已产出，能启动 |

**结论：Tauri 壳本身没问题，缺的是被它托管的后端 `dsh`。**

### 1.2 现有失败路径（读码得出）

```
main.rs: spawn_dsh()  →  cmd /c dsh web --port 0 --no-open
                          └─ cmd 立即报 “'dsh' 不是内部或外部命令…”（写 stderr）
                             stdout 随即 EOF
wait_for_url()        →  rx.recv_timeout → RecvTimeoutError::Disconnected
                      →  错误卡片：「dsh 还没打印地址就退出了」+ 3 条泛泛的可能原因
```

三个具体缺陷：

1. **不可自愈**：只告诉用户"可能没装 Node / 没装 dsh / 目录不可写"，不检测、不引导、不安装。
2. **不精确**：`recv_timeout` 报 `Disconnected` 无法区分"dsh 不存在"和"dsh 崩了"，用户看到的是一段猜谜。
3. **吞掉线索**：`spawn_dsh()` 把 stderr 设为 `Stdio::piped()` 却**从不读取**。cmd/node 的原始报错要么丢失，要么在量大的时候把子进程堵死（管道缓冲区写满）。这是既有代码的潜在缺陷，本轮一并修。

---

## 2. 目标与非目标

**目标**

- 启动前先体检，**分步汇报**（WebView2 → Node → npm → dsh → 启动 dsh），每步有明确结论与版本号。
- 缺东西时不是"报错结束"，而是给**三条出口**：指定安装位置 / 官方途径自动安装 / 退出。
- 安装过程**可见进度**（能到多少给多少，不糊弄）。
- 体检结论可持久化，第二次启动不再问同样的问题。

**非目标（本轮不做，避免范围膨胀）**

- 不解决单实例、Job Object 防孤儿（roadmap 第 1、2 条，独立立项）。
- 不做 dsh 版本更新提示（roadmap 第 4 条，但**共用探测层**，接口留好）。
- 不改动「抓 stdout token → 换 cookie → navigate」这条已验证的核心链路。

---

## 3. 总体设计：从「一次性等待」到「分步流水线」

现状是 `spawn → 等 URL` 一条直线。改造后是一条**带状态的顺序流水线**，每步可暂停下来等用户决策：

```
                    ┌──────────────── 启动页（步骤时间线 + 子日志 + 操作区）──────────────┐
                    │  ① WebView2   ② Node   ③ npm   ④ dsh   ⑤ profile 目录   ⑥ 启动 dsh web  │
                    └───────────────────────────▲──────────────────────┬──────────────────┘
                                                │ 事件(状态/日志/进度)  │ 用户动作(指定路径/安装/退出)
                                    ┌───────────┴──────────────────────▼───────────┐
                                    │            Rust 侧体检状态机                  │
                                    │  probe → ok ? 下一步 : 停在该步等用户选择      │
                                    └───────────────────────────────────────────────┘
```

### 3.1 步骤清单

| # | 步骤 | 探测手段 | 通过条件 | 失败是否阻断 |
| --- | --- | --- | --- | --- |
| ① | WebView2 运行环境 | Tauri 的 `webview_version()` | 能拿到版本号 | 否（拿不到时窗口根本不存在，见 3.2） |
| ② | Node.js | `where.exe node` + `node --version` | 存在且 ≥ 18 | **是** |
| ③ | npm | `where.exe npm` + `cmd /c npm --version` | 存在 | **是** |
| ④ | dsh CLI | `where.exe dsh` + 版本探测（见 3.3） | 存在 | **是** |
| ⑤ | profile 目录可写 | 试建/试写 `%USERPROFILE%\.dsh` | 可写 | 否（告警，可能后面才炸） |
| ⑥ | 启动 `dsh web` | 现有 `spawn` + 抓 token URL | 60s 内打印 URL | 是（沿用现有红卡片，但补上 stderr 详情） |

### 3.2 ① 的诚实说明

WebView2 缺失时，Tauri **连窗口都建不出来**，启动页根本没有机会渲染。所以：

- 页面内的"检查 WebView2"实际是**版本展示 + 过低告警**（本机 152.x 会全绿）。
- 真正缺失的兜底只能靠**原生 Win32 对话框**（`build()` 失败分支弹 MessageBox 给下载链接）。这条列为待确认项 P4，不作为本轮必做。

### 3.3 ④ 的版本探测降级链

| 优先级 | 手段 | 说明 |
| --- | --- | --- |
| 1 | `dsh --version` | **待确认**该 CLI 是否支持（见风险表）。支持则最准。 |
| 2 | `cmd /c npm ls -g @deepseek-ai/dsh --depth=0 --json` | 解析出真实安装版本，顺带证明"它确实由 npm 全局安装" |
| 3 | `npm view @deepseek-ai/dsh version` | 需要网络，只用于"最新版是多少" |

实测数据：该包的 `latest` dist-tag 就是 `0.1.5-rc.1`（官方把 rc 直接挂在 latest 上），所以**不需要 `@next`**，`npm i -g @deepseek-ai/dsh` 即可。包内**未声明 `engines`**，Node 下限暂定 18（其自身发布用 Node 24，实际要求待实测校准）。

---

## 4. 改造点清单

| # | 改造点 | 手段 | 目的 | 预期结果 |
| --- | --- | --- | --- | --- |
| C1 | 引入统一命令助手 `run_capture(cmd, args, timeout)` | 封装 `cmd /c`、`CREATE_NO_WINDOW`、**双流排空**（stdout/stderr 各起线程）、超时 kill、退出码 | 一处修掉"stderr 从不读"的缺陷；探测与安装共用 | 所有外部命令都拿得到 stdout+stderr+exit code，且不会堵死 |
| C2 | 新增体检模块（探测层） | 逐个跑 ①②③④⑤，产出 `Step { id, label, state, detail }` | 把"猜"变成"查" | 每步有明确结论：版本号/路径/缺失 |
| C3 | 流水线状态机 | 顺序执行；遇阻断步**停下等用户**，不直接失败退出 | 失败可从UI续跑，而不是重启程序 | 用户处理完，同一次会话内继续 |
| C4 | 启动页改版：步骤时间线 | 每步一行：状态图标 + 标题 + 详情 + 耗时；下挂可折叠子日志（等宽，最近 N 行） | 让"卡在哪一步"一目了然 | 用户能立刻看到"① ② ③ 绿、④ 红：未检测到 dsh" |
| C5 | 缺失项三出口 UI | 失败步骤下方出现操作区：`指定安装位置…` / `自动安装` / `退出`，另有折叠的 `跳过检查直接启动`（二次确认） | 给明确出路，而不是一段猜谜文字 | 三选一，均有反馈；老手有逃生口 |
| C6 | 安装器（自动安装） | 见 §5 命令链 + 降级链；安装完**重跑该步**验证 | 真正实现"从官方途径帮用户安装" | 点一下即可修好，且装完立刻变绿 |
| C7 | 安装进度与日志 | 流式读取安装进程输出到启动页；winget 解析百分比，npm 用不确定进度条 + 日志尾部 | 让等待可感知，避免被当成卡死 | 有进度条/滚动日志/已用时 |
| C8 | 上行通信通道 | 见 §6 方案对比 | 页面需要把"用户点了什么"送回 Rust | 选型确定后双向畅通 |
| C9 | 配置持久化 | 记住用户指定的 node/npm/dsh 路径与体检开关 | 不重复问 | 第二次启动直接复用 |
| C10 | PATH 注入 | 启动投影 dsh 时，把 `nodePath` 所在目录与 npm 全局 bin 目录前置进子进程 PATH | 「指定安装位置」必须真的生效 | 自定义路径下 dsh 也能正常起来 |
| C11 | 取消/超时/清理 | 探测命令 5s 超时；安装进程记账（`Mutex<Option<Child>>`），取消或退出时 `taskkill /T /F` | 不能留下孤儿 node（design.md 已踩过坑） | 取消后无残留进程，端口不占 |
| C12 | 调试开关 | `DSHELL_DOCTOR_MOCK=missing-dsh` / `missing-npm` 等伪造状态 | 没有坏机器也能回归测试 | 一行环境变量即可复现各失败分支 |

### 涉及文件

**改**

| 文件 | 改动 |
| --- | --- |
| `src-tauri/src/main.rs` | 精简为编排：建窗 → 跑体检 → 成功则沿用现有 handoff |
| `src-tauri/Cargo.toml` | 视决策点决定是否加 `serde_json` / `tauri-plugin-dialog` / `windows-sys` |
| `src-tauri/tauri.conf.json` | 若走 IPC：开 `app.withGlobalTauri` |
| `ui/index.html` | 单行状态 → 步骤时间线 + 操作区 + 进度区（保留现有品牌动效，缩小让位给列表） |
| `README.md` / `docs/roadmap.md` | 更新使用说明；第 5 条标记进行中 |

**新增**

| 文件 | 职责 |
| --- | --- |
| `src-tauri/src/proc.rs` | C1 命令助手 |
| `src-tauri/src/doctor.rs` | C2 探测层 |
| `src-tauri/src/installer.rs` | C6/C7 安装与进度 |
| `src-tauri/capabilities/main.json` | 若走 IPC，声明窗口可用的事件/对话框权限 |
| `docs/plan/01-env-doctor.md` | 本文 |

---

## 5. 缺失项处理：三条出口的具体内容

### 5.1 出口 A · 指定安装位置

| 入口 | 说明 | 依赖 |
| --- | --- | --- |
| 路径输入框 | 直接粘贴，如 `C:\Program Files\nodejs\node.exe` | 零依赖 |
| 拖放 | 把文件夹/exe 拖到窗口上，Tauri 的 drag-drop 事件给出绝对路径 | 零依赖（需事件监听） |
| 原生选择器 | 「浏览…」按钮，系统文件选择框 | 需 `tauri-plugin-dialog` |

**必须"就地验证"**：拿到路径后立刻用它跑一次 `--version`，通过才接受并写入配置。防止用户指错了文件，把问题推迟到后面才炸。

**识别细节（实测）**：PowerShell 里 `Get-Command npm` 命中的是 `npm.ps1`，而 dsh 实际是 `dsh.cmd`。因此所有 npm/dsh 调用**必须走 `cmd /c`**（design.md 第 3 条已确认），路径含空格时由 cmd 侧加引号，不做字符串拼接式拼命令。

### 5.2 出口 B · 官方途径自动安装

| 缺失项 | 首选命令 | 降级链 |
| --- | --- | --- |
| Node.js | `winget install --id OpenJS.NodeJS.LTS --exact --accept-package-agreements --accept-source-agreements` | winget 不可用/失败 → 打开官网 https://nodejs.org/en/download ，把命令与链接一并显示给用户复制 |
| npm | 随 Node 一起装（npm 不单独装）；Node 在但 npm 缺 → 判定为 Node 安装损坏，引导重装 Node | 同上 |
| dsh | `cmd /c npm i -g @deepseek-ai/dsh` | 失败 → 展示原始 stderr + 官网 README 链接；可选项：`--registry=https://registry.npmmirror.com`（默认**不用**镜像，避免版本偏差） |

注意事项（写进实现约束）：

- **winget 会弹 UAC**（MSI 装到 Program Files），必须提前在 UI 上讲清楚，否则用户以为程序卡住。
- **npm 全局装不需要管理员**：实测 prefix 在 `C:\Users\zt\AppData\Roaming\npm`。
- **装完当前进程的 PATH 不会自动更新**，所以安装成功后不能靠 `where node` 复查，要按已知安装目录（如 `%ProgramFiles%\nodejs` / npm prefix）直接定位。
- 全程 `CREATE_NO_WINDOW`，不闪黑框。

### 5.3 出口 C · 退出

点「退出」→ `app.exit(0)`，走既有 `ExitRequested` 清理链（`taskkill /T /F` 收整棵树）。

### 5.4 逃生口

`跳过检查，直接启动`（默认折叠 + 二次确认）。理由：体检本身不能变成新的故障点——已经装好 dsh 的老用户不该被误判挡住。

---

## 6. 上行通信通道（关键决策，需拍板）

启动页现在只会被 `window.eval()` **单向**驱动（base64 载荷）。新增"用户点按钮 + 选择路径"必须有上行通道。

| 方案 | 手段 | 优点 | 代价 |
| --- | --- | --- | --- |
| **A. Tauri IPC**（推荐） | `app.withGlobalTauri=true`；Rust→页面用 `emit` 事件，页面→Rust用 `invoke` | 官方做法、双向、结构清晰；后续单实例/更新提示/托盘都能复用 | 打破"刻意不使用 Tauri IPC"的原始纯度；页面需等 `__TAURI__` 注入就绪；要确认权限声明 |
| B. eval 轮询 | 页面把动作写进 `window.__dshellInbox`，Rust 每 250ms `eval` 读回 JSON | 零新增依赖、零权限、完全沿用现有通道 | 有延迟和竞态，代码很脏；步骤一多就难维护 |
| C. 本地 HTTP 桥 | Rust 起 `127.0.0.1:0` 小服务，页面 `fetch` | 与 DSH 自己的通信模型一致、能用 curl 调试 | 多一个监听端口 + 要自造 token 防护，攻击面变大 |
| D. 纯原生对话框 | 失败时弹 Win32 TaskDialog（3 个按钮），页面只负责显示进度 | 零 IPC，最符合"壳就只是壳" | 交互不精致；"指定路径"仍需要原生文件选择器 |

**推荐 A**，并把「指定路径」交给 `tauri-plugin-dialog`（原生选择器必须有原生能力，绕不过去）。
**若你希望严格保持零 IPC 与零新依赖**，则退为 **B + 路径输入框/拖放**（不引入 dialog 插件）——功能不缺，只是实现更土。

> 待验证：Tauri 2 的 ACL 主要约束*插件*命令；应用自定义的 `#[tauri::command]` 按官方文档可直接调用，但 `core:event`（监听事件）需要 capability 授权。第二轮先写最小 demo 打一发确认，再铺开。

---

## 7. 进度显示的现实边界（先把预期说清楚）

| 步骤 | 能给出什么 | 说明 |
| --- | --- | --- |
| winget 安装 Node | **确定百分比** | winget 自己打印含百分比的进度行，可解析 |
| `npm i -g @deepseek-ai/dsh` | **不确定进度条 + 实时日志尾部 + 已用时** | npm 不提供稳定的百分比。可做的近似：用 `--dry-run --json` 拿到待装包数，再用 `--loglevel=http` 的行数做分母估算——**不精确，不建议作为主 UI**，可作为附加信息 |
| 探测步骤（②③④⑤） | 瞬时完成 | 无进度概念，只有勾/叉 |

结论：**安装阶段给"日志流 + 进度条"，其中 Node 有真百分比，dsh 只有日志流。** 不承诺做不到的数字。

---

## 8. 配置持久化

位置沿用现有目录，与日志同级：

```
%USERPROFILE%\.dshell\config.json
{
  "node": "C:\\Program Files\\nodejs\\node.exe",
  "npm":  "C:\\Program Files\\nodejs\\npm.cmd",
  "dsh":  "C:\\Users\\zt\\AppData\\Roaming\\npm\\dsh.cmd",
  "doctor": { "skip": false }
}
```

读取规则：文件不存在 / 解析失败 / 路径已失效 → **静默回退到 PATH 探测**，绝不因为配置文件坏了而阻断启动。
写入时机：用户指定路径并通过验证后；安装成功后自动回填。

---

## 9. 超时、取消与清理

- 探测命令：**5s 硬超时**（手写 `try_wait` + 轮询，不引依赖），超时按"该步失败"处理并**带上 stderr**。
- 安装命令：不设硬超时（npm 装依赖可能要几分钟），但必须可取消。
- 取消/退出时：对记账中的安装进程 `taskkill /PID <pid> /T /F`（`/T` 关键，npm 会拉起一堆子进程）。
- 与 roadmap 第 2 条（Job Object）的关系：本轮靠显式清理；Job Object 仍建议独立做，两者不冲突。

---

## 10. 验证清单

| # | 场景 | 期望 |
| --- | --- | --- |
| 1 | 本机装好 dsh 后正常启动 | ①~④ 全绿 → ⑥ 抓到 URL → 进入 DSH UI（**回归**，确认改版没破坏主链路） |
| 2 | 伪造缺 dsh（`DSHELL_DOCTOR_MOCK=missing-dsh` 或临时改名 `dsh.cmd`） | ④ 显示红色 + 未检测到；该行出现 `指定…`，底部 `一键安装` 可用 |
| 3 | 伪造缺 npm | ③ 红，自动安装应引导"重装 Node"而非单独装 npm |
| 4 | 指定错误路径（如指向 `notepad.exe`） | 就地验证拒绝，给出"这不是 node/npm/dsh"的明确反馈 |
| 5 | 指定 `C:\Program Files\nodejs\node.exe` | 验证通过 → 写入配置 → ② 变绿 |
| 6 | 自动安装 dsh | 进度区滚动 npm 日志；完成后自动重跑 ④ 并变绿 |
| 7 | 安装中取消 | 进程树被杀，无残留 node（用任务管理器核对） |
| 8 | 点退出 | 窗口关闭，无残留进程，端口释放 |
| 9 | 探测命令超时（mock 一个 sleep 脚本） | 5s 后判失败，不卡死 |
| 10 | 配置写坏（手工塞非法 JSON） | 静默忽略，回退 PATH 探测，仍能正常启动 |
| 11 | WebView2 版本展示 | 页面显示 ① 步骤为 `152.0.4191.66` |
| 12 | 外链/引用链接点击 | 仍交给系统浏览器（回归 design.md 第 5 条） |

---

## 11. 风险与待确认项

| # | 项 | 现状 | 处置 |
| --- | --- | --- | --- |
| P1 | `dsh --version` 是否支持 | **✅ 实测支持**：返回 `0.1.5-rc.1` | 降级链第一环即可用，`npm ls -g` 结果一致 |
| P2 | Node 最低版本 | 包内**无 `engines` 声明** | 暂定 ≥ 18，实测校准后再收紧 |
| P3 | Tauri 2 应用命令是否需要 capability | **✅ 已实测：不需要**（只有 `core:event` / `dialog` 需授权） | 方案 A 落地成功，无需退方案 B |
| P4 | WebView2 完全缺失的兜底 | 窗口建不出来，页面无能为力 | **D3 后降为 TODO**：01 只做版本展示，兜底单独立项 |
| P5 | winget 被企业策略禁用 | 无法预知 | 降级链已含官网下载 + 可复制命令 |
| P6 | 国内网络 npm 慢 | 实测未测 | 提供可选镜像 registry，默认关闭 |
| P7 | 打破"零 IPC"设计原则 | 这是原项目的显式选择 | **已确认接受**（D1） |
| P8 | 安装进程需要管理员权限与否 | npm 侧确认不需要；winget 装 Node 会弹 UAC | UI 上提前提示 UAC |
| P9 | winget 在 stdout 被重定向时是否输出百分比 | 未测 | 退化为不确定进度条，功能不受影响 |

---

## 12. 决策记录（已冻结，2026-09-10）

| # | 决策项 | 结论 | 对方案的影响 |
| --- | --- | --- | --- |
| D1 | 上行通信通道 | **方案 A：Tauri IPC** | 开 `app.withGlobalTauri`；新增 `capabilities/default.json`；Rust→页面用 `emit` 事件，页面→Rust 用 `invoke` |
| D2 | 能否新增依赖 | **可以加** | 新增 `serde_json`（配置）、`tauri-plugin-dialog`（原生文件选择器）。`windows-sys` **不需要**（D3 取消原生弹框） |
| D3 | 自动安装边界 | **Node / dsh 走官方通道自动安装；WebView2 暂 TODO** | 删除 WebView2 bootstrapper 下载执行；① 步骤降级为"版本展示 + 过低告警"，P4 不再阻塞 |
| D4 | 交互姿态 | **全面体检 → 统一汇报 → 缺失项各自可指定 → 底部两个按钮：一键安装 / 退出** | **流水线语义变了**：不再"停在失败步等用户"，改为一次跑完全部探测后汇总呈现（详见 02 文档 §1） |

D4 带来的三处删改：

- 原 §3.1「失败是否阻断」列不再用于控制**执行顺序**（只用于决定"能否进入 ⑥ 启动 dsh"）：所有探测一路跑完，缺失只是状态，不停机。
- 原 §5.2 的"每步下方三按钮"改为：**每行一个 `指定…`（独立处理）+ 底部动作条 `一键安装` / `退出`**。
- 原 §5.4 的"跳过检查直接启动"不再作为按钮，建议降级为角落里的一个低调文字链（次级逃生口，可留可砍，等第三轮前确认）。

---

## 13. 交付节奏

| 轮次 | 产出 |
| --- | --- |
| **第一轮（本文）** | 方案、改造点、手段/目的/预期结果、风险与决策点 —— 已审阅 |
| **第二轮** | `02-env-doctor-code.md`：数据契约、关键代码、UI 接线、第三轮实施顺序 |
| **第三轮** | 代码已落地并构建通过，主链路实测成功 —— 见 §14 |

---

## 14. 实测记录（第三轮，2026-09-10）

release 构建通过后在本机跑了一次完整链路。全部证据在 `%USERPROFILE%\.dshell\dshell-poc.log`：

| 环节 | 日志证据 | 结论 |
| --- | --- | --- |
| 体检 | `doctor: allOk=false missing=["dsh"] auto=["dsh"]` | 只缺 dsh，与手工探测一致 |
| 一键安装 | `install dsh: spawned cmd (pid 19844)` → `install dsh: ok=true cancelled=false code=Some(0)` | `npm i -g @deepseek-ai/dsh` 真实跑通 |
| 装后复检 | `doctor: allOk=true missing=[] auto=[]` | 自动重探，不用用户再点一次 |
| 自动 handoff | `spawned dsh web (pid 29900)` → `dsh> dsh web: http://127.0.0.1:60024/?token=…` → `window navigated to dsh` | 全绿后自动进入 DSH UI |
| 配置回填 | `config.json` 里 `dsh = C:\Users\zt\AppData\Roaming\npm\dsh.cmd`，`npm_prefix` 同步写入 | 「装完不靠 `where` 复查」这条策略生效 |
| 外链拦截 | `external link -> system browser: https://github.com/deepseek-ai/deepseek-harness` | 点「官方页面」正确交给系统浏览器，壳没被顶掉（design.md §5 回归通过） |
| 进程清理 | 只存在 1 个 `cmd /c dsh web`，父进程正是当前 `dshell` | 没有孤儿后端 |

**意外收获（验证了一个实现细节的必要性）**：`where.exe dsh` 输出的**第一行是无扩展名的 `dsh`**（shell 脚本），第二行才是 `dsh.cmd`。
按老办法取第一行会拿到一个 `CreateProcess` 跑不了的东西——`proc::which` 里按扩展名优先级（`exe` > `cmd` > `bat` > `ps1` > 无扩展名）排序不是洁癖，是必需的。

### 尚未实测

| 项 | 原因 |
| --- | --- |
| winget 装 Node（P9：stdout 被重定向时是否输出百分比） | 本机已装 Node，走不到这条分支；实现里已按「拿不到就退不确定进度条」处理 |
| 「指定…」的三条路径（正确通过 / 错误被拒 / 落盘复检） | 未点 |
| 安装中途取消、点退出 | 未点 |
| 完全没有 `%APPDATA%\npm` 目录的干净机器 | 本机 PATH 里本来就有该目录 |

---

## 附注（收口后）

- **收口状态**：本项已实现并实测通过（release 构建，产物 `src-tauri/target/release/dshell.exe`）。实测证据见 §14；
  启动页的实际渲染效果见 `docs/screenshots/env-helper.png`（截图同时证明了 `where.exe npm` 的首行是无扩展名脚本，
  经 `proc::which` 的扩展名排序后取到了可用的 `npm.cmd`）。
- **一处与 02 文档的差异**：02 §7.3 写「安装日志单独写 `dshell-install.log`」，实际实现只用了主日志
  `%USERPROFILE%\.dshell\dshell-poc.log`——安装输出量级没到需要拆分的程度。若将来要拆，属新立项。
- **另一处实现选择**：① 的 WebView2 版本探测最终没有用 Tauri 的 `webview_version()`，改为 `reg query` 读
  `EdgeUpdate\Clients\{F3017226-…}` 的 `pv`，绕开 API 形状的不确定性（见 02 §10 的 P10）。
- **文件位置**：本文与 `02-env-doctor-code.md` 原在 `docs/plan/`，收口后移入 `docs/closed/`。
  正文里出现的 `docs/plan/01-env-doctor.md` 指的就是本文（原文不改）。
