/**
 * 启动页的共享可变状态。
 *
 * 单独成文件的原因：这个标志被两侧读写 —— `steps.js` 的 `renderStep` 读它
 * （淡出期间不再挂交互元素），`app.js` 的 `leave()` / `restarting()` 写它。
 * 放在任一侧都会让另一侧产生反向依赖，所以提出来做中性的一层。
 */

/** 页面是否正在淡出（handoff 中）。 */
let done = false;

/** 淡出中吗？ */
export function isDone() {
  return done;
}

/** 设置淡出状态。 */
export function setDone(value) {
  done = value;
}
