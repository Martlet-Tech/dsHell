/**
 * 安装面板：进度条、百分比、已用时、日志尾部、取消。
 *
 * 由 `install://progress` / `install://done` 事件驱动（见 `app.js` 的接线）。
 * 这份 UI 同样服务于将来的"更新 dsh" —— 它复用的就是 `installer::run`，
 * 发的是同一组事件。
 */

import { panel, barEl, logEl, btnInstall } from "./dom.js";
import { tauri } from "./transport.js";
import { toast } from "./toast.js";

/**
 * 按一次进度事件重画面板。
 * @param p - `Progress { phase, percent, elapsed_ms, lines }`。
 */
export function paint(p) {
  panel.hidden = false;
  document.querySelector("#inst-title").textContent = p.phase || "正在安装";
  document.querySelector("#inst-elapsed").textContent =
    ((p.elapsed_ms || 0) / 1000).toFixed(0) + "s";
  const pct = p.percent;
  if (pct === null || pct === undefined) {
    // npm 给不出真实百分比：不编数字，走不确定进度条
    barEl.classList.add("indeterminate");
    barEl.style.width = "";
    document.querySelector("#inst-pct").textContent = "";
  } else {
    barEl.classList.remove("indeterminate");
    barEl.style.width = pct.toFixed(1) + "%";
    document.querySelector("#inst-pct").textContent = pct.toFixed(0) + "%";
  }
  logEl.textContent = (p.lines || []).slice(-60).join("\n");
  logEl.scrollTop = logEl.scrollHeight;
}

/**
 * 切换"安装进行中"的界面状态（禁用一键安装、放开取消）。
 * @param on - 是否正在进行。
 */
export function installRunning(on) {
  btnInstall.disabled = on;
  installPanelCancel(on);
}

/**
 * 只控制取消按钮与进度条动画。
 * @param on - 是否正在进行。
 */
export function installPanelCancel(on) {
  document.querySelector("#inst-cancel").disabled = !on;
  if (!on) barEl.classList.remove("indeterminate");
}

/**
 * 「一键安装」：队列在 Rust 侧决定（只装官方通道的 node / dsh），前端不参与。
 */
export async function oneClick() {
  const T = tauri();
  if (!T) return;
  installRunning(true);
  panel.hidden = false;
  document.querySelector("#inst-title").textContent = "准备安装";
  logEl.textContent = "";
  try {
    await T.core.invoke("install_missing");
  } catch (e) {
    toast(String(e));
  } finally {
    installRunning(false);
  }
}
