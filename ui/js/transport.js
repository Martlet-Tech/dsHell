/**
 * 与 Rust 侧的通信层。
 *
 * ## 两条通道，职责不重叠
 *
 *   ① `window.__dshell.*` —— 老通道（Rust 单向 `eval` + base64），只负责 handoff
 *      的淡出与失败卡片。已验证过的链路。
 *   ② `doctor://*` / `install://*` 事件 + `invoke` —— 体检/安装双向通道。
 *      页面加载完注册好监听后主动 `invoke('doctor_run')`，这样事件不会丢。
 *
 * ## 为什么 `__TAURI__` 要每次现取
 *
 * 它是 Tauri 注入的全局对象。这里不做模块级缓存，避免"模块先于注入执行"时
 * 永久把 undefined 记下来。
 */

/** Tauri 注入的全局对象；未注入时返回 undefined。 */
export function tauri() {
  return window.__TAURI__;
}

/** 把 base64 载荷解回 UTF-8 文本（穿过 eval + innerHTML 两层上下文都安全）。 */
export function decode(b64) {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return new TextDecoder("utf-8").decode(bytes);
}
