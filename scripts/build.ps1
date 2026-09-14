<#
.SYNOPSIS
    编译 DShell 的 exe，结果留在窗口里让你看完再关。

.DESCRIPTION
    为什么要有这个脚本：在 DSH 会话里跑 `cargo build` 时，构建进程会挂在会话的
    进程树上——会话一断，构建（和它拉起来的 rustc）就被一起收走，于是"编译到一半
    人没了"。把构建放到**独立窗口**里跑，生命周期就归你，不归会话。

    行为（刻意做成这样）：
      1. 先做**前置体检**：cargo 在不在、源码目录在不在、有没有 dshell.exe
         正锁着 target 目录——问题在编译前就说清楚，而不是等 rustc 报一堆错。
      2. 编译到独立构建目录 `target-build\`，**不动** `target\`
         （那里常被正在运行的 DShell 锁着，动了会连带失败）。
      3. 编译结束后**不关窗**，把结果大字写出来：成功给出 exe 路径和大小，
         失败给出**真正那几条 rustc 错误**（不是被截断的尾巴）。
      4. 停在「按回车关闭」等你点。

    默认用 release（= 你平时双击的那个）。加 -Debug 编 debug 版（快得多，
    适合只验证编译能不能过）。

.PARAMETER Dev
    编 debug 版（不优化，几分钟→几十秒），产物在 target-build\debug\dshell.exe。
    注意参数名是 `-Dev` 不是 `-Debug`：`-Debug` 是 PowerShell 的**内置公共参数**，
    重名会让整个脚本在解析期就报 "parameter defined multiple times"。

.PARAMETER Clean
    编译前删掉 target-build\，做一次全新的全量构建（慢，但排除增量缓存的干扰）。

.PARAMETER NoPause
    编译完直接退出，不停下来等按键（给自动化/CI 用）。

.EXAMPLE
    .\build.ps1
    编 release 版，结果留在窗口里。

.EXAMPLE
    .\build.ps1 -Dev
    编 debug 版，只验证能不能编过。

.EXAMPLE
    .\build.ps1 -Clean
    全量重建 release。

.NOTES
    项目根目录 D:\Projects\dsh-shell **不是 git 仓库**，本脚本只做文件系统 + cargo 调用。
    源码在 src-on-github\src-tauri；本脚本不修改任何源码。
#>
[CmdletBinding()]
param(
    [switch]$Dev,
    [switch]$Clean,
    [switch]$NoPause
)

$ErrorActionPreference = 'Stop'

# ── 常量 ────────────────────────────────────────────────────────────
# 从 `$PSScriptRoot` 往上一层就是仓库根（本脚本住在 `<repo>/scripts/`）。
$REPO_ROOT  = Split-Path $PSScriptRoot -Parent
$TAURI_DIR  = Join-Path $REPO_ROOT 'src-tauri'
$TARGET_DIR = Join-Path $TAURI_DIR 'target-build'
$PROFILE    = if ($Dev) { 'debug' } else { 'release' }

function Write-Section($text) {
    Write-Host ''
    Write-Host "── $text " -ForegroundColor Cyan -NoNewline
    Write-Host ('─' * [Math]::Max(0, 58 - $text.Length)) -ForegroundColor DarkGray
}
function Write-Ok($t)   { Write-Host "  [ok]   $t" -ForegroundColor Green }
function Write-Skip($t) { Write-Host "  [skip] $t" -ForegroundColor DarkGray }
function Write-Warn($t) { Write-Host "  [warn] $t" -ForegroundColor Yellow }
function Write-Bad($t)  { Write-Host "  [fail] $t" -ForegroundColor Red }

# 结束时停住，等用户按键（除非 -NoPause）
function Wait-Exit {
    param([int]$Code)
    if (-not $NoPause) {
        Write-Host ''
        Write-Host '  按回车关闭此窗口...' -ForegroundColor DarkGray
        try { [void](Read-Host) } catch { Start-Sleep -Seconds 30 }
    }
    exit $Code
}

Write-Host ''
Write-Host '  DShell 构建' -ForegroundColor White
Write-Host "  配置：$PROFILE        目录：$TAURI_DIR" -ForegroundColor DarkGray

# ── 1. 前置体检 ─────────────────────────────────────────────────────
Write-Section '前置体检'

$fatal = $false

# cargo
$cargo = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $cargo) {
    Write-Bad '找不到 cargo（Rust 工具链没装或不在 PATH 里）'
    Write-Host '         装一个： https://rustup.rs' -ForegroundColor DarkGray
    Write-Host '         已装但不在 PATH 的话，重启窗口再试。' -ForegroundColor DarkGray
    $fatal = $true
} else {
    $ver = (& cargo --version 2>&1 | Select-Object -First 1)
    Write-Ok "cargo $ver"
}

# 源码目录
if (-not (Test-Path $TAURI_DIR)) {
    Write-Bad "源码目录不存在：$TAURI_DIR"
    $fatal = $true
} else {
    $mainRs = Join-Path $TAURI_DIR 'src\main.rs'
    if (Test-Path $mainRs) {
        Write-Ok '源码目录正常（src\main.rs 在）'
    } else {
        Write-Bad "找不到 src\main.rs，目录结构不对？"
        $fatal = $true
    }
}

# 正在运行的 DShell：不影响本脚本（我们编到 target-build），但提醒你测的是哪个
$running = @(Get-Process dshell -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) {
    foreach ($p in $running) {
        $pth = '(路径不可读)'
        try { $pth = $p.Path } catch { }
        Write-Warn "有一个 dshell.exe 在跑：pid=$($p.Id)"
        Write-Host "           $pth" -ForegroundColor DarkGray
    }
    Write-Host '         编译不受影响（产物在 target-build）。' -ForegroundColor DarkGray
    Write-Host '         但要测新功能，得先「完全退出」它再开新 exe。' -ForegroundColor DarkGray
} else {
    Write-Ok '没有运行中的 dshell.exe'
}

if ($fatal) {
    Write-Section '结果'
    Write-Bad '前置体检没过，没有开始编译。'
    Wait-Exit 2
}

# ── 2. 清理（可选）───────────────────────────────────────────────────
if ($Clean) {
    Write-Section '清理'
    if (Test-Path $TARGET_DIR) {
        try {
            Remove-Item -LiteralPath $TARGET_DIR -Recurse -Force -ErrorAction Stop
            Write-Ok '已删除 target-build\（全量重建）'
        } catch {
            Write-Warn "删不掉 target-build\（可能被占用）：$($_.Exception.Message)"
            Write-Host '         继续用增量构建。' -ForegroundColor DarkGray
        }
    } else {
        Write-Skip 'target-build\ 不存在'
    }
}

# ── 3. 编译 ─────────────────────────────────────────────────────────
Write-Section "编译（$PROFILE）"
Write-Host '  cargo build 输出如下（这是给编译器看的，出错时下面会有汇总）' -ForegroundColor DarkGray
Write-Host ''

$env:CARGO_TARGET_DIR = $TARGET_DIR
$cargoArgs = @('build')
if (-not $Dev) { $cargoArgs += '--release' }

# cargo 必须**在 Cargo.toml 所在目录**里跑：本脚本在项目根目录，而 Cargo.toml 在
# src-on-github\src-tauri。用 Push/Pop-Location 而不是 Set-Location：前者保证
# 无论中途怎么退出都会回到原来的目录。
$started = Get-Date
$rawLog  = Join-Path $env:TEMP ("dshell-build-{0}.log" -f (Get-Date -Format 'HHmmss'))
$exitCode = 1
Push-Location $TAURI_DIR
try {
    # 这里有两个坑，别顺手改回去：
    #  1) cargo 把进度和报错统统写到 stderr。一旦用 `2>&1` 合并，在
    #     $ErrorActionPreference='Stop' 下 PowerShell 会把 stderr 行当成**终止性错误**
    #     （NativeCommandError），于是第一行 "Compiling ..." 就把整个构建打断
    #     （实测：7 秒即退出，一个字都没编出来）。所以这里临时降到 'Continue'。
    #  2) 降级后 stderr 变成 ErrorRecord 对象，直接喂给 Tee-Object 会被渲染成带
    #     "At line ... / CategoryInfo" 的红色报错块：既刷屏，又让下面挑 error 行的
    #     正则（^\s*error）失效。所以进 Tee 之前先统一转成普通字符串。
    # 退出码始终以 $LASTEXITCODE 为准。
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & cargo @cargoArgs 2>&1 |
            ForEach-Object { if ($_ -is [System.Management.Automation.ErrorRecord]) { $_.Exception.Message } else { "$_" } } |
            Tee-Object -FilePath $rawLog
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $prevEap
    }
} catch {
    Write-Bad "调用 cargo 失败：$($_.Exception.Message)"
    $exitCode = 1
} finally {
    Pop-Location
}
$elapsed = [Math]::Round(((Get-Date) - $started).TotalSeconds, 1)

# ── 4. 结果 ─────────────────────────────────────────────────────────
Write-Section '结果'

$exe = Join-Path $TARGET_DIR "$PROFILE\dshell.exe"

if ($exitCode -eq 0 -and (Test-Path $exe)) {
    $item = Get-Item $exe
    Write-Host ''
    Write-Host '  编译成功' -ForegroundColor Green
    Write-Host ''
    Write-Host "  耗时：$elapsed 秒" -ForegroundColor DarkGray
    Write-Host "  大小：$([Math]::Round($item.Length / 1MB, 2)) MB" -ForegroundColor DarkGray
    Write-Host ''
    Write-Host '  exe 路径（复制这行去双击/启动）：' -ForegroundColor White
    Write-Host "  $exe" -ForegroundColor Cyan
    Write-Host ''
    Write-Host '  下一步：' -ForegroundColor White
    Write-Host '   1. 先把正在跑的 DShell「完全退出」（托盘右键 → 完全退出）' -ForegroundColor DarkGray
    Write-Host '   2. 再启动上面这个 exe' -ForegroundColor DarkGray
    Write-Host ''
    Write-Host "  完整构建日志：$rawLog" -ForegroundColor DarkGray
} else {
    Write-Host ''
    Write-Bad "编译失败（cargo 退出码 $exitCode，耗时 $elapsed 秒）"
    Write-Host ''

    # 只挑真正的错误行：rustc 的 `error[...]` / `error:` 以及紧跟的 --> 位置行。
    # 不打印整份日志——那会把关键几行淹掉。
    $lines = @()
    if (Test-Path $rawLog) { $lines = Get-Content $rawLog }
    $errIdx = @()
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^\s*error(\[|:)') { $errIdx += $i }
    }

    if ($errIdx.Count -eq 0) {
        Write-Host '  没有解析到 error 行，末尾 15 行原文：' -ForegroundColor Yellow
        $tail = if ($lines.Count -gt 15) { $lines[($lines.Count - 15)..($lines.Count - 1)] } else { $lines }
        foreach ($l in $tail) { Write-Host "    $l" -ForegroundColor DarkGray }
    } else {
        Write-Host "  发现 $($errIdx.Count) 条错误：" -ForegroundColor Yellow
        foreach ($i in $errIdx) {
            Write-Host ''
            # 错误行 + 后面最多 6 行（通常是 --> 位置 和源码片段）
            $end = [Math]::Min($i + 6, $lines.Count - 1)
            for ($j = $i; $j -le $end; $j++) {
                $isHead = ($j -eq $i)
                Write-Host "    $($lines[$j])" -ForegroundColor $(if ($isHead) { 'Red' } else { 'DarkGray' })
            }
        }
    }
    Write-Host ''
    Write-Host "  完整构建日志：$rawLog" -ForegroundColor DarkGray
    Write-Host '  把它发给我，我来改。' -ForegroundColor DarkGray
}

Write-Host ''
Wait-Exit $exitCode
