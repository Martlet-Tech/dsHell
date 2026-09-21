//! dsh 版本台账：查 registry、比 semver、校验来源。
//!
//! **本模块只读** —— 不装、不删、不改配置。执行切换的是 `main.rs` 的 `do_switch_dsh`。
//!
//! 三条口径都是从立项 09 继承来的（都是踩过坑的）：
//!
//! * **registry 用用户自己配置的那个**（不传 `--registry`）。检查与安装必须是同一个
//!   registry，否则会出现"说有大版本、装完却不是"。09 记录的是 `registry.npmmirror.com`，
//!   2026-09-21 实测本机已是 `registry.npmjs.org` —— 硬编码镜像地址就会错。
//! * **版本取全量**，不只看 `latest`。本机是预发布 `0.1.5-rc.1`，而 `latest` 指向
//!   `0.1.5-rc.2`：方向恰好对，但反过来（本机 rc.2、latest rc.1）就会提示降级。
//! * **比较用 semver**：无预发布 > 有预发布（`0.1.5` > `0.1.5-rc.1`），预发布同段按数值
//!   （`rc.10` > `rc.2`）。字符串比较这两条都会判错，而判错的方向是**误导用户**。
//!
//! 所有 npm 调用都带上 `cfg.child_path_env()`：用户在「指定…」里改过 node / npm 路径时，
//! 进程自己的 PATH 并没有变，只按环境变量找 npm 会找到另一个（或找不到）。这与体检里
//! 「一轮查询用同一个 npm」是同一个要求。
//!
//! **registry 那半边有 2 小时磁盘缓存**（见 `TTL_MS`）：本地部分（当前版本 / 来源校验）
//! 每次都现查，因为是本地命令而且必须新鲜。

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::installer::DSH_PACKAGE;
use crate::proc;

/// `npm view` 要联网，给宽一点；超时也算可读的失败，不影响 DShell 其它部分。
const VIEW_TIMEOUT: Duration = Duration::from_secs(45);
/// 本地配置类查询（`npm config get` / `npm prefix -g`）很快。
const LOCAL_TIMEOUT: Duration = Duration::from_secs(15);

/// registry 那半边的缓存有效期（2 小时）。
///
/// 为什么要有缓存：`npm view` 要联网、冷启动数秒，而版本列表在几分钟尺度上不会变。
/// 没有它，**每开一次设置面板就跑 5 条 npm 命令**；反复开关会叠出一串 npm 进程
/// （2026-09-21 实测：一轮测试开了 4 次面板 = 20 次 npm 调用）。
///
/// 策略是"先用上次结果渲染、过期再后台重查"：TTL 内直接给缓存（面板瞬开），
/// 过期则**先把缓存的旧数据推给面板**，随后重查并再推一次覆盖。手动点「刷新」永远无视 TTL。
const TTL_MS: u64 = 2 * 60 * 60 * 1000;

/// 磁盘缓存文件名（与 `config.json` 同目录）。
const CACHE_FILE: &str = "dsh-versions.json";

/// 单条版本视图（面板直接用这份 JSON 渲染）。
#[derive(Clone, Serialize)]
pub struct VItem {
    pub version: String,
    /// 命中的 dist-tag（`latest` / `next` / `alpha`…），已排序
    pub tags: Vec<String>,
    /// 本机当前装的那个
    pub current: bool,
    /// 比当前更旧 —— 切过去是降级，必须警告
    pub older: bool,
}

#[derive(Clone, Serialize)]
pub struct Catalog {
    /// 本机当前版本；`None` = 问不出来（面板不高亮任何行）
    pub current: Option<String>,
    /// 全部已发布版本里 semver 最大的（含预发布）
    pub latest: Option<String>,
    /// 实际生效的 registry，显示出来便于排障
    pub registry: String,
    /// registry 数据的抓取时间距今多少秒（面板显示"数据 N 分钟前"）
    pub registry_age_secs: Option<u64>,
    /// 这份 registry 数据已过期：调用方会随后再推一份新的（面板可显示"正在刷新"）
    pub registry_stale: bool,
    /// 来源校验：false ⇒ 拒绝执行切换（否则会装出第二份 DShell 不会启动的 dsh）
    pub npm_global: bool,
    /// 来源校验不过时的说明（给用户看的一句话）
    pub source_note: Option<String>,
    /// `npm i -g` 的目标目录
    pub npm_prefix: Option<String>,
    /// **已按 semver 降序**（最新在上）；解析不了的排最后
    pub versions: Vec<VItem>,
}

/// registry 那半边（唯一需要联网、唯一值得缓存的部分）。
#[derive(Clone, Serialize, Deserialize)]
struct RegistryCache {
    fetched_at_ms: u64,
    registry: String,
    versions: Vec<String>,
    /// 版本 → 命中的 dist-tag
    tags: BTreeMap<String, Vec<String>>,
}

impl RegistryCache {
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

/// 组装一份台账。
///
/// **registry 部分走缓存**（TTL 见 `TTL_MS`），**本地部分每次都现查** —— 后者都是本地
/// 命令（`npm ls -g` / `npm prefix -g`，秒级），而且必须新鲜：`current` 在一次切换之后
/// 立刻就变了，用缓存会让面板显示错的"当前版本"。
///
/// `force` = 无视缓存重查 registry（面板上的「刷新」）。
pub fn catalog(force: bool) -> Result<Catalog, String> {
    let cfg = Config::load();
    let path = Some(cfg.child_path_env());

    let (reg, stale) = match if force { None } else { load_cache() } {
        Some(c) => {
            let stale = c.age_ms() >= TTL_MS;
            (c, stale)
        }
        // 没有缓存：只能现查（首次打开会等几秒，这是唯一一次）
        None => (fetch_registry(&path)?, false),
    };

    let current = npm(
        &["ls", "-g", DSH_PACKAGE, "--depth=0", "--json"],
        path,
        LOCAL_TIMEOUT,
    )
    .ok()
    .and_then(|s| parse_installed(&s));

    let npm_prefix = npm(&["prefix", "-g"], None, LOCAL_TIMEOUT)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let (npm_global, source_note) = source_check(&cfg, &current, npm_prefix.as_deref());

    let current_parsed = current.as_deref().and_then(|v| Version::parse(v).ok());
    let age_secs = reg.age_ms() / 1000;
    let registry = reg.registry;
    let sorted = sort_desc(reg.versions);
    let latest = sorted.iter().find_map(|(_, p)| p.as_ref().map(|v| v.to_string()));

    let versions = sorted
        .into_iter()
        .map(|(raw, parsed)| VItem {
            tags: reg.tags.get(&raw).cloned().unwrap_or_default(),
            current: current.as_deref() == Some(raw.as_str()),
            older: match (&parsed, &current_parsed) {
                (Some(v), Some(c)) => v < c,
                _ => false,
            },
            version: raw,
        })
        .collect();

    Ok(Catalog {
        current,
        latest,
        registry,
        registry_age_secs: Some(age_secs),
        registry_stale: stale,
        npm_global,
        source_note,
        npm_prefix,
        versions,
    })
}

/// 无视 TTL 重查 registry 并落盘（面板上的「刷新」、以及过期后的后台重查）。
pub fn refresh_registry() -> Result<(), String> {
    let cfg = Config::load();
    fetch_registry(&Some(cfg.child_path_env())).map(|_| ())
}

/// 两条 `npm view` + 一条 `npm config get`，成功后立刻落盘。
fn fetch_registry(path: &Option<String>) -> Result<RegistryCache, String> {
    let raw = npm(
        &["view", DSH_PACKAGE, "versions", "--json"],
        path.clone(),
        VIEW_TIMEOUT,
    )?;
    let versions =
        parse_versions(&raw).ok_or_else(|| "无法解析 registry 返回的版本列表".to_string())?;
    if versions.is_empty() {
        return Err(format!("registry 没有返回 {DSH_PACKAGE} 的任何版本"));
    }

    // dist-tags 失败不算致命：版本列表本身仍然可用，只是没有分组徽标。
    let tags = npm(
        &["view", DSH_PACKAGE, "dist-tags", "--json"],
        path.clone(),
        VIEW_TIMEOUT,
    )
    .ok()
    .and_then(|s| parse_tags(&s))
    .unwrap_or_default();

    let registry = npm(&["config", "get", "registry"], path.clone(), LOCAL_TIMEOUT)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let cache = RegistryCache {
        fetched_at_ms: now_ms(),
        registry,
        versions,
        tags,
    };
    save_cache(&cache);
    Ok(cache)
}

fn cache_path() -> std::path::PathBuf {
    Config::dir().join(CACHE_FILE)
}

/// 读缓存。**任何异常都当作"没有缓存"**：缓存坏了不能变成新的故障点（与 `Config::load` 同理）。
fn load_cache() -> Option<RegistryCache> {
    let s = std::fs::read_to_string(cache_path()).ok()?;
    let c: RegistryCache = serde_json::from_str(&s).ok()?;
    (!c.versions.is_empty()).then_some(c)
}

/// 写缓存：先写临时文件再 rename（与 `Config::save` 同一处置）。
/// 失败只记日志 —— 缓存写不进去不影响这次的结果。
fn save_cache(c: &RegistryCache) {
    let path = cache_path();
    if let Some(dir) = path.parent() {
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
    }
    let tmp = path.with_extension("json.tmp");
    let body = match serde_json::to_string_pretty(c) {
        Ok(b) => b,
        Err(_) => return,
    };
    if std::fs::write(&tmp, body).is_ok() {
        if let Err(e) = std::fs::rename(&tmp, &path) {
            crate::log(&format!("update: could not save the version cache: {e}"));
        }
    }
}

/// 跑一条 npm 命令，返回 stdout。失败（非 0 / 超时 / spawn 不了）给一句能读的诊断。
fn npm(args: &[&str], path_env: Option<String>, timeout: Duration) -> Result<String, String> {
    let mut all = vec!["npm"];
    all.extend_from_slice(args);
    crate::log(&format!("update: npm {}", args.join(" ")));
    let out = proc::run_shell_env(&all, timeout, path_env)
        .map_err(|e| format!("无法执行 npm：{e}"))?;
    if !out.ok() {
        return Err(format!(
            "npm {} 失败：{}",
            args.first().copied().unwrap_or(""),
            out.diagnose()
        ));
    }
    Ok(out.stdout)
}

fn parse_versions(s: &str) -> Option<Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    Some(
        v.as_array()?
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
    )
}

/// `npm ls -g <pkg> --depth=0 --json` → `dependencies.<pkg>.version`。
fn parse_installed(s: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    Some(
        v.get("dependencies")?
            .get(DSH_PACKAGE)?
            .get("version")?
            .as_str()?
            .to_string(),
    )
}

/// `{"next":"0.1.5-rc.2","latest":"0.1.5-rc.2"}` → `{"0.1.5-rc.2":["latest","next"]}`。
fn parse_tags(s: &str) -> Option<BTreeMap<String, Vec<String>>> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let obj = v.as_object()?;
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // BTreeMap 迭代顺序天然稳定（按 tag 名），徽标次序因此可复现
    for (tag, ver) in obj {
        if let Some(ver) = ver.as_str() {
            out.entry(ver.to_string()).or_default().push(tag.clone());
        }
    }
    Some(out)
}

/// 降序：解析得了的在前（semver 比较），解析不了的按字符串排在最后。
fn sort_desc(mut v: Vec<String>) -> Vec<(String, Option<Version>)> {
    let mut items: Vec<(String, Option<Version>)> = v
        .drain(..)
        .map(|raw| {
            let parsed = Version::parse(&raw).ok();
            (raw, parsed)
        })
        .collect();
    items.sort_by(|a, b| match (&a.1, &b.1) {
        (Some(x), Some(y)) => y.cmp(x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.0.cmp(&b.0),
    });
    items
}

/// 来源校验：`npm i -g` 只会更新 npm 全局那一份。
///
/// 若用户的 dsh 来自 pnpm / npx / 手动指定，执行更新会装出**第二份** DShell 根本不启动的
/// dsh —— 比不更新更糟。所以这里先确认两件事：npm 全局里确实有它，且用户配置里那个
/// `dsh` 路径就在 npm 全局目录里。
fn source_check(
    cfg: &Config,
    current: &Option<String>,
    npm_prefix: Option<&str>,
) -> (bool, Option<String>) {
    let Some(prefix) = npm_prefix else {
        return (
            false,
            Some("问不出 npm 全局目录（`npm prefix -g` 失败），无法确认要更新的那一份".into()),
        );
    };
    if current.is_none() {
        return (
            false,
            Some(format!(
                "npm 全局里没有 {DSH_PACKAGE} —— 当前这份可能是 pnpm / npx / 手动指定的，\
                 直接更新会装出第二份 DShell 不会启动的 dsh"
            )),
        );
    }
    if let Some(dsh) = cfg.dsh.as_deref() {
        let dir = Path::new(dsh).parent().map(|p| p.to_string_lossy().to_string());
        if let Some(dir) = dir {
            if !same_dir(&dir, prefix) {
                return (
                    false,
                    Some(format!(
                        "配置里的 dsh（{dsh}）不在 npm 全局目录（{prefix}）下，\
                         更新它只会装出第二份"
                    )),
                );
            }
        }
    }
    (true, None)
}

/// Windows 路径比较：大小写不敏感、分隔符与结尾斜杠都不算数。
fn same_dir(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    };
    norm(a) == norm(b)
}
