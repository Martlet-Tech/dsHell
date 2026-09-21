# 10 · 陈旧 auth cookie 撑爆请求头（HTTP 431）

> **状态**：**已收口**（2026-09-21）。真机四次连续启动验证通过，见 ③。
> **红线**：不碰 dsh 源码。
> **立项日期**：2026-09-21 · **对应 roadmap**：原第 5 条（已随收口删除）

---

## ① 方案

### 要解决什么

DShell 用 `dsh web --port 0`，每次启动系统给**新随机端口**。dsh 的浏览器会话 cookie 名是
`dsh-auth-` + base64url(sha256(authority))，而 authority 是带端口的 Host（`127.0.0.1:64618`）。
于是**每次启动都 mint 一个全新名字的 cookie**，`Max-Age`（`cookieMaxAgeDays`，默认 **30 天**）
内一条都不失效。

浏览器侧又只按 host 存 cookie（Domain 不含端口），这 72 条在它眼里互不相干，**一条都不覆盖一条**。
最终所有 cookie 一起进 `Cookie` 请求头；dsh 的 Node `http` 服务器 `maxHeaderSize` 硬编码
**16384 字节**，一越线就在进入鉴权代码**之前**回 **431**，界面永远进不去。

### 实测数据（2026-09-21，本机）

| 项 | 数据 |
| --- | --- |
| cookie 条数 | **72**（全部 `host_key=127.0.0.1`、全部 `dsh-auth-*`） |
| 单条序列化长度 | **236 字节**（`encrypted_value` 字符长度） |
| Cookie 头合计 | 72 × 236 + 分隔 ≈ **17,568 字节** |
| 431 阈值 | **16,384 字节**（Node `http.maxHeaderSize`；实测 `node v24.16.0`） |
| 超线幅度 | 约 **1.2 KB** |
| 服务端复现 | 对真实 dsh 发 `Cookie: <20KB>` → **431**；`<8KB>` → 303；裸 URL → 303 |
| 反证 | 系统浏览器不吃这 72 条，Cookie 头很小 → 能正常打开（431 是**服务端**按请求头大小判的） |

**推论**：约 **68 次启动**触线。一天调试十几次，约一周复发。这是慢性病，不是一次性故障。

### 关键判据：清理由不必等到知道端口

`cookieName(authority)` 只依赖 authority，纯计算不需要 secret。但最终实现**没用**它 ——
把清理放在拿到 URL **之前**后，所有现存 `dsh-auth-*` 都已确定作废，"哪个是本代的"这个问题
根本不用回答（详见下节「为什么清理要早于拿到 URL」）。

### 三条修法（待用户拍板）

| 方案 | 做法 | 评价 |
| --- | --- | --- |
| **A. 壳层启动自净**（推荐） | navigate 前用 WebView2 CookieManager 删掉 `dsh-auth-*` 里非本代端口的 cookie | 不动 dsh、不碰端口策略；删旧 cookie 不丢登录（dsh 凭据在 `~/.dsh/.credentials.yaml`） |
| B. 固定端口 | 每次用同一端口 → 同名 cookie 覆盖，永远 1 条 | 一行改动治本，但**与 `--port 0` 互斥**：重新引入端口冲突（多实例、TIME_WAIT、他程序占用），且与「可配置端口」是同一件事 |
| C. 缩短 `cookieMaxAgeDays` | 让 cookie 快速过期 | 要往 dsh profile 写配置，动用户环境且有过期副作用，不推荐 |

### 本期落地范围（用户 2026-09-21 指定）

1. 本 plan 文档 + roadmap 条目；
2. `scripts/clean-stale-auth-cookie.ps1`：手动清理陈旧 cookie（**选择器 = 保留端口**）；
3. **方案 A 壳层自净**：导航前删掉非本代 authority 的 `dsh-auth-*`；
4. **端口策略本期不改**：B（固定端口）与「可配置端口」是同一件事，写进 §4 遗留项。

### 方案 A 的实现要点

| 要点 | 说明 |
| --- | --- |
| 选择器 | 前缀 `dsh-auth-`，**全删、不区分端口** |
| 枚举 | `WebviewWindow::cookies()` —— wry 传 `PCWSTR::null()` 给 WebView2 `GetCookies`，**返回整个 cookie 罐**（不只当前页），所以启动页是 `tauri.localhost` 也能看到 `127.0.0.1` 的 cookie |
| 删除 | `delete_cookie` 按 **name/domain/path** 匹配、值无关，这三个都从 cookie 自己身上取 |
| 失败处置 | 任何错误只记日志，绝不拦启动 |
| 超时 | `cookies()` 是同步阻塞调用（wry：发消息给 webview 线程 + 等回调），清理线程用 channel 收尾，主流程 `recv_timeout(2s)`；超时照常导航 |
| 挂载点 | **spawn dsh 之后、`wait_for_url` 之前**。见下方「为什么清理要早于拿到 URL」 |

### 为什么清理要早于拿到 URL（而非接在 `navigate` 前）

原先设想是"导航前清"，后来改成更早，理由有两条：

1. **不押在 `wait_for_url` 身上**：那段里有 `w.show()`（同样是「同步调 webview 线程」），
   万一卡住，接在它后面的清理根本到不了，超时守卫也守不住它前头的东西。放到它前面，
   清理自身才是被守卫的那一段。
2. **此时断言更干净**：新端口刚定 → 上一代 cookie 已确定作废 → 本代还没 mint →
   **所有现存 `dsh-auth-*` 都是废的**，于是不需要算本代 cookie 名（省掉 sha256/base64url
   与全部兜底分支），选择器退化成纯前缀匹配。窗口此时还停在本地启动页，没有任何请求会
   撞上 431，从容清即可；清理与 dsh 启动并行，不占启动时间。

> 代价：本代那条也被删，导航时多一次 303 无 cookie 鉴权。属于必然开销（端口每次都新，
> 本代 cookie 本来就不存在），不是额外损失。

---

## ② 关键代码

### cookie 命名与生命周期（dsh 侧，只读参考）

| 位置 | 作用 |
| --- | --- |
| `dsh-client-connection/lib/index.js` `cookieName()` | `"dsh-auth-" + base64url(sha256(authority))`，authority 带端口 |
| 同上 `requestAuthority()` | 取 Host 头，端口保留 |
| 同上 `sessionCookie()` | `Max-Age=<days*86400>; Path=/; HttpOnly; SameSite=Strict` |
| 同上 `Config.cookieMaxAgeDays` | `z.natural().min(1).default(30)` |

### 新增脚本

`scripts/clean-stale-auth-cookie.ps1` —— 保留端口可显式给 `-KeepPort`，不给就取日志里最后一次
`dsh web:` 的端口；删除该 host 下所有 `dsh-auth-*` 中**非保留端口**的 cookie，其余 cookie 不动。

> 它保留了"精确到端口"的能力（脚本是离线手动工具，多留一条无妨），与壳层那套"全删"不同。
> 脚本内嵌 Node 只是为了**免维护一份 base64url+sha256 实现**。

### 壳层落点

| 位置 | 作用 |
| --- | --- |
| `src-tauri/src/auth_cookie.rs` | **新增**：`purge_stale()`（前缀匹配 + 删除，无加密依赖） |
| `src-tauri/src/main.rs` handoff，spawn dsh 之后 | **新增**：清理线程（与 dsh 启动并行） |
| `src-tauri/src/main.rs` handoff，`navigate` 之前 | **新增**：`recv_timeout(2s)` 收尾，超时照常导航 |
| `src-tauri/src/main.rs` `spawn_dsh()` 里的 `"--port", "0"` | 端口策略的选择点（方案 B 的落点，本期未动） |
| `src-tauri/Cargo.toml` | **无新增依赖** —— 首版用过 `sha2`/`cookie` 算本代 cookie 名，简化后已移除，`Cargo.lock` 回到基线 |

---

## ③ 结案总结

**已收口**（2026-09-21）。真机四次连续启动验证通过。

| 项 | 状态 |
| --- | --- |
| 根因定位 | ✅ 端口随机化 → cookie 名分化 → 30 天不失效 → 累计越 16 KB |
| 定量验证 | ✅ 清理前 72 条 × 236 字节 ≈ 17.5 KB；服务端 20 KB→431 / 8 KB→303 复现 |
| 应急恢复 | ✅ `clean-webview-cache.ps1` 改名 `EBWebView`（实测界面恢复） |
| 手动清理脚本 | ✅ 核心逻辑在备份库上验过（反解端口、保留一条删其余）；真机被占用场景未跑 |
| 壳层自净（方案 A） | ✅ `cargo check` + 6 项单测全绿，**无新增依赖** |
| 方案 A 真机端到端 | ✅ 见下方实测记录 |
| 端口策略（方案 B / 可配置端口） | ⏳ 未做，见 §4.1 |

### 3.1 实测记录（2026-09-21，本机）

四次连续「启动 → 完全退出」后，日志呈稳定形态（`%USERPROFILE%\.dshell\dshell-poc.log`）：

```
spawned dsh web (pid 27636, generation 1)
auth cookie: purged 1 stale
dsh> dsh web: http://127.0.0.1:60075/?token=...
window navigated to dsh
```

| 观测 | 结论 |
| --- | --- |
| 每轮都恰好 `purged 1`，**不递增** | 既证明"进一条清一条"（不累积），也证明 delete 真的生效 |
| 清理行**早于** `dsh web:` 行（同一毫秒级窗口） | 位置正确：spawn 之后、等 URL 之前；与 dsh 启动并行 |
| 四次都 `window navigated to dsh`，无 431 | 用户可见故障消除 |
| 稳态 1 条 = 236 字节 | 距 16 KB 上限两个数量级，不再有触发面 |

> 为什么 `purged N` 这个序列本身就是自证：若 delete 实际失败，那条 cookie 会留下，
> 下一轮就会报 `purged <新那条>` 而库里留 2 条，再下一轮报 `2` —— 形成递增。
> 观测停在 `1`，把"删除有效"和"不累积"一次证完，不必再开库数。

### 3.2 一个会静默炸的 bug（实现中发现并修掉）

首版把日志写成 `format!("… ({p}: {pid})")` —— `p` 是 `u16`，格式串里没有 `p` 参数，
**运行时必 panic**。debug 会炸、release 因 `panic = "abort"` 直接静默退出。
重构时该段整体删除。

### 4. 遗留项

1. **方案 B / 可配置端口**：给一个「指定端口」设置项，默认仍 `--port 0`。需处理端口被占时的
   回退。**与本次修复解耦** —— 它针对的是"将来要做 LAN 访问"这类真实需求，
   而非 cookie 累积（loopback 不经防火墙，见 §4.3）。
2. **第二实例路径不清**：双击开第二份走的是"切到已有窗口"，不跑 handoff，因此不清。
   该路径也不 mint 新 cookie，对 431 无影响，故不做。
3. **背景澄清**：Windows 防火墙不拦 loopback，`--port 0` 的随机端口在纯本地场景下
   与防火墙无关 —— 这条排除了"随机端口才导致问题"的误判方向。
4. 清理后不必重启 DShell 之外的东西；但目录被 WebView2 锁着，手动脚本默认要求先完全退出。
5. `scripts/*.ps1` 是 **UTF-8 无 BOM**，Windows PowerShell 5.1 按 GBK 读会解析失败 —— 用 **PowerShell 7（`pwsh`）** 跑。
