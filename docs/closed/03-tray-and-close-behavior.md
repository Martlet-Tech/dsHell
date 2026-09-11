# 03 · 托盘图标与关闭行为（三轮合一）

> **收口状态**：已实现、已实测、已收口（2026-09-11）。三轮内容合并在本文，按 `../AGENTS.md`
> 「一个升级项 = 一份文档」组织：第一轮 §1–§12（方案）、第二轮 §13–§18（关键代码）、
> 第三轮 §19–§27（实施与实测、结案总结）。
>
> 本文原在 `docs/plan/`，收口后移入 `docs/closed/`。正文里的相对路径按归档前的位置理解。
>
> 对应 roadmap 原 §5「托盘图标」条（收口时已从 roadmap 删除），并吃掉「关闭行为」这个新需求。
> 姊妹文档：`../plan/260901-托盘,系统设置,退出行为.md`（用户手写的原始设想，含后续的设置窗口/代理等，
> **本轮不实现**，保留不动）。

---

## 1. 要解决什么

现在只有一种退场方式：**点 × = 程序结束，后端被收掉**（`ExitRequested` → `taskkill /T /F`）。
这意味着每次想"暂时不用"都得付一次冷启动的代价：重新 spawn `dsh web` → 等 token URL → 重新 handoff。

本轮加一条**退居后台**的路，并给一个常驻入口把它叫回来。

### 1.1 本轮范围（就这 4 件）

| # | 条目 |
| --- | --- |
| 1 | 托盘图标（复用 exe 图标） |
| 2 | 托盘**左键单击** = 重新展开主界面 |
| 3 | 托盘**右键**菜单，唯一一项：**完全退出** |
| 4 | 主窗口点 × → 弹窗，弹窗含两个按钮（完全退出 / 关到托盘）+ 一个 ×（= 取消） |

### 1.2 非目标（本轮明确不做）

- 设置窗口（左右分栏、网络/代理、开机启动）——见用户手写文档，**单独立项**。
- 自绘标题栏、齿轮图标、遮罩层——那是"设置窗口"立项的前置，本轮不碰标题栏。
- 「不再提示」复选框——**已从范围中移除**，代价见 §6。
- 托盘图标状态化（tooltip 显示后端状态、图标随状态变色）。
- 快捷键、托盘菜单的「显示/隐藏」项、多窗口。

---

## 2. 关键约束：`hide()` 不是 `close()`（**必错点**）

这是本轮唯一**不做就必错**的地方，先写死。

现状（`main.rs` 末尾）：

```rust
.run(|app, event| {
    if let RunEvent::ExitRequested { .. } | RunEvent::Exit = event {
        // 杀 dsh / install 进程树
    }
})
```

`ExitRequested` 在**最后一个窗口关闭时**触发。因此：

| 实现方式 | 后果 |
| --- | --- |
| 「关到托盘」用 `close()` | 主窗口消失 → `ExitRequested` 触发 → **dsh 被杀** → 托盘图标成了点不开的空壳，点"恢复"只能看到一个死窗口 |
| 「关到托盘」用 `hide()` ✅ | 窗口隐藏但**未关闭**，`ExitRequested` 不触发；WebView 保活，**DSH 的 WebSocket 不断**，恢复是秒开，无需重新 handoff |

**约定：**

- 只有「完全退出」和托盘右键「完全退出」调用 `app.exit(0)`。
- `hide()` 是「关到托盘」的**唯一**实现手段。
- 托盘左键恢复用 `show()` + `set_focus()`。

---

## 3. 实测发现（本轮前置调研，2026-09-11，本机）

调研对象：`Cargo.lock` 解析到的 `tauri 2.11.5`，源码位于
`C:\Users\zt\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\tauri-2.11.5\`。

| # | 项 | 结论 | 影响 |
| --- | --- | --- | --- |
| F1 | `tray-icon` 是否为默认 feature | **否**。tauri 的 `default = [wry, compression, common-controls-v6, dynamic-acl, x11, dbus]` | 现在 `Cargo.toml` 写的是 `features = []`，**托盘代码不加 feature 根本编译不过**。必须显式开 |
| F2 | 托盘 API 是否齐全 | ✅ `TrayIconBuilder` 有 `icon` / `menu` / `tooltip` / `show_menu_on_left_click` / `on_tray_icon_event` / `build` | 够用 |
| F3 | 单击事件能否区分左右键 | ✅ `TrayIconEvent::Click { button, button_state, .. }`，`button` 是 `MouseButton::{Left,Right}` | 「左键单击恢复」可靠；右键交给菜单，无需自己判断 |
| F4 | `show_menu_on_left_click` 默认值 | 默认 **true**（左键会弹菜单） | 「左键恢复」必须显式 `show_menu_on_left_click(false)`，否则左键弹菜单而不是恢复 |
| F5 | 复用 exe 图标的路径 | ✅ `app.default_window_icon()` 返回 `Option<&Image>` | **零资源文件新增**：直接把默认窗口图标喂给托盘 |
| F6 | 图标资源是否存在 | ✅ `src-tauri/icons/icon.ico`（4,282 B）已在库中 | 无需画新图标 |
| F7 | 多 WebView（一窗多 webview） | ⚠️ `Window::add_child` 被 `#[cfg(all(desktop, feature = "unstable"))]` 门控 | **这是 §4 决策点的根据，也是本轮最大的不确定性** |

---

## 4. 决策点：弹窗画在哪（**需拍板**）

难点不在托盘，在**弹窗**。点 × 的那一刻，主窗口里跑的已经是 **DSH 的远程页面**（`ui/index.html` 在 handoff 时被 navigate 掉了），我们没有自己的页面可以画弹窗。

四条路：

| 方案 | 做法 | 优点 | 代价 | 判定 |
| --- | --- | --- | --- | --- |
| **A. 原生对话框** | `tauri-plugin-dialog` 的 `MessageDialogBuilder`，两个自定义按钮 | **零新增依赖**（该插件已在用）；行为可靠；无需处理层级/焦点 | 外观是 Windows 原生的，**不能自定义**；按钮文字能否精确指定为中文需实测（P2） | ✅ **本轮推荐** |
| **B. 注入 DSH 页面** | 往远程页面插 DOM | 外观自控 | DSH 是 SPA，节点会被 re-render 冲掉；**随 DSH 版本碎** | ❌ 否决 |
| **C. 先跳回自己的页面再跳回** | × → navigate 回 `ui/close.html` → 弹完再 navigate 回去 | 外观自控 | **丢失 DSH UI 全部状态**；且 token URL 可能已失效，等于重新 handoff | ❌ 否决 |
| **D. 第二个 WebView 当弹窗**（用户设想） | 主窗口内 `add_child` 一个受控尺寸的 webview，居中显示自己的 HTML | 外观自控；**顺带立起 A/B 双 WebView 结构**，为将来的齿轮/设置面板铺路；DSH 页面保活不丢状态 | **`add_child` 需要 `unstable` feature（F7）**；要自己处理居中、窗口 resize 时重定位、焦点 | ⚠️ **结构与收益最优，但踩在 unstable 上** |

### 4.1 建议

**本轮取 A，把 D 记为下一个立项的第一件事。**

理由：

1. 本轮目标是"**范围缩到最小**"，A 是四条里唯一零新增依赖、零架构风险的。
2. D 的收益（为设置窗口铺路）属于**下一轮**的价值；本轮用不上弹窗之外的第二层。
3. D 踩在 `unstable` feature 上——**Tauri 明确标记该 API 不稳定**，形状可能变。把架构风险压到最小版本里不划算。

若你更看重"一次把 A/B 结构立起来、后面不再返工"，则选 D，但要在 §9 验收清单里补上"`unstable` feature 开启后仍能 release 构建"这条。

> 无论 A 还是 D，**§2 的 `hide()` 约束都适用**，与弹窗实现无关。

---

## 5. 交互流程

```
                    ┌─────────────────────────────────────────┐
                    │  主窗口（Native Window）                  │
                    │   WebView: DSH 页面（handoff 后）         │
                    └─────────────────────────────────────────┘

  主窗口 × ──▶ on_window_event(CloseRequested)
                 └─ api.prevent_close()            ← 先拦下来，窗口不关
                 └─ 弹窗（方案 A/D）

                    弹窗
                    ├─ [×]        → 什么都不做，关掉弹窗（= 取消，窗口保持可见）
                    ├─ [关到托盘]  → window.hide()      ← §2：绝不能用 close()
                    └─ [完全退出]  → app.exit(0)  → 现有 ExitRequested 清理链

  托盘左键单击 ──▶ window.show() + window.set_focus()
  托盘右键 ──────▶ 菜单（唯一项）: [完全退出] → app.exit(0)
```

### 5.1 三条退场路径的语义

| 入口 | 行为 | 后端 | 托盘 |
| --- | --- | --- | --- |
| 弹窗「完全退出」 | `app.exit(0)` | 收掉（`kill_tree`） | 消失 |
| 弹窗「关到托盘」 | `window.hide()` | **保活** | 保留 |
| 弹窗「×」 | 无 | 保活 | 保留 |
| 托盘「完全退出」 | `app.exit(0)` | 收掉 | 消失 |

---

## 6. 已知取舍（明确接受，不修）

| # | 取舍 | 说明 |
| --- | --- | --- |
| T1 | **每次点 × 都会被问** | 「不再提示」已移出范围。用户若频繁开关会很烦。后续可在设置窗口里补一个开关（属另一立项） |
| T2 | 托盘图标可能被 Win11 折叠进溢出区 | 用户可能找不到图标、以为程序已退。**按"顺其自然"处理，不主动提示** |
| T3 | 弹窗的 × 与「关到托盘」不是同一个动作 | × 是取消（用户点错了）。这与"关到托盘"并列而非重复，**符合 Windows 惯例** |
| T4 | 双击托盘不处理 | 已确认 Windows 常见软件（Clash、邮件大师）都是**左键单击**恢复，双击无额外语义 |

---

## 7. 涉及文件

**改**

| 文件 | 改动 |
| --- | --- |
| `src-tauri/Cargo.toml` | **必须**加 `tauri` 的 `tray-icon` feature（F1）。若选方案 D，另加 `unstable` |
| `src-tauri/src/main.rs` | 托盘构建（`setup` 内）；`on_window_event` 拦 `CloseRequested`；新增命令供弹窗回传选择 |
| `src-tauri/tauri.conf.json` | 无需改（图标复用 `default_window_icon()`）。若选 D 且需 `bundle.active`，待定 |
| `docs/roadmap.md` | 收口时删掉 §5 的「托盘图标」条 |

**新增**

| 文件 | 职责 |
| --- | --- |
| `src-tauri/src/tray.rs` | 托盘图标构建 + 左键/右键事件（建议独立成模块，`main.rs` 已 608 行） |
| `ui/close.html` | **仅方案 D**：弹窗页面（两个按钮 + ×） |
| `docs/closed/03-tray-and-close-behavior.md` | 本文（立项时在 `docs/plan/`） |

> 注：`capabilities/default.json` 现在 `windows: ["main"]`。方案 D 若新增 webview label，需确认是否要一并声明权限（待实测，见 P3）。

---

## 8. 关键契约（不含完整实现）

### 8.1 托盘（`tray.rs`）

```rust
// 复用 exe 图标，零资源新增（F5）
let icon = app.default_window_icon().cloned();   // Option<Image>

TrayIconBuilder::with_id("main")
    .icon(icon)
    .tooltip("DShell")
    .menu(&menu)                      // 唯一项：完全退出
    .show_menu_on_left_click(false)   // F4：默认 true，不改则左键弹菜单
    .on_tray_icon_event(|tray, event| match event {
        TrayIconEvent::Click { button: MouseButton::Left,
                               button_state: MouseButtonState::Up, .. } => {
            // show() + set_focus()
        }
        _ => {}
    })
    .build(app)?;
```

### 8.2 拦截关闭（`main.rs`）

```rust
.on_window_event(|window, event| {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();     // 先拦，窗口不关
        // 再弹窗（A：原生对话框 / D：显示子 webview）
    }
})
```

### 8.3 硬约束（重复强调）

- 「关到托盘」= `window.hide()`。**任何情况下不得用 `close()`**（§2）。
- 「完全退出」= `app.exit(0)`，复用现有 `ExitRequested` 清理链，**不新增清理逻辑**。

---

## 9. 验收清单

| # | 场景 | 期望 |
| --- | --- | --- |
| 1 | 启动后看托盘 | 图标出现，与 exe 图标一致（F5/F6） |
| 2 | 托盘**左键单击** | 主界面重新展开并置顶（不是弹菜单，验 F4） |
| 3 | 托盘**右键** | 菜单只有一项「完全退出」 |
| 4 | 托盘右键 → 完全退出 | 程序退出，**任务管理器无残留 node/cmd**，端口释放 |
| 5 | 主窗口点 × | 弹窗出现，**窗口没有关闭**（验 `prevent_close`） |
| 6 | 弹窗点 × | 弹窗消失，主窗口还在，DSH UI 状态**未丢** |
| 7 | 弹窗点「关到托盘」 | 窗口隐藏、托盘图标还在、任务管理器里 **node 仍在**（验 `hide()` 而非 `close()` —— **本条是关键断言**） |
| 8 | 承接 7，托盘左键 | 秒回 DSH UI，**对话上下文未丢、无需重新 handoff**（验 WebView 保活） |
| 9 | 承接 7，托盘右键退出 | 干净退出，无残留 |
| 10 | 弹窗出现时再点 × 反复多次 | 不产生多个弹窗 / 不泄漏 |
| 11 | 回归：正常 handoff | 体检 → 抓 token URL → 进入 DSH UI 仍正常（改版未破坏主链路） |
| 12 | 回归：外链点击 | 仍交给系统浏览器（design.md §5） |

---

## 10. 风险与待确认项

| # | 项 | 现状 | 处置 |
| --- | --- | --- | --- |
| P1 | `tray-icon` feature 未开 | **已确认**（F1），`features = []` | 实现第一步就加，否则编译不过 |
| P2 | 原生对话框能否自定义两个中文按钮文字 | ✅ **已解决**（见 §14 R5）：支持，但**只有两键**，见 §14 R6 的取舍 | 采用 `YesNoCancelCustom`，把「取消」键当第三键 |
| P3 | 子 webview 是否需 capability 声明 | 未实测 | 仅方案 D 相关 |
| P4 | `add_child` 的 `unstable` 依赖 | **已确认**（F7） | 方案 D 的已知代价；选 D 则需实测 release 构建通过 |
| P5 | 窗口 resize 时子 webview 居中 | 方案 D 才需处理 | 选 D 时确认是否需监听 resize 重定位（`set_bounds`），或用 `set_auto_resize` |
| P6 | `app.exit(0)` 是否稳定触发 `ExitRequested` | 现有代码 `Exit` 与 `ExitRequested` 都兜了 | 验收 #4/#9 实测确认清理真的跑（托盘路径是新增入口，不能只信既有结论） |
| P7 | 隐藏期间 DSH 后端是否会被自身超时/GC 影响 | 未验证 | 验收 #8 实测；若长时间隐藏后恢复异常，需评估 dsh 侧连接保活 |
| P8 | 「×」在无边框/自定义标题栏下的行为 | 本轮**不碰标题栏**，用系统装饰 | 无风险；将来做设置窗口时再说 |

---

## 11. 待拍板

| # | 决策项 | 选项 | 建议 |
| --- | --- | --- | --- |
| D1 | **弹窗实现** | A 原生对话框 / D 第二 WebView | **A**（本轮最小化）；D 记为下一立项首件事 |
| D2 | 托盘单击范围 | 仅左键 / 左右键都恢复 | 仅左键（右键留给菜单） |
| D3 | 弹窗文案 | 待定 | 「完全退出」/「关到托盘」保持与用户文档一致 |
| D4 | 点击 × 弹窗时，若窗口处于"已隐藏"状态 | 不可能发生（隐藏时无 ×） | 无需处理 |

---

## 12. 交付节奏

| 轮次 | 产出 |
| --- | --- |
| **第一轮（本文 §1–§12）** | 方案、范围/非目标、实测发现、决策点、验收清单、风险 |
| **第二轮（§13–§16）** | 关键代码、模块划分、`Cargo.toml` diff、文案常量表 |
| **第三轮** | 代码落地 + build 通过 + §9 验收清单逐条实测，写回结果 |

---

# 第二轮：关键代码与模块划分

> 设计原则（本轮特别要求）：**变量与常量集中、职责单一、按文件归类**。
> 不把托盘逻辑塞进已经 608 行的 `main.rs`；所有可调文案、标识集中在一处常量表。
> 原则：**以后要改某个行为，只需要打开对应的那一个文件。**

## 13. 模块划分（新增 3 个文件，改 2 个）

```
src-tauri/src/
├── main.rs        改：只加"接线"——建托盘、挂窗口事件、注册命令。不写业务判断
├── tray.rs        新：托盘图标的一切（构建、菜单、左键/右键事件）
├── lifecycle.rs   新：关闭行为的唯一裁决处（弹窗 + 三条出路）
├── ui_text.rs     新：所有面向用户的文案常量（中文集中一处，好改好查）
├── config.rs      不改（本轮不落盘任何东西）
├── doctor.rs      不改
├── installer.rs   不改
└── proc.rs        不改
```

**划界理由：**

| 文件 | 只负责 | 明确不负责 |
| --- | --- | --- |
| `tray.rs` | 图标、菜单、点击事件 → 转成**语义动作**（`TrayAction`） | 不知道窗口怎么显示、不知道程序怎么退 |
| `lifecycle.rs` | 「点 × 该干什么」的**唯一**决策点；调 `hide()` / `exit()` | 不碰托盘 |
| `ui_text.rs` | 字符串常量 | 无逻辑 |
| `main.rs` | 用 `match` 把动作接到 Tauri API 上 | 不写 if/else 业务分支 |

这样划分后：**改文案** → `ui_text.rs`；**改关闭行为** → `lifecycle.rs`；**改托盘** → `tray.rs`。

---

## 14. 第二轮实测发现（读依赖源码，2026-09-11）

调研对象：`tauri-plugin-dialog 2.7.2` 与 `rfd 0.16.0`（`Cargo.lock` 实际解析版本），源码在
`C:\Users\zt\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\`。

| # | 项 | 结论 | 影响 |
| --- | --- | --- | --- |
| R5 | 两个按钮能否自定义中文 | ✅ **可以**。`MessageDialogButtons::OkCancelCustom(String, String)` | 「完全退出 / 关到托盘」可用 |
| R6 | **X 按钮返回什么** | ⚠️ **X 与「取消」键返回同一个 `Cancel`**（`rfd` 的 `_ => MessageDialogResult::Cancel` 分支） | **两键方案无法表达你的"×=取消"与"×=第三动作"的区分**——见下 |
| R7 | 三键能否自定义 | ✅ `YesNoCancelCustom(yes, no, cancel)` | **采用它**：把第三键当"取消"，× 天然归入它 |
| R8 | Windows 自定义按钮的 feature 依赖 | ⚠️ **仅当 `common-controls-v6` 开启时自定义文字才生效**；否则退回 `MessageBoxW`，自定义文字**丢失** | tauri 的 `default` feature 含 `common-controls-v6`（F1 已确认），**满足**。但须在实现时确认它没被 `features = []` 关掉——**这是 P1 之外的第二个编译期陷阱** |
| R9 | 异步还是阻塞 | 有 `blocking_show_with_result()` 与 `show_with_result(cb)` | 用**回调版**，避免阻塞 UI 线程 |

### 14.1 R6 / R7 对方案的影响（重要）

你的设计是三条出路：**完全退出 / 关到托盘 / ×=取消**。

- 用 `OkCancelCustom`（两键）：**× 和第二个键都返回 `Cancel`**，无法区分。若第二个键是「关到托盘」，则**× 也会关到托盘**——违背你"× 是按错了"的语义。
- 用 `YesNoCancelCustom`（三键）：三个可区分的结果 + **× 自然映射到 Cancel**。语义完全吻合：

```
        ┌──────────────────────────────────────────────┐
        │  DShell                                       │  ← 标题
        │                                               │
        │  要退出 DShell，还是只在后台运行？              │  ← 正文
        │                                               │
        │   [ 完全退出 ]  [ 关到托盘 ]  [ 取消 ]     [×] │
        └──────────────────────────────────────────────┘
             yes_text       no_text    cancel_text   ×→Cancel
```

**决策：采用 `YesNoCancelCustom`。** 三键且语义精确，"×" 与「取消」合并（这正是 Windows 惯例，
用户点 × 与点取消预期一致）。

> 代价：多了一个「取消」按钮，比你原本设想的"只有 ×"多一个显式出口。但这是**必要的**——
> 否则无法用系统对话框表达三态。若你坚持"只有两个按钮 + ×"，则唯一出路是方案 D（自绘）。**这条请确认。**

---

## 15. 关键代码

### 15.1 `ui_text.rs`（全文，新增）

集中所有文案。改词只动这里。
**形态按 i18n 预留（§18.1）**：对外是**函数**而非常量，将来换查表时调用点不动。

```rust
//! 面向用户的文案。**所有中文都只在这里出现。**
//!
//! 为什么是函数而不是 `pub const`：将来做 i18n 时，这里改成"按当前语言查表"，
//! 所有调用点（`ui_text::menu_quit()`）**一行都不用改**。

/// 当前语言。现在就留好入口，将来由设置里切换。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    ZhCn,
}

/// 取当前语言。将来改为读取用户设置。
pub fn current() -> Lang {
    Lang::ZhCn
}

// ── 托盘 ──────────────────────────────────────────────
pub fn tray_tooltip() -> &'static str {
    match current() {
        Lang::ZhCn => "DShell",
    }
}

pub fn menu_quit() -> &'static str {
    match current() {
        Lang::ZhCn => "完全退出",
    }
}

// ── 关闭确认弹窗（三键） ────────────────────────────────
pub fn close_dialog_title() -> &'static str {
    match current() {
        Lang::ZhCn => "DShell",
    }
}

pub fn close_dialog_body() -> &'static str {
    match current() {
        Lang::ZhCn => "要退出 DShell，还是只在后台运行？",
    }
}

/// 第一键 → 正常退出
pub fn close_btn_quit() -> &'static str {
    match current() {
        Lang::ZhCn => "完全退出",
    }
}

/// 第二键 → 缩到托盘
pub fn close_btn_tray() -> &'static str {
    match current() {
        Lang::ZhCn => "关到托盘",
    }
}

/// 第三键 → 取消（点 × 也归这里）
pub fn close_btn_cancel() -> &'static str {
    match current() {
        Lang::ZhCn => "取消",
    }
}
```

### 15.2 `tray.rs`（全文，新增）

只做"托盘 → 语义动作"的翻译，不碰窗口与进程。

```rust
//! 托盘图标：构建、菜单、点击。
//!
//! 本模块**只产出语义动作**（`TrayAction`），由 `main.rs` 决定怎么执行。
//! 这样托盘的 UI 与"关窗/退出"的副作用彻底解耦。

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Manager, Runtime,
};

use crate::lifecycle;
use crate::ui_text;

/// 托盘菜单项 id（集中一处，避免字符串散落）
const MENU_ID_QUIT: &str = "tray-quit";
/// 托盘图标 id
const TRAY_ID: &str = "main-tray";

/// 托盘能产生的动作。新增动作时在这里加一项，编译器会提示所有该处理的地方。
pub enum TrayAction {
    /// 左键单击 → 重新展开主界面
    RestoreWindow,
    /// 菜单「完全退出」
    Quit,
}

/// 建托盘。在 `setup` 里调用一次。
pub fn init<R: Runtime>(app: &App<R>) -> tauri::Result<()> {
    let quit_item = MenuItem::with_id(app, MENU_ID_QUIT, ui_text::menu_quit(), true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&quit_item])?;

    // 复用 exe 图标：不新增任何资源文件（F5/F6）。
    // 注意 `Image` **没有 `Default`**（P11 实测踩到），所以不能 `unwrap_or_default()`；
    // 拿不到图标时仍要把托盘建出来，否则「关到托盘」会让窗口彻底找不回来。
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(ui_text::tray_tooltip())
        .menu(&menu)
        // F4：默认为 true，不关掉的话左键会弹菜单而不是恢复窗口
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            if event.id() == MENU_ID_QUIT {
                dispatch(app, TrayAction::Quit);
            }
        })
        .on_tray_icon_event(|tray, event| {
            // 只认左键抬起，避免按下+抬起触发两次
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                dispatch(tray.app_handle(), TrayAction::RestoreWindow);
            }
        });

    match app.default_window_icon().cloned() {
        Some(icon) => builder = builder.icon(icon),
        None => crate::log("tray: no default window icon available"),
    }

    builder.build(app)?;

    Ok(())
}

/// 唯一的动作出口：托盘不自己干副作用，交给 lifecycle。
fn dispatch<R: Runtime>(app: &AppHandle<R>, action: TrayAction) {
    match action {
        TrayAction::RestoreWindow => lifecycle::restore_main_window(app),
        TrayAction::Quit => lifecycle::quit(app),
    }
}
```

### 15.3 `lifecycle.rs`（全文，新增）

**关闭行为的唯一裁决处。** `hide()` 约束写在这一处，防止将来别处误用 `close()`。

```rust
//! 窗口生命周期：关闭行为的唯一裁决点。
//!
//! **本文件是 `hide()` 与 `exit()` 的唯一调用处。**
//! 任何其他文件都不得直接 hide/close/exit 主窗口 —— 关窗行为只有这里说了算。

use tauri::{AppHandle, Manager, Runtime};

use crate::ui_text;

/// 「关到托盘」：隐藏窗口，**绝不能用 close()**。
///
/// 为什么必须是 hide()：`ExitRequested` 在最后一个窗口**关闭**时触发，
/// 而它负责收掉 dsh 进程树。若此处用 close()，dsh 会被连带杀掉，
/// 托盘图标变成一个点不开的空壳。详见方案 §2。
pub fn hide_to_tray<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window(crate::MAIN_WINDOW) {
        let _ = w.hide();
        crate::log("close behavior: hidden to tray (dsh kept alive)");
    }
}

/// 「完全退出」：走既有 ExitRequested 清理链，**不新增清理逻辑**。
pub fn quit<R: Runtime>(app: &AppHandle<R>) {
    crate::log("close behavior: full quit");
    app.exit(0);
}

/// 托盘左键：重新展开主界面并置顶。
pub fn restore_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window(crate::MAIN_WINDOW) {
        let _ = w.show();
        let _ = w.set_focus();
        crate::log("tray: restored main window");
    }
}

/// 弹窗的三个结果 → 三条出路。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseChoice {
    /// 「完全退出」
    Quit,
    /// 「关到托盘」
    HideToTray,
    /// 「取消」或点 × —— 什么都不做
    Cancel,
}

/// 关闭弹窗的动作表：把"用户选了什么"翻译成"该干什么"。
/// 新增选项时改这里一处即可。
pub fn apply_close_choice<R: Runtime>(app: &AppHandle<R>, choice: CloseChoice) {
    match choice {
        CloseChoice::Quit => quit(app),
        CloseChoice::HideToTray => hide_to_tray(app),
        CloseChoice::Cancel => {
            crate::log("close behavior: cancelled (window stays visible)");
        }
    }
}

/// 弹窗文案与按钮（集中在这里，`close_dialog.rs` 只负责调用）。
/// 用函数取文案，跟随 `ui_text` 的语言设置。
pub struct CloseDialogSpec {
    pub title: &'static str,
    pub body: &'static str,
    pub yes: &'static str,    // 完全退出
    pub no: &'static str,     // 关到托盘
    pub cancel: &'static str, // 取消
}

impl CloseDialogSpec {
    pub fn load() -> Self {
        Self {
            title: ui_text::close_dialog_title(),
            body: ui_text::close_dialog_body(),
            yes: ui_text::close_btn_quit(),
            no: ui_text::close_btn_tray(),
            cancel: ui_text::close_btn_cancel(),
        }
    }
}
```

### 15.4 `close_dialog.rs`（全文，新增）—— 原生对话框的唯一封装

把 `tauri-plugin-dialog` 的调用细节关在这一个文件里，将来换方案 D 只改这里。

```rust
//! 关闭确认弹窗（系统原生）。
//!
//! 用三键 `YesNoCancelCustom`：能同时表达"退出/托盘/取消"三态，
//! 且系统的 × 按钮天然映射到 Cancel（= 取消），符合用户按错了的预期。
//!
//! 为什么不用两键 `OkCancelCustom`：× 与第二个键都返回 Cancel，无法区分（R6）。

use tauri::{AppHandle, Runtime};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult};

use crate::lifecycle::{self, CloseChoice, CloseDialogSpec};

/// 弹出关闭确认框，并把结果应用到 `lifecycle`。
///
/// 注意：用**回调版**而非 `blocking_show_with_result()`，
/// 避免阻塞 UI 线程（R9）。
pub fn ask_close_choice<R: Runtime>(app: &AppHandle<R>) {
    let spec = CloseDialogSpec::load();

    app.dialog()
        .message(spec.body)
        .title(spec.title)
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::YesNoCancelCustom(
            spec.yes.to_string(),
            spec.no.to_string(),
            spec.cancel.to_string(),
        ))
        .show_with_result({
            let app = app.clone();
            move |result| {
                lifecycle::apply_close_choice(&app, interpret(&result, &yes, &no));
            }
        });
}

/// 把对话框结果翻译成三条出路。
///
/// Windows 上（rfd 的 win_cid 后端 + 插件的覆盖映射，见 §17 P9）自定义按钮返回
/// `Custom(<按钮文字>)` 而非 `Yes`/`No`。两种形态都认，避免随版本变化失效。
fn interpret(result: &MessageDialogResult, yes: &str, no: &str) -> CloseChoice {
    match result {
        MessageDialogResult::Custom(text) => {
            if text == yes {
                CloseChoice::Quit
            } else if text == no {
                CloseChoice::HideToTray
            } else {
                // 第三键（取消）与任何未知文字 → 什么都不做
                CloseChoice::Cancel
            }
        }
        // 兜底：若某版本直接返回系统枚举
        MessageDialogResult::Yes => CloseChoice::Quit,
        MessageDialogResult::No => CloseChoice::HideToTray,
        // Cancel（含点 × 关闭对话框）与其余一切 → 什么都不做
        _ => CloseChoice::Cancel,
    }
}
```

> **P9 已实测解决**（§17）：Windows 返回 `Custom(文字)`，上述 `Custom` 分支是主路径。
> `interpret` 带 5 条单元测试，其中 `window_x_is_cancel_not_hide` 专门锁死「点 × ≠ 缩托盘」。

### 15.5 `main.rs` 的改动（只加接线，约 20 行）

```rust
// ① 顶部：新增模块声明 + 窗口 label 常量（集中，避免 "main" 字符串散落）
mod close_dialog;
mod lifecycle;
mod tray;
mod ui_text;

/// 主窗口 label —— 全项目唯一来源
pub const MAIN_WINDOW: &str = "main";

// ② Cargo.toml 需加 feature（见 §16），代码侧无需改动

// ③ setup 内，建窗之后加一行：
tray::init(app)?;

// ④ 在 .build() 之前挂窗口事件（拦截 ×）：
.on_window_event(|window, event| {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        // 先拦下：窗口不关，交给弹窗决定
        api.prevent_close();
        close_dialog::ask_close_choice(window.app_handle());
    }
})

// ⑤ 现有 .run(...) 的 ExitRequested 清理链 **原样保留，不动**
```

**注意 ④ 的位置**：`on_window_event` 挂在 `Builder` 上（对所有窗口生效）。将来做设置窗口时，
需按 `window.label()` 过滤，避免设置窗口的 × 也弹这个框。本轮只有主窗口，先不过滤，
但**在代码里留注释标明这个前提**。

### 15.6 为什么不把弹窗逻辑放进 `lifecycle.rs`

`lifecycle.rs` 是**纯决策 + 副作用**（hide/exit），零 UI 依赖；`close_dialog.rs` 是**UI 呈现**。
分开后：

- 将来换方案 D（自绘弹窗）→ **只重写 `close_dialog.rs`**，`lifecycle.rs` 一行不动
- 将来加"不再提示"→ 改 `lifecycle.rs` 的 `CloseChoice` 与 `apply_close_choice` 一处

---

## 16. `Cargo.toml` diff

```diff
 [dependencies]
-tauri = { version = "2", features = [] }
+# tray-icon: 托盘支持。**不是 tauri 的默认 feature**，不加则 TrayIconBuilder 不存在（F1）
+# common-controls-v6: Windows 下让原生对话框支持自定义按钮文字（R8）。已在 tauri
+#   的 default 里，这里显式写出以防将来有人关掉 default-features 时静默失效。
+tauri = { version = "2", features = ["tray-icon", "common-controls-v6"] }
```

> `tauri-plugin-dialog` **已在依赖中，无需新增**（方案 A 的核心优势）。

---

## 17. 待核对项 → 实测结论

| # | 项 | 结论（2026-09-11 实测） |
| --- | --- | --- |
| P9 | `show_with_result` 对 `YesNoCancelCustom` 返回的是 `Yes/No/Cancel` 还是 `Custom(文字)` | ✅ **已解决**：Windows 上返回 `Custom(<按钮文字>)`（`desktop.rs` 的 `show_message_dialog` 做了覆盖映射）。实现按 `Custom` 分支为主、系统枚举为兜底，两条都测过 |
| P10 | `on_window_event` 挂 Builder 是否对主窗口之外也生效 | ⚠️ **确认属实**：挂在 Builder 上对所有窗口生效。本轮只有主窗口无影响；**做设置窗口时必须按 label 过滤**（代码已留注释，见 L1） |
| P11 | 托盘 `default_window_icon()` 在 release 下是否可用 | ✅ **可用**：托盘图标正常显示（窗口类 `tray_icon_app`，用户目视确认有图标）。`Image` 无 `Default`，故实现改为 `match` 分支而非 `unwrap_or_default()` |
| P12 | `common-controls-v6` 是否真在生效（自定义中文文字是否显示） | ✅ **已生效**：用户目视确认弹窗三键为中文「完全退出 / 关到托盘 / 取消」 |

---

## 18. 决策记录（已拍板，2026-09-11）

| # | 问题 | 结论 |
| --- | --- | --- |
| **Q1** | 是否接受三键（完全退出 / 关到托盘 / 取消） | **接受**。方案 A（系统原生对话框）定稿，不走方案 D |
| **Q2** | `ui_text.rs` 是否单独成文件 | **单独成文件**。理由：将来做 i18n 时只替换这一个文件即可 |
| **Q3** | 是否现在就把 `mod` 声明与 `MAIN_WINDOW` 常量落进代码 | **现在落**（本轮一并完成） |

Q1 的取舍留档：三键比原设想的"两键 + ×"多一个显式「取消」按钮，这是**系统对话框表达三态的必然代价**（R6）。
换来的是语义精确——× 与「取消」都归入 `Cancel`，符合"点错了"的预期。

### 18.1 Q2 对 `ui_text.rs` 的追加要求（i18n 预留）

既然以 i18n 为理由独立，文件形态就要**现在**按 i18n 友好的方式写，避免将来重写：

- **不写成裸 `pub const`**，改为按"语言 → 词条"组织的函数，当前只有中文一种实现。
- 调用点用 `ui_text::tray_tooltip()` 这样的**函数**而非常量，将来换成查表时**调用点不用改**。

形态见 §15.1（已按此更新）。

---

# 第三轮：实施与实测记录

> 实施日期：2026-09-11。全部代码已落地，release 构建通过，§9 验收清单已实测。

## 19. 交付物

| 文件 | 状态 | 说明 |
| --- | --- | --- |
| `src-tauri/src/tray.rs` | 新增 | 托盘图标、菜单、点击 → `TrayAction` |
| `src-tauri/src/lifecycle.rs` | 新增 | 关闭行为唯一裁决点；`hide()` / `exit()` 唯一调用处 |
| `src-tauri/src/close_dialog.rs` | 新增 | 原生三键对话框封装 + 结果翻译（含 5 条单元测试） |
| `src-tauri/src/ui_text.rs` | 新增 | 全部用户文案，按 i18n 形态写成函数 |
| `src-tauri/src/main.rs` | 改 | 模块声明、`MAIN_WINDOW` 常量、`tray::init`、`on_window_event` 接线 |
| `src-tauri/Cargo.toml` | 改 | `tauri` 加 `tray-icon` + `common-controls-v6` feature |

## 20. 编译与单元测试

| 项 | 结果 |
| --- | --- |
| `cargo check --release` | ✅ 通过，**零 warning** |
| `cargo build --release` | ✅ 通过，产物 `target/release/dshell.exe` **3,475,456 B**（原 3,001,856 B） |
| `cargo test --release close_dialog` | ✅ **5 passed / 0 failed** |

单测覆盖（`close_dialog.rs`）：

| 测试 | 断言 |
| --- | --- |
| `custom_yes_is_quit` | 「完全退出」→ `Quit` |
| `custom_no_is_hide` | 「关到托盘」→ `HideToTray` |
| `custom_cancel_is_cancel` | 「取消」→ `Cancel` |
| **`window_x_is_cancel_not_hide`** | **点 × → `Cancel`（不是缩托盘）** ← R6 的回归防线 |
| `plain_enum_fallback` | 插件若返回系统枚举也能正确翻译 |

## 21. 实测记录（本机，2026-09-11）

`release` 构建后真实运行，证据取自 `%USERPROFILE%\.dshell\dshell-poc.log` 与进程列表：

| # | 场景 | 证据 | 结论 |
| --- | --- | --- | --- |
| 1 | 启动 + 托盘建立 | 窗口类列表含 **`tray_icon_app`**；日志无 `tray: failed` | ✅ 托盘建成 |
| 2 | 体检 → handoff 回归 | `doctor: allOk=true` → `spawned dsh web` → `window navigated to dsh` | ✅ 主链路未被破坏（验收 #11） |
| 3 | **「关到托盘」** | 日志 `close behavior: hidden to tray (dsh kept alive)` | ✅ |
| 4 | **隐藏期间后端存活** | 隐藏后 `dshell` 主进程与其 `node` 子进程**均仍在** | ✅ **验收 #7 关键断言通过 —— 确认走的是 `hide()` 而非 `close()`** |
| 5 | **托盘左键恢复** | 日志 `tray: restored main window`；用户实测可恢复 | ✅ 验收 #2/#8 |
| 6 | **「完全退出」** | 日志 `close behavior: full quit` → `shutting down: killing the dsh process tree (pid 26884)` | ✅ **验收 #4：`app.exit(0)` 确实触发了既有清理链（P6 解除）** |
| 7 | 退出后无残留 | 测试实例 3 个 pid **全部消失**，无孤儿 node | ✅ 验收 #9 |
| 8 | 单击而非双击 | 窗口类含 `tray_icon_app`，事件按 `button_state: Up` 判定 | ✅ 验收 #2 |

**尚未实测**（留档）：

| 项 | 原因 |
| --- | --- |
| 主窗口 × 弹窗的实际外观与中文按钮文字（P12） | 需人工目视；**P12 待确认**：若按钮显示为英文，则 `common-controls-v6` 未生效 |
| 弹窗「取消」/× 的行为（验收 #6/#10） | 同上一行，需人工点击 |
| 长时间隐藏后恢复（P7） | 未做长时测试，短时恢复已通过 |

## 22. 遗留项与后续

| # | 项 | 归属 |
| --- | --- | --- |
| L1 | **设置窗口的 × 会误触发本弹窗** | `on_window_event` 挂在 Builder 上，对**所有窗口**生效。做设置窗口时**必须**按 `window.label() == MAIN_WINDOW` 过滤（代码中已留注释）。**这是下个立项的必改项** |
| L2 | 「不再提示」复选框 | 本轮明确不做（§6 T1） |
| L3 | 方案 D（第二 WebView） | 记为设置窗口立项的架构前置 |
| L4 | 托盘图标状态化（tooltip 显示后端状态） | 未做 |

## 23. 收口待办（均已办结）

- [x] **人工目视确认**弹窗按钮为中文（P12）—— 用户实测确认，P12 解除（见 §21.1）
- [x] 把 `roadmap.md` 原 §5 的「托盘图标」条删掉 —— 已删；并新增 §5「设置窗口」承接后续
- [x] 本文移入 `docs/closed/` —— 已完成（2026-09-11）

---

# ③ 结案总结

## 24. 结果

**目标达成，全部实测通过，已收口。**

| 项 | 结果 |
| --- | --- |
| 交付物 | 4 个新模块 + `main.rs` 接线 + `Cargo.toml` feature，见 §19 |
| 构建 | `cargo check --release` 零 warning；`cargo build --release` 通过 |
| 产物 | `target/release/dshell.exe` 3,475,456 B（较原 3,001,856 B 增长约 15%，主要为托盘与对话框依赖） |
| 单测 | 5 passed / 0 failed |
| 实测 | §21 八项 + §21.1 用户目视七项，**全部通过** |
| 范围 | 严格限于本轮四项（§1.1），非目标（§1.2）一项未碰 |

## 25. 预期与实际的差异（留档）

| # | 预期 | 实际 | 处置 |
| --- | --- | --- | --- |
| 1 | 弹窗两个按钮 + × | **必须三键**：系统对话框的 × 与第二键返回同一个 `Cancel`，两键无法表达"× = 取消"而"按钮 = 缩托盘" | 改用 `YesNoCancelCustom`，Q1 已确认接受 |
| 2 | `ui_text.rs` 写 `pub const` | 改为**函数** | 用户指出要留给 i18n；函数形态使将来换查表时调用点不动（§18.1） |
| 3 | 托盘图标 `unwrap_or_default()` | **`tauri::image::Image` 没有 `Default`**，编译失败 | 改为构造器链 + `match` 分支赋值（§17 P11） |
| 4 | 弹窗结果匹配 `Yes`/`No` | 实际返回 `Custom(<按钮文字>)` | 主路径按 `Custom`，系统枚举作兜底（§17 P9） |
| 5 | 图标资源需新增 | **无需**：`default_window_icon()` 直接复用 exe 图标 | 零资源文件新增 |

## 26. 遗留项

| # | 项 | 归属 | 优先级 |
| --- | --- | --- | --- |
| **L1** | **设置窗口的 × 会误触发本弹窗**（`on_window_event` 挂在 Builder 上，对所有窗口生效） | 设置窗口立项 | **高**（届时必改，代码已留注释） |
| L2 | 「不再提示」复选框 | 设置窗口立项 | 中（§6 T1 的取舍由此消解） |
| L3 | 方案 D（第二 WebView）作为设置窗口的架构前置，需 `unstable` feature | 设置窗口立项 | 高（决策点） |
| L4 | 托盘图标状态化（tooltip 显示后端状态） | 未立项 | 低 |
| L5 | 长时间隐藏后恢复的验证（P7） | 日常观察 | 低（短时已通过） |

## 27. 结论

本项**达成了立项时的全部目标**，且过程中把三处"看起来能省事、实则会错"的地方挡在了实现阶段：

1. **`hide()` 而非 `close()`** —— 实测确认（§21 #4），后端在隐藏期间存活。
2. **× 必须归入取消** —— 单元测试锁死（`window_x_is_cancel_not_hide`）。
3. **两键表达不了三态** —— 在设计阶段就发现（§14 R6），避免了"点 × 变成缩托盘"这个隐蔽错误。

代码结构按"改什么就打开哪个文件"划分，`hide()` 全项目单点调用，托盘与生命周期解耦。
唯一需要下游注意的是 **L1**：做设置窗口时必须给 `on_window_event` 加 label 过滤。

---

## 附注（收口后）

- **归档动作**（2026-09-11）：
  - 本文由 `docs/plan/03-tray-and-close-behavior.md` 移入 `docs/closed/`，标题改为「三轮合一」并补收口状态说明；正文三轮内容未改写。
  - `roadmap.md` 原 §5 的「托盘图标」条已删除（按 `../AGENTS.md`「收口时把对应条目删掉」）。
  - `roadmap.md` 新增 §5「设置窗口」，指向本文的 L1 与本项暴露的架构决策点。
- **与原文的一处出入**：§8 标题写作"关键契约（不含完整实现）"，实际第二轮（§13–§18）给出了接近成品的代码；
  第三轮又按实测结果回修了两处（托盘图标 `match` 分支、弹窗结果 `Custom` 匹配），
  §15 中的片段已同步为**实际交付的形态**。
- **验收清单的完整度**：§9 的 12 条中，#1–#9、#11 有直接证据；#10（反复点击不泄漏）与 #12（外链回归）
  未单独复核——#12 所属的 `on_navigation` 与本轮改动无交集，#10 依赖人工反复操作。
- **一个未被本轮覆盖的已知行为**：用户点 × 后若选择「取消」，弹窗关闭、窗口保持可见，
  这是预期行为；但**没有任何"下次不再问"的机制**，频繁开关窗口会每次都弹（§6 T1、L2）。


