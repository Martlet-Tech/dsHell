/**
 * DOM 查询与元素引用。
 *
 * 页面元素在模块加载时一次取好（启动页是静态结构，元素不会中途出现或消失）。
 */

/** `document.querySelector` 的简写。 */
export const $ = (s) => document.querySelector(s);

export const stage = $("#stage");
export const stepsEl = $("#steps");
export const failEl = $("#fail");
export const panel = $("#install");
export const barEl = $("#inst-bar");
export const logEl = $("#inst-log");
export const btnInstall = $("#btn-install");
export const btnQuit = $("#btn-quit");
export const skipEl = $("#skip");
export const toastEl = $("#toast");
