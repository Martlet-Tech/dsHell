# Roadmap

当前可用，但离开箱即用还有距离。按优先级排列。

## 1. 强杀防孤儿进程

正常关窗是干净的（`ExitRequested` → `taskkill /T /F`，端口释放）。但**任务管理器强杀时 `ExitRequested` 不触发**，清理代码不跑，留下孤儿 node 占着端口。

方案：Windows **Job Object** + `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`，把 dsh 子进程绑进 job，父进程无论怎么死内核都会收掉它。

## 2. 交付物下载

`present` 交付物的下载走 WebView2 自己的下载流程，Tauri 默认不接管。可能点了没反应——**未验证**。

## 3. 设置页（覆盖层）与 dsh 版本管理

用户不会自己用 npm 更新 dsh（现状是让智能体代劳）。目标：在 DShell 里**看见**本机装的是哪个版本、
**选**一个目标版本、**一键切换**，且切换完自动生效。

承载它的界面就是设置页：**主窗口内的覆盖层**（不是独立窗口，也不是导航——导航会销毁 dsh 文档），
多标签 + 共享底栏，本轮只放「dsh 版本管理」与「关于」两个标签。

注意本机 dsh 是 `0.1.5-rc.1`（**预发布版**）。如果更新检查只看 `latest` dist-tag，会得出错误结论（可能提示降级）。

应该取 `npm view @deepseek-ai/dsh dist-tags --json` 的所有 tag，做 **semver 预发布比较**，而不是字符串比较。查到新版后**要问用户**，不要静默升级。

**前置（均已完成）**：

| 前置 | 为什么 | 状态 |
| --- | --- | --- |
| 单实例 | 只要还有**任意一个** dsh 后端活着，Windows 上被 node 加载的 native addon 就锁着要覆盖的文件，`npm i -g` 会失败或留下半个安装 | ✅ [closed/07](closed/07-single-instance.md) |
| 重启 dsh 后端 | 更新执行完必须重起 dsh 才生效 | ✅ [closed/08](closed/08-restart-dsh.md) |

→ **在办**：[docs/plan/11-settings-overlay-and-dsh-version.md](plan/11-settings-overlay-and-dsh-version.md)
（承接原 `plan/09-dsh-update.md`，后者已在工作区删除，有效结论并入 11）

> **原 §4「设置窗口」已并入本项**，那条"新增窗口后 × 会误触发关闭确认弹窗、必须按
> `window.label()` 过滤"的警告在本轮**不成立**——设置是主窗口内的覆盖层，没有第二个窗口。
> 将来真做独立设置窗口时，那条警告仍然有效。

## 4. 其余

- 记住上次窗口大小和位置
- WebView2 runtime **完全缺失**时的兜底：现在启动页只能展示版本与过低告警；
  真正缺失时窗口根本建不出来，需要原生对话框（Win32 TaskDialog）给下载指引
- web / headless 两个 profile 切换
- 托盘图标状态化（tooltip 显示后端状态；本轮只做了图标与两个动作）
- 关闭确认的「不再提示」选项（本轮明确未做，每次点 × 都会问）
- 自带插件（`dshell-directory-picker`）的版本感知刷新：`plugin::ensure_installed`
  现在是"目标存在且完整就永不覆盖"，所以 DShell 升级后，已装用户手里仍是**旧副本**
  （开发机是 junction，不受影响）。见 [closed/08](closed/08-restart-dsh.md) 的附注
