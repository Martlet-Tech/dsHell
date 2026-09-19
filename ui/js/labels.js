/**
 * 体检步骤的展示文案。
 *
 * id 与 Rust 侧 `doctor::IDS`（`doctor.rs`）一一对应 —— 那边是唯一权威，
 * 这里只负责中文名。
 */

export const LABELS = {
  webview: "WebView2 运行环境",
  node: "Node.js 运行时",
  npm: "npm 包管理器",
  dsh: "dsh 命令行（DeepSeek Harness）",
  profile: "dsh 配置目录可写",
  picker: "原生目录选择器（自带插件）",
  launch: "启动 DeepSeek Harness",
};

/** 支持「指定…」手动选路径的步骤：只有这几项对应真实的可执行文件。 */
export const PICKABLE = ["node", "npm", "dsh"];

/** 首屏要预渲染的步骤 id（不含 `launch` —— 它由后端事件动态产生）。 */
export const STEP_IDS = ["webview", "node", "npm", "dsh", "profile", "picker"];
