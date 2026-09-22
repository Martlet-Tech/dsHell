/**
 * 设置面板的编排层：标签注册表 + 两个标签 + 共享底栏 + 与壳层通信。
 *
 * ## 它住在哪
 *
 * 面板跑在主窗口里的一个全屏 iframe 内（壳层注入的覆盖层，见 `src-tauri/src/settings.rs`）。
 * 父文档是 dsh 页面——**跨源**，所以面板碰不到父文档，也**不能自己关闭自己**：
 * 关闭一律是"回传一个动作给壳层，由壳层移除覆盖层"。
 *
 * ## 两条通道
 *
 *   * 收：壳层 `eval` → 宿主 shim → `postMessage`（`{ __dshell: 1, payload: "<json>" }`）
 *   * 发：`sendBeacon` → `http://127.0.0.1:<port>/_dshell/<nonce>/settings/<action>`
 *
 * 走 `sendBeacon` 而不是 Tauri IPC：面板在 dsh 的文档里，给它开 IPC 等于给 dsh 页面里的
 * 一切内容开壳层的命令。beacon 是简单请求（无预检、无自定义头），够用且不越权。
 *
 * ## 状态是数据，不是文件
 *
 * 标签切换是同一份 DOM 上的显隐（`view.on`），**不是导航**。理由与启动页一样：
 * 导航会销毁文档，而"进度/数据发给一个没有监听者的页面"正是 08 修掉的那类缺陷。
 */

const PAGE = 25;

/** 查询参数：端口与 nonce 由宿主 shim 拼在 iframe 的 URL 上。 */
const query = new URLSearchParams(location.search);
const PORT = query.get("b");
const NONCE = query.get("n");
const PREVIEW = !PORT || !NONCE;
const BASE = PREVIEW ? "" : `http://127.0.0.1:${PORT}/_dshell/${NONCE}/settings/`;

const $ = (id) => document.getElementById(id);

const views = Array.from(document.querySelectorAll(".view"));
const tabsEl = $("tabs");
const noteEl = $("note");
const toastEl = $("toast");
const modal = $("modal");

let toastTimer = null;

/** 面板状态。`catalog` / `about` 由壳层推来，其余是本地 UI 状态。 */
const S = {
  catalog: null,
  about: null,
  error: null,
  active: "versions",
  selected: null,
  rendered: 0,
  primed: false,
  busy: false,
};

// ───────────────────────── 与壳层通信 ─────────────────────────

/** 回传一个动作。地址栏打开（预览）时自动静默：那时没有壳层可回。 */
function beacon(action) {
  if (PREVIEW) {
    toast("预览模式：没有壳层，动作已忽略");
    return;
  }
  try {
    navigator.sendBeacon(BASE + action);
  } catch (e) {
    toast("无法与 DShell 通信：" + String(e));
  }
}

window.addEventListener("message", (e) => {
  const d = e.data;
  if (!d || d.__dshell !== 1) return;
  let msg;
  try {
    msg = JSON.parse(d.payload);
  } catch (err) {
    return;
  }
  if (msg.about) S.about = msg.about;
  if (!S.primed && msg.about) {
    // 选中项由壳层持有：点「确定」提交、点「取消」丢弃。只在首次数据里取，
    // 之后以本地操作为准（否则刷新会把用户刚点的选择顶掉）。
    S.selected = msg.about.selected || null;
    S.primed = true;
  }
  if (msg.type === "data") {
    S.catalog = msg.catalog;
    S.error = null;
    S.busy = false;
  } else if (msg.type === "refresh_error") {
    // 刷新失败**不清列表**：面板上那份旧数据仍然可用，而且带时间戳。
    toast("刷新失败：" + msg.text);
    S.busy = false;
  } else if (msg.type === "error") {
    S.error = msg.text;
    S.busy = false;
  render();
});

// ───────────────────────── 标签与底栏 ─────────────────────────

const TABS = [
  {
    id: "versions",
    title: "dsh 版本管理",
    note: "选一个版本后点「切换」：dsh 会先停止，装完自动重启。",
    render: renderVersions,
  },
  {
    id: "about",
    title: "关于",
    note: "",
    render: renderAbout,
  },
];

function buildTabs() {
  tabsEl.textContent = "";
  for (const t of TABS) {
    const b = document.createElement("button");
    b.textContent = t.title;
    b.onclick = () => setTab(t.id);
    b.dataset.tab = t.id;
    tabsEl.appendChild(b);
  }
}

function setTab(id) {
  S.active = id;
  for (const b of tabsEl.querySelectorAll("button")) {
    b.classList.toggle("on", b.dataset.tab === id);
  }
  for (const v of views) {
    v.classList.toggle("on", v.dataset.view === id);
  }
  const tab = TABS.find((t) => t.id === id);
  noteEl.textContent = tab ? tab.note : "";
  render();
}

/** 底栏：取消=丢弃这次的选择；确定=保留选择。两者都由壳层关闭面板并把 dsh 切回前台。 */
function wireFooter() {
  $("ok").onclick = () => {
    beacon("apply?version=" + encodeURIComponent(S.selected || ""));
  };
  $("cancel").onclick = () => beacon("cancel");
  $("close").onclick = () => beacon("cancel");
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    if (!modal.hidden) {
      hideModal();
      return;
    }
    beacon("cancel");
  });
}

function render() {
  const tab = TABS.find((t) => t.id === S.active);
  if (tab) tab.render();
}

function toast(text) {
  toastEl.textContent = text;
  toastEl.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (toastEl.hidden = true), 3200);
}

// ───────────────────────── 标签：dsh 版本管理 ─────────────────────────

function renderVersions() {
  const c = S.catalog;
  const summary = $("v-summary");
  const src = $("v-source");
  const list = $("v-list");
  const more = $("v-more");

  if (S.error) {
    summary.textContent = "查询失败";
    src.hidden = false;
    src.textContent = S.error;
    list.textContent = "";
    more.textContent = "";
    updateSel();
    return;
  }
  if (!c) {
    summary.textContent = "正在查询 registry…";
    src.hidden = true;
    return;
  }

  const cur = c.current || "未知";
  summary.innerHTML =
    `当前 <b>${esc(cur)}</b>` +
    (c.latest ? ` · 最新 <b>${esc(c.latest)}</b>` : "") +
    (c.registry ? ` · registry ${esc(c.registry)}` : "") +
    ` · 共 ${c.versions.length} 个版本` +
    ageText(c);

  if (!c.npm_global) {
    src.hidden = false;
    src.textContent =
      (c.source_note || "无法确认当前 dsh 由 npm 全局安装") + "（切换已禁用）";
  } else {
    src.hidden = true;
  }

  // 列表按 25 条一批渲染，滚到底部哨兵再出下一批。
  // **诚实说明**：按当前 registry 上 22 个版本，第一批就是全部——分页机制
  // 只有版本数超过 25 之后才真正生效。
  list.textContent = "";
  S.rendered = 0;
  renderMore();
  observeMore();
  updateSel();
}

function renderMore() {
  const c = S.catalog;
  if (!c) return;
  const list = $("v-list");
  const next = c.versions.slice(S.rendered, S.rendered + PAGE);
  for (const v of next) {
    list.appendChild(row(v));
  }
  S.rendered += next.length;
}

/** 滚到底部哨兵 → 再渲染一批。 */
let moreObserver = null;
function observeMore() {
  if (moreObserver) moreObserver.disconnect();
  if (typeof IntersectionObserver !== "function") return;
  moreObserver = new IntersectionObserver(
    (entries) => {
      if (!entries.some((e) => e.isIntersecting)) return;
      if (!S.catalog || S.rendered >= S.catalog.versions.length) return;
      renderMore();
    },
    { root: $("v-scroll"), rootMargin: "120px" }
  );
  moreObserver.observe($("v-more"));
}

function row(v) {
  const li = document.createElement("li");
  li.className = "vrow";
  if (v.current) li.classList.add("cur");
  if (S.selected === v.version) li.classList.add("sel");
  li.dataset.version = v.version;

  const tick = document.createElement("span");
  tick.className = "tick";

  const ver = document.createElement("span");
  ver.className = "ver";
  ver.textContent = v.version;

  const badges = document.createElement("span");
  badges.className = "badges";
  for (const t of v.tags) {
    const i = document.createElement("i");
    // tag 名来自 registry，只用来做 class 后缀——去掉可能破坏选择器的字符
    i.className = "tag " + String(t).replace(/[^a-z0-9-]/gi, "");
    i.textContent = t;
    badges.appendChild(i);
  }

  const mark = document.createElement("span");
  mark.className = "mark";

  // 右列：**当前版本**那行说"当前版本"（那是用户第一眼要找的信息），
  // 其余行说发布日期；两者都缺就空着 —— 不写"未知"占位，那会把一列日期变成一列噪声。
  if (v.current) {
    mark.textContent = "当前版本";
  } else {
    const date = dateText(v.published);
    if (date) {
      mark.textContent = date;
      // 完整时刻放 title：压缩过的日期看不出时区，鼠标一悬停就有确切值
      mark.title = fullDateText(v.published);
    }
  }

  li.append(tick, ver, badges, mark);
  li.onclick = () => {
    S.selected = v.version;
    for (const el of $("v-list").querySelectorAll(".vrow")) {
      el.classList.toggle("sel", el.dataset.version === v.version);
    }
    updateSel();
  };

  return li;
}

/**
 * registry 给的 RFC3339 → 面板上那一列短日期。
 *
 * 三条约定：
 *
 *   * **按本地时区显示**。registry 一律是 UTC（`…Z`），而"哪天发的"对用户是按自己
 *     的钟说的；`new Date()` + `Intl` 干的就是这件事，不需要我们掺和。
 *   * **年份只在不是今年时出现**。列表全是同年的版本时，一列 `2026-09-10` 里那个
 *     `2026` 重复 24 遍、白占宽度；跨年了才把它露出来。
 *   * **解析不了就返回空串**（老缓存 / registry 没给时刻），由调用方决定不显示 —— 
 *     `new Date("")` 是 Invalid Date，`toLocaleDateString` 会吐 "Invalid Date" 上屏。
 */
function dateText(iso) {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const sameYear = d.getFullYear() === new Date().getFullYear();
  return d.toLocaleDateString("zh-CN", {
    year: sameYear ? undefined : "numeric",
    month: "2-digit",
    day: "2-digit",
  });
}

/** 完整到分钟的本地时刻，给 `title` 用（悬停才看到，所以不吝长度）。 */
function fullDateText(iso) {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleString("zh-CN", { hour12: false });
}

function updateSel() {
  const c = S.catalog;
  const el = $("v-sel");
  const btn = $("v-switch");
  const sel = S.selected;
  if (!sel) {
    el.textContent = "未选择目标版本";
  } else {
    const v = `<b>${esc(sel)}</b>`;
    el.innerHTML =
      c && sel === c.current ? `已选 ${v}（与当前相同 → 重装）` : `已选 ${v}`;
  }
  btn.disabled = !sel || !c || !c.npm_global || S.busy;
  $("v-refresh").disabled = S.busy;
}

function askSwitch() {
  const c = S.catalog;
  if (!c || !S.selected) return;
  const target = S.selected;
  const cur = c.current;
  const item = c.versions.find((v) => v.version === target);
  const older = !!(item && item.older);

  $("m-title").textContent = older ? "降级 dsh？" : "切换 dsh 版本";
  // 目标版本的发布日期：降级场景下它是**决策依据**（越旧的版本越可能踩到已有的坑），
  // 所以确认框里也给它一个位置，而不是只在列表上闪一下。
  const when = dateText(item && item.published);
  const stamp = when ? `（${when} 发布）` : "";
  $("m-body").textContent =
    target === cur
      ? `重新安装 ${target}${stamp}（等同修复一次安装）。\n\ndsh 会先停止，装完自动重启。`
      : `当前 ${cur || "未知"} → 目标 ${target}${stamp}。\n\ndsh 会先停止，装完自动重启，进行中的对话会中断。`;

  const warn = $("m-warn");
  const risks = [];
  if (older) {
    // 两条风险都要说，而且都有实证。降级不是"少用几个新功能"那么轻：
    //   · 数据单向：dsh 自己的 AGENTS.md 写了 predecessors imply neither fallback nor
    //     downgrade support。
    //   · **可能直接装不起来**：dsh 用 `^` 范围引用同级包，旧核心会配到新依赖。
    //     实测 0.1.6-alpha.1 —— 新版 @deepseek-ai/dsh-app-boot 删掉了
    //     `watchUserPatches`，旧核心 import 它，dsh 一启动就 SyntaxError 退出。
    risks.push(
      "· <b>数据</b>：会话格式是单向的，旧版可能读不到新版写出的会话。" +
        "<br>· <b>可能装不起来</b>：dsh 的包之间用 <code>^</code> 版本范围引用，旧核心会配到新版依赖" +
        "（实测 <code>0.1.6-alpha.1</code> 就是这样）。" +
        "<br>所以降级会用<b>发布时间窗</b>安装（npm <code>--before</code>，把依赖解析拉回该版本发布时的样子）。"
    );
  }
  // 磁盘空间：换版本要重装 400+ 个包（写 npm 缓存所在的盘）。接近写满的 SSD 上，
  // 写入突发会把整个系统拖住 —— 2026-09-21 实测：系统盘 12G 可用/95% 已用，机器直接死机。
  const free = S.about && S.about.disk ? S.about.disk.free_gb : null;
  if (typeof free === "number" && free < 20) {
    risks.push(
      `· <b>磁盘快满了</b>：系统盘只剩 <b>${free} GB</b>。换版本要重装 400+ 个包` +
        "（写的是 npm 缓存所在的盘），接近写满时磁盘掉速可能拖住整个系统。" +
        "<br>建议先清理，或把缓存挪到别的盘：<code>npm config set cache D:\\npm-cache</code>"
    );
  }
  if (risks.length) {
    warn.hidden = false;
    warn.innerHTML = (older ? "降级有两条风险：" : "切换前提醒：") + "<br>" + risks.join("<br>");
  } else {
    warn.hidden = true;
  }

  $("m-go").textContent = older ? "仍要降级" : "切换";
  modal.hidden = false;
}

function hideModal() {
  modal.hidden = true;
}

// ───────────────────────── 标签：关于 ─────────────────────────

function renderAbout() {
  const a = S.about || {};
  const c = S.catalog || {};
  $("a-version").textContent = a.version || "—";
  $("a-dsh").textContent = c.current || "未知";
  $("a-path").textContent = a.dsh_path || "（使用 PATH 上的 dsh）";
}

function esc(s) {
  return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/**
 * "· 数据 5 分钟前"，过期时补一句"正在后台刷新"。
 *
 * 版本列表来自 2 小时的缓存（否则每开一次面板都要跑 5 条 npm 命令、联网数秒）。
 * 把时间戳显示出来是有意的：用户有权知道眼前这份列表有多旧。
 */
function ageText(c) {
  const s = c.registry_age_secs;
  if (s === null || s === undefined) return "";
  const age =
    s < 60 ? "刚刚" : s < 3600 ? `${Math.floor(s / 60)} 分钟前` : `${Math.floor(s / 3600)} 小时前`;
  return ` · 数据 ${age}` + (c.registry_stale ? "（正在刷新…）" : "");
}

// ───────────────────────── 启动 ─────────────────────────

buildTabs();
wireFooter();
$("v-refresh").onclick = () => {
  S.busy = true;
  $("v-summary").textContent = "正在查询 registry…";
  updateSel();
  beacon("refresh");
};
$("v-switch").onclick = askSwitch;
$("m-cancel").onclick = hideModal;
$("m-go").onclick = () => {
  hideModal();
  S.busy = true;
  updateSel();
  beacon("switch?version=" + encodeURIComponent(S.selected || ""));
};

setTab("versions");
observeMore();

if (PREVIEW) {
  noteEl.textContent = "预览模式（直接在浏览器里打开，未接壳层）";
  $("v-summary").textContent = "预览模式：拿不到壳层推送的数据";
} else {
  // 先告诉壳层"面板起来了"，壳层收到才开始推数据（避免推给还没监听的页面）。
  beacon("ready");
}

// 走到这里说明模块加载并执行成功：让 settings.html 里那段内联兜底闭嘴。
window.__settingsBooted = true;
