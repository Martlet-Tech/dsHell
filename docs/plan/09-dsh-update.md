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

**做**：拆分 stop / start；托盘两项；版本检查（含来源校验）；**版本选择器**（含降级确认）；
更新执行（停→装→起）；更新进度显示与模式切换。

**不做**：自动 / 静默更新；后台轮询检查；DShell 自身（exe）的更新——那是另一件事；
设置窗口本体（独立窗口，roadmap §4）；**第二个本地页**（见"运行中临时显示别的内容"：
该用覆盖层，不用导航）。

### 怎么测试（更新是"一次性"动作，但**不必**只测一次）

顾虑：更新成功后本机就是最新版了，第二次点「检查更新」只会说"已是最新"，**没有可更新的目标**了。

这个顾虑**不成立**，核实后的理由：

1. **动作本身可反复测**：`npm i -g @deepseek-ai/dsh` 对**同一个版本**再跑一次就是重装。
   「更新 dsh」不因"已是最新"而禁用（见上：它同时是**修复**入口），所以
   "停 → 装 → 起"这条链可以无限次重跑。
2. **真实存在可测的升级对**。实测 registry（`registry.npmmirror.com`，2026-09-20）：

   | dist-tag | 版本 |
   | --- | --- |
   | `latest` | `0.1.5-rc.2` |
   | `next` | `0.1.5-rc.2` |
   | `alpha` | `0.1.6-alpha.2` |

   本机当前是 `0.1.5-rc.1`。所以存在**两个**真实可测目标：
   `0.1.5-rc.1 → 0.1.5-rc.2`（同 minor 的 rc 递增），以及
   `0.1.5-rc.1 → 0.1.6-alpha.2`（换 minor 的 alpha）。
   **顺带这是"只看 `latest` 会判错"的活证据**：本机是 rc.1，而 `latest` 是 rc.2
   —— 方向恰好是对的，但如果反过来（本机 rc.2、`latest` rc.1）就会提示降级。
3. **降级也是一条被测路径**：`npm i -g @deepseek-ai/dsh@0.1.5-rc.1` 可以装回旧版。
   "改完测完，把环境降回起点"这条路必须成立，否则测试不可重复。**这一点要在实现里
   显式支持**（见下）。

**因此实现必须带一个"安装指定版本"的能力**，而不是只有"装最新"。这个能力的
**用户可见形态就是版本选择器**（见下节），环境变量只是它的自动化旁路。

### 版本选择器（对齐 ComfyUI Manager 的 Select Version）

用户提议：做成 ComfyUI Manager 插件那种版本列表，让用户自己选装哪个版本。**采纳**，
理由是它一次解决三件事，而不是多做一个 UI：

| 解决了什么 | 说明 |
| --- | --- |
| 预发布困境 | 本机是 `0.1.5-rc.1`，"有没有新版"本身就没有唯一答案。让用户**看见**可选版本，比我们替他猜"最新"更诚实 |
| 回滚 | 新版 dsh 出问题时能装回旧版 —— 这是"更新"功能的**必要配套**，只有单向升级的更新是不完整的 |
| 测试地基 | 我原本要用环境变量手工做的"装指定版本"，正是这个功能。**不必造两套** |

**实测数据（`registry.npmmirror.com`，2026-09-20）**：共 22 个已发布版本，
**全部是预发布版**（`-rc.N` / `-alpha.N`），**没有一个稳定版**，且没有 `0.1.4`。

| dist-tag | 版本 |
| --- | --- |
| `latest` | `0.1.5-rc.2` |
| `next` | `0.1.5-rc.2` |
| `alpha` | `0.1.6-alpha.2` |

所以列表形态应当**沿用 ComfyUI Manager 的两段式**：先列 dist-tag（语义化的
"最新 / 预先体验"），再列具体版本号（降序）。**但不要写成 "stable"** —— 这个包
没有稳定版，那样会误导。

**渠道**：`npm view @deepseek-ai/dsh versions --json` 给全部版本，dist-tags 另取。
两者都只读，不改动系统状态。

### ⚠️ 降级风险：必须先警告，不能默认放行

`src-of-dsh` 自己的 `AGENTS.md` 明确写着：

> Adjacent migration may add a version-named successor but never move, overwrite, or
> delete committed generations; **predecessors imply neither fallback nor downgrade
> support**. SQLite uses monotonic `SCHEMA_VERSION`.

**含义**：dsh 的会话数据格式是**单向**的 —— 用新版 dsh 写出的会话，旧版**不保证能读**。
所以"装个旧版回去"**不是**无副作用的操作，可能让用户读不到新版本期间产生的会话。

因此版本选择器必须：

1. **默认只允许往前**（`>= 当前版本`）；
2. 选更低版本时**明确警告**（"dsh 的会话格式不支持降级，旧版可能读不到新版本写的会话"）；
3. 需要**显式确认**才执行。

这与上面的"回滚"需求不矛盾 —— 回滚仍然可用，只是不能是**静默**的。测试用的降级
（`rc.2 → rc.1`）同理：测试期由我们显式确认，不走默认路径。

### 运行中临时显示别的内容（设置页 / 版本选择器）

用户提议：运行中用本地页面暂时顶替 dsh 页面，比如把 DShell 设置页或版本选择器放进去。

**机制：注入覆盖层（overlay），不要导航。** 这是本节唯一正确的做法，理由与"状态是数据，
不是文件"是同一条：**导航会销毁当前文档**，dsh 页面里的一切（输入框内容、滚动位置、
当前会话、进行中的流）都会随之丢失。而覆盖层只是往现有 DOM 里插一个 `position:fixed`
元素，**当前文档自始至终没有换过**，dsh 页面只是被盖住，不是被卸载。

> **更正记录**：本节初稿写的是"导航到另一个本地页，与重启走同一条路"，并据此做了一个
> `hello.html` 的实验。**那是错的**，而且自相矛盾 —— 同文档的"前端模块化"一节刚说过
> "状态是数据，不是文件…不是加载另一个页面"。已改为覆盖层方案，`hello.html` 已删除。
> 教训：把"运行中的状态切换"与"重启时的页面导航"混为一谈。重启导航是**必要**的
> （旧页面绑定的是已被杀掉的 dsh），运行中切内容则**完全不需要**导航。

| 机制 | 当前文档 | dsh 页面状态 | 适用 |
| --- | --- | --- | --- |
| **注入覆盖层**（`eval` 插 DOM） | **不变** | **保全** | 设置面板、版本选择器、任何"临时盖住" |
| 导航到另一个本地页 | 换掉 | **全部丢失**（整页重载） | 仅用于**重启**（那时 dsh 反正已经死了） |
| 独立窗口 | 不变 | 保全 | 设置窗口（roadmap §4）；不打断 dsh |

**覆盖层这一侧是确定可行的**（就是插一个 DOM 元素）。**真正待实测的是盖住之后 dsh
页面会怎样**：

- 被完全盖住时，dsh 页面的计时器 / 流式输出会不会被 WebView2 判定为"后台页面"而降频？
- 覆盖层移除后，dsh 页面是否**完好如初**、不需要重新握手？

### ✅ 实测结论（2026-09-20，最小实验已通过）

探针：托盘两项「【测试】显示测试覆盖层」/「【测试】移除测试覆盖层」，
`eval` 注入 `position:fixed` 的 div，交替点 3 轮。

| 待验项 | 结果 | 证据 |
| --- | --- | --- |
| 注入不引起导航 | ✅ | 日志 3 次 `overlay: injected (no navigation)`；注入与移除之间**没有任何** `window navigated` / `splash url` |
| 同一份文档始终未重载 | ✅ | 覆盖层里的计数器 **1 → 2 → 3 递增**（它存在 JS 全局变量里，重载就会归零） |
| 无错误 | ✅ | 全日志 `FATAL/ERROR/panicked/failed/unparsable` **0 次** |
| 可反复 | ✅ | 注入/移除成对 3 轮，干净 |
| 不留孤儿进程 | ✅ | 结束后 `full quit` + `killing the dsh process tree`；实测 `dshell` 只剩 1 个 |
| **dsh 任务不受影响** | ✅ **强证据** | **对话进行中、AI 正在输出时**开覆盖层再关闭，**对话继续正常输出** |
| 无渲染降频 | ✅ | 同上——覆盖期间输出未卡顿 |

**第 6 项是本次实验最有价值的结论，且它推翻了我原先的归因。** 我原本把"不会被降频"
归因于建窗时那组 `--disable-backgrounding-occluded-windows` / `--disable-renderer-backgrounding`
/ `--disable-background-timer-throttling` 开关。**这个归因是错的**：本次覆盖层是
`position:fixed; inset:0` 盖满整个视口，但 WebView2 **并没有把它当作"窗口被遮挡"** ——
窗口本身始终可见、有焦点、在前台。那组开关针对的是**窗口级遮挡**（另一个窗口盖在它上面），
而 DOM 内部的覆盖层根本不触发那条判定。

正确的理解：**覆盖层方案之所以安全，是因为它压根不触碰 WebView2 的遮挡/后台判定**，
而不是因为我们预先禁用了那些特性。这个区别有实际后果——它意味着覆盖层方案的可靠性
**不依赖于**那组开关是否生效（那些开关是 `additional_browser_args`，一旦被覆盖或
WebView2 升级改变行为，本方案不受牵连）。

**覆盖层方案的已知代价**（比导航小得多，但要说清）：

| 项 | 说明 |
| --- | --- |
| 元素可见性 | 覆盖层是 dsh 文档里的一个元素，dsh 自己的快捷键 / 全局监听仍会收到事件，必要时要 `stopPropagation` |
| 样式冲突 | 必须用足够高的 `z-index` 与独立命名，避免被 dsh 的样式影响；反向也要避免影响它 |
| 不适用于"启动前" | 启动页阶段没有 dsh 文档可注入 —— 那时本来就用启动页，不需要覆盖层 |
| 探针的可观测性不足 | `window.eval` 是**单向**的，Rust 侧拿不到 JS 返回值，所以日志区分不出"真的插入了"与"发现已存在就跳过"。本次三轮都是交替操作，不影响结论；**将来若复用此探针，需让 JS 反向 `invoke` 一次才能把结果记进日志** |

### （原"前置 bug"一条已撤销）

初稿还据此提了一个"`splash_url` 会被第二个本地页覆盖"的前置 bug 与修复。**既然不再新增
本地页，那个改动一并撤销** —— 它属于对 08 已验证过的重启代码做无关行为改动，不该混进
本项。若将来真要做第二个本地页（如 `settings.html`），**届时必须先修它**：`on_page_load`
现在的判定 `matches!(url.host_str(), Some("tauri.localhost"))` 会把任何本地页都当成启动页，
于是 `splash_url` 被覆盖 → 「重启 dsh」导航到那个页面 → 进度与失败卡片发到没有监听者的
页面上（与 08 修掉的 `about:blank` 是同一个缺陷类）。

### 测试矩阵（本轮验收用）

| # | 场景 | 期望 |
| --- | --- | --- |
| T1 | 起点 `0.1.5-rc.1`，点「检查更新」 | 报"有新版 → `0.1.5-rc.2`"，**不自动装** |
| T2 | 点「更新 dsh」 | 回启动页 → 显示"正在更新" + 实时进度 → 装成功 → 自动体检全绿 → 进 dsh |
| T3 | 更新后看 dsh 版本 | `0.1.5-rc.2`（**新进程**，不是旧页面缓存） |
| T4 | 降回 `0.1.5-rc.1`，重跑 T2 | 同上（证明可重复） |
| T5 | 已是最新时点「更新 dsh」 | 重装同版本，流程照常成功（**不是**灰按钮） |
| T6 | 更新中途点「取消」 | dsh 已停 → 停在失败卡片 + 「重新启动 dsh」按钮，**不自动**起回 |
| T7 | 伪装来源（把 `cfg.dsh` 指向非 npm 那份） | **拒绝更新**并说明原因，不装出第二份 |
| T8 | 更新中强制关掉 DShell | 不留孤儿 node（与强杀用例同法判读：看进程/端口，不数日志行） |
| T9 | 版本选择器：全量列表 + dist-tag 分组 | 显示 22 个版本（含 `0.1.6-alpha.2`），标记当前版本 |
| T10 | 选**更低**版本 | **警告 + 需确认**；确认后能装回 `0.1.5-rc.1` |
| T11 | 注入覆盖层 → 移除 | ✅ **已实测通过**（2026-09-20）：计数器递增证明未重载；对话进行中开/关覆盖层，**输出继续** |
| T12 | 覆盖层盖住期间 dsh 仍在输出 | ✅ **已实测通过**（同上）。**注意归因**：不是那组 `--disable-*` 开关的功劳，而是 DOM 覆盖层不触发 WebView2 的窗口级遮挡判定 |

T1 的关键是**只看不动**：现有 `npm view` 调用是只读的，但必须确认 UI 没有副作用。

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
4. 版本检查：registry 感知的 dist-tags / versions 采集 + 预发布 semver 比较 + 来源校验
5. **版本选择器**（列表 + dist-tag 分组 + 降级警告/确认）—— 它同时就是测试地基，
   否则更新链只能测一次
6. 更新执行：`stop → installer::run(Dsh, target) → start`，闩跨全程
7. 前端 `updating()` 模式切换 + 失败后的"重新启动"出口
8. （若要"运行中显示别的内容"）**覆盖层机制已验证可用**（T11/T12，2026-09-20）。
   直接在此之上做设置面板 / 版本选择器；**不要**改用导航

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

1. **取消的语义** —— **已定**（2026-09-20，采纳"语义更诚实"方案）：更新中途点「取消」时
   dsh 已经被停了，所以取消**不自动**起回旧后端。停在失败卡片上，给一个明确的
   「重新启动 dsh」按钮，让用户自己决定何时把后端起回来。理由：自动起回会掩盖
   "更新没成功"这个事实，而用户此刻最需要知道的就是它。
2. **要不要引 `semver` crate** —— **已定：引**（2026-09-20 更正原"倾向手写"）。
   核实发现 `semver 1.0.28` **已经在 `Cargo.lock` 里**，是 `tauri-build` /
   `tauri-codegen` / `tauri-utils` / `cargo_metadata` / `rustc_version` 的传递依赖
   （433 个包里 6 处引用）。所以增列它为直接依赖**不新增任何需要下载或编译的包**，
   只是把已有的提升为直接可见 —— 原"零新增依赖"的顾虑在这里**不成立**。

   为什么仍值得引而不是手写：预发布比较的规则本身不复杂，但**容易错在边界上**
   （`0.1.5-rc.1` vs `0.1.5`、`rc.2` vs `rc.10` 的数值段、build metadata 应被忽略），
   而这里判错方向是**误导用户**（比如提示降级）。用标准实现 + 补几条针对性测试，
   比手写几十行再想办法证明它对要划算。这与 `src-of-dsh` 自己的取向也一致
   （AGENTS.md：「Prefer maintained dependencies over hand-rolling when they genuinely
   delete owned code and tests」）。

   **但注意**：`semver` 只解决"比较"，不解决"取哪些版本"。`npm view dist-tags --json`
   给的是**命名 tag → 版本**的映射（`latest` / `next` / …），要把这些**全部**纳入
   比较并选最大者，而不是只取 `latest` —— 这一步仍要自己写。
