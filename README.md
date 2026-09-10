# DShell

把 **DeepSeek Harness 的 Web GUI** 包成一个桌面程序：双击即用，不用挂命令行、不用敲 `dsh web`、不用自己去浏览器输地址。

这是一个 **薄壳（thin shell）** —— 复用你机器上已装的 Node.js + `@deepseek-ai/dsh`，exe 只有几 MB。

> **状态**：可用。核心链路 + 启动动画 + 错误界面均已实机验证。
> 仍未做：单实例、托盘、窗口位置记忆、交付物下载接管。

---

## 它解决的核心问题

直接开 `http://127.0.0.1:3080/` **进不去**。DSH 的 `BrowserAuth.authorizeIndex()` 逻辑是：

- 带 `/?token=<每进程随机 launchToken>` → 种 HttpOnly cookie，303 跳回 `/`
- 带有效 cookie → 放行
- **其余一律极简 401**

而 WebView 有**自己独立的 cookie 罐**，所以冷启动开裸地址必定 401。

**钥匙在 stdout 里**：`dsh web` 启动时会打印

```
dsh web: http://127.0.0.1:PORT/?token=xxxxxxxx
```

DShell 的全部诀窍就是四步：

1. 隐藏启动 `dsh web --port 0 --no-open`（无控制台窗口）
2. 读它的 stdout，抓出那行带 token 的 URL
3. 用这个 URL 打开 WebView —— 303 换来 cookie，才拿到真 UI
4. 关窗时收掉整棵进程树，不留孤儿 node

```
┌──────────────┐   spawn (hidden)   ┌──────────────┐
│  DShell.exe  │ ─────────────────▶ │   dsh web    │
│              │                    │  --port 0    │
│   splash     │ ◀───────────────── │              │
│  (animated)  │   stdout: token URL│  127.0.0.1:N │
└──────┬───────┘                    └──────────────┘
       │ navigate(token URL)
       ▼
   303 → Set-Cookie → 200  ⟹  真 UI
```

---

## 构建

需要 Rust 工具链 + MSVC linker + WebView2 runtime（Win10 1803+ 通常自带）。

```powershell
cd src-tauri
cargo build --release
# 产物：target/release/dshell.exe
```

首次编译要联网拉 crates；本机缓存已有全套 tauri 依赖时约 1 分 20 秒。

## 运行

双击 `dshell.exe` 即可。日志在 `%USERPROFILE%\.dshell\dshell-poc.log`。

调试用环境变量：

| 变量 | 作用 |
| --- | --- |
| `DSHELL_SPLASH_HOLD=1` | 停在启动页不切换，方便截图 / 调动画 |

---

## 实机验证结论

| # | 验证项 | 结果 |
| --- | --- | --- |
| 1 | `dsh web --port 0 --no-open` 打印 token URL | ✅ `dsh web: http://127.0.0.1:49864/?token=...` |
| 2 | 裸 `GET /` | ✅ **401**（复现硬约束） |
| 3 | `GET /?token=<正确>` | ✅ **303** + `Set-Cookie: dsh-auth-...; HttpOnly; SameSite=Strict` |
| 4 | 带 cookie 再 `GET /` | ✅ **200**，27660 字节，`<title>DeepSeek Harness</title>` |
| 5 | 错误 token | ✅ **401** |
| 6 | 全链路 | ✅ 窗口标题 `DShell — DeepSeek Harness` |
| 7 | **启动页先于 dsh 出现** | ✅ t+0.5s 窗口已在屏幕上，此时 dsh 尚未输出任何内容 |
| 8 | 启动页确实渲染（非白屏 401） | ✅ 像素分析：93.9% 深色 / 3.6% 白 / 0.8% 彩 |
| 9 | 失败时留住窗口 + 可读错误卡片 | ✅ 假 dsh 测试：窗口不退出，卡片渲染（粉色像素 157） |
| 10 | 正常关窗清理 | ✅ `ExitRequested` → `taskkill /T /F` 退出码 0，端口释放，无孤儿 |

第 3 步用的 token 是从 exe 自己写的日志里读出来的 —— 证明「抓 stdout → 建窗口」这条链路真实工作，而不是手工拼的地址。

---

## 关键设计点（都已实测）

### 1. 必须用 token URL，不能开裸地址
见上。这是整个方案唯一的硬骨头。

### 2. 启动页必须先弹
原实现把阻塞的 `wait_for_url` 放在 `setup` 里，导致 dsh 起来前**窗口根本不存在**，用户看到的是几秒空白。
现在改为：`setup` 立刻建窗加载**本地**启动页（瞬间绘制、不依赖 dsh），阻塞逻辑丢到 worker 线程，拿到 URL 后再 `navigate` + 淡出。

### 3. cookie 按端口隔离
cookie 名 = `dsh-auth-` + base64url(sha256(authority))，authority **含端口**。
所以 `--port 0` 每次新端口，cookie 复用不了 —— 但无所谓，token 路径永远可用，而且**天然免端口冲突**。这是推荐做法。

### 4. 必须 `cmd /c dsh`，且 `CREATE_NO_WINDOW`
Windows 上 dsh 是 `dsh.cmd` / `dsh.ps1`，`CreateProcess` 不解析 `.cmd`，直接 spawn 会失败。
另外必须设 `creation_flags(0x0800_0000)`，否则每次启动闪黑框（`cmd` 和 `taskkill` 都要设）。

### 5. 外链必须拦，但 `tauri.localhost` 要放行
非本机的导航一律丢给系统浏览器，否则点引用链接会把 UI 顶掉且回不来。
注意：Tauri 在 Windows 上从 `http://tauri.localhost` 提供打包页面 —— **启动页本身也是这个 origin**，漏掉它会导致每次启动都往系统浏览器弹一个标签页。

### 6. 跨进 JS 的文本用 base64 传
错误卡片文本要穿过两层上下文（JS 字符串字面量 → innerHTML）。手写转义很脆：dsh 报错里一个引号或 `<` 就能破坏结构。
base64 载荷只含 `[A-Za-z0-9+/=]`，在两种上下文里都是惰性的，换行也能精确保留。

### 7. 版本只有一份
web profile 的 `dependencies` 为空，bundle 全从全局安装解析 → **更新全局即可，不用重装 profile**。

---

## 已知缺口

1. **单实例未做** —— 双击多次会开多个 `dsh web` 后端，内存成倍。且**每个后端都持有会话锁**，可能干扰 DSH 自身的 session resume。
2. **任务管理器强杀会留孤儿 node** —— 正常关窗是干净的（见验证 #10），但强杀时 `ExitRequested` 不触发。
   正解：Windows **Job Object** + `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`。
3. **交付物下载未接管** —— Tauri 默认不接管 WebView2 下载流程，`present` 的文件点了可能没反应。
4. **托盘、窗口位置记忆** —— 未做。
5. **版本更新提示** —— 未做。注意本机是 `0.1.5-rc.1`（**预发布版**），
   不能只看 `latest` dist-tag 否则可能提示**降级**；应取 `npm view @deepseek-ai/dsh dist-tags --json`
   的所有 tag 做 semver 预发布比较。

---

## 文件

```
src-tauri/src/main.rs         全部逻辑
src-tauri/tauri.conf.json     Tauri 配置（无 IPC、无 capabilities）
src-tauri/Cargo.toml          tauri 2 + tauri-build 2
src-tauri/build.rs            tauri_build::build()
src-tauri/icons/icon.ico      32x32 32bpp BMP 格式 ICO
ui/index.html                 启动页（深色流光动画，纯 CSS/JS 无外部依赖）
```

设计上**刻意不用 Tauri IPC**：DSH UI 只跟自己的后端走 HTTP/WebSocket，所以壳就只是壳。
