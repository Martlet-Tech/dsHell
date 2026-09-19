/**
 * 屏幕底部的临时错误提示。
 *
 * 只用于"用户需要立刻知道、但不值得占一整个失败卡片"的情况
 * （交互调用抛错、复制失败等）。真正的失败结论仍走 `fail` 卡片。
 */

import { toastEl } from "./dom.js";

/** 显示一条提示，6 秒后自动消失。 */
export function toast(msg) {
  toastEl.textContent = msg;
  toastEl.hidden = false;
  clearTimeout(toastEl._t);
  toastEl._t = setTimeout(() => (toastEl.hidden = true), 6000);
}
