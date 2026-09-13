# 04 · 添加工作区后界面不刷新（需再点一下鼠标/键盘才生效）

> **立项日期**：2026-09-12
> **状态**：**根因已定位**（R10，2026-09-13）。前 18 轮方向已被否证，机理已查清，待定修法。
> **现象提出**：用户实测（截图见下）
> **对应 roadmap**：`roadmap.md` §6
> **历次尝试台账**：[`05-attempts-summary.md`](05-attempts-summary.md)（含 M1–M9 教训，动手前必读）

---

## 0. 根因（2026-09-13，R10 实测证实）

> **读完本节即可开工；§4 及以后的 H1–H4 已被否证，仅作留档。**

**用户真实时序**（用户澄清，台账 §4.1）：点"选这个目录" → 对话框消失 →
**手离开键鼠等 5 秒以上** →（界面没动）→ 再点一下。

**关键矛盾**：日志里那条 `mousedown` 出现在对话框关闭后 **4ms**，而用户补点在 **5 秒之后**。

⇒ **那条 `mousedown` 不是用户点的**，是凭空出现的输入事件。

### 0.1 R10 实测：**已证实**（2026-09-13 08:15，台账 §10）

| 事实 | 数据 |
| --- | --- |
| 点 `+` 后 dsh 合成 Alt-down | `08:15:32.526 kdn alt=true trusted=true` |
| 对话框开启 **23.9 秒**（页面冻结，解冻后 11 条心跳在 38ms 内补发） | 台账 §10.3 |
| 关闭瞬间 Alt-up 才被投递（页面时间戳与 down **完全相同**） | `kup t=…32524` = `kdn t=…32524` |
| **激活交接副产物：一次 `trusted=true` 的合成鼠标点击** | `08:15:56.495 down hit=div.uV2eYG_input detail=1 buttons=1`，**用户手已离开键鼠** |

⇒ **幽灵点击实锤**。它来自 dsh 的 `pressAltForForeground()`
（`win32-dialog-bindings.ts:148-151`，`keybd_event` 合成 Alt）
与 `Show(null)`（无 owner，`:160`）在激活交接时的共同作用。

### 0.2 因果链

```
合成 Alt-down → 原生对话框弹出（无 owner，抢前台）→ WebView2 页面冻结
  → 用户选目录（约 24 秒）→ 点确定 → 对话框关闭
  → 合成的 Alt-up 此刻才投递 → 激活交接产生一次合成鼠标点击
  → 该输入唤醒页面 → 积压定时器补发 → React 拿到调度 → 界面刷新
```

**用户感知**：界面在"不该有输入"的时刻自己刷新—— **但这条（C20）已被否证，见 §0.4。**

### 0.3 修法方向（分轮）

问题在**激活交接**，不在渲染、也不在 dsh 前端逻辑。**不碰 dsh 源码**（§6 红线）。

| 轮次 | 方案 | 做法 | 状态 |
| --- | --- | --- | --- |
| **R11** | **F1：壳层主动收回激活态** | 监听 `WindowEvent::Focused`；gap ≤ 1500ms 判定为对话框交接 → `set_focus()` | ✅ 已实测：**未复现 bug，但无法归因**（台账 §12） |
| **R11-b** | **对照轮：关掉 F1、保留探针** | 同操作复现，确认 bug 是否仍在 | ⏳ **下一步必做** |
| **R12** | **F2：壳层主动触发页面重绘** | `eval`，不依赖任何输入/激活态 | ⏳ 待 R11-b 之后 |

### 0.3.1 R11 实测的关键发现（台账 §12）

| 事实 | 数据 |
| --- | --- |
| F1 **确实触发** | `focus: dialog roundtrip detected (gap=2ms) -> reclaim set_focus=true` |
| 界面**自己刷新了** | `changed=true` 出现在用户补点**之前 798ms** |
| **但 R10（无 F1）也自刷新了** | ⇒ **无法归因于 F1** |
| F1 的 gap 只有 **1–2ms** | 与"切窗口"（秒级）差三个量级 ⇒ 窗口可能**从未真正失焦**、`set_focus()` 是空操作 ⇒ **F1 大概率没打在要害上** |
| 幽灵点击**既非唤醒源也非刷新源** | 晚于解冻 36ms，刷新又晚于它 980ms |

⇒ **结论：R11 不能作为修复证据。必须先做对照轮（R11-b）。**

### 0.4 C20 已两次作废

用户明确：**"我不主动点鼠标/键盘 Alt/Shift, 可能永远不能刷新。"**

| 次 | 依据 | 结论 |
| --- | --- | --- |
| 第一次 | 台账 §10.7：解冻（`.381`）**先于**幽灵点击（`.495`），刷新（`.866`）又晚 371ms | 幽灵点击不是**唤醒**机制 |
| 第二次 | 台账 §12.4：R11 里幽灵点击（`.838`）晚于解冻 36ms，刷新（`50.818`）又晚 980ms | 幽灵点击也不是**刷新**源 |

⇒ **幽灵点击是激活交接的副产物（已实锤），但与刷新因果无关。**
真正待查的仍是：**解冻之后，是什么在阻止 React 提交**。

### 0.5 **最终解法：换掉对话框本身（R15，2026-09-13）**

前面所有轮次都在**症状层**搏斗（冻结 → 通知 → 微任务 → 幽灵点击），
从未问过一个问题：**为什么 Windows 上必须用原生对话框？**

答案在 `packages/host/directory-picker-auto/src/resolve.ts:50-52`：

```ts
if (facts.bindHost !== '127.0.0.1') return 'browse'
if (facts.ssh) return 'browse'
if (facts.platform === 'darwin' || facts.platform === 'win32') return 'native'  // ← 无条件
```

**win32 + loopback ⇒ 必然 native** ⇒ 必然"另一个进程弹模态窗口"⇒
必然失焦 ⇒ 必然挂起 11.7 秒。

而这个选择**是 dsh 官方支持覆盖的**。`packages/bundle/web-app/cordis.patch.yml`
在该行上方的注释原文：

> **Mount `-native` or `-browse` directly in an overlay to pin the interaction.**

⇒ **修法：在用户级 profile patch 里钉死 `-browse`。**

| | `-native`（原状） | `-browse`（改后） |
| --- | --- | --- |
| 对话框位置 | 独立 node 子进程的原生 Win32 窗口 | **网页内对话框** |
| 页面失焦 | ✅ 会 | ❌ **不会** |
| 页面挂起 | ✅ **11.7 秒** | ❌ **不会** |
| 关闭时刻可见 | ❌ 在进程层 | ✅ **就在页面 JS 里** |

**这不是绕过症状，是消除产生症状的结构。**

**落地**（用户级配置，**不改 dsh 源码**，升级不失效）：

文件 `%USERPROFILE%\.dsh\profiles\web\cordis.patch.yml`：

```yaml
- id: directory-picker
  disabled: true

- insert:
    - id: directory-picker-browse
      name: '@deepseek-ai/dsh-host-directory-picker-browse'
    - id: directory-picker-browse-surface
      name: '@deepseek-ai/dsh-client-ui-directory-picker-browse'
```

两个包在 `dsh@0.1.5-rc.1` 里已随包安装，无需另外安装。
原文件已备份为 `cordis.patch.yml.bak-original`。

**同时保留的探针改动**（用于验证）：R14 的 `initialization_script` 注入
（**已证明可用**，见台账 §16）。

#### 0.5.1 实测结果：**挂起消失，但方案被否决**（2026-09-13）

| 观察 | 结果 |
| --- | --- |
| 变成网页内对话框 | ✅ 是 |
| 不再挂起 | ✅ 是（结构性问题消失） |
| **可用** | ❌ **不可用** —— 看不到「我的电脑」，**没法跨盘** |

原因：`browse` 的 `crumbs` 是"从根到目标的祖先链"
（`directory-picker-browse/src/index.ts:28-38`），Windows 上"根"是 `C:\`，
**面包屑永远跨不了盘**（POSIX 单目录树建模的必然结果，非 bug）。

⇒ **换 browse 只消症状，不交付功能。已回滚。**

### 0.6 **正确方向：自己弹原生对话框**（R16，2026-09-13）

横向参考 [anywhere-labs/dsh-desktop](https://github.com/anywhere-labs/dsh-desktop)
（25,973 star 的 Electron 版 DSH 桌面端）后，得到关键洞察：

> **dsh 的 `-native` 后端本身没问题，问题在于"谁弹这个对话框"。**

| | DSH Desktop（Electron） | DShell（现状） |
| --- | --- | --- |
| 谁弹对话框 | **自己进程** | dsh 的**独立 node 子进程** |
| parent | ✅ `showOpenDialog(window, …)` | ❌ `Show(null)` **无 owner** |
| 主窗口失焦 | ❌ 不会 | ✅ **会** |
| 页面挂起 | ❌ 不会 | ✅ **11.7 秒** |

**另一进程弹的无 owner 窗口 ⇒ 必然抢前台 ⇒ 必然失焦 ⇒ 必然挂起。**

**Tauri 版可行性已核实**（两个前提都已落实）：

| 前提 | 状态 |
| --- | --- |
| Tauri 对话框能设 parent（不失焦） | ✅ `set_parent(&window)`（`tauri-plugin-dialog-2.7.3/src/commands.rs:130,230,282`）；插件**已装** |
| 能注入脚本接管 dsh 的 `pick` | ✅ `initialization_script` **已证可用**（台账 §16） |

**这也正是 dsh 官方 seam 笔记预告的用法**：

> native 后端保留。插件化正是目的：多个提供方都能提供该 seam
> （**Electron 壳可以经自己的对话框 API 提供 `native` 交互**）。

**下一步（A/B/C 待定）**：

| 选项 | 说明 |
| --- | --- |
| **A（推荐）** | 照搬 DSH Desktop 架构：Rust 侧自建端点 + Tauri 原生对话框（设 parent）+ 注入脚本接管 `pick` |
| B | 回 `-native`（原状），接受"点一下" |
| C | 回 `-native` + 注入脚本在解冻后主动推页面（治症状） |

---

## 1. 现象

在 DShell（Tauri + WebView2 桌面壳）里点侧边栏「工作区」标题右侧的 **添加工作区按钮**
（`IconProjectAddOutline16`，见截图红箭头），选中文件夹并点击系统对话框的**确定**之后：

| 观察点 | 现象 |
| --- | --- |
| 工作区列表 | **不出现**新工作区 |
| 会话区 | **不跳转**到新工作区 |
| 补一个动作后 | 随便点一下鼠标或敲一下键盘，新工作区**才出现**并跳过去 |

同一个操作在 **`dsh web` 的浏览器页面**里（Chrome/Edge 直接开 `http://127.0.0.1:PORT/`）：

| 观察点 | 现象 |
| --- | --- |
| 工作区列表 | 点确定后**立即**出现 |
| 会话区 | **立即**跳转 |

**差异只在宿主壳，不在 dsh 本身。** 这是本立项要解释并修掉的核心事实。

### 1.1 涉及的两个 UI 入口

| 入口 | 位置 | 代码 |
| --- | --- | --- |
| 侧边栏「工作区」头部 `+` | `WorkspaceBrowser` 头部 actions | `packages/client/ui-workspace/src/client/rows/WorkspaceBrowser.tsx:1201-1215` |
| 会话空态的「添加工作区…」 | 菜单项 | `packages/client/ui-workspace/src/client/WorkspacePicker.tsx:102` |

两者共用 `WorkspacePickFlow`，都会走「选目录 → 采纳（adopt）」这一条路。
用户截图里箭头指的是**前者**（侧边栏头部），但两者共享同一段采纳逻辑，因此**很可能同因同果**。

---

## 2. 调用链（读码结论，`src-of-dsh` = dsh 0.1.5-rc.1）

```
① 用户点 + 按钮
   WorkspaceBrowser.tsx:1208   onClick → setWsPickerOpen(v => !v)
        ↓
② WorkspacePickFlow 收到 open=true
   WorkspacePicker.tsx:155-157 useEffect: open && addIsTheOnlyEntry && !flowBusy
        → openDirectoryFlow()
        ↓
③ openDirectoryFlow()  WorkspacePicker.tsx:136-141
   onClose(); setFlowOpen(true)
        ↓
④ renderDirectoryFlow(flowOwner)  WorkspacePicker.tsx:198
   → 槽位 'sidebar.workspaces.directoryFlow'
        ↓
⑤ NativeDirectoryFlow（renderless）  ui-directory-picker-native/src/client/flow.ts:46-63
   useEffect [open, pick] → armed 置位 → pick()
        ↓
⑥ ctx.uiWorkspace.pickDirectory()  ui-workspace/src/client/navigation.ts:179-183
   → remote.directoryPicker.pick()   ← Typert RPC over HTTP
        ↓
⑦ Host：DirectoryPickerController.pick(signal)
   api/workspace-controller/src/directory-picker.ts:54-62
   → capability.pick(signal)
        ↓
⑧ NativeDirectoryPicker.capability().pick
   host/directory-picker-native/src/index.ts:26  → pickNativeDirectory(signal)
        ↓
⑨ win32: pickWin32Directory（win32-dialog.ts:66）
   spawn 一个**子进程** win32-dialog-worker.ts，子进程在
   runFolderDialog → dialog.show()（IFileOpenDialog::Show）里**阻塞**
        ↓
   用户选文件夹 → 点确定 → 子进程 post { kind:'done', path }
        ↓
⑩ 回到 ⑥ 的 await → ⑤ 的 pick().then(path)
   → outcome.current.onPicked(path)   flow.ts:56
        ↓
⑪ flowOwner.onPicked  WorkspacePicker.tsx:163-166
   setPickingFolder(true); adoptDirectory(path)
        ↓
⑫ adoptDirectory  WorkspacePicker.tsx:126-134
   createWorkspace({ path })  → … → setFlowOpen(false) → onPick(workspaceId)
        ↓
⑬ onPick  WorkspaceBrowser.tsx:1228-1231
   setWsPickerOpen(false); startSession(workspaceId)
        ↓
⑭ navigation.ts:154-173 startSession → openWorkspace → connectWorkspace
   创建/复用 Session → openSession → sessions.open(id) + layout.selectPanel(null)
        ↓
⑮ 列表与主区应随之更新（model.ts:276-290 upsert → invalidate → queueMicrotask → flush）
```

整条链在前端是**异步 Promise**，跨了一趟 HTTP RPC（⑥→⑦）和一次子进程 IPC（⑨→⑩）。
任何一环的「回调被推迟到下一帧之后」都会让 ⑮ 的渲染落到**下一次用户交互**上。

---

## 3. 关键事实（已读码确认，非推测）

| # | 事实 | 出处 |
| --- | --- | --- |
| F1 | 选目录走的是 **Host 原生 `IFileOpenDialog`**（Windows 真实进程 + COM），不是浏览器 `showDirectoryPicker` | `host/directory-picker-native/src/win32-dialog-logic.ts:119-147` |
| F2 | 对话框在**独立子进程**里 `Show` 阻塞，Host 事件循环不被阻塞 | `win32-dialog-worker.ts:1-13, 44-55` |
| F3 | 后端选择是**开机一次性**的：`bindHost==='127.0.0.1' && !ssh && win32` → `native` | `directory-picker-auto/src/resolve.ts:49-55` |
| F4 | 因此 DShell 与浏览器**用的是同一个 native picker**——差异不在 Host 选谁 | F3 |
| F5 | `NativeDirectoryFlow` 是 **renderless** 组件，`return null`；它只靠 `useEffect` 驱动 | `flow.ts:25, 64` |
| F6 | 每次 `open` 上升沿只跑一次 `pick()`，靠 `armed` ref 去重 | `flow.ts:46-63` |
| F7 | 采纳成功后 `flush()` 走 `queueMicrotask` 发布（不是同步） | `api/workspace-controller/src/client/model.ts:315-332` |
| F8 | 新工作区行有挂载淡入动画 `row-in 150ms` | `rows/Rows.module.css:104-122` |
| F9 | 侧边栏有 `wide-in 200ms` 动画与多处 `transition` | `rows/WorkspaceBrowser.module.css:68-207, 324` |
| F10 | 子进程 `Show` 前会**合成一次 Alt 按键**（`keybd_event`）来抢前台 | `win32-dialog-logic.ts:134`、`win32-dialog-bindings.ts:148-151` |

> **F10 值得单独记一笔**：它往系统里注入了一次真实按键。虽然方向是「注入到本进程争前台」，
> 但下面 P3 会讨论它是否与「必须补一次鼠标/键盘才刷新」直接相关。

---

## 4. 候选成因（按可能性排序）

### H1（最可能）：WebView2 的渲染/合成在窗口失焦期间被节流

**机理**：原生对话框抢走前台焦点后，主窗口进入 `document.hidden` 之外的
「**occluded / 非前台**」状态。Chromium 系（WebView2 同源）对非前台窗口有分级节流：

| 状态 | `requestAnimationFrame` | 定时器 | 渲染/合成 |
| --- | --- | --- | --- |
| 前台可见 | 每帧 | 正常 | 正常 |
| 被遮挡 / 失焦 | **暂停或降到极低频** | 降到 1s | 可能停止提交帧 |
| 最小化 | 停止 | 降频 | 停止 |

**为什么恰好是「点一下鼠标就好」**：一次真实输入事件会**立刻唤醒**被节流的
渲染管线；被 `queueMicrotask`/React 调度推迟的那一次 commit 就在唤醒后立刻落地。
这与用户描述**完全吻合**——不是「数据没来」，而是「帧没提交」。

**为什么浏览器里不出现**：浏览器是**独立顶层窗口**，原生对话框弹出时浏览器窗口
同样失焦，但：
- 浏览器标签页通常不被判定为 occluded（对话框是另一个进程的窗口，遮挡计算不同）；
- 或者浏览器窗口本来就**不是**被判定「后台」的那个（用户刚点过它）。
而 DShell 的窗口是**被自己的子进程窗口直接盖住**，遮挡判定更明确。

**佐证/反证**：需要在 WebView2 里实测 `document.visibilityState`、
`document.hasFocus()`、以及 `requestAnimationFrame` 在对话框期间是否停摆。

### H2：`open` 状态机把 `pick` 的回归「吃掉」了一次渲染

`flow.ts` 里 `armed` 是 ref，**不触发渲染**；`flowOwner.onPicked` 先
`setPickingFolder(true)`（渲染一次，但 `busy` 只用于禁用菜单项），
再异步 `adoptDirectory`。若 `adoptDirectory` 期间 React 恰好处于
**一次未提交的并发渲染**中，`setFlowOpen(false)` 与 `onPick` 的两次 setState
会被合并——但**合并本身不会导致延迟到下个输入事件**。所以 H2 单独不足以解释现象，
**只有在 H1 成立时才会表现为「看起来没反应」**。

### H3：`WindowEvent::Focused` / 窗口事件未把焦点还给 WebView

原生对话框关闭后，**焦点没有回到 WebView2**，因此 WebView2 仍认为自己非活动，
继续走节流路径。用户点一下窗口才真正 `SetFocus` 到 WebView。
——**这其实是 H1 的具体化**，且很可能与 H1 叠加：即使没有渲染节流，
「焦点不回来」也会让 `document.hasFocus()===false`，进而影响 Chromium 的
`Page Visibility` / 输入优先级。

### H4（较弱）：`keybd_event` 合成 Alt 干扰了输入状态

`pressAltForForeground()` 按下并抬起了 `VK_MENU`。若这次合成的 Alt 抬键
在某个窗口的消息队列里被吞（例如对话框子进程退出太快），
**主窗口可能残留「Alt 处于按下」的键盘状态**；此时第一次真实按键会被
当作 Alt 组合键处理，而不是普通事件。但这**不能解释鼠标点击也能唤醒**，
故列为次要。

### 已排除

| 猜想 | 排除理由 |
| --- | --- |
| 后端没返回 | F2：对话框在子进程，Host 不阻塞；RPC 正常返回（否则会弹错误框） |
| 采纳报错被吞 | `adoptDirectory` 的 `.catch` 会开错误弹窗（`WorkspacePicker.tsx:130-134`），用户没看到弹窗 |
| 走了 browse 后端 | F3/F4：win32 + loopback 必为 native |
| 列表数据没更新 | F7 的 `queueMicrotask` 在事件循环里必跑，不需要用户输入 |
| **dsh 前端状态机有 bug** | F5–F7 全是标准 React/微任务路径，与宿主无关；浏览器端表现正常已证伪 |
| **`+` 按钮的菜单/浮层拦住了结果** | `WorkspaceBrowser.tsx:1201` 的 `+` 只在 `directoryFlowAvailable` 时渲染，且 `addOnly` 下 `addIsTheOnlyEntry` 恒为 true，`open` 一到就直接进 flow（`WorkspacePicker.tsx:151-157`），不存在"要再点一次才展开"的分支 |

### 4.1 一句话结论（初判）

**数据是通的，帧是断的。** 采纳链在对话框关闭后就已经跑完并写了模型
（`model.ts:87 upsert → 315 invalidate → 327 queueMicrotask → 334 flush`），
但 WebView2 在被自己的模态子窗口盖住的那段时间里**停止了提交帧**，
且**对话框关闭后没有被通知恢复**——直到下一次真实输入事件（点鼠标/敲键盘）
把渲染管线唤醒，那次早已排队的 commit 才落到屏幕上。

这解释了用户观察到的**全部三个细节**：
1. 「没有反应」——不是没发生，是没画出来；
2. 「随便点一下鼠标**或者**动一下键盘」——任意真实输入都能唤醒，说明不是某一条特定事件的锅；
3. 「浏览器里直接就加了」——浏览器窗口的遮挡判定与焦点恢复路径不同（H1 的对照）。

---

## 5. 验证计划（**动手改之前必须先测**）

目标是**先用证据把 H1/H3 与 H2 分开**，避免改错地方。

| # | 验证项 | 方法 | 期望（若 H1/H3 成立） |
| --- | --- | --- | --- |
| V1 | 对话框期间 WebView2 的可见性 | 在 dsh 页面注入探针，记录 `visibilitychange`、`document.hasFocus()`、`window.blur/focus` 时间线 | 对话框弹出时 `hasFocus()→false`；点确定后**不自动回到 true**，直到用户点窗口 |
| V2 | rAF 是否停摆 | 注入 `requestAnimationFrame` 循环打点，输出到 `console`/`localStorage`（DSH 无远程控制台，可写 `document.title` 再读，见 design.md「`w.title()` 读到缓存值」的教训——**改用 localStorage + 页面内可见区域**） | 对话框期间帧间隔远大于 16ms |
| V3 | 采纳时刻 vs 渲染时刻 | 在 ⑫ `adoptDirectory` 完成后打点，与 V2 的下一帧时间对比 | 数据在 T 时刻就绪，帧到 T+Δ（Δ = 用户下次交互） |
| V4 | 浏览器对照 | 同一 dsh 端口用 Edge 打开，跑同一探针 | 无 Δ 或 Δ≈16ms（复现用户报告） |
| V5 | 焦点归还实验 | 不改 dsh 代码，仅在 `main.rs` 的窗口事件里观察 `Focused(false/true)` 的到达时刻 | 对话框关闭后**没有** `Focused(true)`，直到用户点击 |

**V5 是本立项的关键实验**：它决定修法是「把焦点还回去」还是「强制重绘」。

> 探针实现方式待定，最省事的是在 `ui/index.html`（启动页）里没有用处——
> 探针必须跑在 **dsh 的远程页面**上。可选：
> ① 用 `window.eval` 从 Rust 侧注入一段脚本（`main.rs` 已有 `window.eval` 的先例，见
> `splash_status`），把结果写到 `localStorage` 再读回；
> ② 临时挂一个 devtools（WebView2 支持 `--remote-debugging-port`，需实测 Tauri 是否放行）。

---

## 5. 验证记录（本机实测，2026-09-12）

### 5.1 V-A：WebView2 运行时是否具备遮挡节流机制 —— ✅ 已确认

对 `C:\Program Files (x86)\Microsoft\EdgeWebView\Application\152.0.4191.66\msedge.dll`
做 ASCII 全量扫描（`[System.IO.File]::ReadAllBytes` + `Encoding.ASCII.GetString`）：

| 字符串 | 结果 | 含义 |
| --- | --- | --- |
| `CalculateNativeWinOcclusion` | **FOUND** | Chromium 的**原生窗口遮挡计算**存在。窗口被判定 occluded 后，渲染按后台处理 |
| `disable-backgrounding-occluded-windows` | **FOUND** | 存在关闭该行为的开关 |
| `disable-renderer-backgrounding` | **FOUND** | 存在关闭「渲染进程降级」的开关 |
| `disable-background-timer-throttling` | **FOUND** | 存在关闭后台定时器降频的开关 |

**结论**：H1 的**机制在本机运行时中真实存在**，且三个对症开关都在。
这与用户现象（被自己的模态子窗口盖住 → 帧停摆 → 下次输入唤醒）**一致**。

### 5.2 V-B：WebView2 的附加参数通道 —— ✅ 已确认可用

读 `wry-0.55.1/src/webview2/mod.rs:294-327`（`tauri 2.11.5` 的底层）：

```rust
// wry 默认注入的 args（只在调用方没有显式指定时）
let additional_browser_args = pl_attrs.additional_browser_args.unwrap_or_else(|| {
    let default_args = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection";
    ...
});
...
options.set_additional_browser_arguments(additional_browser_args);
```

对应 Tauri 侧 API（`tauri-2.11.5/src/webview/webview_window.rs:1014-1020`）：

```rust
pub fn additional_browser_args(mut self, additional_args: &str) -> Self
```

**两条重要事实**：

| # | 事实 | 影响 |
| --- | --- | --- |
| V-B1 | `unwrap_or_else` 意味着**一旦我们调用 `additional_browser_args`，wry 的三个默认参数会被整体替换掉** | **必须手工补回** `--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`，否则 mini menu / SmartScreen 会回来 |
| V-B2 | 该 API 是 `WebviewWindowBuilder` 的方法，本项目**已经在用这个 builder**（`main.rs:536`） | 改动是**纯增量的一行链式调用**，不涉及架构 |

### 5.3 探针采集记录（两轮，2026-09-12）

为拿到「数据就绪时刻 vs 帧提交时刻」的直接证据，写了临时探针
（`src-tauri/src/probe.rs`，`DSHELL_PROBE=1` 开启）。**两轮都没采到数据，
但两轮各暴露了一个明确事实。**

#### 第一轮：探针自己的 bug（已修，但被第二轮推翻）

| # | 配置 | 结果 |
| --- | --- | --- |
| R1 | `initialization_script` + 按计数去重 | 只有 `probe: attached (events=0 gaps=0)`，**且出现在 `window navigated to dsh` 之前** |

**当时的误判**：以为探针挂到了启动页，导航后新文档的 buffer 重置而计数未归零，
导致新事件被吞。据此改成「按页面 id 识别换文档」。

#### 第二轮：真正的原因 —— `initialization_script` **根本没生效**

| # | 配置 | 结果 |
| --- | --- | --- |
| R2 | `initialization_script` + 按页面 id 去重 | **`probe: attached page=` 一次都没出现**，连启动页都没有 |

对比 R1 与 R2 锁定了范围：

| 环节 | 判断 |
| --- | --- |
| 轮询线程 | ✅ 活着（`probe: enabled` 每次都打，线程未 panic） |
| `eval_with_callback` 回读 | ❌ 一直拿不到 → `dump()` 返回 `None` → `continue` |
| **`initialization_script` 注入** | ❌ **从未生效**（连启动页都没有 `__dshellProbeDump`） |

读 wry 实现确认它走的是 WebView2 的 `AddScriptToExecuteOnDocumentCreated`
（`wry-0.55.1/src/webview2/mod.rs:492-495, 1306-1314`）——链路存在但本项目实测不生效。

> **结论：`initialization_script` 在本项目不可用，改用 `eval` 注入。**
> `eval` 是本项目**已验证可靠**的通道（`splash_status`、`__dshell.leave()` 一直走它）。
> 代价是导航换文档后探针随旧文档消失，所以改为**每次轮询重注入一次**，
> 脚本用 `window.__dshellProbe` 做幂等。

#### 仍然没测到的（留档）

| # | 项 | 状态 |
| --- | --- | --- |
| V1 | 对话框期间 `hasFocus`/`visibilitychange` 时间线 | ❌ 未采集 |
| V2 | rAF 帧间隔是否拉长 | ❌ 未采集 |
| V5 | `Focused(true)` 是否自动回来 | ❌ 未采集 |

**为什么放弃继续修探针**：三次构建 + 人工复现的往返成本已经超过收益。
已知事实（§3 F1–F4 + §5.1 + §5.2）足以确定**修法方向**；
V1/V2/V5 只能区分 H1/H3 的**主次**，而这件事**用方案 C 本身就能验证**——
落地后现象消失即 H1 成立，只好了半截则需要补 H3（见 §9）。

> 另一个卡点：探针实例必须由**用户的终端**启动。本会话的 DSH 文件沙箱
> 禁止任何子进程写 `%USERPROFILE%\.dsh`（实测 `node` 直接 `EPERM`），
> 而 `dsh web` 后端启动时要写 `profiles/web/cordis.yml`。
> 所以 agent 侧无法自行拉起可用的 DShell 实例。

---

## 6. 候选修法

| 方案 | 做法 | 对应成因 | 代价 | 判定 |
| --- | --- | --- | --- | --- |
| **C. 关掉窗口节流** | `WebviewWindowBuilder` 上链式加 `additional_browser_args(...)`，补回 wry 默认三项 + 加 `--disable-backgrounding-occluded-windows --disable-renderer-backgrounding` | H1 | **一行**；已验证 API 存在（V-B）。副作用见 R1 | ✅ **已落地**（§6.2） |
| **A. 对话框关闭后归还焦点** | 观察 `WindowEvent::Focused(true)`，对 webview 补 `set_focus()` | H3 | 需确认事件是否真的到达；可能多余 | ⏳ 待 C 验证后决定 |
| **B. 强制 WebView 重绘** | 在可能关闭的时点 `eval` 触发样式重算 | H1 | 时机靠猜，脆弱 | ❌ 有 C 就不需要 |
| **D. 改 dsh 侧** | 在 `flow.ts` 里做手脚 | — | 改原版 dsh，违背薄壳定位，升级即失效 | ❌ **原则性否决** |
| **E. 不改** | — | — | 就是本 bug | ❌ |

**D 是本项目的红线**：DShell 的定位是薄壳（`README.md`：「刻意不使用 Tauri IPC——
DSH UI 只跟自己的后端走 HTTP/WebSocket，壳就只是壳」）。**修法不得进入 dsh 源码**，
只能在 `src-tauri/` 内侧解决。方案 C 完全满足这条。

### 6.1 方案 C 的关键代码（**已落地**）

`src-tauri/src/main.rs` 的 `WebviewWindowBuilder` 链上，`on_navigation` 之前：

```rust
// 立项 04：关掉 WebView2 的「窗口被遮挡/失焦就降级」行为。
//
// 为什么需要：点「添加工作区」会弹一个**原生 IFileOpenDialog**，
// 它由 dsh 的 win32-dialog-worker 子进程持有、盖在主窗口上。
// WebView2 判定本窗口被遮挡 → 停止提交渲染帧；对话框关闭后
// 没有任何东西通知它恢复，于是那一刻已经排队的 React commit
// 迟迟不上屏，要等下一次真实输入（点鼠标/敲键盘）才被唤醒。
// 这就是「选完文件夹点确定没反应，动一下才生效」的成因。
//
// 注意：**必须原样保留 wry 的默认参数**。wry 用的是
// `unwrap_or_else`（wry-0.55.1/src/webview2/mod.rs:294），
// 一旦我们调用本方法，它的默认值会被整体替换而不是追加；
// 漏掉这三个会让 WebView2 的 mini menu / SmartScreen 回来。
.additional_browser_args(concat!(
    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection ",
    "--disable-backgrounding-occluded-windows ",
    "--disable-renderer-backgrounding"
))
```

> 是否要连 `--disable-background-timer-throttling` 一起加，取决于 R1 的评估：
> 前两个影响的是**渲染**，第三个影响**定时器**，而本 bug 的核心是渲染提交。
> 决定**先只加前两个**，把副作用压到最小；不够再考虑第三个。

### 6.2 二进制校验（2026-09-12）

构建产物 `target-fix/release/dshell.exe`（3,484,160 B）已确认包含全部参数：

| 特征串 | 结果 |
| --- | --- |
| `disable-backgrounding-occluded-windows` | ✅ FOUND |
| `disable-renderer-backgrounding` | ✅ FOUND |
| `msWebOOUI` / `msPdfOOUI` / `msSmartScreenProtection` | ✅ FOUND（wry 默认值已补回） |
| `__dshellProbe` / `probe: attached page` | ✅ FOUND（探针仍在，用于本次验证） |

**尚未做**：真机复现（§9 验收 #1）。原因见 §5.3 末尾——agent 侧无法拉起
可用的 DShell 实例（沙箱禁止子进程写 `%USERPROFILE%\.dsh`）。

---

## 7. 验收清单（修完后逐条实测）

| # | 场景 | 期望 |
| --- | --- | --- |
| 1 | DShell：点 `+` → 选文件夹 → 点确定 | 工作区**立即**出现，会话区**立即**跳转，**无需**补鼠标/键盘 |
| 2 | DShell：同上但点**取消** | 不新增工作区，无错误弹窗 |
| 3 | DShell：同一文件夹**重复添加** | 幂等，不重复出现（走 `createWorkspace` 的 idempotent 解析） |
| 4 | DShell：添加**不存在的路径** | 错误弹窗出现（回归 `adoptDirectory` 的 catch 分支） |
| 5 | 浏览器对照：同一动作 | 行为与 DShell 一致（消除差异即达标） |
| 6 | 回归：关闭行为 | 托盘/关窗弹窗仍正常（`docs/closed/03`） |
| 7 | 回归：`+` 之外的两个头部按钮 | 搜索、视图选项不受影响 |
| 8 | 回归：连续添加 3 个工作区 | 每次都要即时刷新，不能"第一次不刷第二次才刷" |

---

## 8. 风险

| # | 风险 | 说明 |
| --- | --- | --- |
| R1 | 关闭节流（方案 C）影响后台耗电 | 托盘隐藏期间 WebView 仍在跑 DSH 的 SSE/WebSocket；若不节流，隐藏时的 CPU 占用会上升。需要评估是否只在"窗口可见但失焦"时关闭节流。**本次只加了两个渲染相关开关**，未动 `--disable-background-timer-throttling`，把副作用压到最小 |
| R2 | WebView2 是否支持这些 Chromium 开关 | ✅ **已确认支持**：`msedge.dll` 全量扫描到全部三个开关字符串（§5.1）。Tauri 侧走 `ICoreWebView2EnvironmentOptions::set_additional_browser_arguments`（§5.2），**不是** `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 环境变量——原判断有误，已更正 |
| R3 | 焦点归还可能与用户正在做的事打架 | 若用户在对话框关闭后立刻切到别的窗口，自动 `set_focus()` 会把用户拽回来——需要判断是否只在"对话框是由本窗口发起"时归还。**方案 A 未落地**，此风险暂不存在 |
| R4 | 现象可能不止一个成因 | H1+H3 叠加时，只修一个可能表现为"好了一半"；验收 #1 正是用来区分这件事的探针 |

---

## 9. 下一步

| 顺序 | 动作 | 状态 |
| --- | --- | --- |
| 1 | ~~落地方案 C（§6.1）~~ + ~~真机复现~~ | ✅ 已完成，但 **H1/H3 已被否证**（台账 §3.7、§3.12） |
| 2 | 清理 18 轮无效探针（`winprobe.rs`、V1/V2 开关、临时依赖、12 个 `target-*`） | ✅ 已完成（台账 §9.4） |
| 3 | ~~R10 实测~~：验证「幽灵点击」 | ✅ **成立**（台账 §10）；但 C20 被用户否证（§10.7） |
| 4 | **R11（本轮）**：方案 F1，壳层监听焦点事件主动收回激活态（`focus_fix.rs`） | ✅ 已构建，**待用户实测** |
| 5 | **R12**：若 F1 无效 → 方案 F2，壳层主动触发页面重绘 | ⏳ 待 R11 结果 |
| 6 | 跑完 §7 全部 8 条验收 | ⏳ |
| 7 | 删掉探针（`probe.rs` + 接线），保留 §6.1 的 R7 开关 | ⏳ |
| 8 | 按 `docs/AGENTS.md` 补 ③ 结案总结，移入 `docs/closed/`，并从 `roadmap.md` 删条目 | ⏳ |

### 9.1 给用户的操作（R11）

**构建产物**：`src-tauri\target-r11\release\dshell.exe`（3,500,032 B，2026-09-13 08:28）
> 为什么不是 `target\`：当前正在跑的 DShell 占着 `target\release\dshell.exe`（os error 5）。
> R11 测完会把这批临时目录一并清掉。

**操作要点（照做，顺序很重要）**：

1. 等界面进到 DSH 后，点「工作区」右边 `+`
2. 在弹出的**文件对话框**里选好文件夹
3. 点「**选这个目录**」
4. ⚠️ **点完立刻把手完全离开鼠标和键盘**——不要碰触摸板，不要碰任何键
5. **心里数到 15**（比平时更久，给足观察窗口）
6. 数完后，再随便点一下窗口（这一步是为了对照"用户补点"长什么样）

**看日志**：

```powershell
Select-String "$env:USERPROFILE\.dshell\dshell-poc.log" -Pattern "probe kind=|focus: " -Encoding UTF8 |
  Select-Object -Last 120 | ForEach-Object { $_.Line }
```

**先确认探针活着**（这两行没有就别往下读，该轮作废）：

```
probe: enabled (DSHELL_PROBE=1)
probe: 回传监听 127.0.0.1:45790
```

以及 `kind=selfcheck` 那行里 **`href=` 应该指向 `127.0.0.1:<端口>`**（说明注入到了 DSH 页面，
而不是只注入到启动页）。

**R11 的核心判据（新增两条）**：

| 你看到的 | 结论 |
| --- | --- |
| 有 `focus: dialog roundtrip detected (gap=...ms)`，且 `set_focus=true` | ✅ 修复**已触发** |
| 该行之后，**在你补点之前**就出现 `kind=snap ... changed=true` | ✅ **F1 有效**：界面自己刷新了，不用补点 |
| 该行之后直到你补点都没有 `changed=true` | ❌ **F1 无效** → 进 R12（F2） |
| 只有 `focus: regained after ...ms (user switch, no action)` | ⚠️ 时间窗没命中（gap > 1500ms）→ 说明对话框往返比预期慢，需调窗口 |
| 完全没有 `focus:` 行 | ❌ Tauri 的 `Focused` 事件没到达 → 换 F2 |

> **最关键的对照**：你在第 6 步"再点一下"之前，`kind=snap` 里有没有出现过新工作区名。
> 有 ⇒ 成了；没有 ⇒ 没成。

### 9.2 R10 的判读结果（已完成，留档）

| 你看到的 | 结论 |
| --- | --- |
| 对话框关闭瞬间有 `kind=down trusted=true`，而你**手没碰键鼠** | ✅ **幽灵点击实锤** → 假设成立（**R10 已证实**） |

---

## 10. 结案总结

> **待填**。写完这一段才算收口。
>
> 已知的结案素材（待第 3 步结果补齐后成文）：
> - 根因判定：H1（WebView2 遮挡节流）为主因，H3（焦点未归还）待第 3 步区分
> - 修法：方案 C，`src-tauri/` 内侧一行，未触碰 dsh 源码
> - 意外收获：**`initialization_script` 在本项目不生效**（§5.3 R2），
>   这条对将来任何"往 DSH 页面注入"的需求都成立，值得写进 `design.md`

---

## 附：本文的读码环境

| 项 | 值 |
| --- | --- |
| dsh 版本 | `0.1.5-rc.1`（`dsh --version`） |
| dsh 源码 | `D:\Projects\dsh-shell\src-of-dsh\deepseek-harness`（clone 的原版，用于参考） |
| DShell 源码 | `D:\Projects\dsh-shell\src-on-github` |
| WebView2 Runtime | `152.0.4191.66` |
| 宿主平台 | Windows（`platform === 'win32'` → native picker） |
| 记录时间 | 2026-09-12 |
