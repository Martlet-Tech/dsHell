<#
.SYNOPSIS
    本地开发构建：编出 DShell 的 exe + 自带插件，放进 target\ 下的时间戳子目录。

.DESCRIPTION
    一次构建产出一个**自包含**的目录，双击里面那个 exe 就能跑：

        src-tauri\target\20260914-091530\
            dshell.exe
            plugin\dshell-directory-picker\   ← exe 启动时要读它（见下）

    ## 为什么要时间戳子目录

    正在运行的 DShell 会锁住 `target\release\dshell.exe`（exe 被进程占用时
    无法覆盖）。开发时最常见的动作就是「改一行 → 重新编译 → 重启 exe」，
    如果产物固定写在一个路径上，就必须先关掉旧实例才能编译下一版——
    而调试中往往不想关（会话在里头）。

    每次构建用 `yyyyMMdd-HHmmss` 另开一个目录，新旧互不干扰：
    编译永远能成功，你可以留着旧实例，去新目录启动新版做对比。

    ## 为什么 plugin\ 必须和 exe 放一起

    Tauri 的 `bundle.resources` 在 `bundle.active: false`（本项目就是）
    时**不嵌进 exe**，而是导出到 exe 同级的 `plugin\` 目录。exe 启动体检的
    第 ⑥ 项靠这个目录把插件装进 dsh 的 profile——找不到它，那一项会降级成
    黄字，选目录的即时刷新失效。

    所以本脚本**总是把插件一起放好**，不给你留"忘了拷"的机会。

.PARAMETER Clean
    编译前删掉 target\ 全量重建（慢，排除增量缓存问题）。

.PARAMETER Keep
    保留的最近构建个数，默认 5。更老的会自动清理（只删本脚本建的
    `yyyyMMdd-HHmmss` 形态的目录，不碰 cargo 的 release\ / debug\）。

.PARAMETER NoPause
    结束后不停留等按键（给自动化用）。

.EXAMPLE
    .\dev-build.ps1
    产出 target\20260914-091530\{dshell.exe, plugin\}

.EXAMPLE
    .\dev-build.ps1 -Keep 10
    保留最近 10 次构建。
#>
[CmdletBinding()]
param(
    [switch]$Clean,
    [int]$Keep = 5,
    [switch]$NoPause
)

$ErrorActionPreference = 'Stop'

# 本脚本住在 <repo>/scripts/，往上一层是仓库根
$REPO_ROOT = Split-Path $PSScriptRoot -Parent
$TAURI_DIR = Join-Path $REPO_ROOT 'src-tauri'
$TARGET    = Join-Path $TAURI_DIR 'target'
$STAMP     = Get-Date -Format 'yyyyMMdd-HHmmss'
$OUT_DIR   = Join-Path $TARGET $STAMP

function Write-Section($t) {
    Write-Host ''
    Write-Host "── $t " -ForegroundColor Cyan -NoNewline
    Write-Host ('─' * [Math]::Max(0, 58 - $t.Length)) -ForegroundColor DarkGray
}
function Write-Ok($t)   { Write-Host "  [ok]   $t" -ForegroundColor Green }
function Write-Skip($t) { Write-Host "  [skip] $t" -ForegroundColor DarkGray }
function Write-Warn($t) { Write-Host "  [warn] $t" -ForegroundColor Yellow }
function Write-Bad($t)  { Write-Host "  [fail] $t" -ForegroundColor Red }

function Wait-Exit([int]$Code) {
    if (-not $NoPause) {
        Write-Host ''
        Write-Host '  按回车关闭此窗口...' -ForegroundColor DarkGray
        try { [void](Read-Host) } catch { Start-Sleep -Seconds 30 }
    }
    exit $Code
}

Write-Host ''
Write-Host '  DShell 开发构建' -ForegroundColor White
Write-Host "  输出目录：target\$STAMP" -ForegroundColor DarkGray

# ── 前置检查 ────────────────────────────────────────────────────────
Write-Section '前置检查'

$fatal = $false

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Bad 'PATH 里找不到 cargo（装 Rust： https://rustup.rs）'
    $fatal = $true
} else {
    Write-Ok "cargo $((& cargo --version 2>&1 | Select-Object -First 1))"
}

if (-not (Test-Path (Join-Path $TAURI_DIR 'Cargo.toml'))) {
    Write-Bad "找不到 src-tauri\Cargo.toml（仓库结构不对？）"
    $fatal = $true
} else {
    Write-Ok 'src-tauri 正常'
}

# 插件源码：构建会把它作为 resource 导出，缺了产物就是残的
$PLUGIN_SRC = Join-Path $REPO_ROOT 'plugin\dshell-directory-picker'
if (Test-Path (Join-Path $PLUGIN_SRC 'package.json')) {
    Write-Ok '插件源码在（会被一起导出）'
} else {
    Write-Bad "找不到插件源码：$PLUGIN_SRC"
    $fatal = $true
}

# 运行中的实例只是提醒——时间戳目录让它们不冲突
$running = @(Get-Process dshell,DShell-test -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) {
    Write-Skip "有 $($running.Count) 个实例在跑（不冲突：本次产物是新目录）"
}

if ($fatal) {
    Write-Section '结果'
    Write-Bad '前置检查没过，没有开始编译。'
    Wait-Exit 2
}

# ── 清理（可选）─────────────────────────────────────────────────────
if ($Clean) {
    Write-Section '清理'
    # 只删 cargo 的 release\ debug\，保留历史构建目录（它们是你要留着的）
    foreach ($sub in @('release', 'debug')) {
        $p = Join-Path $TARGET $sub
        if (Test-Path $p) {
            try { Remove-Item $p -Recurse -Force -ErrorAction Stop; Write-Ok "已删 target\$sub" }
            catch { Write-Warn "删不掉 target\$sub（被占用）：继续增量构建" }
        }
    }
}

# ── 编译 ────────────────────────────────────────────────────────────
Write-Section '编译（release）'
Write-Host '  cargo 输出如下（出错时下面会汇总真正那几条错误）' -ForegroundColor DarkGray
Write-Host ''

# 编到 target\release（cargo 默认），产物之后再拷进时间戳目录。
# 这样 cargo 的增量缓存跨构建复用，不必每次全量编译。
$started = Get-Date
$rawLog  = Join-Path $env:TEMP ("dshell-devbuild-{0}.log" -f $STAMP)
$exitCode = 1

Push-Location $TAURI_DIR
try {
    # 坑：cargo 把进度和报错都写 stderr，`2>&1` 后在 EAP='Stop' 下会被当成
    # 终止性错误（第一行 Compiling 就中断）。所以临时降到 Continue，
    # 并把 ErrorRecord 拍平成字符串（否则 Tee 出来是红色报错块，
    # 下面挑 error 行的正则也会失效）。退出码只看 $LASTEXITCODE。
    $prev = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & cargo build --release 2>&1 |
            ForEach-Object { if ($_ -is [System.Management.Automation.ErrorRecord]) { $_.Exception.Message } else { "$_" } } |
            Tee-Object -FilePath $rawLog
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $prev
    }
} catch {
    Write-Bad "调用 cargo 失败：$($_.Exception.Message)"
    $exitCode = 1
} finally {
    Pop-Location
}
$elapsed = [Math]::Round(((Get-Date) - $started).TotalSeconds, 1)

# ── 结果 ────────────────────────────────────────────────────────────
Write-Section '结果'

$BIN = Join-Path $TARGET 'release\dshell.exe'

if ($exitCode -ne 0 -or -not (Test-Path $BIN)) {
    Write-Host ''
    Write-Bad "编译失败（cargo 退出码 $exitCode，耗时 $elapsed 秒）"
    Write-Host ''
    $lines = if (Test-Path $rawLog) { @(Get-Content $rawLog) } else { @() }
    $errIdx = @()
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^\s*error(\[|:)') { $errIdx += $i }
    }
    if ($errIdx.Count -eq 0) {
        Write-Host '  没解析到 error 行，末尾 15 行原文：' -ForegroundColor Yellow
        $tail = if ($lines.Count -gt 15) { $lines[($lines.Count - 15)..($lines.Count - 1)] } else { $lines }
        foreach ($l in $tail) { Write-Host "    $l" -ForegroundColor DarkGray }
    } else {
        Write-Host "  发现 $($errIdx.Count) 条错误：" -ForegroundColor Yellow
        foreach ($i in $errIdx) {
            Write-Host ''
            $end = [Math]::Min($i + 6, $lines.Count - 1)
            for ($j = $i; $j -le $end; $j++) {
                Write-Host "    $($lines[$j])" -ForegroundColor $(if ($j -eq $i) { 'Red' } else { 'DarkGray' })
            }
        }
    }
    Write-Host ''
    Write-Host "  完整日志：$rawLog" -ForegroundColor DarkGray
    Wait-Exit $exitCode
}

# ── 组装产物目录 ────────────────────────────────────────────────────
Write-Section "组装 target\$STAMP"

New-Item -ItemType Directory -Force -Path $OUT_DIR | Out-Null
Copy-Item $BIN (Join-Path $OUT_DIR 'dshell.exe') -Force
Write-Ok 'dshell.exe'

# Tauri 把 resource 导出到 target\release\plugin\ —— 原样搬过去。
# 这一步是**必须**的：exe 靠它旁边的 plugin\ 找插件。缺了不会崩，
# 但选目录的即时刷新会静默失效，所以这里宁可硬失败。
$pluginExported = Join-Path $TARGET 'release\plugin'
if (-not (Test-Path $pluginExported)) {
    Write-Bad "cargo 没有导出 plugin\ —— tauri.conf.json 的 bundle.resources 丢了吗？"
    Write-Bad "产物不完整，已放弃。"
    Wait-Exit 3
}
Copy-Item $pluginExported (Join-Path $OUT_DIR 'plugin') -Recurse -Force
$pluginFiles = @(Get-ChildItem (Join-Path $OUT_DIR 'plugin') -Recurse -File)
Write-Ok "plugin\  ($($pluginFiles.Count) 个文件)"

# 插件入口必须在——半个复制比不装更糟
$entry = Join-Path $OUT_DIR 'plugin\dshell-directory-picker\lib\index.js'
if (-not (Test-Path $entry)) {
    Write-Bad "插件入口缺失：$entry"
    Wait-Exit 3
}

# ── 清理旧构建 ──────────────────────────────────────────────────────
$olds = @(Get-ChildItem $TARGET -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -match '^\d{8}-\d{6}$' } |
    Sort-Object Name -Descending |
    Select-Object -Skip $Keep)
if ($olds.Count -gt 0) {
    Write-Section "清理（保留最近 $Keep 次）"
    foreach ($o in $olds) {
        try { Remove-Item $o.FullName -Recurse -Force -ErrorAction Stop; Write-Skip "删 $($o.Name)" }
        catch { Write-Warn "删不掉 $($o.Name)（可能有实例在跑）" }
    }
}

# ── 汇总 ────────────────────────────────────────────────────────────
$exe = Get-Item (Join-Path $OUT_DIR 'dshell.exe')
Write-Section '完成'
Write-Host ''
Write-Host '  构建成功' -ForegroundColor Green
Write-Host ("  耗时 {0} 秒 · exe {1:n2} MB" -f $elapsed, ($exe.Length / 1MB)) -ForegroundColor DarkGray
Write-Host ''
Write-Host '  双击这个（或复制这行去启动）：' -ForegroundColor White
Write-Host "  $(Join-Path $OUT_DIR 'dshell.exe')" -ForegroundColor Cyan
Write-Host ''
Write-Host '  启动后看体检第 ⑥ 项「原生目录选择器（自带插件）」：' -ForegroundColor DarkGray
Write-Host '    绿 = 插件已就绪     黄 = 降级（选目录会退回"要补点一下"）' -ForegroundColor DarkGray
Write-Host ''
Write-Host "  构建日志：$rawLog" -ForegroundColor DarkGray
Write-Host ''

Wait-Exit 0
