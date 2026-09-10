# Roadmap

当前可用，但离开箱即用还有距离。按优先级排列。

## 1. 单实例

双击多次会开多个 `dsh web` 后端，内存成倍增长。更麻烦的是**每个后端都持有 DSH 的会话锁**，会干扰 DSH 自身的 session resume（实测报过 `SessionAlreadyOwnedError`）。

方案：命名互斥体（`CreateMutexW`）检测已有实例，存在则聚焦那个窗口。

## 2. 强杀防孤儿进程

正常关窗是干净的（`ExitRequested` → `taskkill /T /F`，端口释放）。但**任务管理器强杀时 `ExitRequested` 不触发**，清理代码不跑，留下孤儿 node 占着端口。

方案：Windows **Job Object** + `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`，把 dsh 子进程绑进 job，父进程无论怎么死内核都会收掉它。

## 3. 交付物下载

`present` 交付物的下载走 WebView2 自己的下载流程，Tauri 默认不接管。可能点了没反应——**未验证**。

## 4. 版本更新提示

注意本机 dsh 是 `0.1.5-rc.1`（**预发布版**）。如果更新检查只看 `latest` dist-tag，会得出错误结论（可能提示降级）。

应该取 `npm view @deepseek-ai/dsh dist-tags --json` 的所有 tag，做 **semver 预发布比较**，而不是字符串比较。查到新版后**要问用户**，不要静默升级。

## 5. 其余

- 托盘图标：状态、显示/隐藏、退出
- 记住上次窗口大小和位置
- WebView2 runtime **完全缺失**时的兜底：现在启动页只能展示版本与过低告警；
  真正缺失时窗口根本建不出来，需要原生对话框（Win32 TaskDialog）给下载指引
- web / headless 两个 profile 切换
