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

## 设置面板：为什么是"注入 iframe"，以及两条通道（2026-09-21）

### 先更正一条被推翻的结论：注入通道没有死

`picker.rs` 的模块注释里写着（R17–R19 的结论）：

> **实测证明 `initialization_script` 与 `eval` 两条注入通道都是死的**。

**这条不成立**，2026-09-20 的覆盖层实验直接推翻了它（立项 09 的 T11）：`eval` 注入到 dsh
页面的覆盖层真的出现了，且覆盖层里的计数器 `1 → 2 → 3` 递增 —— 计数器存在 JS 全局变量里，
页面只要重载就会归零，递增只可能是"同一份文档里我们的代码连续跑了三次"。

R18 那次为什么得出相反结论：它的判据是"探针用 `sendBeacon` 回传一次都没到"，**测的其实是
回传方向，却拿来否定注入方向**。这与 05 自己的教训 M13 是同一类错误的反面 ——
*不能用一条未经验证的通道，去否定另一条通道*。旁证：启动页上 `__TAURI__` 明明存在
（整个应用依赖它），"注入通道全死"至少对启动页就不成立。

### dsh 页面允许被嵌入（实测，端口 53546）

| 检查 | 结果 |
| --- | --- |
| `Content-Security-Policy` 响应头 | ❌ 没有 |
| `X-Frame-Options` 响应头 | ❌ 没有 |
| HTML 里的 `<meta http-equiv="Content-Security-Policy">` | ❌ 没有 |

⇒ 可以往 dsh 文档里注入全屏 iframe。**这是"设置页与 dsh 共享主窗口"能成立的前提。**

### 两条通道，刻意都不用 Tauri IPC

| 方向 | 通道 |
| --- | --- |
| Rust → 面板 | `eval`（注入 shim 与推数据）→ shim `postMessage` → iframe |
| 面板 → Rust | `sendBeacon` → `127.0.0.1:<bridge>`，路径里带一次性 nonce |

不用 IPC 的理由是权限面：面板活在 **dsh 的文档**里，给它开 IPC 等于给 dsh 页面里的一切内容
开 `app_quit` / `install_missing` 这类命令。`sendBeacon` 是简单请求（无预检、无自定义头），
够用且不越权，`capabilities/default.json` 因此一行都不用改。

### 形态：全屏 iframe，不是往 dsh 文档里插裸 DOM

| | 裸 DOM | **iframe** |
| --- | --- | --- |
| 与 dsh 的样式互相影响 | 会（需命名前缀 + 高 z-index） | ❌ 不会（独立文档） |
| dsh 的全局快捷键收到面板按键 | 会（需 `stopPropagation`） | ❌ 不会（不跨 iframe 冒泡） |
| 能不能自己关闭自己 | 能 | ❌ 不能（跨源父文档碰不到）→ 由宿主 shim 代劳 |

代价换来的是两笔已知坑都不用处理 —— 而"不能自我关闭"这件事由我们自己的 shim 补齐，
不是外部限制。

## 版本台账的实测口径（2026-09-21）

| 项 | 值 |
| --- | --- |
| `npm config get registry` | `registry.npmjs.org`（09 记录的是 `registry.npmmirror.com`） |
| 已发布版本数 | 22，**全部是预发布**（`-rc.N` / `-alpha.N`），没有一个稳定版 |
| `latest` / `next` | `0.1.5-rc.2` |
| `alpha` | `0.1.6-alpha.2` |
| 本机装的 | `0.1.5-rc.1`（`where dsh` → `%APPDATA%\npm\dsh.cmd`） |

**registry 变了这件事本身就是"不许硬编码镜像地址"的证据** —— 检查用哪个 registry 必须与安装
同一个，而那个由用户 `.npmrc` 决定。代码里一律不传 `--registry`，并在面板上把它显示出来。

### 版本台账的缓存：为什么只缓存 registry 那半边

`npm view` 要联网、冷启动数秒。没有缓存时**每开一次设置面板就跑 5 条 npm 命令**
（2026-09-21 实测：一轮测试开 4 次面板 = 20 次 npm 调用），反复开关还会叠出一串并行 npm 进程。

所以拆成两半，只缓存需要联网的那半：

| 半边 | 来源 | 缓存 |
| --- | --- | --- |
| registry（版本列表 / dist-tags / registry 名） | `npm view` ×2 + `npm config get` | **2 小时磁盘缓存** |
| 本地（当前版本 / npm 全局目录 / 来源校验） | `npm ls -g` + `npm prefix -g` | **不缓存** —— 都是本地命令、秒级，而且 `current` 在切换后立刻就变了，缓存会让面板显示错的"当前版本" |

实测（2026-09-21）：冷启动 5 条 npm → 落盘 639 字节；TTL 内只剩 2 条本地命令、**零联网**；
过期（回填 3 小时时间戳）则是"先推旧数据 → 后台重查 → 再推一份覆盖"，两段都不让用户干等。

## npm 装包时的"静默一分钟"：为什么要 `--loglevel=http`

真机上从 `0.1.5-rc.2` 切到 `0.1.6-alpha.2` 要动 430+ 个包、约两分钟。这两分钟里，
**npm 默认什么都不说**：只在开头吐几条 deprecation warning，结尾吐一行
`added 58 packages, removed 90 packages, and changed 430 packages in 2m`。
面板上只剩一个不确定进度条在转，看着像卡死（用户 2026-09-21 的原话："只能看进度条在刷动画"）。

实测对照（`npm i -g @deepseek-ai/dsh@0.1.6-alpha.2 --dry-run`，同机同缓存）：

| loglevel | stderr 行数 | stdout 行数 | 内容 |
| --- | --- | --- | --- |
| `notice`（默认） | **0** | 490（`--dry-run` 专有的 `change pkg a => b` 列表；真装时没有） | 只有 warnings 与结尾汇总 |
| `http` | **564**（`cache hit` 564 / `cache miss` 0） | 490 | 逐条 `npm http cache/fetch … 12ms` |

所以安装命令加 `--loglevel=http`（外加 `--no-audit --no-fund` 省两次与安装无关的网络往返），
让面板有连续的流水可看。代价是几百行噪声 —— 面板只显示尾部 60 行，且这些行**不进日志文件**
（`installer` 的回调只推事件、不落盘），所以 `dshell-poc.log` 不会被刷爆。

顺带一条：`parse_percent` 必须**跳过含 `://` 的行**。http 流水里全是 URL，而 URL 里的
`%2B` / `%2f` 转义一旦被当百分比解析，进度条会乱跳。

## 换版本时的磁盘写突峰会把整台机器拖死（2026-09-21 实测）

一次真机事故与它的取证。用户降级 dsh 到 `0.1.5-rc.2` 时**整个系统死机**，硬复位重启
（`System` 日志 `Kernel-Power 41`，13:58:02）。日志里那次安装只有
`install dsh: spawned cmd (pid 23864)`，**没有 ok 行、也没有任何退出行** —— 进程是被硬复位带走的。

**结论：不是降级逻辑的问题**（同一次会话里，这个降级与随后的升级都成功跑完了）。
环境数据指向磁盘：

| 项 | 值 |
| --- | --- |
| C 盘 | 237 GB 总 / **12 GB 可用（95% 已用）** |
| C 盘硬件 | INTEL SSDPEKKW256G8 **SSD**（256 G NVMe，老型号） |
| **npm 缓存** | **14.93 GB / 61963 个文件**，就在 C 盘 |
| `%TEMP%` | 1.65 GB |
| dsh 安装树 | 0.55 GB |
| 近 30 天 `Kernel-Power 41` | **1 次**（就这次，不是复发性硬件故障） |
| 内存 | 48 GB（38 GB 空闲）→ 排除内存/页面文件 |

机制：换版本要一次性重装 400+ 个包（几千个文件），npm 先往已经 15 G 的缓存里写新内容。
**接近写满的 SSD 没有预留空间做 GC/磨损均衡，写入突峰会掉到很低甚至卡住**；系统盘一卡，
整机就"死机"。这也解释了为什么同样 440 个包，上一次升级没事、这一次死了 —— 满盘 SSD 的表现
本来就是忽好忽坏的。

**因此加的两样东西**（不是治本，是把"事后解释"变成"事前提醒"）：

1. `win32::free_space_bytes`（`GetDiskFreeSpaceExW`，跟 `win32.rs` 现有的手写绑定一路，
   不引 `windows-sys`），台账里带上 `disk_free_gb`。
2. 面板在切换确认框里按 `free < 20 GB` 给一句警告 + 具体出路（清缓存 / 把缓存挪到别的盘）。
   实测：日志 `catalog ok (… disk_free=11.8GB)`，与 PowerShell 报的 12 GB 一致。

**给用户的治本动作**（记在这里，因为它会再犯）：`npm cache clean --force`（回收约 15 G）；
或 `npm config set cache D:\npm-cache` 把缓存挪到 2 T 机械盘、顺便把写入负载从系统 SSD 移走。
系统盘仍在 90%+ 时，同类卡顿还会再来。


## 版本列表上的发布日期：数据早就在缓存里，只是没接到面板

面板右列原来只在"比当前更旧"的行上写"更旧"—— 那是**当前版本的镜像**，不是信息：`0.1.6-alpha.2`
下面那 24 行全是"更旧"，读者真正想知道的是"这一版什么时候发的"。改成在右列显示发布日期
（当前版本那一行仍写"当前版本"，那是第一眼要找的东西），确认框里也带上目标的发布日期 ——
降级时它是决策依据（越旧越可能踩到已有的坑）。

**这件事的成本几乎为零，因为数据早就在取**：`update.rs` 为了算降级的 `--before` 时间窗，
本来就在抓 `npm view <pkg> time --json` 并把结果存进 `%USERPROFILE%\.dshell\dsh-versions.json`
的 `times` 字段。缺的只是"把它放进 `VItem` 发给面板"这一段。

**但顺带查出一个真 bug：`times` 一旦落盘就再没人刷新。**

原来的代码只在 `times` 为空时才去查（`if !c.times.is_empty() { return Ok(c.times) }`），于是：

| 症状 | 后果 |
| --- | --- |
| 新发布的版本没有发布日期 | 而它们**恰恰是用户最想看的那几行**（2026-09-23 实测：缓存里 22 条版本、`times` 只有 20 条，正好缺 `0.1.5-rc.3` 与 `0.1.7-alpha.1`） |
| `resolve_before` 查不到目标版本的时刻 | **静默放弃时间窗** → 降级退回"朴素装旧版"，也就是会把 dsh 装坏的那条路 |

判据不能写成"`times` 条目数少于 `versions` 就重抓"：registry 若对某一版真的不给时刻，
那就是**每次开面板都后台重查**的永久空转。所以记的是**抓 `times` 那一刻的版本数**
（`times_seen`，`#[serde(default)]` = 0）：新版本一出现 ⇒ `times_seen < versions.len()`
⇒ 重抓一次 ⇒ 再次相等。老缓存因为缺这个字段被判为落后一次，**自愈**地补上，正是想要的。

## 注入 shim 的 `atob` 出乱码：一个一直存在、直到第一批成段中文才暴露的缺陷

用户 2026-09-23 报"更新内容能看了，但**乱码**"：面板上显示 `ä½?éª?ä¼?å?` 这类字符。

**根因**：注入 shim 里写的是裸 `atob(b64)`。`atob` 给的是 **Latin-1**（一个字符一个字节），
而 Rust 侧 `b64()` 编出去的是 **UTF-8 字节**。`体验优化` 的 UTF-8 是
`E4 BD 93 E9 AA 8C E4 BC 98 E5 8C 96`，按 Latin-1 读回来就是 `ä½?éª?ä¼?å?` —— 与截图完全吻合。

| 位置 | 修前 | 修后 |
| --- | --- | --- |
| shim `push()` | `atob(b64)` | `dec(b64)` |
| shim `status()` | `atob(b64)` | `dec(b64)` |
| shim 的 `__URL__` / `__NONCE__` | `atob(...)` | `dec(...)` |

`dec()` 就是 `ui/js/transport.js` 从第一天起就在用的那个解码（`atob` → `Uint8Array` →
`TextDecoder("utf-8")`）。**启动页那条通道一直是对的，只有注入的 shim 漏了这一步。**

### 为什么它藏了这么久

推过去的字段以前**几乎全是 ASCII**：版本号、日期、`registry` URL、`npm prefix`、
`source_note` 多数时候是空。ASCII 在 Latin-1 与 UTF-8 下**逐字节等价**，所以走哪条路都对。
更新内容是**第一批成段中文**，一下就炸了 —— 而 `source_note`（中文）只在来源校验失败时
才出现，那是个罕见分支，从没被看见过。

### 为什么我上一轮的验证没抓到它

因为**预览夹具绕过了 shim**：它直接用 `window.postMessage` 把 JSON 喂给面板，
而乱码恰好发生在 shim 的解码那一步。夹具跳过了出问题的那一层，于是全程绿灯。
这是"用替身验证替身"的典型教训 —— 与 05 的 M13（*不能用一条未经验证的通道去否定另一条*）同类。

**修法不只是改代码，也改了验证方式**：现在的端到端测试从 `settings.rs` 里**抠出真的
SHIM 源码**、填上占位符、让页面真执行它，再用 shim 自己的 `push()` 推含中文的数据 ——
链路上每一环都是产品代码。实测：修前形状 `ä½?éª?ä¼?å?`、修后 `体验优化`，
并且断言渲染结果里**不许出现 Latin-1 乱码特征字符**。

## 更新内容只能从 GitHub Releases 拿，npm 里没有

用户问"拉版本信息的时候有没有更新内容"。**npm 侧一点都没有**（2026-09-23 实测）：

| 来源 | 结果 |
| --- | --- |
| 单版本 manifest 的 `changelog` / `changes` / `releaseNotes` | **0/25 个版本有** |
| 单版本 `gitHead`（拿它也还得自己去比对 commit） | **0/25** |
| packument 顶层 | 只有 `readme`（包说明，不是更新日志） |
| 仓库里的 `CHANGELOG.md` | **不存在**（`/`、`/docs/`、`/apps/cli/` 三个路径全 404） |
| 已安装包内 | 只有第三方依赖自己的 CHANGELOG |
| **GitHub Releases** | ✅ **20 条，中英双语**（新增功能 / 问题修复 / 其他变更） |

覆盖 18/25 个已发布版本；缺的 7 个是 6 个远古版（`0.0.1-rc.*` / `0.1.0-rc.*`）加
`0.1.5-rc.3`（发了 npm 但没建 release）。**拉不到不是错误**，面板照常显示列表。

### 用 node 去拉，不引 HTTP 依赖，也不用 curl

`release_notes.rs` 通过 `node -e fetch(...)` 发这一次 GET。理由与 `update::midpoint` 同源：
**node 从来就是这个应用的硬依赖**（npm 与 dsh 都跑在它上面），而为一个 GET 引
`ureq` / `reqwest` 是一笔真实的依赖与编译代价。

`curl` **实测也不行**：本机 schannel 直接 `SEC_E_NO_CREDENTIALS`，不是"能不能带参数"的问题。

| 实测（2026-09-23 本机） | 值 |
| --- | --- |
| `node -e fetch` 拉 100 条 | **1117 ms** / 140 KB |
| 匿名限流 | **60/h**（`x-ratelimit-remaining: 56`） |
| 面板实际频率 | 2 小时缓存 + **只在用户点开某一版时**才查 → 绰绰有余，无需 token |

### 懒加载：不进 `catalog()`

更新内容**不进**首屏那次查询。面板首屏仍只跑本地命令 + 缓存的 registry；
用户点某一行的日期才去拉一次全量（一条请求拿回全部 20 条），之后点任何版本都是瞬开。

### 正文渲染：绝不让外部文本进 DOM 解析

release 正文是**未受信的外部文本**，而面板手里握着 `switch?version=` 那条回传通道
（能真的让 dsh 换版本）。所以：

| 层 | 做法 |
| --- | --- |
| `release_notes::extract_cn` | 切出中文段 |
| `release_notes::normalize_markup` | 压成**纯文本 + 三种记号**：`### ` 标题、`- ` 列表、其余段落；**所有尖括号清零** |
| `ui/js/settings.js` | 只按行 `textContent` 建文本节点，**从不 `innerHTML`** |

面板侧那条约定由 Rust 的 `markup_is_flattened_to_a_tiny_markdown_subset` 守着：
它断言抽取结果里**一个 `<` 都不许剩**。

### 抽取规则是**实测**定的，不是猜的

这批 release 的排版换过三次，三种都在单测里：

| 形状 | 例 |
| --- | --- |
| `<h3 id="cn-vX">` … `<h3 id="en-vX">` | 18/20 条 |
| `<h2 id="chinese">X · 中文</h2>` … `<h2 id="english">` | `0.1.3-alpha.2` |
| `<h3 id="cn">` … `<h3 id="en">` | `0.1.0-rc.7` |

所以判据是**锚点 id 的前缀**（`cn`/`chinese`/`en`/`english`），不是某个固定字符串。

**全量核对（20 条真正文）揪出一个只有 1 条命中的坑**：`0.1.5-rc.1` 的**中文段内部**
自己带一行 `Full Changelog`，其后是一段英文散文，然后才是英文锚点。只按"英文锚点"
切除的话，那段英文的 `### Bug Fixes` 会跟着切进中文段。修法是段内也按 `Full Changelog`
切一刀 —— 而这条**我最早手挑的 3 个夹具全都没覆盖**（挑的是典型形状，恰好漏掉唯一那条）。
现已固化成 `truncates_at_full_changelog_inside_the_chinese_section`。

`absent` 集合：缓存里"没有这一版"必须区分**问过没有**与**还没问**。不区分的话，
每次点一个没有 release 的旧版都要重拉一次；反过来一律信"缓存里没有就是没有"，
则**刚发布的版本**会被判成"没有说明"直到 TTL 过期（最长 2 小时）—— 而那恰恰是
用户最想看的一版。

## 降级为什么要带"发布时间窗"（npm `--before`）

dsh 对同级的 `@deepseek-ai/*` 包用 `^` 范围引用，所以**朴素地装旧版会配到新版依赖**：

```
npm i -g @deepseek-ai/dsh@0.1.6-alpha.1     # 报 ok=true code=0，装出来却是混合树
→ dsh web 启动即退出：SyntaxError: The requested module '@deepseek-ai/dsh-app-boot'
  does not provide an export named 'watchUserPatches'
```

（alpha.1 的 core 需要这个符号；npm 按 `^0.1.6-alpha.1` 取了最大值 alpha.2，而 alpha.2 把它删了。）

解法是 `--before`：把 npm 的解析视野拉回"那个版本发布时"，装出**一致的旧树**。已实测：
装出 `dsh-app-boot` alpha.1（符号在）→ `dsh web` 打印 token URL → 带 cookie 请求 200
+ `<title>DeepSeek Harness</title>`。

**取哪个时刻是关键**：不能取目标版本自己的发布时刻。这批包是**错峰发布**的：

| 版本 | dsh 自己 | 同级包 | 结论 |
| --- | --- | --- | --- |
| `0.1.6-alpha.1` | `09-15T03:23Z` | `03:09Z` … **`04:17Z`** | 同一批比 dsh 自己还晚 54 分钟 → 取自身时刻会 `ETARGET` |
| `0.1.6-alpha.2` | `09-17T13:52Z` | `13:38Z` 起 | 与上一波隔了两天 |

所以取**目标与下一个版本发布时间的中点**（本次 ≈ `09-16T08:37Z`），它落在两个发布波之间。
实现上这个减法**交给 node 算**（`node -e "new Date((Date.parse(a)+Date.parse(b))/2)"`）：
npm 本身是 node，两边对日期字符串的理解天然一致；为一个减法引 `time`/`chrono` 不值当，
手写历法换算更是这类项目里最典型的坑。目标就是**最新那一版**时不存在"下一个版本"，自然不需要窗口。

**已知局限**：这是启发式（假定同一批发布在同一窗口内完成）。真撞上 `ETARGET` 时，
`installer` 会把 npm 的原始错误翻译成"该版本发布时同批的包还没发全"，
并保留原始输出 —— 失败卡片上仍有「装回上一个版本」的出口。



