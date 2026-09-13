/**
 * DShell 的原生目录选择器后端。
 *
 * ## 为什么需要这个包
 *
 * dsh 自带的 `-native` 后端在 Windows 上会在一个**独立 node 子进程**里弹
 * `IFileOpenDialog`，而且是 `Show(null)` —— **没有 owner**。无主窗口必然抢前台，
 * DShell 主窗口随即失焦；WebView2 把失焦窗口判定为「被遮挡的后台窗口」，
 * **挂起整个渲染进程**（实测 11.7 秒）。挂起期间事件与定时器全停，解冻后
 * React 也不提交那次更新，于是用户必须再点一下鼠标才看到新工作区。
 *
 * dsh 的 `-native` 在源码里写得很清楚（`win32-dialog-host.js`）：
 *
 * > 子进程自己把对话框打开成前台：`runFolderDialog` 在 `Show` 之前**合成一次
 * > Alt 按下**，这在「后台宿主 spawn 子进程」时是必要的。
 *
 * 那句注释就是病根的自述：**正因为没有 owner，才需要合成按键去抢前台。**
 *
 * ## 这个包怎么解决它
 *
 * 不在 dsh 侧弹框，而是**转交给 DShell 的 Rust 进程**去弹。DShell 用
 * `tauri-plugin-dialog`，它走 rfd，最终落到 `IFileDialog::Show(owner)`，
 * 而 owner 就是**主窗口自己的 HWND**（`tauri-plugin-dialog-2.7.3/src/commands.rs:130`
 * 的 `set_parent(&window)`）。有主的模态框不会让父窗口失去前台，
 * 所以 WebView2 不挂起，用户也不需要补点。
 *
 * 这正是官方 DSH Desktop 的做法（Electron 的 `dialog.showOpenDialog(window, …)`），
 * 本包是它的 Tauri 对应物。
 *
 * ## 为什么用 HTTP 而不是别的通道
 *
 * dsh 是**独立进程**，DShell 是另一个进程。两者之间最省事、最少权限的通道是
 * loopback HTTP：
 *
 * ```
 * 本包（dsh host 进程） --POST--> 127.0.0.1:<port>/_dsh/desktop/pick-directory
 *                                        |
 *                            DShell Rust 弹原生对话框（有 owner）
 *                                        |
 * 本包 <---- {"path": "..." | null} ------+
 * ```
 *
 * 端口由 DShell 启动时分配（内核选），写在环境变量 `DSHELL_PICKER_PORT` 里，
 * 由 DShell spawn `dsh web` 时注入 —— 这样两边不需要约定固定端口，
 * 也不会和用户机器上别的服务撞车。同时注入的 `DSHELL_PICKER_TOKEN` 是
 * 本次运行的共享令牌，端点据此**只认启动它的那次 dsh**。
 *
 * ## 没有 DShell 时的行为
 *
 * 环境变量不存在（比如用户自己在终端里跑 `dsh web`）时，**本插件不注册**，
 * 让 dsh 的默认 picker 照常工作。绝不能因为装了 DShell 的插件就让普通
 * `dsh web` 失去选目录的能力。
 *
 * @module dshell-directory-picker
 */

/** DShell 注入的环境变量名：原生对话框服务的端口。 */
const PORT_ENV = 'DSHELL_PICKER_PORT'

/** DShell 注入的环境变量名：本次运行的共享令牌。 */
const TOKEN_ENV = 'DSHELL_PICKER_TOKEN'

/** 与 DShell 侧约定的端点路径。 */
const PICK_PATH = '/_dsh/desktop/pick-directory'

/** 单次请求的超时：用户可能盯着对话框想很久，所以给得宽松，只用来防死等。 */
const PICK_TIMEOUT_MS = 10 * 60 * 1000

/**
 * 读 DShell 给的原生对话框端口。
 * @returns 端口号，或 undefined（不在 DShell 里跑）。
 */
function pickerPort() {
  const raw = process.env[PORT_ENV]
  if (typeof raw !== 'string' || raw.trim() === '') return undefined
  const port = Number(raw)
  return Number.isInteger(port) && port > 0 && port < 65536 ? port : undefined
}

/**
 * 读本次运行的共享令牌。
 * @returns 令牌，或 undefined（DShell 没注入，或给了空值）。
 */
function pickerToken() {
  const raw = process.env[TOKEN_ENV]
  return typeof raw === 'string' && raw.trim() !== '' ? raw.trim() : undefined
}

/**
 * 让 DShell 弹一次原生目录选择框。
 * @param signal - 调用方生命周期；中止时放弃这次请求。
 * @param port - DShell 服务端口。
 * @param token - 本次运行的共享令牌。
 * @returns 选中的绝对路径；用户取消时为 null。
 */
async function pickViaDShell(signal, port, token) {
  // `signal` 按 dsh 的 capability 契约是必传的，但**不假设它一定在**：
  // 这里要是抛 TypeError，症状是"选不了目录"，而真实原因（调用方没传 signal）
  // 会藏在栈里很难看出来。缺省当作"永不中止"更符合意图。
  if (signal?.aborted) throw new Error('native directory picker aborted')

  // 超时用 AbortSignal.timeout 叠加调用方的 signal：任一触发都放弃。
  // 不合并的话，用户关掉页签时这个请求会一直挂着。
  const timeout = AbortSignal.timeout(PICK_TIMEOUT_MS)
  const combined = signal === undefined ? timeout : AbortSignal.any([signal, timeout])

  const headers = { accept: 'application/json' }
  if (token !== undefined) headers['x-dshell-token'] = token

  let response
  try {
    response = await fetch(`http://127.0.0.1:${port}${PICK_PATH}`, {
      method: 'POST',
      headers,
      signal: combined,
    })
  } catch (cause) {
    // 调用方主动中止要如实抛出，不能伪装成"取消选择"。
    if (signal?.aborted) throw new Error('native directory picker aborted')
    throw new Error(`dshell directory picker is unreachable on port ${port}: ${String(cause)}`)
  }

  if (!response.ok) {
    throw new Error(`dshell directory picker failed: HTTP ${response.status}`)
  }

  let body
  try {
    body = await response.json()
  } catch (cause) {
    throw new Error(`dshell directory picker returned invalid JSON: ${String(cause)}`)
  }

  if (typeof body !== 'object' || body === null || !('path' in body)) {
    throw new Error('dshell directory picker returned an invalid response')
  }
  const path = body.path
  if (path !== null && typeof path !== 'string') {
    throw new Error('dshell directory picker returned an invalid path')
  }
  return path
}

/**
 * 插件名（cordis 用它做日志与依赖标识）。
 */
export const name = 'dshell-directory-picker'

/**
 * 依赖的服务。
 *
 * `directoryPicker` 是 **dsh 的 `-auto` 后端**注册的。声明这个依赖有两个作用，
 * 而且第二个才是关键：
 *
 *  1. 保证本插件在**它之后**启动（dsh 的服务先就位）；
 *  2. 保证 dsh 那个服务**确实存在** —— 我们的接管是"换掉已有的"，不是"从零建
 *     一个"。如果哪天 dsh 不再挂 picker，本插件的 `apply` 不会被调用，
 *     而不是留下一个"谁都提供不了这个服务"的坏状态。
 *
 * 为什么不像最初那样"禁用 dsh 的行、由我们独家提供"：因为本插件在拿不到
 * `DSHELL_PICKER_PORT` 时**故意不注册**（普通 `dsh web` 必须照常能用）。
 * 两个条件叠加就会出现"一个后端都没有" ⇒ 界面上的「添加工作区」按钮消失。
 * 详见本包 `cordis.patch.yml` 的完整说明。
 */
export const inject = ['directoryPicker']

/**
 * 插件主体：把 dsh 已注册的 `directoryPicker` 的能力**换成**转交 DShell 的实现。
 *
 * ## 替换是怎么做到的（以及为什么不重新注册）
 *
 * 直觉做法是 `new MyPicker(ctx)` 让它重新 provide 一次。**行不通**，cordis 在
 * `reflect.provide` 里明确拒绝：
 *
 * ```js
 * if (this.store[key]) throw new Error(`service "${name}" has been registered at <...>`)
 * ```
 *
 * 那先 `dispose()` 掉 dsh 那个 fiber 再注册呢？也不行——`reflect.set` 有一句
 * `if (impl.fiber !== this.ctx.fiber) throw`，服务实现**只能由拥有它的 fiber
 * 改写**；强行卸载别人的 fiber 会连带拆掉那个插件的一切，副作用远大于收益。
 *
 * 所以这里用**最小侵入**的做法：拿住服务实例本身，把它内部的 capability
 * 换掉。`DirectoryPicker` 只有一个对外方法 `capability()`，消费方
 * （`ui-workspace`）读的也只有它，所以改这一个点就等于换掉了整个交互后端，
 * 而**服务的所有权、生命周期、依赖关系全都没动**。
 *
 * 这也让"卸载还原"变得简单：把原方法放回去即可，不需要重建任何东西。
 *
 * @param ctx - host 侧 cordis 根上下文。
 */
export function apply(ctx) {
  const port = pickerPort()
  if (port === undefined) {
    // 不在 DShell 里（旧版 DShell，或用户在终端自己跑的 `dsh web`）：
    // 什么都不做，dsh 原来的 picker 保持在工作。这正是 `inject` 声明和
    // patch 层都刻意保留 dsh 那一行的原因。
    ctx.logger?.info?.(
      `dshell-directory-picker: ${PORT_ENV} is not set — the built-in picker stays in charge`,
    )
    return
  }
  const token = pickerToken()

  // `inject` 已保证这个服务就绪，所以这里同步拿得到实例。
  const service = ctx.directoryPicker
  if (service === undefined || typeof service.capability !== 'function') {
    // 不该发生；真发生了就保持 dsh 的行为，绝不要制造"没人提供这个服务"的空洞。
    ctx.logger?.warn?.(
      'dshell-directory-picker: directoryPicker has an unexpected shape — leaving the chooser alone',
    )
    return
  }

  ctx.logger?.info?.(
    `dshell-directory-picker: native chooser delegated to DShell on 127.0.0.1:${port}`,
  )

  // 稳定的 capability 对象：消费方会跨调用缓存它，每次返回新对象会破坏那个假设。
  const capability = {
    kind: 'native',
    pick: (signal) => pickViaDShell(signal, port, token),
  }

  // 只换 `capability` 这一个方法，并把原来的留着以便还原。
  const original = service.capability
  service.capability = () => capability

  // 挂在 cordis 的生命周期上：本插件卸载/热重载时自动还原，
  // 不会留下一个失灵的服务。
  ctx.effect(
    () => () => {
      if (service.capability !== original) service.capability = original
    },
    'dshell-directory-picker: DShell native chooser',
  )
}

export { pickViaDShell, pickerPort, pickerToken, PICK_PATH, PORT_ENV, TOKEN_ENV }
