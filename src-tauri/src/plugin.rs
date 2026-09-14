//! 自带插件（`dshell-directory-picker`）的安装与自愈。
//!
//! ## 这个模块解决什么
//!
//! 光编出 exe **不够**。选目录要即时刷新，需要把壳层弹原生对话框这条路接上，
//! 而"接上"的那一半住在 **dsh 的 profile 里**（一个普通 dsh 插件）。exe 负责的
//! 只是"提供端口"（`DSHELL_PICKER_PORT`），插件负责"消费端口"。两半缺一，
//! 功能就静默退回旧行为（选完目录要补点一下鼠标）——不报错、不崩溃，这正是
//! 最容易被忽略的失败方式。
//!
//! 所以 exe 在启动体检时顺手把插件装好，用户双击即用，不需要另跑 ps1。
//!
//! ## 为什么不用官方的 `dsh plugin --profile web add ...`
//!
//! dsh 确实有这条命令（`apps/cli/src/plugin.ts`），但它是个 **pnpm 转发器**：
//! `spawnSync('pnpm', ...)`，pnpm 不在 PATH 上时直接返回 127
//! （"pnpm not found on PATH"）。终端用户机器上很可能没有 pnpm——dsh 自己才是
//! 全局装的那个包。为了一个只读的 patch 层插件去要求用户装 pnpm，是把部署成本
//! 从"我们"转移给"用户"，代价还不小。
//!
//! 本模块走**纯文件系统**路线：插件零依赖（`lib/index.js` 全文没有任何
//! `import`/`require`），dsh 解析 bundle 时先看安装锚点、再看
//! `<profile>/node_modules/<name>`（`app-boot/src/profile.ts` 的 `resolveBundleDir`），
//! 所以只要把目录**复制**进去、再把名字写进 `dsh.profile.bundles`，就够了。
//!
//! ## 为什么是复制而不是 junction
//!
//! 开发机上 junction 到源码目录很省事（改完不用重装），但**发布版必须复制**：
//! junction 记住的是 `D:\Projects\...` 这样的本机路径，用户机器上不存在，
//! 链接悬空 → 插件加载失败 → 又是静默退回旧行为。
//!
//! ## 三条不变量
//!
//! 1. **不覆盖已有的安装。** 目标已存在（很可能是开发期的 junction，或用户
//!    自己装的版本）时只读不写。用户的东西优先级最高。
//! 2. **绝不因为失败而拦住启动。** 插件是增强项，不是前置条件；失败只降级成
//!    体检里的一条黄字（`StepState::Warn`）。
//! 3. **写清单是原子的。** 先写临时文件再 rename，避免"写一半断电"留下
//!    半个 JSON——那样 dsh 直接起不来，比不装插件严重得多。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// 插件包名。同时用作 `node_modules` 下的目录名和 `dsh.profile.bundles` 里的条目。
pub const PLUGIN_NAME: &str = "dshell-directory-picker";

/// DShell 只启动 `dsh web`，所以只装 `web` profile。
pub const PROFILE: &str = "web";

/// 插件随 exe 一起发布的目录名（Tauri resource）。
const BUNDLED_DIR: &str = "plugin/dshell-directory-picker";

/// 装一次的结果——体检步骤直接由它渲染，不需要再去探测一遍。
pub enum Status {
    /// 已经装好了（这次没动它）。
    Present { detail: String },
    /// 这次刚装上。
    Installed { detail: String },
    /// 没装成，但不该拦住启动。
    Degraded { detail: String },
}

/// `%USERPROFILE%\.dsh` —— 与 `doctor::probe_profile` 保持一致。
///
/// **不读 `DSH_HOME`**：用户如果在环境里设了它，`dsh web` 会跟着走，而本模块
/// 也得跟着走，否则装到了 dsh 根本不看的地方。`DSH_HOME` 优先，其次默认值。
fn dsh_home() -> PathBuf {
    if let Ok(v) = std::env::var("DSH_HOME") {
        let t = v.trim();
        // dsh 自己把空/纯空白的 DSH_HOME 当作没设（`home-paths/src/index.ts`）
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    let home = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".dsh")
}

fn profile_dir() -> PathBuf {
    dsh_home().join("profiles").join(PROFILE)
}

/// 插件随 exe 发布的位置。
///
/// 开发期（`cargo run` / 直接跑 target 里的 exe）resource 目录不存在，退回
/// 用 `CARGO_MANIFEST_DIR` 找到仓库里的源码副本——这样开发时改完插件直接生效，
/// 不用先跑打包。
fn bundled_plugin_dir() -> Option<PathBuf> {
    // 1) 发布版：exe 旁边的 resource 目录
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join(BUNDLED_DIR);
            if cand.is_dir() {
                return Some(cand);
            }
        }
        // tauri 的 resource 也可能落在 exe 同级的 resources/ 下
        if let Some(dir) = exe.parent() {
            let cand = dir.join("resources").join(BUNDLED_DIR);
            if cand.is_dir() {
                return Some(cand);
            }
        }
    }
    // 2) 开发期：仓库里的 src-tauri/../plugin/dshell-directory-picker
    let dev = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .join("plugin")
        .join(PLUGIN_NAME);
    dev.is_dir().then_some(dev)
}

/// 目标安装位置：`<profile>/node_modules/<name>`。
fn install_target() -> PathBuf {
    profile_dir().join("node_modules").join(PLUGIN_NAME)
}

/// 插件目录是不是"完整可用"的：`package.json` 与入口都在。
///
/// 只查 `package.json` 不够——半个复制（比如上次被中断）会留下一个能被
/// `bundles` 解析到、但 import 时炸掉的包，那比不装更糟。
fn looks_complete(dir: &Path) -> bool {
    dir.join("package.json").is_file() && dir.join("lib").join("index.js").is_file()
}

/// 读 profile 清单；不存在或坏了都返回 `None`（调用方据此决定是"不能装"）。
fn read_manifest(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// 原子写清单：先写 `.tmp` 再 rename。
fn write_manifest(path: &Path, v: &Value) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(v)? + "\n";
    let tmp = path.with_extension("json.dshell-tmp");
    std::fs::write(&tmp, text)?;
    // Windows 的 rename 不覆盖已存在的目标，先删
    let _ = std::fs::remove_file(path);
    std::fs::rename(&tmp, path)
}

/// 递归复制目录（不引 walkdir：就这一处用，几十行而已）。
///
/// 符号链接/目录联接一律**跳过**而不是跟随：插件是纯自足的，不需要链接；
/// 跟随反而可能把用户机器上的别的东西拖进来。
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else if ft.is_file() {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// 确保插件已装好。幂等：已经装好时什么都不做。
///
/// 调用点见 `doctor::reprobe` 的 `"picker"` 分支——它排在 ⑤ profile 之后，
/// 因为它依赖 profile 目录存在且可写。
pub fn ensure_installed() -> Status {
    let profile = profile_dir();
    let manifest_path = profile.join("package.json");
    let target = install_target();

    // ── 已经有完整安装了 ────────────────────────────────────────────
    // 可能是开发期的 junction，也可能是上次装好留下的。**不碰它。**
    // 但仍然要确认它确实在 bundles 列表里——手工装过的人可能只放了目录。
    if looks_complete(&target) {
        return match ensure_listed(&manifest_path) {
            Ok(_) => Status::Present {
                detail: format!("已就绪 · {}", target.display()),
            },
            Err(e) => Status::Degraded {
                detail: format!(
                    "插件已在 {}，但无法确认它被 profile 启用：{e}。\
                     功能可能不生效（表现为选完目录要补点一下）。",
                    target.display()
                ),
            },
        };
    }

    // ── profile 还不存在 ────────────────────────────────────────────
    // 正常流程里 ⑤ 之前 `dsh web` 从没跑过，profile 就还没被创建。
    // **不能代 dsh 建**：profile 的初始清单（bundles / patchReload）是 dsh 的
    // 模板资产，手搓一份会和 `PROFILE_TEMPLATES` 漂移。这一次跳过，等用户
    // 跑过一次 dsh 之后下次启动就会装上。
    if !manifest_path.is_file() {
        return Status::Degraded {
            detail: format!(
                "dsh 的 {PROFILE} profile 还没创建（首次启动 dsh 后会生成）。\
                 本次先跳过；下次启动 DShell 会自动装上。"
            ),
        };
    }

    let Some(src) = bundled_plugin_dir() else {
        return Status::Degraded {
            detail: "找不到随程序发布的插件目录（打包时漏了 resource？）。本次跳过。".into(),
        };
    };

    // ── 复制进去 ────────────────────────────────────────────────────
    // 目标存在但不完整（上次被中断）→ 先清掉再复制，否则会混着两份。
    if target.exists() {
        if let Err(e) = std::fs::remove_dir_all(&target) {
            return Status::Degraded {
                detail: format!(
                    "{} 已存在但不完整，且删不掉：{e}。请完全退出 DShell 后重试。",
                    target.display()
                ),
            };
        }
    }
    if let Some(parent) = target.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Status::Degraded {
                detail: format!("无法创建 {}：{e}", parent.display()),
            };
        }
    }
    if let Err(e) = copy_dir(&src, &target) {
        // 失败就清掉半成品，绝不留下"解析得到但 import 会炸"的包
        let _ = std::fs::remove_dir_all(&target);
        return Status::Degraded {
            detail: format!("复制插件到 {} 失败：{e}", target.display()),
        };
    }

    // ── 写进 bundles ────────────────────────────────────────────────
    match ensure_listed(&manifest_path) {
        Ok(_) => Status::Installed {
            detail: format!("已自动安装 · {}", target.display()),
        },
        Err(e) => {
            // 目录放好了但清单没写上 → dsh 不会加载它，等于没装。
            // 回滚掉目录，保持"要么全装、要么全不装"，免得留下误导性的残留。
            let _ = std::fs::remove_dir_all(&target);
            Status::Degraded {
                detail: format!(
                    "插件目录已复制，但写入 profile 清单失败：{e}。已回滚。\
                     可手动运行 scripts\\install-picker-plugin.ps1。"
                ),
            }
        }
    }
}

/// 把插件名加进 `dsh.profile.bundles`（以及 `dependencies`）。
///
/// 幂等；返回是否发生了改动。**只追加、不重排**——列表顺序就是 patch 层的
/// 应用顺序，我们插在最后（`cordis.patch.yml` 的 `insert` 需要排在
/// `dsh-web-app` 之后）。
fn ensure_listed(manifest_path: &Path) -> Result<bool, String> {
    let raw = std::fs::read_to_string(manifest_path).map_err(|e| e.to_string())?;
    let mut v: Value = serde_json::from_str(&raw).map_err(|e| format!("清单不是合法 JSON：{e}"))?;

    let is_profile = v
        .get("dsh")
        .and_then(|d| d.get("profile"))
        .map(|p| !p.is_null())
        .unwrap_or(false);
    if !is_profile {
        return Err("这不是一个 dsh profile 清单（缺 dsh.profile）".into());
    }

    let mut changed = false;

    // dependencies[<name>] —— dsh 用它判断"这个 bundle 是不是依赖管理的"
    let dep_present = v
        .get("dependencies")
        .and_then(|d| d.get(PLUGIN_NAME))
        .is_some();

    // bundles 里有没有
    let listed = v
        .get("dsh")
        .and_then(|d| d.get("profile"))
        .and_then(|p| p.get("bundles"))
        .and_then(|b| b.as_array())
        .map(|a| a.iter().any(|x| x.as_str() == Some(PLUGIN_NAME)))
        .unwrap_or(false);

    // 已经两样都齐 → 真的没什么可做的
    if listed && dep_present {
        return Ok(false);
    }

    if !listed {
        let arr = v
            .get_mut("dsh")
            .and_then(|d| d.get_mut("profile"))
            .and_then(|p| p.get_mut("bundles"))
            .and_then(|b| b.as_array_mut())
            .ok_or("清单里 dsh.profile.bundles 不是数组")?;
        arr.push(Value::String(PLUGIN_NAME.to_string()));
        changed = true;
    }

    if !dep_present {
        // 用 `file:` 指向实际安装位置，语义上与 pnpm 的本地依赖一致。
        // 注意：dsh 的 `dsh plugin` 命令会用 pnpm 重写这一段，我们不依赖它。
        let dep = v
            .as_object_mut()
            .ok_or("清单顶层不是对象")?
            .entry("dependencies")
            .or_insert_with(|| json!({}));
        if let Some(obj) = dep.as_object_mut() {
            obj.insert(
                PLUGIN_NAME.to_string(),
                Value::String(format!("file:{}", install_target().display())),
            );
            changed = true;
        }
    }

    if changed {
        write_manifest(manifest_path, &v).map_err(|e| e.to_string())?;
    }
    Ok(changed)
}

/// 卸掉插件（`scripts/install-picker-plugin.ps1 -Uninstall` 的 Rust 版）。
///
/// 目前没有 UI 入口，保留它是为了给"以后做设置页/卸载"留一个和安装对称的出口，
/// 也方便手工用它排障。**开发期的 junction 会被删掉**——这是调用方的责任，
/// 所以这个函数不自动调用。
#[allow(dead_code)]
pub fn uninstall() -> Result<(), String> {
    let target = install_target();
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(|e| {
            format!(
                "删不掉 {}：{e}（DShell 可能正占用，先完全退出）",
                target.display()
            )
        })?;
    }
    let manifest_path = profile_dir().join("package.json");
    if let Some(mut v) = read_manifest(&manifest_path) {
        let mut changed = false;
        if let Some(arr) = v
            .get_mut("dsh")
            .and_then(|d| d.get_mut("profile"))
            .and_then(|p| p.get_mut("bundles"))
            .and_then(|b| b.as_array_mut())
        {
            let before = arr.len();
            arr.retain(|x| x.as_str() != Some(PLUGIN_NAME));
            changed |= arr.len() != before;
        }
        if let Some(deps) = v.get_mut("dependencies").and_then(|d| d.as_object_mut()) {
            changed |= deps.remove(PLUGIN_NAME).is_some();
        }
        if changed {
            write_manifest(&manifest_path, &v).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
