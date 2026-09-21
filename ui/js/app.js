/**
 * 启动页的编排层：接线事件、切换状态、维持 `window.__dshell` 老通道。
 *
 * ## 状态是数据，不是文件
 *
 * 这个页面有四种状态（体检中 / 安装中 / 失败 / 淡出），它们**共用同一份 DOM
 * 和同一条事件总线**。所以状态切换全都是同一份文档上的函数调用，
 * **不是加载另一个页面** —— 导航会销毁文档，而"导航到一个没有监听者的页面"
 * 正是 08 花大力气修掉的那个缺陷（`about:blank` / `restart_guard`）。
 *
 * ## `window.__dshell` 必须显式挂回 window
 *
 * Rust 侧用 `window.eval("window.__dshell&&window.__dshell.xxx()")` 单向调用
 * （`main.rs` 的 `splash_status` / `splash_fail` / `leave` / `restarting`）。
 * module 有自己的作用域、**不会**把导出挂到 `window` 上，所以这里必须手动挂。
 * 漏挂一个的后果是**静默无反应**（Rust 那个 `&&` 会安静跳过），不是报错。
 */

import { STEP_IDS } from "./labels.js";
import { stage, failEl, panel, logEl, stepsEl, btnInstall, btnBack, btnQuit, skipEl } from "./dom.js";import { decode, tauri } from "./transport.js";
import { toast } from "./toast.js";
import { ensureRow, renderStep } from "./steps.js";
import { paint, installRunning, installPanelCancel, oneClick } from "./install.js";
import { setDone } from "./state.js";

/**
 * 老通道：Rust 单向 `eval` 的落点。**契约，不要改名。**
 */
window.__dshell = {
  /** 启动页上那一行状态文字（base64 载荷）。 */
  statusB64(payload) {
    const li = ensureRow("launch");
    li.dataset.state = "checking";
    li.querySelector(".detail").textContent = decode(payload);
  },
  /** handoff 成功：整页淡出，随后窗口被导航到 dsh。 */
  leave() {
    setDone(true);
    stage.classList.add("leaving");
  },
  /** 失败卡片（base64 载荷，可能含 dsh 的原始输出，必须走转义过的插入方式）。 */
  failB64(payload) {
    const li = ensureRow("launch");
    li.dataset.state = "failed";
    li.querySelector(".detail").textContent = "启动失败";
    failEl.className = "fail show";
    failEl.innerHTML = decode(payload).replace(/\n/g, "<br>");
  },
  /** 「重启 dsh 后端」把窗口导航回本页后调用：把上次留下的失败卡片、
   *  步骤状态和安装面板清干净，否则用户会看到上一次会话的残留结论。 */
  restarting() {
    setDone(false);
    stage.classList.remove("leaving");
    failEl.className = "fail";
    failEl.innerHTML = "";
    panel.hidden = true;
    logEl.textContent = "";
    btnBack.hidden = true;
    document.querySelector("#inst-title").textContent = "正在安装";
    document.querySelector("#inst-pct").textContent = "";
    document.querySelector("#inst-elapsed").textContent = "";
    stepsEl.innerHTML = "";
    STEP_IDS.forEach((id) => renderStep({ id: id, state: "pending", detail: "等待检查" }));
  },
  /** 切换 dsh 版本期间：这条时间线的语义不对（六行"等待检查"杵在安装面板上方），
   *  收起来；同时把「一键安装」与「跳过检查」收走 —— 更新期间这两个动作要么无意义、
   *  要么危险（点下去会在文件正被覆盖的中途去起 dsh）。装完由 Rust 调 `updating(false)`。 */
  updating(on) {
    stepsEl.hidden = !!on;
    btnInstall.hidden = !!on;
    skipEl.hidden = true;
    btnBack.hidden = true;
  },
  /** 更新失败且 dsh 已被停止：把「跳过检查，直接启动」这个出口露出来。
   *  它启动的是**当前已装**的版本 —— 失败后唯一的恢复路径。 */
  updateFailed() {
    skipEl.hidden = false;
  },
  /** 「装完 dsh 起不来」时的回退出口（Rust 只在记下了上一版时调用）。
   *
   *  这一步是必要的：上游用 `^` 范围引用同级包，**降级可能装出一个起不来的组合**
   *  （实测 0.1.6-alpha.1：新版依赖删了个符号，旧核心配上去直接报 SyntaxError 退出）。
   *  那时用户需要的是一个明确的回退目标，而不是自己去查 npm。 */
  offerSwitchBack(b64version) {
    const v = decode(b64version);
    btnBack.hidden = false;
    btnBack.disabled = false;
    btnBack.textContent = "装回 " + v;
    btnBack.onclick = () => {
      btnBack.disabled = true;
      tauri()?.core.invoke("app_switch_back");
    };
  },
};

/**
 * 注册事件监听并做首次接线。
 *
 * **监听就绪后才叫体检** —— 顺序不能反，否则首批 `doctor://step` 事件会丢在
 * 还没注册的窗口上。
 */
async function wire() {
  const T = tauri();
  if (!T) {
    toast("无法与后端通信（__TAURI__ 未注入），请查看日志文件。");
    return;
  }
  await T.event.listen("doctor://step", (e) => renderStep(e.payload));

  await T.event.listen("doctor://done", (e) => {
    const p = e.payload || {};
    const auto = p.auto || [];
    btnInstall.disabled = auto.length === 0;
    btnInstall.textContent = auto.length ? "一键安装" : "无需安装";
    if (p.allOk) {
      btnInstall.hidden = true;
      skipEl.hidden = true;
    } else {
      btnInstall.hidden = false;
      skipEl.hidden = false;
    }
  });

  await T.event.listen("install://progress", (e) => paint(e.payload));

  await T.event.listen("install://done", (e) => {
    const p = e.payload || {};
    // 收尾：停掉不确定进度条、禁用「取消」—— 安装已经结束，它们必须立刻停下来，
    // 否则装完之后面板还在演"正在更新"，用户分不清到底跑完没有。
    installPanelCancel(false);
    if (p.cancelled) {
      document.querySelector("#inst-title").textContent = "已取消";
    } else if (p.ok) {
      document.querySelector("#inst-title").textContent = "安装完成，正在复检…";
    } else {
      document.querySelector("#inst-title").textContent = "安装失败";
      if (p.error) toast(p.error);
    }
  });

  document.querySelector("#inst-cancel").onclick = () => T.core.invoke("install_cancel");
  btnInstall.onclick = oneClick;
  btnQuit.onclick = () => T.core.invoke("app_quit");
  skipEl.onclick = (e) => {
    e.preventDefault();
    T.core.invoke("app_skip_doctor");
  };

  // 监听就绪后再叫体检，事件一条都不会丢
  T.core.invoke("doctor_run").catch((e) => toast(String(e)));
}

// 首屏先把步骤列出来（"等待检查"），体检结果到了再逐行覆盖。
STEP_IDS.forEach((id) => renderStep({ id: id, state: "pending", detail: "等待检查" }));

wire();

// 走到这里说明所有模块都加载并执行成功了。置这个标志让 index.html 里那段
// 内联兜底闭嘴 —— 否则它会为「已恢复」的早期错误报一张假卡片。
window.__dshellBooted = true;
