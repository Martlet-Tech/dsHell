# DShell

把 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) 的 Web GUI 包成一个桌面程序：双击即用，不用挂终端、不用敲 `dsh web`、不用自己去浏览器输地址。

薄壳设计——复用你机器上已装的 Node.js 和 `@deepseek-ai/dsh`，exe 只有几 MB。

> 可用，但尚未完成。详见 [Roadmap](docs/roadmap.md)。

## 为什么需要它

直接开 `http://127.0.0.1:3080/` 会 **401**。DSH 要求先带 `?token=<每进程随机>` 访问一次换取 cookie，而 WebView 有自己的 cookie 罐，冷启动开裸地址必然被拒。

那个 token 藏在 `dsh web` 的 stdout 里。DShell 做的事就是：

1. 后台静默启动 `dsh web --port 0 --no-open`
2. 从 stdout 抓出带 token 的 URL
3. 用它打开窗口 → 换来 cookie → 真 UI 加载
4. 关窗时收掉整棵进程树

```
DShell.exe ──spawn──▶ dsh web --port 0
     │                      │
     │◀────stdout: token URL─┘
     │
     └──navigate──▶ 303 → Set-Cookie → 200 ✓
```

## 构建

需要 Rust 工具链、MSVC linker、WebView2 runtime（Win10 1803+ 通常自带）。

```powershell
cd src-tauri
cargo build --release
```

产物在 `src-tauri/target/release/dshell.exe`。

## 使用

双击运行。日志写在 `%USERPROFILE%\.dshell\dshell-poc.log`。

调试时可以设 `DSHELL_SPLASH_HOLD=1`，程序会停在启动页不跳转，方便截图或调动画。

## 文件

```
src-tauri/src/main.rs        全部逻辑
src-tauri/tauri.conf.json    Tauri 配置
ui/index.html                启动页（深色流光动画，无外部依赖）
```

刻意不使用 Tauri IPC——DSH UI 只跟自己的后端走 HTTP/WebSocket，壳就只是壳。

## 更多

- [设计决策与验证记录](docs/design.md) — 关键约束、实测数据、踩过的坑
- [Roadmap](docs/roadmap.md) — 还没做的部分
