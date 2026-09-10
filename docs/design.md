# 设计决策与验证记录

主 README 只讲"是什么、怎么跑"。这里放推导过程：为什么这么写、实测数据是什么、哪些坑踩过。

## 核心约束：为什么不能开裸地址

`dsh-client-connection` 的 `BrowserAuth.authorizeIndex()` 逻辑：

- 请求带 `/?token=<每进程随机 launchToken>` → 种 `HttpOnly; SameSite=Strict` cookie，303 跳回 `/`
- 带有效 cookie → 放行
- **其余一律极简 401**

WebView2 有自己独立的 cookie 罐，所以冷启动必定 401。这是整个方案唯一的硬骨头。

**钥匙在 stdout**：`dsh web` 启动时打印

```
dsh web: http://127.0.0.1:PORT/?token=xxxxxxxx
```

`printUrl` 默认为 true 且没有关闭开关，所以这行稳定可依赖。

## 实测验证

| # | 验证项 | 结果 |
| --- | --- | --- |
| 1 | `dsh web --port 0 --no-open` 打印 token URL | ✅ `dsh web: http://127.0.0.1:49864/?token=...` |
| 2 | 裸 `GET /` | ✅ **401** |
| 3 | `GET /?token=<正确>` | ✅ **303** + `Set-Cookie: dsh-auth-...; HttpOnly; SameSite=Strict` |
| 4 | 带 cookie 再 `GET /` | ✅ **200**，27660 字节，`<title>DeepSeek Harness</title>` |
| 5 | 错误 token | ✅ **401** |
| 6 | 全链路 | ✅ 窗口标题 `DShell — DeepSeek Harness` |
| 7 | 启动页先于 dsh 出现 | ✅ t+0.5s 窗口已在屏幕上，此时 dsh 尚未输出任何内容 |
| 8 | 启动页确实渲染（非白屏 401） | ✅ 像素分析：93.9% 深色 / 3.6% 白 / 0.8% 彩 |
| 9 | 失败时留住窗口 + 可读错误卡片 | ✅ 假 dsh 测试：窗口不退出，卡片渲染（粉色像素 157） |
| 10 | 正常关窗清理 | ✅ `ExitRequested` → `taskkill /T /F` 退出码 0，端口释放，无孤儿 |

第 3 步用的 token 是从程序自己写的日志里读出来的——证明「抓 stdout → 建窗口」这条链路真实工作，而不是手工拼的地址。

第 8 条需要说明验证方法：因为当时用的模型不支持读图，没法用眼睛确认动画，就改成截图做像素统计。深色底占比 93.9% 排除了「白色 401 页面」的可能，0.8% 彩色对应光晕和渐变字。

## 关键设计点

### 1. 启动页必须先弹

原实现把阻塞的 `wait_for_url` 放在 `setup` 里，导致 dsh 起来前**窗口根本不存在**，用户看到几秒空白。

现在：`setup` 立刻建窗加载**本地**启动页（瞬间绘制、不依赖 dsh），阻塞逻辑丢到 worker 线程，拿到 URL 后再 `navigate` + 淡出。

### 2. `--port 0` 是推荐做法

cookie 名 = `dsh-auth-` + base64url(sha256(authority))，authority **含端口**。所以随机端口下 cookie 下次用不上——但无所谓，token 路径永远可用，而且**天然免端口冲突**。

### 3. 必须 `cmd /c dsh`

Windows 上 dsh 是 `dsh.cmd` / `dsh.ps1`，`CreateProcess` 不解析 `.cmd`，直接 `Command::new("dsh")` 会失败。走 `cmd /c` 才能命中 `C:\Users\zt\AppData\Roaming\npm\dsh.cmd`。

### 4. 隐藏窗口要 `CREATE_NO_WINDOW`

`creation_flags(0x0800_0000)`，`cmd` 和 `taskkill` 都要设，否则每次启动闪黑框。

### 5. 外链拦截要放行 `tauri.localhost`

非本机导航一律丢给系统浏览器，否则点引用链接会把 UI 顶掉且回不来。

**坑**：Tauri 在 Windows 上从 `http://tauri.localhost` 提供打包页面——启动页本身也是这个 origin。漏掉它会导致每次启动都往系统浏览器弹一个标签页。

### 6. 跨进 JS 的文本用 base64 传

错误卡片文本要穿过两层上下文（JS 字符串字面量 → innerHTML）。手写转义很脆：dsh 报错里一个引号或 `<` 就能破坏结构。

base64 载荷只含 `[A-Za-z0-9+/=]`，在两种上下文里都是惰性的，换行也能精确保留。

### 7. 版本只有一份

web profile 的 `dependencies` 为空，bundle 全从全局安装解析。所以**更新全局即可，不用重装 profile**。

## 踩过的坑

**日志中文乱码**：`log()` 写 UTF-8 但被按 GBK 读，`dsh 还没打印地址` 显示成 `dsh 杩樻病鎵撳嵃`。解决：首次写入时加 UTF-8 BOM。

**错误卡片换行塌成一行**：`\n` 在 innerHTML 里折叠成空格，项目符号全挤一起。解决：改用 base64 传递，页面侧解码后再转 `<br>`。

**`w.title()` 读到缓存值**：曾想用它读取渲染后的 DOM 状态来验证启动页，但读回的一直是初始标题。功能不受影响，只是说明这条路不通，最后改用截图分析。

## 调试技巧

设 `DSHELL_SPLASH_HOLD=1` 启动，程序会停在启动页不跳转。配合截图可以检查动画，不会跟 dsh 的启动速度赛跑。

## 一个操作教训

反复启动 exe 做测试时，每次都会拉起一个 `dsh web --port 0` 后端。如果测试脚本只杀父进程、没收子进程树，这些后端会一直挂着，**每个都持有 DSH 的会话锁**，最终导致 DSH 自身的 session resume 报 `SessionAlreadyOwnedError`。

exe 本身的退出清理是好的（正常关窗会 `taskkill /T /F` 收掉整棵树）。问题出在绕过正常关窗路径直接杀父进程。测试完记得检查有没有残留 node 进程。
