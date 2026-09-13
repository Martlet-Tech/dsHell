# DShell

把 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) 的 Web GUI 包成一个桌面程序：双击即用，不用挂终端、不用敲 `dsh web`、不用自己去浏览器输地址。

薄壳设计——复用你机器上已装的 Node.js 和 `@deepseek-ai/dsh`，exe 只有几 MB。

> 可用，但尚未完成。详见 [Roadmap](docs/roadmap.md)。

![DShell 把 DeepSeek Harness 装进桌面窗口](docs/screenshots/main.png)

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

或者用封装好的脚本（编译到独立的 `target-build\`，不碰可能被运行中的 DShell
锁住的 `target\`；结束后停住展示结果，方便看错误）：

```powershell
.\scripts\build.ps1            # release
.\scripts\build.ps1 -Dev       # debug，快很多，只验证能不能编过
```

## 让「添加工作区」即时刷新（必做）

**光编出 exe 是不够的**——那个"选完目录要点一下鼠标才刷新"的问题，修法是把
选目录的交互交给壳层，而这一步需要往 dsh 的 profile 里装一个插件：

```powershell
.\scripts\install-picker-plugin.ps1
```

装完**必须重启 DShell**（dsh 只在启动时读一次 profile）。想恢复原状：

```powershell
.\scripts\install-picker-plugin.ps1 -Uninstall
```

原理：dsh 在 Windows 上会用**独立子进程**弹一个**没有 owner** 的原生对话框，
主窗口因此失焦，WebView2 随即挂起渲染进程（实测 11.7 秒），解冻后界面不提交
更新。插件把选目录转交给壳层，由壳层用**带 owner** 的对话框弹出，主窗口不失焦，
挂起的前提就消失了。详见 [docs/plan/04](docs/plan/04-workspace-add-no-refresh.md)。

插件是纯自足的（不 import 任何东西），所以只要把本仓库所在路径交给安装脚本即可，
不需要额外装依赖。

## 使用

双击运行。启动页会先做一次环境体检，逐项汇报：

```
① WebView2 运行环境   ② Node.js   ③ npm   ④ dsh   ⑤ dsh 配置目录可写
```

缺什么就显示什么，并且给得出路：

- **指定…**：单独指定某一项的可执行文件路径（会就地验证一次，通过才记住）
- **一键安装**：走官方通道补齐（Node 用 `winget`，dsh 用 `npm i -g @deepseek-ai/dsh`），装完自动复检
- **退出**

全部通过就自动接管：起 `dsh web` → 抓 token URL → 换 cookie → 进入真正的 DSH UI。
装完/指定过的路径记在 `%USERPROFILE%\.dshell\config.json`，日志写在 `%USERPROFILE%\.dshell\dshell-poc.log`。

调试时可以设 `DSHELL_SPLASH_HOLD=1`：跑完体检但停在报告页不接管，方便截图或调动画。

体检与安装的设计与实测记录见 [docs/closed](docs/closed/)。

![首次运行的环境体检](docs/screenshots/env-helper.png)

## 文件

```
src-tauri/src/main.rs        启动、体检编排、handoff、托盘
src-tauri/src/picker.rs      原生目录选择框服务（只监听回环 + 共享令牌）
src-tauri/tauri.conf.json    Tauri 配置
ui/index.html                启动页（深色流光动画，无外部依赖）
plugin/dshell-directory-picker/  把 dsh 的选目录转交给壳层的 dsh 插件
scripts/build.ps1            构建封装（产物在 target-build\）
scripts/install-picker-plugin.ps1  装/卸上面那个插件
```

刻意不使用 Tauri IPC——DSH UI 只跟自己的后端走 HTTP/WebSocket，壳就只是壳。

## 更多

- [设计决策与验证记录](docs/design.md) — 关键约束、实测数据、踩过的坑
- [Roadmap](docs/roadmap.md) — 还没做的部分
