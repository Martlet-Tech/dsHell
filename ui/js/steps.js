/**
 * 体检步骤时间线：一行一个检查项，含状态图标、说明、提示与「指定…」按钮。
 *
 * 状态由 Rust 侧推来（`doctor://step` 事件），这里只负责画。
 */

import { LABELS, PICKABLE } from "./labels.js";
import { stepsEl } from "./dom.js";
import { tauri } from "./transport.js";
import { toast } from "./toast.js";
import { isDone } from "./state.js";

/**
 * 取得（必要时创建）某一步的 `<li>`。
 * @param id - 步骤 id，与 `doctor::IDS` 对应。
 * @returns 该步的列表项元素。
 */
export function ensureRow(id) {
  let li = stepsEl.querySelector('.step[data-id="' + id + '"]');
  if (li) return li;
  li = document.createElement("li");
  li.className = "step";
  li.dataset.id = id;
  li.dataset.state = "pending";
  li.innerHTML =
    '<span class="ico"></span>' +
    '<div class="body"><b></b><span class="detail"></span><div class="hint"></div></div>' +
    '<div class="acts"></div>';
  li.querySelector("b").textContent = LABELS[id] || id;
  stepsEl.appendChild(li);
  return li;
}

/**
 * 按 Rust 推来的步骤对象重画一行。
 * @param s - `Step { id, label?, state, detail, hint? }`。
 */
export function renderStep(s) {
  if (!s || !s.id) return;
  const li = ensureRow(s.id);
  li.dataset.state = s.state || "pending";
  if (s.label) li.querySelector("b").textContent = s.label;
  li.querySelector(".detail").textContent = s.detail || "";
  // 淡出中：不再挂交互元素，避免用户点到一个正在离开的页面
  if (isDone()) return;

  const acts = li.querySelector(".acts");
  acts.textContent = "";
  if (PICKABLE.indexOf(s.id) >= 0) {
    const b = document.createElement("button");
    b.textContent = "指定…";
    b.onclick = () => pick(s.id, b);
    acts.appendChild(b);
  }

  const hint = li.querySelector(".hint");
  hint.textContent = "";
  const showHint =
    s.hint && (s.state === "missing" || s.state === "failed" || s.state === "warn");
  if (showHint) {
    if (s.hint.command) {
      const code = document.createElement("code");
      code.textContent = s.hint.command;
      hint.appendChild(code);
      const cp = document.createElement("button");
      cp.textContent = "复制";
      cp.style.fontSize = "11px";
      cp.style.padding = "3px 8px";
      cp.onclick = async () => {
        try {
          await navigator.clipboard.writeText(s.hint.command);
          cp.textContent = "已复制";
        } catch (_) {
          cp.textContent = "复制失败";
        }
        setTimeout(() => (cp.textContent = "复制"), 1400);
      };
      hint.appendChild(cp);
    }
    if (s.hint.url) {
      const a = document.createElement("a");
      a.href = s.hint.url;
      a.textContent = "官方页面";
      hint.appendChild(a);
    }
  }
}

/**
 * 「指定…」：弹原生文件选择框，把路径交给 Rust **就地验证**一次
 * （跑不出东西就不接受，绝不留到后面才炸）。
 * @param id - 步骤 id。
 * @param btn - 触发按钮（用来显示"验证中…"并临时禁用）。
 */
async function pick(id, btn) {
  const T = tauri();
  if (!T) return;
  try {
    const p = await T.dialog.open({
      multiple: false,
      directory: false,
      title: "选择 " + id + " 的可执行文件",
      filters: [{ name: "可执行文件", extensions: ["exe", "cmd", "bat"] }],
    });
    if (!p) return;
    const old = btn.textContent;
    btn.textContent = "验证中…";
    btn.disabled = true;
    try {
      const step = await T.core.invoke("doctor_set_path", { id: id, path: p });
      renderStep(step);
    } finally {
      btn.textContent = old;
      btn.disabled = false;
    }
  } catch (e) {
    toast(String(e));
  }
}
