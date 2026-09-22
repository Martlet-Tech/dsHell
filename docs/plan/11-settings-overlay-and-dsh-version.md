# 11 · 设置页（覆盖层）与 dsh 版本管理

> **立项日期**：2026-09-21
> **状态**：**在办**（已实施，主链路 T7/T8 实测通过；未验收项见 ③ 遗留项）
> **对应 roadmap**：原 §3（版本更新）+ §4（设置窗口）**合并为一项**
> **承接**：`09-dsh-update.md`（已删除，有效结论并入本文）
> **前置**：[closed/07 单实例](../closed/07-single-instance.md)、[closed/08 重启 dsh](../closed/08-restart-dsh.md)、[closed/10 陈旧 auth cookie](../closed/10-stale-auth-cookie.md)（均已收口）
> **红线**：不碰 dsh 源码

---

## ① 方案

### 要解决什么

用户不会自己用 npm 更新 dsh（现状是让智能体代劳）。要做的是：在 DShell 里**看见**本机装的是哪个版本、
**选**一个目标版本、**一键切换**，且切换完自动生效——不用关掉 DShell 再打开。

承载它的界面就是 DShell 的设置页。用户明确要求：**设置页与 dsh 的 web 共享主窗口**（不是独立窗口），
多标签设计，标签共享同一组底部「取消 / 确定」按钮。本轮只放两个标签：**dsh 版本管理** 与 **关于**。

### 与 09 的关系：落点变了，地基全继承

09 把落点定在"托盘加两项（检查更新 / 更新 dsh）"。现在落点改为**设置页**——因为用户要的是一个能继续长
（系统 / 网络标签）的设置容器，版本选择器放进容器里比挂在托盘上更合理，托盘只留一个「设置」入口。

09 已实测/已实施的部分**全部继承**，不重做：

| 09 的产出 | 状态 | 在本项中的位置 |
| --- | --- | --- |
| `stop_dsh` / `start_dsh` / `show_splash` 三原语拆分 | ✅ 已实施并实测（连续 5 次重启回归） | 切换动作的三段 |
| 覆盖层机制可行（T11/T12：计数器递增证明未重载；对话进行中开关覆盖层输出不中断） | ✅ 已实测 | 设置页的承载形态 |
| 版本检查的判定（registry 感知、全量 dist-tags、预发布 semver 比较、来源校验） | 📄 方案 | `update.rs` |
| 降级警告 / 需显式确认 | 📄 方案 | 版本管理页 |
| `semver` crate 已确认在 `Cargo.lock`（1.0.28，传递依赖） | 📄 已核实 | 升为直接依赖，零新增下载 |
| 取消语义（更新中途取消**不**自动起回旧后端） | 📄 已定 | 失败出口的「重新启动 dsh」 |

**不继承**：托盘两项（落点被设置页取代）。是否保留一个托盘直达入口见「决策点」。

### 形态：覆盖层，不是新窗口，也不是导航

| 机制 | 当前文档 | dsh 页面状态 | 本轮用不用 |
| --- | --- | --- | --- |
| **注入覆盖层** | **不变** | **保全** | ✅ 用这个 |
| 独立窗口（原 roadmap §4 设想） | 不变 | 保全 | ❌ 用户明确要求共享主窗口 |
| 导航到另一个本地页 | 换掉 | **全部丢失** | ❌ 仅用于重启/切换（那时 dsh 反正要被停） |

顺带一个收益：**roadmap §4 那条"新增窗口后 × 会误触发关闭确认"的警告，在本轮不成立**——
没有第二个窗口，`on_window_event` 不用按 `window.label()` 过滤。（将来真做独立设置窗口时仍需处理。）

原 [`260901-托盘,系统设置,退出行为.md`](./260901-托盘,系统设置,退出行为.md) 里最优先的那个决策点
（**设置窗口：独立窗口 vs DSH 内页面**）**至此拍板：DSH 内页面（覆盖层）**。那份文档的分栏设想
（左窄条导航 + 右详情）被本轮的**顶部标签**取代（见决策点 D5）；系统 / 网络两个大类的内容不动，
仍留在那份文档里等待后续立项。

**覆盖层 = 注入一个全屏 iframe，而不是往 dsh 文档里插裸 DOM。** 09 记录的裸 DOM 代价（样式冲突、
dsh 的全局快捷键仍会收到事件、需要 `stopPropagation`）在 iframe 里自然消失：iframe 内部是独立文档，
我们的 CSS 与它的 CSS 互相看不见，键盘事件也不会跨 iframe 边界冒泡。代价是 iframe **不能自己关闭自己**
（跨域父文档不可触碰），这件事由宿主 shim 代劳——它本来就在我们手里。

**已核实的放行条件**（2026-09-21 实测，`dsh web --port 0`，端口 53546）：

| 检查 | 结果 |
| --- | --- |
| 响应头有没有 `Content-Security-Policy` | ❌ **没有**（只有 `content-type` / `vary` / `date` / `connection` / `Transfer-Encoding`） |
| 响应头有没有 `X-Frame-Options` | ❌ **没有** |
| HTML 里有没有 `<meta http-equiv="Content-Security-Policy">` | ❌ **没有** |

⇒ dsh 页面**没有对我们的 iframe 设任何限制**。这是本方案能成立的前提，已实测确认。（实测用的
临时后端已收掉，端口已释放，无孤儿进程。）

### 两个宿主阶段，一条代码路径

窗口里只有两种文档：启动页（`tauri.localhost`，我们自己的）和 dsh 页（`127.0.0.1:<port>`）。
两种都要能弹设置，所以注入**统一走同一条路**——不再区分"启动页直接 import 本地模块"和
"dsh 页注入字符串"两套实现（两套实现就会有两套 bug，08 的 `about:blank` 就是这么来的）。

```
Rust ──eval(注入 shim)──▶ 当前文档的 DOM 里出现一个全屏 iframe
                            shim 拿到 iframe.contentWindow，建立 postMessage 双向通道
Rust ──eval/postMessage──▶ 面板 JS（tauri.localhost/settings.html）
面板 JS ──sendBeacon/fetch(loopback, 路径带 nonce)──▶ Rust
```

面板本体是 `ui/settings.html`，由 Tauri 从 `frontendDist` 提供，**普通本地页面**——可以直接在浏览器里
打开调样式，不需要 dsh 在跑。

**面板不需要 Tauri IPC**（页面在 dsh 文档的 iframe 里，`__TAURI__` 在不在、ACL 放不放行都是未知数，
不该赌）：

| 方向 | 通道 | 为什么行得通 |
| --- | --- | --- |
| Rust → 面板 | `eval` → shim → `postMessage` | `eval` 是本项目既有惯例（`splash_status` / `splash_fail`）；postMessage 跨源安全 |
| 面板 → Rust | `sendBeacon`（或 `fetch`）到 `127.0.0.1:<bridge>`，路径含每次启动的随机 nonce | 不需要 CORS 预检（beacon 是简单请求）、不需要自定义头、不需要 IPC |

⇒ **`capabilities/default.json` 不用动**，不需要给 dsh 的 origin 开 IPC 权限（那等于把 `app_quit` /
`install_missing` 这类敏感命令暴露给 dsh 页面里的一切内容）。这是选择 beacon 而不是 IPC 的**主要**理由。

### ⚠️ 开工前必须实测的两件事（并且有一个文档矛盾要更正）

`picker.rs` 的模块注释里写着（R17–R19 的结论）：

> **实测证明 `initialization_script` 与 `eval` 两条注入通道都是死的**（脚本第一行的 `sendBeacon`
> 一次都没到）——页面里从来没有我们的代码。

而 09 的 T11 实测（2026-09-20）是相反的：`eval` 注入的覆盖层**真的出现了**，且覆盖层里的计数器
`1 → 2 → 3` 递增（存在 JS 全局变量里，页面一重载就会归零）。

两者不能同时为真。**倾向于 09 是对的，R18 的否定不成立**，理由：启动页上 `__TAURI__` 明明存在
（整个应用依赖它，否则启动页一行都跑不起来），所以"注入通道全死"至少对启动页不成立。R18 的判据是
"`sendBeacon` 回传一次都没到"——它测的其实是**回传**方向，却拿来否定**注入**方向。这与 05 自己的
教训 M13 是同一类错误的反面：*不能用一条未经验证的通道，去否定另一条通道。*

所以第 1 步不是写功能，是**探针 + 日志**，把三件事一次测完（这也是本项目的既定规矩：读运行时状态的
地方，第一次落地就把结果记进日志）：

| # | 待测 | 判据 | 失败时的退路 |
| --- | --- | --- | --- |
| P1 | `eval` 注入在 **dsh 页**生效 | Rust 日志出现 `overlay: injected`，日志里**同时**出现由页面回报的 `ready` | 退化为"裸 DOM 注入"（09 已证可行），样式隔离靠命名前缀 + `all: initial` |
| P2 | dsh 文档里的 iframe 能**加载** `settings.html` | 面板回报 `ready` | 同上 |
| P3 | 面板 → Rust 的 **loopback 回传**到达 | 日志出现 `settings: <action> from <origin>` | 换 `fetch` + 显式预检处理（端点已按此预留） |
| P4 | Rust → 面板的 **postMessage** 到达 | 面板把收到的 catalog 摘要回报一次 | 面板改为轮询端点拉数据（多一个 GET 路由） |

**探针本身要有可观测性**（这是 09 记的教训：`eval` 是单向的，Rust 侧拿不到返回值，所以日志区分不出
"真的插入了"与"发现已存在就跳过"）。因此探针必须**双向都留痕**，不能只看 `eval` 返回的 `Ok(())`。

测完把结论写进 `docs/design.md`（只增不删），并**顺手改掉 `picker.rs` 里那段已过时的断言**——
它是 R17–R19 的现场记录，结论现在被推翻了，留着会误导下一个读它的人。

### 版本数据

数据源与判断，全部沿用 09 已定的口径：

| 要点 | 做法 | 为什么 |
| --- | --- | --- |
| registry | **用用户自己配置的那个**（`npm view` 不传 `--registry`） | 检查用的 registry 与装用的不是同一个 ⇒ "说有大版本、装完却不是"。**不覆盖用户 `.npmrc`** |
| 取哪些版本 | `npm view @deepseek-ai/dsh versions --json`（全量）+ `npm view @deepseek-ai/dsh dist-tags --json` | 只看 `latest` 会判错方向：本机是预发布 `0.1.5-rc.1` |
| 排序 | **semver 降序**（数值段按数值，预发布按 semver 规则），最新在上 | 字符串比较会把 `rc.2` / `rc.10` 判错 |
| 当前版本 | `npm ls -g @deepseek-ai/dsh --depth=0 --json`（即 `doctor::npm_global_version`） | 它同时回答"装的是哪个版本"和"确实由 npm 全局装着" |
| 来源校验 | 若 `cfg.dsh` 已显式指定，其父目录必须 == `npm prefix -g`，否则**拒绝执行** | 否则 `npm i -g` 会装出**第二份** DShell 根本不启动的 dsh，比不更新更糟 |
| 降级 | `target < current` ⇒ **警告 + 显式确认**，默认只允许往前 | dsh 的会话格式是单向的（其 AGENTS.md：predecessors imply neither fallback nor downgrade support），静默降级可能让用户读不到新版写的会话 |

**实测数据（2026-09-21，本机 `npm config get registry` = `registry.npmjs.org`）**：

| 项 | 值 |
| --- | --- |
| 本机 dsh | `0.1.5-rc.1`（`where dsh` → `%APPDATA%\npm\dsh.cmd`） |
| 已发布版本数 | **22**（全部是预发布，**没有一个稳定版**） |
| `latest` / `next` | `0.1.5-rc.2` |
| `alpha` | `0.1.6-alpha.2` |

⚠️ **registry 变了**：09 记录的是 `registry.npmmirror.com`，今天是 `registry.npmjs.org`。这恰好是"不许
硬编码 registry"的活证据——把镜像地址写进代码，下次用户换镜像就又错了。

22 条全量数据很小（可以整包一次取回），所以"懒加载 25 条"只是**渲染**策略：列表自带滚动容器，
滚到底部的哨兵再渲染下一批 25 条。**诚实说明**：按当前 22 个版本，第一页就是全部——分页机制现在
跑不出第二条，只有版本数超过 25 之后才真正生效。

### 切换动作

顺序就是用户描述的那个顺序，且**每一步都在挡一个具体的失败**（沿用 08/09 的分析）：

```
切换(target) =
  ① 来源校验（不合格 ⇒ 拒绝，什么都不做）
  ② 关闭设置覆盖层
  ③ show_splash            ← 导航回启动页，否则进度与失败卡片没有监听者（08 的 about:blank 类缺陷）
  ④ stop_dsh               ← 等它真退出（kill_tree + wait_process_exit）
  ⑤ installer::run(Dsh, target)   ← npm i -g @deepseek-ai/dsh@<target>
  ⑥ start_dsh              ← 体检 + handoff
```

- **④ 不能省**：`npm i -g` 要覆盖全局安装目录里的文件，活着的 dsh 进程正锁着它们（native addon）。
- **闩要跨 ④+⑤ 全程**：`restart_guard` 现在的职责其实不是"重启守卫"而是"**抑制启动页自发的体检**"
  （启动页加载完就 `invoke('doctor_run')`）。若在装的过程中放行，会 handoff 出**第二个** dsh 后端——
  而且是在文件被覆盖的中途起来的。只在 ⑥ 之前放开。
- **进度条零新增 UI**：`installer::run` 发的就是启动页在听的 `install://progress` / `install://done`。
- **失败/取消的出口**：dsh 已经被停了，所以**不自动**起回旧后端（09 已定的"语义更诚实"方案）——
  停在启动页的失败卡片 + 一个明确的「重新启动 dsh」按钮，由用户决定何时起回来。

`installer` 需要一处改造：`Kind::Dsh` 的命令现在是写死的 `npm i -g @deepseek-ai/dsh`，要能带版本。

### 关于页

| 内容 | 来源 | 备注 |
| --- | --- | --- |
| 软件名 / 版本号 | `app.package_info().version`（即 `tauri.conf.json` 的 `version`） | 单一来源，不在 JS 里再抄一遍；`Cargo.toml` 的版本由 CI 保证同步 |
| dsh 后端版本 + 路径 | `doctor::npm_global_version` + `cfg.dsh` | 顺手就有，排查时最常用的一行 |
| 开源清单 | 见下 | |

**开源清单有两个做法**（见「决策点」）：

| 方案 | 内容 | 成本 |
| --- | --- | --- |
| **A** 手写直接依赖 | 约 8 条：Tauri / wry / WebView2 / serde / serde_json / semver / tauri-plugin-dialog / rfd，含 license 与仓库链接 | ≈20 行静态数据，零构建步骤 |
| **B** 脚本生成全量 | `Cargo.lock` 共 **433** 个包，其中 **425** 个能在本机 cargo registry 缓存里读到 `license` 字段；脚本产出 `ui/credits.json`，面板里折叠展示（默认收起） | 一个生成脚本 + 一个提交进仓库的产物 |

**倾向 B**："开源清单"字面要求完整，而 B 是可以真做全的（本机 `~/.cargo/registry/src` 已核实可用，
425/433 覆盖）。缺 license 的 8 个单独标"未声明"。DShell 仓库自身**没有 LICENSE 文件**，所以关于页
不声称 DShell 自己的许可证——只列第三方组件。

### 决策点（已拍板）

| # | 问题 | 结论 |
| --- | --- | --- |
| D1 | 底部「取消 / 确定」的语义 | ✅ **两个按钮都关闭设置页并把 dsh 页面切回前台**；「取消」= 放弃这次的选择（清掉），「确定」= 保留选择（下次打开仍是它）。本轮两个标签都没有别的可落盘字段 —— 不为它发明假功能 |
| D2 | 托盘要不要一个直达「更新 dsh」 | ✅ **不要**，托盘只加一项「设置」 |
| D3 | 开源清单 A 还是 B | ✅ **暂时不做，占个位置**（关于页放一个说明块，等验证通过再补） |
| D4 | 本轮 DShell 版本号 | ✅ 升到 **0.1.5**（`Cargo.toml` 与 `tauri.conf.json` 一致，已随 `67d381f` 提交） |
| D5 | 标签栏位置 | ✅ 顶部横排（取代原 260901 文档的左右分栏设想） |


### 非目标（本轮不做）

- 设置页的**系统 / 网络**标签（自启、关闭行为、代理）——容器留好接口，内容不写
- 自动 / 静默更新；后台轮询检查（检查只在打开面板时发生，且有一个显式「刷新」）
- **DShell 自身（exe）的更新**——另一件事
- 端口设置（`--port 0` 保持不变；见项目记忆里的 431 决策，那是已收口的既有决定）
- 独立设置窗口（那才会触发 `on_window_event` 的窗口过滤问题）
- i18n（`ui_text.rs` 已经留好了入口，本轮不动语言维度）

### 验收矩阵

| # | 场景 | 期望 |
| --- | --- | --- |
| T1 | 托盘「设置」（当前在 dsh 页，对话正在输出） | 面板出现；**对话继续输出**（09 的 T12 已证），关闭后 dsh 页完好如初、无需重新握手 |
| T2 | 托盘「设置」（当前在启动页） | 同样出现面板（统一路径，不是两套实现） |
| T3 | 重复点「设置」 | 不叠第二个面板；已开则前置/聚焦 |
| T4 | 版本页首屏 | 22 条，最新在上，当前版本 `0.1.5-rc.1` 高亮，`latest/next/alpha` 三个徽标挂在对应行 |
| T5 | 滚到底 | 渲染下一批 25 条（当前 22 条 ⇒ 无第二批，机制不报错） |
| T6 | 「刷新」 | 重查 registry，不装任何东西 |
| T7 | 选 `0.1.5-rc.2` → 确认切换 | 面板关闭 → 启动页 → "正在更新" + 实时进度 → 体检全绿 → 进 dsh |
| T8 | 切换后确认版本 | `dsh --version` = `0.1.5-rc.2`，且是**新进程**（不是旧页面缓存） |
| T9 | 选**更低**版本 | 出现降级警告，需显式确认；确认后能装回去（保证测试可重复） |
| T10 | 选**同一个**版本 | 允许（等同重装=修复入口），流程照常成功 |
| T11 | 更新中途「取消」 | 停在启动页失败卡片 + 「重新启动 dsh」；**不自动**起回 |
| T12 | 来源伪装（`cfg.dsh` 指向非 npm 那份） | **拒绝**并说明原因，不装出第二份 |
| T13 | npm 不可达 / registry 超时 | 面板显示可读的失败原因，**DShell 一切照旧**（检查失败不带任何副作用） |
| T14 | 面板打开时锁屏 / 切到别的窗口再回来 | 无残留、无重影（覆盖层是 DOM 元素，不涉及 WebView2 遮挡判定） |
| T15 | 面板打开时点 dsh 页面里的外链 | 仍走系统浏览器（`on_navigation` 不受影响） |

回归（本轮动了 `installer` 与 `main.rs`，必须一起跑）：08 的重启回归、10 的 431 无回归（每次启动
日志里 `purged N` 仍在）、单实例仍生效。

### 实施顺序与估算

| 步 | 内容 | 估算 |
| --- | --- | --- |
| 1 | **桥接探针**（P1–P4 一次测完 + 日志 + 更正 `picker.rs` 的过时断言） | 0.5 天 |
| 2 | `settings.rs`：注入 shim、覆盖层状态、loopback 端点（nonce + Origin 校验） | 0.5 天 |
| 3 | 面板骨架：`settings.html` + 标签栏 + 共享底栏 + **关于**标签（先放 A 版清单） | 0.5–1 天 |
| 4 | `update.rs`：catalog 采集 + semver 排序 + 来源校验；版本标签（列表 / 懒加载 / 高亮 / 徽标 / 降级警告） | 1 天 |
| 5 | 切换动作：`installer` 带版本 + 编排 + 闩 + 失败出口；托盘「设置」项 + `ui_text` 文案 | 1 天 |
| 6 | 关于页清单 B（脚本 + 产物）、文档收口（design.md / roadmap / 本文件 ③） | 0.5 天 |

合计约 **4–4.5 天**。第 1 步是硬前置：它的结论决定第 2 步是"注入 iframe"还是"注入裸 DOM"。

---

## ② 关键代码

### 桥接协议（三个契约，都不许改名）

```js
// ① 宿主 shim（Rust 侧字符串常量，eval 注入到当前文档）
//    幂等：文档里已有 shim 就复用它（重复点托盘「设置」不该叠第二个 iframe）。
window.__dshellSettings = {
  open(),          // 建 iframe（position:fixed; inset:0; z-index:2147483000）
  hide(),          // 移除 iframe
  push(b64Json),   // 转发给面板：iframe.contentWindow.postMessage({ __dshell: 1, payload })
  status(b64Text), // 面板没起来时在页面上留一行可见线索（看门狗用）
};
// shim 还负责 Esc 兜底：焦点不在 iframe 里时（事件落在 dsh 文档上），Esc 回传 `cancel`。
// 面板自己那次加载不会走到这里（按键不跨 iframe 冒泡），所以不会重复触发。
```

```js
// ② 面板侧（ui/js/settings.js）
//    起来先回报 ready，壳层收到才开始推数据（避免推给还没监听的页面——08 的教训）。
navigator.sendBeacon(BASE + "ready");
// 其余动作都是 beacon；参数走 query（beacon 不能带自定义头）。
// BASE = http://127.0.0.1:<port>/_dshell/<nonce>/settings/
//   ready                    面板起来了
//   refresh                  重查 registry（只读）
//   cancel                   取消：丢弃选择 → 壳层关面板 + 把 dsh 切回前台
//   apply?version=<v|空>      确定：保留选择 → 同上（空 = 没选，等价于清掉）
//   switch?version=<v>        执行切换（见下）
```

```rust
// ③ Rust 侧数据结构（update.rs）
pub struct VItem { pub version: String, pub tags: Vec<String>, pub current: bool, pub older: bool }
pub struct Catalog {
    pub current: Option<String>,      // 现查（本地命令）；None = 问不出来
    pub latest: Option<String>,       // 全量版本里 semver 最大的（含预发布）
    pub registry: String,             // 实测生效的 registry，显示出来（排障用，且证明不硬编码）
    pub registry_age_secs: Option<u64>, // 面板显示"数据 N 分钟前"
    pub registry_stale: bool,           // 已过期：调用方随后会再推一份新的
    pub npm_global: bool,             // 来源校验：false ⇒ 面板禁用「切换」
    pub source_note: Option<String>,  // 不过时给用户看的一句话
    pub npm_prefix: Option<String>,
    pub versions: Vec<VItem>,         // 已按 semver 降序
}
pub fn catalog(force: bool) -> Result<Catalog, String>;   // registry 走缓存，本地部分现查
pub fn refresh_registry() -> Result<(), String>;          // 无视 TTL 重查并落盘
```

**registry 那半边有 2 小时磁盘缓存**（`%USERPROFILE%\.dshell\dsh-versions.json`）。
本地部分（`current` / 来源校验）每次都现查：都是本地命令、秒级，而且**必须新鲜** ——
`current` 在一次切换之后立刻就变了，用缓存会让面板显示错的"当前版本"。

推送是两段式的：缓存过期时**先把旧数据推给面板**（面板瞬开、带时间戳），随后后台重查再推一份覆盖。
所以一次过期不会让用户对着"正在查询"发呆。

### 切换编排（`main.rs`，与 `do_restart_dsh` 并列）

```rust
fn do_switch_dsh(app: &AppHandle, target: &str)   // 下面就是实际实现
pub(crate) fn start_switch_dsh(app: &AppHandle, target: String)   // 带闩 + 线程 + catch_unwind
```

```rust
fn do_switch_dsh(app: &AppHandle, target: &str) {
    settings::hide(app);                                                   // ① 关面板
    guard.store(true);                                                     //    跨 ③+④ 全程
    if !show_splash(app, &window) { /* 放弃：进度无监听者 */ return }
    lifecycle::restore_main_window(app);
    window.eval("window.__dshell.updating(true)");                          // 收起步进时间线
    if !stop_dsh(app) { guard.store(false); /* 放弃，避免两个后端抢会话锁 */ return }
    let outcome = installer::run(app, Kind::Dsh, &state, Some(target));      // ③
    guard.store(false);                                                    // ⑤ 前放开
    if !outcome.ok { splash_fail(..); window.eval("__dshell.updateFailed()"); return }
    settings::clear_selection();
    window.eval("window.__dshell.updating(false)");
    start_dsh(app, &window);                                               // ⑤
}
```

`start_switch_dsh` 与「重启 dsh 后端」**共用 `restarting` 闩**：两者都是"停旧 → 起新"的链，
同时跑会互相杀，也会撞上会话锁。

### `installer` 的改造点（唯一一处对外行为变化）

```rust
fn command_for(kind: Kind, target: Option<&str>, resolve_before: Option<&str>) -> (String, Vec<String>);
//   Kind::Node                  → winget install --id OpenJS.NodeJS.LTS …
//   Kind::Dsh + None            → npm i -g --no-audit --no-fund --loglevel=http @deepseek-ai/dsh
//   Kind::Dsh + Some("x.y")     → 同上，末尾是 @deepseek-ai/dsh@x.y
//   Kind::Dsh + resolve_before  → 再插一个 --before=<时刻>（装"不是最新那一版"时必须，见 ③）
//   （`--loglevel=http` 是为了让面板看得见动静，理由见 ③）
pub fn run(
    app: &AppHandle, kind: Kind, state: &AppState,
    target: Option<&str>, resolve_before: Option<&str>,
) -> Outcome;
// 既有调用点（install_missing）传 None, None ⇒ 行为不变，只是输出更啰嗦、少两次网络往返。
```

### 发布时间窗（`update.rs`）

```rust
/// 降级/重装旧版时要给 npm 的 `--before`；None = 目标就是最新那版（不需要）。
pub fn resolve_before(target: &str) -> Option<String>;
//   内部：version_times()（`npm view <pkg> time --json`，合并进同一个 registry 缓存）
//        → 按发布时间排序取"紧跟在 target 之后发布的那一版"
//        → midpoint(t0, t1)
//   取中点、不取 target 自己的时刻，理由与实测见 ③（错峰发布）。
```

- `midpoint` 交给 **node** 算（`node -e "new Date((Date.parse(t0)+Date.parse(t1))/2)"`）：
  npm 本身是 node，两边对日期字符串的理解天然一致；为一个减法引 `time`/`chrono` 不值当，
  手写历法换算更是典型坑。
- 窗口计算与 `stop_dsh` **并行**（`mpsc`），首次那一次查询（约 1-2s）被停 dsh 的时间盖住。

### 托盘与文案

```rust
const MENU_ID_SETTINGS: &str = "tray-settings";
enum TrayAction { RestoreWindow, OpenInBrowser, RestartDsh, OpenSettings, Quit }
// ui_text.rs：menu_settings() → "设置"
```

### 面板文件（新增，尽量少）

| 文件 | 职责 |
| --- | --- |
| `ui/settings.html` | 骨架：标签栏 + 两个标签页 + 共享底栏。**保留内联兜底**（同启动页的理由：面板加载不出来时得有处说话） |
| `ui/settings.css` | 面板样式独立成文件（它要活在 dsh 页面的 iframe 里，不能共用启动页的 `app.css`） |
| `ui/js/settings.js` | 标签注册表（`{id, title, render, onOk, onCancel}`）+ 两个标签 + 底栏状态机 + 与宿主/桥接通信 |
| `ui/credits.json` | 开源清单产物（D3 选 B 时才有），由脚本生成后**提交进仓库** |
| `scripts/gen-credits.py` | 从 `Cargo.lock` + 本机 cargo registry 抽 `name/version/license`（只读，幂等） |
### 涉及文件（改动清单）

| 文件 | 改动 |
| --- | --- |
| `src-tauri/src/settings.rs` | **新增**：shim 常量、覆盖层状态（`AtomicBool`）、loopback 监听 + 路由 + nonce/Origin 校验 |
| `src-tauri/src/update.rs` | **新增**：catalog 采集、semver 排序、来源校验 |
| `src-tauri/src/installer.rs` | `command_for` / `run` 增加 `target` |
| `src-tauri/src/tray.rs` | 「设置」菜单项 + 一个 `TrayAction` 变体 |
| `src-tauri/src/main.rs` | `mod settings/update`、`do_switch_dsh`、新 `#[tauri::command]`（若面板走 IPC 才需要，当前方案不需要）、托盘动作分发 |
| `src-tauri/src/ui_text.rs` | 设置页与托盘文案 |
| `src-tauri/Cargo.toml` | `semver = "1"`（已在 `Cargo.lock`，无新增下载）；版本号按 D4 |
| `src-tauri/tauri.conf.json` | 版本号按 D4 |
| `docs/design.md` | 注入通道的**更正记录** + 探针实测数据（只增） |
| `docs/roadmap.md` | §3 §4 合并为本项；收口时删条目 |
| `docs/plan/09-dsh-update.md` | **删除**（有效结论并入本文，随本次提交落地） |

**不动**：`capabilities/default.json`（本方案不需要给 dsh origin 开 IPC）、`picker.rs` 的端点行为
（只改那段过时注释）、`proc.rs`、`lifecycle.rs`。

---

## ③ 结案总结

**未收口。** 已实测的部分如下，还差遗留项 3 里那几项的人工验收。

### 实测（2026-09-21，产物 `target\20260921-114105`）

| # | 场景 | 结果 | 证据 |
| --- | --- | --- | --- |
| 1 | 回传端点的边界 | ✅ | 错 nonce → **404**；错 / 无 `Origin` → **403**；GET → **405**；未知动作 → **404**（curl 实测） |
| 2 | 台账查询（真跑 npm） | ✅ | `catalog ok (current=0.1.5-rc.1, latest=0.1.6-alpha.2, 22 个版本, npm_global=true)` |
| 3 | **P1 注入 dsh 文档** | ✅ **真实使用中通过** | 日志 `settings: shim injected (panel=http://tauri.localhost/settings.html)` |
| 4 | **P2 iframe 加载面板 + P4 postMessage** | ✅ **真实使用中通过** | 面板里的「切换」按钮**只在 `S.catalog` 非空时可点** —— 用户真的点成了，等于证明 Rust →`eval`→ shim → `postMessage` → 面板这条链通了 |
| 5 | 切换动作（T7） | ✅ | `switch requested -> 0.1.5-rc.2` → `splash: navigated back` → `stop dsh: dsh (pid 16856) exited` → `install dsh: ok=true code=Some(0)` → `doctor: allOk=true` → `spawned dsh web (pid 11652, generation 3)` → `window navigated to dsh` |
| 6 | 切换真的生效（T8） | ✅ | `dsh --version` = **0.1.5-rc.2**（新进程，不是页面缓存） |
| 7 | 闩的持有范围 | ✅ | 日志里 `doctor: suppressed (a restart is in progress)` 出现的位置**正好在 stop 之后、装之前** |
| 8 | 代数递增 | ✅ | 1 → 3（stop 时 +1、start 时 +1，与设计一致） |
| 9 | 无回归 | ✅ | `FATAL` / `panicked` / `431` / `SessionAlreadyOwned` / `Failed to load plugins` **各 0 次**；每次启动的 `auth cookie: purged N` 仍在 |
| 10 | 无孤儿 | ✅ | 切换后 dsh 是新进程；「完全退出」后 `node.exe` 与监听端口都归零 |

覆盖层在**两种宿主**上都跑通了：切换**前**（dsh 页）与切换**后**（新 dsh 页）各有一轮
`shim injected` + `panel reported ready`。

### 预期与实际的差异

| 项 | 说明 |
| --- | --- |
| 头一轮测试用的不是最新产物 | 用户跑的是 `20260921-114105`，比 `114446` 少两行日志（`bridge path` / `catalog ok`），所以那几轮看不到台账行。**两版行为完全相同、只差日志**（已用二进制字符串核对：`catalog ok` 只在 114446 里）。 |
| 「确定」在未选中时 | 日志出现 `applied (selection=None)`：没选任何版本时点确定 = 清空选择并关闭。符合 D1（确定=保留选择），不是缺陷。 |

### 第二轮真实使用暴露的问题与修法（2026-09-21，产物 `20260921-123041`）

用户在真机上从 `0.1.5-rc.2` 切到 `0.1.6-alpha.2`（约 2 分钟），提了三点不适：

| # | 现象 | 定性 | 修法 |
| --- | --- | --- | --- |
| 1 | 开头一分多钟只有不确定进度条在转，日志区**空白** | **npm 的行为**，不是卡死：真装时默认只吐 warnings + 最后一行 `added N packages … in 2m` 汇总。实测 dry-run：notice 级 stderr **0 行**；`--loglevel=http` 有 **564 行**带耗时的 http 流水 | ① dsh 安装命令加 `--no-audit --no-fund --loglevel=http`（省两次与安装无关的网络往返 + 让 npm 真的吐流水）；② `installer` 起跑就先写两行日志：要跑的命令 + "npm 在解析依赖时会安静一会儿，通常 1-3 分钟"；③ `guess_phase` 认 `npm http` → "正在解析依赖 / 正在下载" |
| 2 | "更新没跑完，那边开始启动了" | **观感 bug（真 bug）**：切换路径**没发 `install://done`**，于是面板标题永远停在"正在更新"、进度条一直转、取消按钮一直可点。日志证明顺序其实是对的（`install dsh: ok=true` 在 `spawned dsh web` **之前**） | 抽出 `emit_install_done()`：补齐队列与切换路径共用同一个事件形状；启动页在 `install://done` 里调 `installPanelCancel(false)`，停掉不确定动画、禁用取消 |
| 3 | 更新期间「一键安装」还挂在那儿（**可点**） | **第二个门**：`install_missing` 不检查 `restart_guard`，点下去会跑 doctor → handoff → 在**文件正被 npm 覆盖的中途**起第二个 dsh 后端 | ① `updating(true)` 把「一键安装」「跳过检查」都收走；② `install_missing` 自己也挡一道（与 `doctor_run` 同法） |

顺带在 `parse_percent` 里加了一条：**带 `://` 的行不参与百分比解析** —— http 流水里的 `%2B`
之类转义会被误判成百分比，让进度条乱跳。

日志上限也从 200 行下调到 120 行（面板只显示尾部 60 行，而 http 级会刷几百行，队列越大
序列化进事件的 JSON 越大）。

### 缓存实测（2026-09-21，产物 `target\20260921-123041`）


| 状态 | npm 调用 | 日志证据 |
| --- | --- | --- |
| 冷启动（无缓存） | **5 条**（`view versions` / `view dist-tags` / `config get` / `ls -g` / `prefix -g`）→ 落盘 639 字节 | `catalog ok (… age=2s, stale=false)` |
| 热缓存（TTL 内） | **2 条本地命令**（`ls -g` / `prefix -g`），**零联网** | `catalog ok (… age=6s, stale=false)` |
| 过期（把 `fetched_at_ms` 回拨 3 小时） | 先只跑本地命令推旧数据 → 再 `view ×2 + config` 重查 → 重推 | `catalog ok (… age=10812s, stale=true)` → `cached catalog is stale - re-querying in the background` → `catalog ok (… age=2s, stale=false)` |
| 切换后的 `current` | 每次现查，不吃缓存 | 切换完成后立刻显示 `0.1.5-rc.2`（不是缓存的 rc.1） |

手动「刷新」= 无视 TTL 强制重查；失败时推 `refresh_error` 给面板，**不清空已有列表**。

> 面板上会显示 `· 数据 N 分钟前`，过期时补 `（正在刷新…）` —— 用户有权知道眼前这份列表有多旧。

### 第三轮真实使用：降级把 dsh 装坏了（2026-09-21，产物 `20260921-125429`）

用户从 `0.1.6-alpha.2` **降级到 `0.1.6-alpha.1`**，`npm i -g` 报 `ok=true code=0`，但 dsh 一启动就退出：

```
SyntaxError: The requested module '@deepseek-ai/dsh-app-boot' does not provide an export
named 'watchUserPatches'
    at runCli (…/npm/node_modules/@deepseek-ai/dsh/lib/bin.js:145)
```

**根因（实测取证，不是猜）**：

| 事实 | 证据 |
| --- | --- |
| `dsh@0.1.6-alpha.1` 的 core 需要 `dsh-app-boot` 导出 `watchUserPatches` | 它的 `lib/profile-boot-CuwbWsnH.js` 里出现 3 次（`bin.js` → 85 字节的 `DB8eVKBx` → 它） |
| 装上去的 `dsh-app-boot` 是 **0.1.6-alpha.2**，里面**没有**这个符号 | 对安装目录全量 grep：0 命中 |
| `dsh-app-boot@0.1.6-alpha.1` **有**这个符号 | `npm pack` 下载该版本后 grep：2 命中 |
| 为什么配到了 alpha.2 | `dsh` 对同级包用 `^` 范围（如 `@deepseek-ai/dsh-acp-app: ^0.1.6-alpha.1`）→ npm 取范围内最大值 = alpha.2 |

⇒ **`0.1.6-alpha.1` 今天装不出能跑的树**（清空重装也一样，npm 仍会选 alpha.2 的依赖）。
这是**上游的发布问题**（预发布用 `^` 范围 + 新版删了符号），不是 DShell 的 bug ——
但它落在我们的功能路径上，所以必须防。

**产品侧的两处改动**：

| 改动 | 理由 |
| --- | --- |
| 降级警告从"会话格式单向"扩成**两条**，并点名 `0.1.6-alpha.1` 这个实测案例 | 原来的警告让用户以为降级只是"少用几个新功能"，实际可能直接把 dsh 装成起不来 |
| 「装完 dsh 起不来」时，失败卡片出现**「装回 <上一版>」按钮**（`app_switch_back`） | 原来那条路上唯一的出口是「跳过检查，直接启动」——**对"dsh 已经坏了"毫无用处**（必然再失败）。用户的现实困境就是"面板里的版本我都不敢点了" |

`PREVIOUS_DSH`（切换前那一版的版本号）在 `do_switch_dsh` 开头记下（`npm ls -g`，本地命令），
切换成功不再需要它；**刻意不自动回滚** —— 自动回滚会掩盖"新版不能用"这个事实，
与 09 定下的取消语义同因。

> **未做（值得单独立项）**：装完**先验证**再宣布成功；若验证失败则**自动**装回上一版。
> 本轮只做"不自动"的那一半。
>
> ⚠️ **验证不能只跑 `dsh --version`**（我原本打算这么写，实测推翻）：那个坏掉的
> `0.1.6-alpha.1` 现在 `dsh --version` **正常返回**，只有 `dsh web` 会炸 —— 两条命令走
> 的代码路径不同（`--version` 在 commander 那层就返回了，没进 profile-boot）。
> 真正的验证必须走**它自己的启动路径**：要么直接复用 `start_dsh` 的 doctor + handoff
> （现在就是这么发现问题的），要么单独跑一次 `dsh web --port 0` 看它能不能打印地址。

### 第四轮：带发布时间窗的降级（已实现，2026-09-21）

第三轮证明"朴素降级会把 dsh 装坏"。用户提议用 `npm --before` 试 —— **先手工验证机制，再改造**，
两步都做了。

**机制验证（不改代码，直接跑 npm）**：

| 步骤 | 命令 / 检查 | 结果 |
| --- | --- | --- |
| 第一次尝试 | `--before=2026-09-15T04:00Z`（取 alpha.1 自己发布时刻附近） | ❌ `ETARGET`：`dsh-client-ui-sidebar-documentpreview@^0.1.6-alpha.1` 那时还不存在 |
| 原因 | `npm view <pkg> time` 逐个查 | 同一批包**错峰发布**：alpha.1 那波从 `03:09Z` 排到 **`04:17Z`**（比 dsh 自己的 `03:23Z` 还晚）；alpha.2 那波是 09-17 `13:38Z` 起 |
| 第二次尝试 | `--before=2026-09-16T12:00Z`（两个发布波之间） | ✅ `added 1, removed 48, changed 440 in 30s` |
| 树是否一致 | `dsh-app-boot` 的版本；`watchUserPatches` | ✅ **0.1.6-alpha.1**，符号在里面 |
| **能不能跑** | `dsh web --port 0 --no-open` | ✅ 打印 token URL，监听 60622 |
| 页面是否真能服务 | 带 token → 303 + cookie；带 cookie → 200 | ✅ `<title>DeepSeek Harness</title>` |
| 收尾 | 杀进程后查端口与 node | ✅ 归零，无孤儿 |

**因此定下的实现**（本节 ② 的两个片段）：

1. `update::resolve_before(target)`：查发布时间表（`npm view … time`，合并进同一个 registry 缓存），
   取"紧跟在 target 之后发布的那一版"，算出**中点**。目标就是最新版 ⇒ 没有 next ⇒ 返回 `None`（不需要窗口）。
   - **不取 target 自己的时刻**（实测会 ETARGET）；取中点是因为它落在两个发布波之间。
   - 这是个**启发式**（假定同一批发布在同一窗口内完成）。真撞上 `ETARGET` 时，
     `installer` 会把 npm 的原始错误翻译成一句能懂的话（"该版本发布时同批的包还没发全"），
     并保留原始输出，同时失败卡片上仍有「装回 …」。
2. 窗口计算与 `stop_dsh` **并行**（`mpsc`）：首次那次 `view time`（约 1-2s）被停 dsh 的时间盖住，
   用户感觉不到；之后都在 2 小时缓存里。
3. 降级警告补了一句"会用发布时间窗安装"，让用户知道为什么这一步与升级不一样。

**预期日志**（降级到 `0.1.6-alpha.1` 时）：

```
update: window for 0.1.6-alpha.1: 2026-09-15T03:23:13.750Z .. 2026-09-17T13:52:10.201Z (next=0.1.6-alpha.2) -> --before=2026-09-16T08:37:41.975Z
install dsh: resolving as of 2026-09-16T08:37:41.975Z
```
面板的日志区也会显示完整命令（含 `--before=…`）：`display_command` 把它一并打出来。

**未验的部分**：这条链在 DShell 里的端到端表现（点「切换」→ 日志出现上面两行 → 装完能起）
需要人工点一次。手工机制验证已通过，且 DShell 侧的编排就是"同一组命令 + 同一个验证路径"。

### 第五轮：版本列表显示发布日期（2026-09-23）

用户看面板时的原话是"那些『更旧』那个地方，写每个版本的发布日期，这个日期能拉得到么"。
答案是**能，而且早就在拉了** —— 数据不为这个需求新增任何 npm 调用。

| 项 | 第一版（"更旧"） | 现在 |
| --- | --- | --- |
| 右列内容 | 当前版本那行 `当前版本`，其余行在 `v.older` 时写 `更旧` | 当前版本那行仍 `当前版本`，**其余行写发布日期**（本地时区，同年不显示年份；完整时刻在 `title` 里） |
| 数据来源 | — | `npm view @deepseek-ai/dsh time --json`，**降级算 `--before` 时间窗时本来就在抓**，只是没发给面板 |
| 确认框 | 只写"当前 → 目标" | 目标版本带上 `（MM/DD 发布）` —— 降级时这是决策依据 |
| 新增 npm 调用 | — | **0**（用的就是缓存里的 `times`） |

为什么不是 `更旧` 三个字：它是**当前版本的镜像**，不是信息。`0.1.6-alpha.2` 下面 24 行全是
"更旧"，一列重复 24 遍的字不解决任何问题；而发布日期天然按 semver 递减排好（因为 semver 序
与发布序在这批数据里一致），一列日期本身就是时间线。

**顺带查出一个真 bug**（不是这次改出来的，是原来就有的）：`times` 一旦落盘就再没人刷新。

| | 老代码 | 现在 |
| --- | --- | --- |
| 何时重抓 `times` | 只在 `times` 为空时 | `times_seen != versions.len()` 时（见 `design.md`） |
| 症状 | 新发布的版本**没有日期**；更糟的是 `resolve_before` 查不到目标时刻 ⇒ **静默放弃时间窗** ⇒ 降级退回"朴素装旧版"（会把 dsh 装坏的那条路） | 新版本一出现就重抓一次 |
| 实测（2026-09-23 本机缓存） | `versions` **22** 条、`times` **20** 条 —— 缺的正是 `0.1.5-rc.3` 与 `0.1.7-alpha.1` | 补上后两者都在 |

判据刻意**不是**"条目数少于版本数就重抓"：registry 若对某一版真的不给时刻，那会变成每次开
面板都后台重查的永久空转。记"抓取时的版本数"则两种情况都不会发生（理由与老缓存自愈见
`design.md`）。

**改动**：

| 文件 | 改动 |
| --- | --- |
| `src-tauri/src/update.rs` | `VItem` 增 `published: Option<String>`（原样传 registry 的 RFC3339）；`RegistryCache` 增 `times_seen` + `times_fresh()`；`catalog()` 在版本数对不上时补抓一次；`version_times()` 改用同一判据（`fetch_times()` 只跑命令、不碰缓存）；4 条单元测试 |
| `ui/js/settings.js` | `dateText()` / `fullDateText()`；`row()` 右列改写日期；确认框写目标发布日期 |
| `ui/settings.css` | `.vrow .mark` 右对齐 + `tabular-nums` + `min-width`，让一列日期真的成一列 |

**验证**（2026-09-23）：

| # | 项 | 结果 |
| --- | --- | --- |
| 1 | `cargo test --bins` | ✅ **10 passed**（含 4 条新增：快照判据三态、老缓存自愈、`parse_times` 过滤元数据键、`published` 原样进 JSON） |
| 2 | `cargo check --release` | ✅ 通过 |
| 3 | 日期格式判定 | ✅ 同年不带年 / 跨年带年 / 空串 / `null` / 坏字符串都不上屏 "Invalid Date"；`2026-09-15T03:23:13.750Z` 在本机（UTC+8）解为 `09-15 11:23` |
| 4 | **真实数据渲染** | ✅ 用真 registry 的 24 条版本喂**未改动的** `ui/js/settings.js`（走真通道 `postMessage`，不是绕过去调内部函数），无头 Edge 截图：`0.1.7-alpha.1 → 09/22`、`0.1.6-alpha.2 → 当前版本`、`0.1.6-alpha.1 → 09/15`、`0.1.5-rc.2 → 09/10`…，右列对齐、无 `Invalid Date` |
| 5 | 真机端到端 | ⏳ **待用户实测**（见遗留项 3） |

**遗留项**：第 4 条的截图证明的是"面板拿到这份数据会渲染成这样"，**不等于**"壳层会把
`published` 发过来"——最后这一段（Rust 序列化 → `settings.rs` 的 `push` → `eval` → `shim` →
`postMessage`）要开一次真面板才看得到。预期日志不变（`catalog ok (…)`），判据是**列表右列
出现日期**（而不是"更旧"）。

### 遗留项

1. ~~**每开一次面板跑 5 条 npm 命令**~~ → **已做**（2026-09-21）：registry 那半边 2 小时磁盘缓存 +
   "先用上次结果渲染、过期再后台重查"（实测三态见上文「缓存实测」）。**代价**：过期那次会多跑一遍
   本地命令（后台重查后再推一份时 `current` 重新现查一次），约 1 秒，只发生在过期路径上。
2. `shutting down: killing the dsh process tree (pid N)` 打印**两次**：既有代码里
   `RunEvent::ExitRequested` 与 `RunEvent::Exit` 被同一个分支匹配，清理跑两遍。
   无害（第二次是 kill 一个已死的 pid），**非本项引入**（`git diff` 未触及那段）。
   理论隐患：pid 在两遍之间被系统回收给别的进程时会误杀（概率极低）。
   修法两行（kill 完把 pid 清零），**待拍板**。
3. **人工验收**：
   - **T9 降级**：机制**已手工验证**（npm `--before` 那条链，见「第四轮」）；**DShell 内那条路径
     还没点过** —— 预期日志见第四轮末尾，出现那两行且装完能起即算通过。
   - 仍未验：T10 同版本重装、T11 更新失败后的出口、T12 来源伪装拒绝、
     D3 关于页的开源清单（本轮明确只占位）。
4. **装完自动验证 + 自动回滚**：未做，且已确认**不能拿 `dsh --version` 当验证**
   （第三轮末尾的自我纠正）。真要做，验证得复用 `start_dsh` 的 doctor+handoff（或单跑一次
   `dsh web --port 0`），失败则自动装回上一版 —— 但自动回滚会掩盖"新版不能用"这个事实，
   与 09 定下的取消语义冲突，**得先拍板**。

