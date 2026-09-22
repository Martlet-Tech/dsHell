//! dsh 各版本的**发布日期说明**（更新内容）—— 数据来自 GitHub Releases。
//!
//! ## 为什么不是 npm
//!
//! 2026-09-23 实测：`@deepseek-ai/dsh` 的 packument 里**没有**任何更新内容可用。
//!
//! | 来源 | 结果 |
//! | --- | --- |
//! | 单版本 manifest 的 `changelog` / `changes` / `releaseNotes` 字段 | 25 个版本**一个都没有** |
//! | 单版本 `gitHead` | 0/25 |
//! | packument 顶层 | 只有 `readme`（那是包说明，不是更新日志） |
//! | 仓库里的 `CHANGELOG.md` | 不存在（`/`、`/docs/`、`/apps/cli/` 三个路径全 404） |
//! | 已安装包内 | 只有第三方依赖自己的 CHANGELOG |
//! | **GitHub Releases** | ✅ 20 条，**中英双语**，含「新增功能 / 问题修复 / 其他变更」 |
//!
//! 覆盖 18/25 个已发布版本。缺的 7 个：6 个远古版（`0.0.1-rc.*`、`0.1.0-rc.*`）加
//! `0.1.5-rc.3`（当天发的，没建 release）。**拉不到不是错误**，面板只显示"没有"。
//!
//! ## 为什么用 node 去拉，而不是 Rust 自己发 HTTP
//!
//! 与 `update.rs` 的 `midpoint()` 同因：**node 本来就是这个应用的硬依赖**
//! （`npm` 跑在它上面，`dsh` 也是），而为一个 GET 请求引 `ureq` / `reqwest` 是
//! 一笔真实的依赖与编译代价。实测（2026-09-23 本机）`curl` 也不行：schannel 直接
//! `SEC_E_NO_CREDENTIALS`，而 `node -e fetch` 1117 ms 拿到 140 KB。
//!
//! 同一次实测：**匿名**限流 60/h（够用 —— 这份结果有 2 小时磁盘缓存，
//! 且只在用户点开某一版的更新内容时才去查），无需 token。
//!
//! ## 懒加载：只在用户点的时候才拉
//!
//! 不进 `catalog()`。面板首屏一次 npm 调用都不该多，而更新内容是**按需**的：
//! 用户点某一版的日期 → `notes(version)` → 有缓存就直接给，没有就拉一次全量
//! （一条请求拿回全部 20 条）→ 之后点任何版本都是瞬开。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::proc;

/// `dsh` 的 `package.json` 里 `repository.url` 指向这里（见其 npm manifest）。
const RELEASES_API: &str =
    "https://api.github.com/repos/deepseek-ai/deepseek-harness/releases?per_page=100";

/// tag 前缀：`dsh-v0.1.7-alpha.1` → `0.1.7-alpha.1`。
const TAG_PREFIX: &str = "dsh-v";

/// 拉取超时。实测 ~1.1 s，给足余量；超时算可读的失败，不影响面板其它部分。
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// 磁盘缓存有效期。与版本台账同一个 2 小时口径：发布说明变得比版本列表还慢。
const TTL_MS: u64 = 2 * 60 * 60 * 1000;

const CACHE_FILE: &str = "dsh-releases.json";

/// 一条发布说明。
#[derive(Clone, Debug, Serialize, Deserialize)]
struct RawNote {
    /// release 的网页地址（面板上作为**可复制的文本**给出，不做导航）
    url: String,
    /// GitHub 给的**原始正文**（Markdown + 内联 HTML、中英双语）
    body: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct NotesCache {
    /// `serde(default)` 是**策略**不是便利：缓存文件缺字段（手工删过、将来加了新字段
    /// 而用户从旧版升上来）必须还能读，否则"缓存坏了"就成了新的故障点 —— 与
    /// `update::RegistryCache` 同一处置。
    #[serde(default)]
    fetched_at_ms: u64,
    /// 版本 → 原始正文。存原始而不是存抽好的中文段：抽取规则将来改了
    /// （GitHub 的排版确实换过两次），不必为此重拉一次网络。
    #[serde(default)]
    notes: BTreeMap<String, RawNote>,
    /// **问过、并确认 GitHub 上没有对应 release** 的版本。
    ///
    /// 为什么必须单独记一笔：`notes` 里"没有"和"还没查"是同一个样子。若不区分，
    /// 每次点一个没有 release 的旧版（实测 7 个）都要重拉一次网络；反过来若一律
    /// 信任"缓存里没有就是没有"，那**刚发布的版本**会被判成"没有说明"直到 TTL 过期
    /// （最长 2 小时），而那恰恰是用户最想看的一版。
    #[serde(default)]
    absent: BTreeSet<String>,
}

impl NotesCache {
    fn age_ms(&self) -> u64 {
        now_ms().saturating_sub(self.fetched_at_ms)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 面板要的成品：某个版本的发布说明。
#[derive(Clone, Serialize)]
pub struct Note {
    pub version: String,
    /// **中文段**原文（已切掉英文段与末尾的 Full Changelog 行）。
    /// 面板负责渲染 —— 它是未受信内容，见 `ui/js/settings.js` 里的渲染约定。
    pub body: String,
    /// release 地址
    pub url: String,
    /// 这条说明来自哪份缓存：`true` = 命中磁盘缓存，没有联网
    pub cached: bool,
}

/// 取某个版本的更新内容。
///
/// * 缓存新鲜 → 直接给（**零网络**）
/// * 缓存过期或这个版本不在里面 → 拉一次全量并落盘
/// * 拉不到 → `Err`，面板显示"拉取失败"，**不影响列表本身**
///
/// 版本在 registry 里存在但 GitHub 上没有对应 release（实测 7 个），返回
/// `Ok(None)` —— 那是"这一版没有说明"，不是错误。
pub fn notes(version: &str) -> Result<Option<Note>, String> {
    if let Some(c) = load_cache() {
        if c.age_ms() < TTL_MS {
            if let Some(n) = c.notes.get(version) {
                return Ok(Some(build(version, n, true)));
            }
            // 新鲜 + **确认过**这版没有 release ⇒ 不必联网再试
            if c.absent.contains(version) {
                return Ok(None);
            }
            // 新鲜但**没问过**这一版：可能是缓存落盘之后才发布的新版本，
            // 不能凭"缓存里没有"就判它没有说明 —— 去拉一次。
        }
    }

    let fresh = fetch_releases()?;
    let hit = fresh.get(version).cloned();
    // 顺手把"这一批里没有的版本"记进 absent：下次点它们就不再联网
    let absent = {
        let mut prev = load_cache().map(|c| c.absent).unwrap_or_default();
        if hit.is_none() {
            prev.insert(version.to_string());
        }
        prev
    };
    save_cache(&NotesCache {
        fetched_at_ms: now_ms(),
        notes: fresh,
        absent,
    });
    Ok(hit.map(|n| build(version, &n, false)))
}

fn build(version: &str, raw: &RawNote, cached: bool) -> Note {
    Note {
        version: version.to_string(),
        body: extract_cn(&raw.body),
        url: raw.url.clone(),
        cached,
    }
}

fn cache_path() -> PathBuf {
    Config::dir().join(CACHE_FILE)
}

/// 读缓存。**任何异常都当作"没有缓存"**（与 `update::load_cache` 同理：
/// 缓存坏了不能变成新的故障点）。
fn load_cache() -> Option<NotesCache> {
    let s = std::fs::read_to_string(cache_path()).ok()?;
    let c: NotesCache = serde_json::from_str(&s).ok()?;
    (!c.notes.is_empty()).then_some(c)
}

/// 先写临时文件再 rename（同 `Config::save`）。失败只记日志。
fn save_cache(c: &NotesCache) {
    let path = cache_path();
    if let Some(dir) = path.parent() {
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
    }
    let tmp = path.with_extension("json.tmp");
    let Ok(body) = serde_json::to_string(c) else {
        return;
    };
    if std::fs::write(&tmp, body).is_ok() {
        if let Err(e) = std::fs::rename(&tmp, &path) {
            crate::log(&format!("notes: could not save the release cache: {e}"));
        }
    }
}

/// 拉一次 GitHub Releases，全部落成 `版本 → 原始正文`。
fn fetch_releases() -> Result<BTreeMap<String, RawNote>, String> {
    let raw = fetch_json(RELEASES_API)?;
    let out = parse_releases(&raw).ok_or_else(|| "无法解析 GitHub Releases 的返回".to_string())?;
    if out.is_empty() {
        return Err("GitHub Releases 里没有任何 dsh-* 的发布记录".into());
    }
    Ok(out)
}

/// 用 node 发一次 GET（理由见模块头）。
///
/// URL 走 argv 而不是拼进脚本：脚本里出现 `?` `&` 会被 `cmd /c` 的引号规则咬到，
/// 与 `proc::validate_user_path` 挡 `&` 是同一个原因。
fn fetch_json(url: &str) -> Result<String, String> {
    const JS: &str = "fetch(process.argv[1],{headers:{'user-agent':'DShell',\
                      'accept':'application/vnd.github+json'}})\
                      .then(r=>{if(!r.ok)throw new Error('HTTP '+r.status);return r.text()})\
                      .then(t=>process.stdout.write(t))\
                      .catch(e=>{process.stderr.write(String(e&&e.message||e));process.exit(1)});";

    let node = Config::load()
        .get("node")
        .unwrap_or_else(|| PathBuf::from("node"));
    let out = proc::run_path(&node, &["-e", JS, url], FETCH_TIMEOUT)
        .map_err(|e| format!("无法执行 node 去拉取发布说明：{e}"))?;
    if !out.ok() {
        return Err(format!("拉取发布说明失败：{}", out.diagnose()));
    }
    Ok(out.stdout)
}

/// GitHub Releases 的 JSON → `版本 → 原始正文`。
///
/// 只认 `tag_name` 形如 `dsh-v<semver>` 的条目：这个仓库里还有别的 tag。
fn parse_releases(s: &str) -> Option<BTreeMap<String, RawNote>> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let arr = v.as_array()?;
    let mut out = BTreeMap::new();
    for item in arr {
        let Some(tag) = item.get("tag_name").and_then(|t| t.as_str()) else {
            continue;
        };
        let Some(version) = tag.strip_prefix(TAG_PREFIX) else {
            continue;
        };
        // 草稿是没发布的，不该出现在面板上
        if item.get("draft").and_then(|d| d.as_bool()).unwrap_or(false) {
            continue;
        }
        let body = item
            .get("body")
            .and_then(|b| b.as_str())
            .unwrap_or_default()
            .to_string();
        let url = item
            .get("html_url")
            .and_then(|u| u.as_str())
            .unwrap_or_default()
            .to_string();
        out.insert(version.to_string(), RawNote { url, body });
    }
    Some(out)
}

/// 从 release 正文里切出**中文段**。
///
/// 实测这批 release 有**三种**排版（都在本函数的测试里）：
///
/// | 形状 | 例 |
/// | --- | --- |
/// | `<h3 id="cn-vX">…</h3>` … `<h3 id="en-vX">…</h3>` | 0.1.7-alpha.2 等 18 条 |
/// | `<h2 id="chinese">X · 中文</h2>` … `<h2 id="english">` | 0.1.3-alpha.2 |
/// | `<h3 id="cn">…</h3>` … `<h3 id="en">…</h3>` | 0.1.0-rc.7 |
///
/// 所以判据是**锚点 id 的前缀**（`cn` / `chinese` / `en` / `english`），
/// 不是某一个固定字符串。切不出来时回退到"整段去掉导航行与末尾链接"——
/// 宁可多显示一点，也不要给用户一个空窗口。
pub fn extract_cn(body: &str) -> String {
    let cn = ["id=\"chinese\"", "id=\"cn\"", "id=\"cn-", "id=\"cn_"];
    let en = ["id=\"english\"", "id=\"en\"", "id=\"en-", "id=\"en_"];

    let start = earliest(body, &cn);
    let text = match start {
        // 有中文锚点：从**这个标签之后**开始（丢掉标签本身，标题由面板另画）
        Some(i) => {
            let from = body[i..].find('>').map(|j| i + j + 1).unwrap_or(i);
            let rest = &body[from..];
            match earliest(rest, &en) {
                Some(j) => {
                    // 英文锚点前通常有一个 `---` 分隔线，连同它一起去掉
                    let cut = rest[..j].rfind('<').unwrap_or(j);
                    &rest[..cut]
                }
                None => rest,
            }
        }
        // 没有中文锚点：整段，但先切掉开头的导航行与结尾的 Full Changelog
        None => {
            let b = strip_nav(body);
            match b.find("Full Changelog") {
                Some(j) => &b[..j],
                None => b,
            }
        }
    };

    // 中文段的**第一个标题标签的开口已被切掉**（上面是跳到它的 `>` 之后再取的），
    // 剩下的是 `新增功能</h3>` 这种半截。所以在这里补回 `### `，
    // 否则全篇第一个小标题会跟正文混在一起、丢掉层级。
    let text = if start.is_some() {
        format!("### {text}")
    } else {
        text.to_string()
    };

    let out = trim_decoration(&normalize_markup(&text));
    // 段内也切一刀 `Full Changelog`：实测 0.1.5-rc.1 的中文段尾先有一行 Full Changelog，
    // 之后才是英文锚点。不切的话英文段的 `### Bug Fixes` 会跟着进来
    // —— 这一条是全量核对（20 条真正文）才发现的。
    let out = match out.find("Full Changelog") {
        Some(i) => trim_decoration(&out[..i]),
        None => out,
    };
    // 切完只剩导航行（正文是空的，或压根没有中文段）：给空串。
    // 不这么兜的话，`[中文](#cn) | [English](#en)` 这行会当成正文显示给用户。
    if is_nav_line(out.trim()) {
        return String::new();
    }
    out
}

/// 把 release 正文里的内联 HTML 收敛成**一小撮 Markdown**，面板只认这一小撮。
///
/// 为什么在 Rust 侧做：面板那边要渲染的是**未受信的外部文本**（GitHub 上的正文），
/// 而面板手里握着 `switch?version=` 那条回传通道 —— 让它去解析 HTML 等于给自己开一个
/// XSS 口子。这里先把它压成纯文本 + 三个记号（`### ` 标题、列表符号、行内样式），
/// 面板就只需要按行建文本节点，永远不碰 `innerHTML`。
///
/// 保留的记号只有：
/// * `<h2>` / `<h3>` → `### ` 行（这批正文里的层级来源）
/// * `<br>` → 换行
/// * 其余标签一律**去掉标签、保留文字**（`<b>`、`<code>`、`<a>` … 都不必认）
fn normalize_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        // 找到标签的结束 `>`（没有就说明剩下的不是标签，原样收下）
        let Some(j) = tail.find('>') else {
            out.push_str(tail);
            return collapse_blank(&out);
        };
        let tag = tail[..=j].to_ascii_lowercase();
        if tag.starts_with("<h2") || tag.starts_with("<h3") {
            // 小标题：独立成行并带 `### ` 记号（面板据此加粗放大）
            if !out.ends_with('\n') && !out.is_empty() {
                out.push('\n');
            }
            out.push_str("\n### ");
        } else if tag.starts_with("<br") {
            out.push('\n');
        }
        // 其它标签（含 `</h3>` 这类闭合）直接丢掉，只留它们包着的文字
        rest = &tail[j + 1..];
    }
    out.push_str(rest);
    collapse_blank(&out)
}

/// 连着的空行压成一个（正文里 `<h3>` 前后本来就有空行，不压会出现大段留白）。
fn collapse_blank(s: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in s.lines() {
        let t = line.trim_end();
        // 连续的空白行只留一行
        if t.trim().is_empty() && out.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
            continue;
        }
        out.push(t);
    }
    out.join("\n").trim().to_string()
}

/// 只认开头的 `[中文](#…) | [English](#…)` 导航行。
fn strip_nav(s: &str) -> &str {    let t = s.trim_start();
    if t.starts_with("[中文]") {
        // 导航行以换行结束；整段就是导航行时返回空
        match t.find('\n') {
            Some(i) => &t[i + 1..],
            None => "",
        }
    } else {
        t
    }
}

/// 整段都是那行中英导航（没有换行、或切完只剩它）。
fn is_nav_line(s: &str) -> bool {
    // 形状：`[中文](#…) | [English](#…)`，中间的分隔符允许空格差异
    s.starts_with("[中文]") && s.contains("[English]")
}

/// `needles` 里最早出现的那个的位置。
fn earliest(s: &str, needles: &[&str]) -> Option<usize> {
    needles.iter().filter_map(|n| s.find(n)).min()
}

/// 收尾：去掉尾部的换行、`---` 分隔线与悬挂的列表符号。
fn trim_decoration(s: &str) -> String {
    let mut lines: Vec<&str> = s.lines().collect();
    while let Some(last) = lines.last() {
        let t = last.trim();
        if t.is_empty() || t == "---" || t == "***" || t == "___" || t.starts_with("Full Changelog") {
            lines.pop();
        } else {
            break;
        }
    }
    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 第一种排版（18/20 条）：`<h3 id="cn-vX">` … `<h3 id="en-vX">`。
    const H3_CN_EN: &str = "[中文](#cn-v0.1.5-rc.2) | [English](#en-v0.1.5-rc.2)\r\n\r\n<h3 id=\"cn-v0.1.5-rc.2\">体验优化</h3>\r\n\r\n- 优化反馈提交体验。@yixiangihsiang\r\n- 优化交付文件卡片。@yixiangihsiang\r\n\r\n---\r\n\r\n<h3 id=\"en-v0.1.5-rc.2\">Improvements</h3>\r\n\r\n- Improve feedback submission. by @yixiangihsiang\r\n\r\nFull Changelog: https://github.com/x/y/compare/a...b";

    /// 第二种（0.1.3-alpha.2）：`<h2 id="chinese">` … `<h2 id="english">`。
    const H2_CHINESE: &str = "[中文](#chinese) | [English](#english)\r\n\r\n<h2 id=\"chinese\">0.1.3-alpha.2 · 中文</h2>\r\n\r\n<h3>新增功能</h3>\r\n\r\n- 升级 pi-ai 到 0.85.1。 — @tianyicui\r\n\r\n<h2 id=\"english\">0.1.3-alpha.2 · English</h2>\r\n\r\n<h3>New Features</h3>\r\n\r\n- Upgrade pi-ai to 0.85.1. by @tianyicui";

    /// 第三种（0.1.0-rc.7）：`<h3 id="cn">` … `<h3 id="en">`。
    const H3_CN_SHORT: &str = "[中文](#cn) | [English](#en)\r\n\r\n<h3 id=\"cn\">新增功能</h3>\r\n\r\n* 各插件可自行注册设置卡片\r\n\r\n---\r\n\r\n<h3 id=\"en\">New Features</h3>\r\n\r\n* Enable plugins to register their own settings cards by @LegGasai\r\n\r\n---\r\n\r\nFull Changelog: https://github.com/x/y/compare/abc...def";

    /// 三种排版都要切出中文段，且**一点英文都不许漏进来**。
    #[test]
    fn extracts_chinese_across_all_three_layouts() {
        for (name, body) in [
            ("h3 cn-vX", H3_CN_EN),
            ("h2 chinese", H2_CHINESE),
            ("h3 cn", H3_CN_SHORT),
        ] {
            let out = extract_cn(body);
            assert!(out.contains("优化反馈提交体验") || out.contains("升级 pi-ai") || out.contains("各插件可自行注册"),
                "{name}: 中文正文没切出来，得到：{out:?}");
            for bad in ["Improvements", "New Features", "Bug Fixes", "by @", "Full Changelog"] {
                assert!(!out.contains(bad), "{name}: 英文段漏进来了（{bad}）：{out:?}");
            }
            assert!(!out.contains("id="), "{name}: 锚点标签没切干净：{out:?}");
            assert!(!out.starts_with("---"), "{name}: 开头留着分隔线：{out:?}");
        }
    }

    /// 边界：没有中文锚点的正文（理论上的第四种）不能返回空 —— 宁可多显示。
    #[test]
    fn falls_back_to_whole_body_when_no_chinese_anchor() {
        let body = "[中文](#cn) | [English](#en)\n\n恢复数据丢失的会话\n\nFull Changelog: https://x/y";
        let out = extract_cn(body);
        assert!(out.contains("恢复数据丢失的会话"), "回退路径不能给空窗口：{out:?}");
        assert!(!out.contains("Full Changelog"), "回退也要切掉末尾链接：{out:?}");
    }

    /// 中文段**段内**自己带一行 `Full Changelog`，后面还有一段英文散文，然后才是英文锚点。
    ///
    /// 这是 `0.1.5-rc.1` 的真实形状（全量核对 20 条正文时只有它这样，我最早的 3 个
    /// 手挑夹具全都没覆盖到）。不在段内切一刀的话，后面那段英文的 `### Bug Fixes`
    /// 会跟着切进中文段里。
    #[test]
    fn truncates_at_full_changelog_inside_the_chinese_section() {
        let body = "[中文](#cn-v1) | [English](#en-v1)\r\n\r\n<h3 id=\"cn-v1\">新增功能</h3>\r\n\r\n- 中文条目一\r\n\r\nFull Changelog: https://github.com/x/y/compare/a...b\r\n\r\n---\r\n\r\nThis release candidate summarizes the changes since `v0.1.2`.\r\n\r\n<h3 id=\"en-v1\">New Features</h3>\r\n\r\n- English entry by @someone";
        let out = extract_cn(body);
        assert!(out.contains("中文条目一"), "{out:?}");
        assert!(!out.contains("Full Changelog"), "段内的 Full Changelog 要切掉：{out:?}");
        assert!(!out.contains("release candidate"), "英文散文漏进来了：{out:?}");
        assert!(!out.contains("New Features") && !out.contains("English entry"), "英文段漏进来了：{out:?}");
    }

    /// 空正文 / 只有导航行的正文：给空串，不给 panic。
    #[test]
    fn handles_empty_bodies() {
        assert_eq!(extract_cn(""), "");
        assert_eq!(extract_cn("[中文](#cn) | [English](#en)"), "");
    }

    /// `tag_name` 只认 `dsh-v<semver>`；草稿要排除；缺字段不能 panic。
    #[test]
    fn parses_only_dsh_version_tags_and_skips_drafts() {
        let raw = r#"[
          {"tag_name":"dsh-v0.1.7-alpha.1","body":"甲","html_url":"https://x/1"},
          {"tag_name":"dsh-v0.1.7-alpha.2","body":"乙","html_url":"https://x/2","draft":true},
          {"tag_name":"desktop-v2.0.0","body":"丙","html_url":"https://x/3"},
          {"tag_name":"dsh-v0.1.6-alpha.2","html_url":"https://x/4"}
        ]"#;
        let m = parse_releases(raw).expect("能解析");
        assert_eq!(m.len(), 2, "只留 dsh-v* 且非草稿：{m:?}");
        assert!(m.contains_key("0.1.7-alpha.1"));
        assert!(!m.contains_key("0.1.7-alpha.2"), "草稿不该出现");
        assert!(!m.contains_key("desktop-v2.0.0") && !m.contains_key("2.0.0"));
        assert_eq!(m["0.1.6-alpha.2"].body, "", "缺 body 要落到空串而不是崩");
    }

    /// 缓存里"没有这一版"必须区分**问过没有**与**还没问**：
    /// 前者别再联网，后者必须去查（否则刚发布的版本会被判成"没有说明"，最长 2 小时）。
    #[test]
    fn cache_round_trips_and_tolerates_missing_fields() {
        let c = NotesCache {
            fetched_at_ms: 123,
            notes: BTreeMap::from([(
                "0.1.5-rc.2".to_string(),
                RawNote { url: "u".into(), body: "b".into() },
            )]),
            absent: BTreeSet::from(["0.0.1-rc.1".to_string()]),
        };
        let j = serde_json::to_string(&c).unwrap();
        let back: NotesCache = serde_json::from_str(&j).unwrap();
        assert_eq!(back.notes["0.1.5-rc.2"].body, "b");
        assert_eq!(back.fetched_at_ms, 123);
        assert!(back.absent.contains("0.0.1-rc.1"));
        assert!(!back.absent.contains("0.1.7-alpha.2"), "没问过的不能算 absent");

        // 缺字段（手工删过 / 将来加了字段）：给默认值，不是读失败
        let partial: NotesCache = serde_json::from_str(r#"{"notes":{}}"#).unwrap();
        assert_eq!(partial.fetched_at_ms, 0);
        assert!(partial.absent.is_empty());
    }

    /// `note.cached` 是面板显示"（缓存）"的依据，必须如实反映来源。
    #[test]
    fn note_reports_whether_it_came_from_cache() {
        let raw = RawNote { url: "https://x".into(), body: H3_CN_EN.into() };
        assert!(build("0.1.5-rc.2", &raw, true).cached);
        assert!(!build("0.1.5-rc.2", &raw, false).cached);
        assert_eq!(build("0.1.5-rc.2", &raw, true).url, "https://x");
    }

    /// 小标题要变成 `### ` 行，其余标签（`<b>` / `<code>` / `<a>`）只留文字。
    /// 这一条守的是**面板不做 HTML 解析**这个前提：正文里不许再有尖括号。
    #[test]
    fn markup_is_flattened_to_a_tiny_markdown_subset() {
        let out = normalize_markup("<h3>新增功能</h3>\n- 支持 <b>粗体</b> 与 <code>代码</code>\n<a href=\"x\">链接</a>");
        assert!(out.contains("### 新增功能"), "{out:?}");
        assert!(out.contains("支持 粗体 与 代码"), "{out:?}");
        assert!(out.contains("链接"), "{out:?}");
        assert!(!out.contains('<') && !out.contains('>'), "不许给面板留标签：{out:?}");
    }
}
