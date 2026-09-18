# 06 · 启动后停在 `Failed to load plugins`（WebView2 数据目录坏状态）

> **立项日期**：2026-09-18
> **状态**：**根因已定位，修法已实测有效**（清 WebView2 数据目录）。壳层自愈**暂不做**，本项未收口。
> **现象提出**：用户实测（每次启动必现）
> **对应 roadmap**：`roadmap.md`（条目待补）
> **红线**：不碰 dsh 源码（同 04 §6）

---

## 0. 根因

**DShell 的 WebView2 数据目录（`%LOCALAPPDATA%\com.dshell.poc\EBWebView`）处于坏状态，
导致 dsh 那个 11 MB 的插件组合脚本写不进 HTTP 缓存，`<script>` 触发 error 事件。**

### 0.1 现象

启动页停在：

```
Failed to load plugins
failed to import loader entry 08c80e29 (@deepseek-ai/dsh-client-hmr):
client-modules: bundle script /plugins/??<52 个插件>/client.js&rev=f90d8e180337 failed to load
```

**每次必现**，不是偶发。同一时间用系统 Edge 打开同一个 token URL **完全正常**。

### 0.2 因果链

```
dsh 把 52 个 client bundle 拼成一个脚本（11,091,255 字节）
  响应头 Cache-Control: public, max-age=31536000, immutable  → WebView2 必须落缓存
    → 缓存目录坏状态，写不进去
      → <script> 触发 error 事件（不是 404，也不是 dsh 后端失败）
        → client-modules 抛 "bundle script … failed to load"
          → cordis loader 抛 "failed to import loader entry …"
            → boot.ts 的 catch → 启动页 "Failed to load plugins"
```

### 0.3 为什么每次必现

坏的是**持久化在磁盘上的缓存状态**，不是内存里的瞬时错误——每次启动都撞同一块。
小的 bootstrap 批次（`@deepseek-ai/dsh-client-modules`，几十 KB）仍能通过，
所以模块系统起得来、还能把失败画到页面上。

### 0.4 为什么只点名 `dsh-client-hmr`

它声明了 `immediately: true`，boot 时第一个被 prefetch，**由它第一个去拉这份共享脚本**；
`Promise.all` 遇到第一个 rejection 就整体抛出，页面只显示它一行。
**不是它坏了**——这份脚本一挂，同批 52 个全都起不来。

### 0.5 关键实测数据（2026-09-18，本机）

| 事实 | 数据 |
| --- | --- |
| 组合脚本体积 | **11,091,255 字节** |
| 组合脚本 URL | 52 条目、2514 字节、`rev=f90d8e180337` |
| 服务端响应 | `200`、`Transfer-Encoding: chunked`、**无** `Content-Encoding`、约 272 ms |
| 缓存目录（坏） | 813 文件 / **310.0 MB** |
| 缓存目录（改名后重建） | 193 文件 / **20.5 MB**，界面**立即恢复正常** |
| 改名后连跑多次 | 均正常进入界面 |
| WebView2 Runtime vs Edge | **同为 153.0.4234.32**（同引擎，排除版本差） |
| 磁盘剩余 | C 盘 442 GB 空闲（排除空间不足） |
| 环境 | dsh `0.1.5-rc.1`；DShell `target-build\release\dshell.exe`（09-18 00:43 编） |

---

## 1. 排查过程：被否证的假设

按时间顺序，含两条我自己先给错、后被实测推翻的结论（留档，避免重蹈）。

| # | 假设 | 否证依据 |
| --- | --- | --- |
| H1 | 缺某个插件 | 52 个包的 `package.json` 与 `exports["./client"]` 全部存在，0 缺失；真缺包 dsh 启动时会抛 `client bundles not found`，连 URL 都打不出来 |
| H2 | 服务端 404 / URL 漂移（插件图重排 ≥2 代） | 30 秒内 40 次采样：条目恒为 52、`rev` 恒为 `f90d8e180337`、HEAD 恒为 200 |
| H3 | 压缩或请求头差异（Chromium 的 `Accept-Encoding`） | 用浏览器头 GET：200 / 11,091,255 字节 / 无 `Content-Encoding` |
| H4 | 引擎版本差异 | WebView2 Runtime 与 Edge 同为 153.0.4234.32 |
| H5 | DShell 那组 `--disable-*` 参数 | A/B 实测：**不带参数能进、带参数也能进** |
| H6 | 窗口失焦导致渲染进程冻结（**我先后给过两次，均被推翻**） | 失败那一轮日志只有 2ms / 3ms 的极短 blur，都被 `brief blur gap (ignored)` 忽略，未触发 `freeze detected` |
| H7 | 偶发中断 | 用户反馈"每次都进不去"，与偶发矛盾 |
| H8 | profile 配置被改坏 | `cordis.yml`、`cordis.patch.yml` 均为空的原始状态；`.dsh-module-fallback` 是空壳 |
| H9 | DShell 自带插件（`dshell-directory-picker`）导致 | 它没有 `dsh.client` 声明、无 `./client` 导出，不产生任何浏览器模块，不在 52 个条目里 |
| — | **H10 数据目录坏状态** | **成立**：改名后立刻恢复 |

### 1.1 教训

1. **"能手动访问 ≠ 同一条路径"**：PowerShell / Edge 用的是**别的**用户数据目录，
   它们能加载，证明不了 DShell 也能。这让我在 H1–H4 上耗了很久。
2. **先量再猜**：40 次采样把"服务端漂移"彻底排除后，嫌疑面才收敛到壳层自己的状态。
3. **"每次必现"优先找持久化状态**，不要往时序/竞态上想。

---

## 2. ① 方案

### 2.1 当前方案（已采纳，已落地）

新增脚本 `scripts/clean-webview-cache.ps1`：

- 默认**改名**而非删除（`EBWebView.bak-<时间戳>`，可还原）；
- 先做进程体检：有 `dshell.exe` 在跑就只报告，要真清得显式加 `-KillProcesses`；
- `-PurgeBackups` 清历史备份；支持 `-WhatIf`。

用法：

```powershell
.\scripts\clean-webview-cache.ps1                 # 改名备份
.\scripts\clean-webview-cache.ps1 -PurgeBackups   # 连备份一起删
```

### 2.2 待定方案（**本次不做**，留档）

| 方案 | 做法 | 评价 |
| --- | --- | --- |
| **S1 壳层自愈** | `main.rs` 导航后起看门狗：检测到页面处于失败态 → 结束/清理 WebView2 数据目录 → 重导航一次（上限 2 次） | 一劳永逸，但要动壳层、要处理"目录被占用时如何清"，且失败态探测依赖页面文本 |
| **S2 纳入日常清理** | 把 `EBWebView` 加进 `clean-logs.ps1` 的清理范围 | 一行改动，仍需手动 |
| **S3 只加"失败重导航"**（不清缓存） | 失败就重导航一次 | **对本根因无效**：缓存是坏的，重来几次都一样 |

⇒ 若日后复发频繁，做 **S1**；若只是偶发，保持 2.1 即可。

---

## 3. ② 关键代码位置（本次未改）

| 位置 | 作用 |
| --- | --- |
| `...\dsh-client-modules\lib\index.js` `comboUrl()` | 生成 `/plugins/??<id>/client.js,…&rev=<12 位内容哈希>` |
| 同上 `MAX_COMBO_URL_BYTES = 3*1024`、`partitionComboRecords()` | URL 长度上限（我们的 URL 2514 字节，未超限） |
| 同上 `bundleResource()` | **只认「当前一代 + 上一代」URL**（`previousBatchResponses`），否则 **404** |
| 同上 `IMMUTABLE_CACHE` | `public, max-age=31536000, immutable` ← 必须落缓存的根源 |
| dsh 客户端 `client/modules/src/client/system.ts` `defaultLoadBundle()` | `<script>` 的 error 事件 → `bundle script … failed to load` |
| dsh 客户端 `client/web/src/boot.ts` `run()` 的 catch | 整条错误丢给启动页 → `Failed to load plugins` |
| 壳层 `src-tauri/src/main.rs:227-229` | `w.navigate(parsed)` 之后**没有任何事后检查**（S1 的落点） |
| 壳层 `src-tauri/src/main.rs:590-596` | 那组 `--disable-*`（A/B 实测无罪） |
| `scripts/clean-webview-cache.ps1` | 本次新增：清理脚本 |

---

## 4. ③ 结案总结

**未收口**。已完成的与遗留的：

| 项 | 状态 |
| --- | --- |
| 根因定位 | ✅ WebView2 数据目录坏状态 |
| 修法验证 | ✅ 改名重建后界面恢复正常（20.5 MB） |
| 清理脚本 | ✅ `scripts/clean-webview-cache.ps1`（已 `-WhatIf` 冒烟） |
| 旧目录清理 | ✅ 310 MB 备份已删除 |
| 壳层自愈（S1） | ⏳ **本次明确不做** |
| `roadmap.md` 条目 | ⏳ 待补 |
| 缓存为什么会坏 | ❓ **未深挖**：是某个缓存项损坏、还是索引/容量问题，需翻 Chromium 缓存内部结构才能定性 |
| 是否应让 dsh 拆小组合包 | ❓ 属于 dsh 侧，**不碰**（红线） |

### 4.1 复发时的判定与处置

1. 现象是"每次都停在 Failed to load plugins"、而系统浏览器能开 → 直接判定本根因；
2. 完全退出 DShell → `.\scripts\clean-webview-cache.ps1` → 重启；
3. 若清完仍失败，才回到 H2/H5/H6 那条线重查（届时先看 `%USERPROFILE%\.dshell\dshell-poc.log`）。
