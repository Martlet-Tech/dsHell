# Roadmap

当前可用，但离开箱即用还有距离。按优先级排列。

## 1. 单实例

双击多次会开多个 `dsh web` 后端，内存成倍增长。更麻烦的是**每个后端都持有 DSH 的会话锁**，会干扰 DSH 自身的 session resume（实测报过 `SessionAlreadyOwnedError`）。

方案：命名互斥体（`CreateMutexW`）检测已有实例，存在则聚焦那个窗口。

→ **在办**：[docs/plan/07-single-instance.md](plan/07-single-instance.md)

## 2. 强杀防孤儿进程

正常关窗是干净的（`ExitRequested` → `taskkill /T /F`，端口释放）。但**任务管理器强杀时 `ExitRequested` 不触发**，清理代码不跑，留下孤儿 node 占着端口。

方案：Windows **Job Object** + `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`，把 dsh 子进程绑进 job，父进程无论怎么死内核都会收掉它。

## 3. 交付物下载

`present` 交付物的下载走 WebView2 自己的下载流程，Tauri 默认不接管。可能点了没反应——**未验证**。

## 4. 版本更新提示与更新

注意本机 dsh 是 `0.1.5-rc.1`（**预发布版**）。如果更新检查只看 `latest` dist-tag，会得出错误结论（可能提示降级）。

应该取 `npm view @deepseek-ai/dsh dist-tags --json` 的所有 tag，做 **semver 预发布比较**，而不是字符串比较。查到新版后**要问用户**，不要静默升级。

**依赖关系（顺序不能反）**：

| 前置 | 为什么 |
| --- | --- |
| #1 单实例 | 只要还有**任意一个** dsh 后端活着，Windows 上被 node 加载的 native addon 就锁着要覆盖的文件，`npm i -g` 会失败或留下半个安装 |
| **重启 dsh 后端** | 更新执行完必须重起 dsh 才生效 |

→ 重启部分**在办**：[docs/plan/08-restart-dsh.md](plan/08-restart-dsh.md)（它不绑在更新上，独立就有价值：改了 dsh 路径、装了插件、dsh 卡住都用得上）

## 5. 设置窗口

托盘与关闭行为已做完（见 `docs/closed/03-tray-and-close-behavior.md`）。下一步是设置窗口本身：
左右分栏、开机启动、代理（安装用 / 对话用两组）。原始设想见
`docs/plan/260901-托盘,系统设置,退出行为.md`。

**动手前必读该结案文档的 L1**：`on_window_event` 现挂在 Builder 上、对所有窗口生效，
新增窗口后其 × 会误触发关闭确认弹窗，必须按 `window.label()` 过滤。

## 6. 其余

- 记住上次窗口大小和位置
- WebView2 runtime **完全缺失**时的兜底：现在启动页只能展示版本与过低告警；
  真正缺失时窗口根本建不出来，需要原生对话框（Win32 TaskDialog）给下载指引
- web / headless 两个 profile 切换
- 托盘图标状态化（tooltip 显示后端状态；本轮只做了图标与两个动作）
- 关闭确认的「不再提示」选项（本轮明确未做，每次点 × 都会问）
