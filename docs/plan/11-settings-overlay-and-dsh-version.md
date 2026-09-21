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
| D4 | 本轮 DShell 版本号 | ✅ 验证通过后升到 **0.1.5**（实施期间不动，保持 0.1.4） |
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
fn command_for(kind: Kind, target: Option<&str>) -> (String, Vec<String>);
//   Kind::Dsh + None        → npm i -g @deepseek-ai/dsh          （现状，一键安装用）
//   Kind::Dsh + Some("x.y") → npm i -g @deepseek-ai/dsh@x.y
pub fn run(app: &AppHandle, kind: Kind, state: &AppState, target: Option<&str>) -> Outcome;
// 既有调用点（install_missing）传 None ⇒ 行为不变。
```

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

### 遗留项

1. ~~**每开一次面板跑 5 条 npm 命令**~~ → **已做**（2026-09-21）：registry 那半边 2 小时磁盘缓存 +
   "先用上次结果渲染、过期再后台重查"。实测三种状态见下表。**代价**：过期那次会多跑一遍本地命令
   （先推旧数据、后台重查后再推一份时，`current` 重新现查一次），约 1 秒，只发生在过期路径上。
2. `shutting down: killing the dsh process tree (pid N)` 打印**两次**：既有代码里
   `RunEvent::ExitRequested` 与 `RunEvent::Exit` 被同一个分支匹配，清理跑两遍。
   无害（第二次是 kill 一个已死的 pid），**非本项引入**（`git diff` 未触及那段）。
   理论隐患：pid 在两遍之间被系统回收给别的进程时会误杀（概率极低）。
   修法两行（kill 完把 pid 清零），**待拍板**。
3. **还没人工验收**：降级警告（T9）、同版本重装（T10）、更新失败后的出口（T11）、
   来源伪装拒绝（T12）、关于页与开源清单占位（D3 → 本轮明确只占位）。

### 缓存实测（2026-09-21，产物 `target\20260921-123041`）

| 状态 | npm 调用 | 日志证据 |
| --- | --- | --- |
| 冷启动（无缓存） | **5 条**（`view versions` / `view dist-tags` / `config get` / `ls -g` / `prefix -g`）→ 落盘 639 字节 | `catalog ok (… age=2s, stale=false)` |
| 热缓存（TTL 内） | **2 条本地命令**（`ls -g` / `prefix -g`），**零联网** | `catalog ok (… age=6s, stale=false)` |
| 过期（把 `fetched_at_ms` 回拨 3 小时） | 先只跑本地命令推旧数据 → 再 `view ×2 + config` 重查 → 重推 | `catalog ok (… age=10812s, stale=true)` → `cached catalog is stale - re-querying in the background` → `catalog ok (… age=2s, stale=false)` |
| 切换后的 `current` | 每次现查，不吃缓存 | 切换完成后立刻显示 `0.1.5-rc.2`（不是缓存的 rc.1） |

手动「刷新」= 无视 TTL 强制重查；失败时推 `refresh_error` 给面板，**不清空已有列表**。

> 面板上会显示 `· 数据 N 分钟前`，过期时补 `（正在刷新…）` —— 用户有权知道眼前这份列表有多旧。


