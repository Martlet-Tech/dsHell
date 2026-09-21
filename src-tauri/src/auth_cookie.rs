//! 清掉**上一代** dsh 浏览器会话 cookie。
//!
//! 为什么必须做：DShell 用 `dsh web --port 0`，每次启动换随机端口；而 dsh 的会话
//! cookie 名 = `dsh-auth-` + base64url(sha256(authority))，authority **带端口**。
//! 于是每启动一次就 mint 一个新名字的 cookie，`Max-Age` 默认 30 天，一条都不失效。
//!
//! 浏览器又只按 host 存 cookie（Domain 不含端口），这几十条互不相干、请求时全部一起发。
//! 攒到约 68 条（每条序列化 236 字节）就越过 dsh 那个 Node `http` 服务器的
//! `maxHeaderSize`（16384 字节），由**服务端**直接回 431 —— 界面永远进不去，
//! 而后端本身完全健康。实测 72 条 ≈ 17.5 KB，只超线约 1.2 KB。
//!
//! 删的是所有 `dsh-auth-*`，别的 cookie（含启动页自己的）一律不碰。删旧的不会丢登录：
//! 凭据在 `~/.dsh/.credentials.yaml`，与 cookie 无关；本代那条会在这次 handoff 的
//! 303 交换里重新 mint。
//!
//! **失败绝不拦启动**：错误交给调用方记日志。

use tauri::WebviewWindow;

/// dsh 的会话 cookie 前缀（`dsh-client-connection` 的 `COOKIE_PREFIX`）。
const COOKIE_PREFIX: &str = "dsh-auth-";

/// 删掉所有 `dsh-auth-*`，返回删除条数。
///
/// 不区分端口，因为调用点刻意放在**拿到 token URL 之前**（spawn dsh 之后、`wait_for_url`
/// 之前）：那时上一代已经确定作废，而本代还没 mint 出来，所有现存 auth cookie 都是废的。
/// 好处是清理与 dsh 启动并行、且窗口还停在本地启动页，没有任何请求会撞上 431。
///
/// **必须在非主线程调用**：Tauri 文档明确 `cookies()` 在 Windows 的同步命令 /
/// 事件处理器里会死锁 —— wry 是「发消息给 webview 线程 + 等回调」的同步调用。
pub fn purge_stale(window: &WebviewWindow) -> std::io::Result<usize> {
    let all = window
        .cookies()
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    let mut removed = 0usize;
    for cookie in all
        .iter()
        .filter(|c| c.name().starts_with(COOKIE_PREFIX))
    {
        // 按 name/domain/path 匹配删除（值无关），这三个都从 cookie 自己身上取。
        if window.delete_cookie(cookie.clone()).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::COOKIE_PREFIX;

    /// 选择器就是前缀匹配 —— 只有 `dsh-auth-*` 会被删，别的一律不碰。
    #[test]
    fn prefix_selects_only_dsh_auth_cookies() {
        let auth = |n: &str| n.starts_with(COOKIE_PREFIX);
        assert!(auth("dsh-auth-PXif6sCVwC7-iYFjpDcAJV1pDdkiJOL1zAxal6Ds9a4"));
        assert!(!auth("dshell_picker_token"));
        assert!(!auth("__Host-something"));
    }
}
