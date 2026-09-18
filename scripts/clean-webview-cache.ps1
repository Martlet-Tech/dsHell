<#
.SYNOPSIS
    清掉 DShell 的 WebView2 数据目录（缓存），修「Failed to load plugins」。

.DESCRIPTION
    为什么要有这个脚本：DShell 用 WebView2 显示 dsh 的网页。dsh 把 52 个
    client bundle 拼成**一个**脚本返回给浏览器（约 11 MB），响应头是
    `Cache-Control: public, max-age=31536000, immutable`，WebView2 必须把它
    写进自己的 HTTP 缓存。缓存目录一旦处于坏状态，这个 11 MB 的脚本就写不进去，
    `<script>` 标签随即触发 error 事件，于是启动页停在：

        Failed to load plugins
        failed to import loader entry …：client-modules: bundle script
        /plugins/??…&rev=… failed to load

    实测（2026-09-18）：把那个目录改个名（＝让 WebView2 重建一份）后，
    界面立刻恢复正常。所以"每次启动都进不去"不是缺插件、不是 dsh 后端的问题，
    而是**这个持久化的缓存目录坏了**——它坏了就会一直坏，每次启动都撞同一块。

    行为（刻意做成这样）：
      1. 先做**进程体检**：有 dshell.exe 在跑就说明目录被占用，动不了；
         默认只报告并退出，加 -KillProcesses 才会先结束它（会中断会话）。
      2. 默认**改名而不删除**，改成 `EBWebView.bak-<时间戳>`，随时能还原
         （把名字改回 `EBWebView` 即可）。
      3. 同时报告体积，方便你判断要不要连备份一起清。
      4. 支持 -WhatIf，先看会做什么再决定。

    被点名的插件（通常是 @deepseek-ai/dsh-client-hmr）不是坏掉的那个：
    它是第一个去拉这份共享脚本的，`Promise.all` 遇到第一个失败就整体抛出，
    所以页面上只会显示它一行。

.PARAMETER PurgeBackups
    连历史备份（EBWebView.bak*）一起删除。不加就只处理当前那份。

.PARAMETER KillProcesses
    清理前先结束 dshell.exe。**会中断正在使用 DShell 的会话**，默认关闭。
    不结束它的话，目录被 WebView2 锁着，改名会失败。

.EXAMPLE
    .\clean-webview-cache.ps1
    把当前缓存改名备份（可还原），并报告体积。

.EXAMPLE
    .\clean-webview-cache.ps1 -WhatIf
    只报告将要做什么，不实际改动。

.EXAMPLE
    .\clean-webview-cache.ps1 -KillProcesses
    先结束 DShell，再清（适合界面已经卡死、只能靠脚本收场时）。

.EXAMPLE
    .\clean-webview-cache.ps1 -PurgeBackups
    连旧的 .bak* 备份一起删，腾出磁盘。

.NOTES
    项目根目录 D:\Projects\dsh-shell **不是 git 仓库**，本脚本只做文件系统与进程操作。
    数据目录来自 tauri.conf.json 的 identifier（读不到就退回 com.dshell.poc）。
    清完**必须重启 DShell**：目录在下次启动时自动重建（约 20 MB）。
#>
[CmdletBinding(SupportsShouldProcess)]
param(
    [switch]$PurgeBackups,
    [switch]$KillProcesses
)

$ErrorActionPreference = 'Stop'

# ── 常量 ────────────────────────────────────────────────────────────
# 本脚本住在 <repo>/scripts/，上一层是仓库根。
$REPO_ROOT     = Split-Path $PSScriptRoot -Parent
$CONF          = Join-Path $REPO_ROOT 'src-tauri\tauri.conf.json'
$IDENT_FALLBACK = 'com.dshell.poc'

function Write-Section($text) {
    Write-Host ''
    Write-Host "── $text " -ForegroundColor Cyan -NoNewline
    Write-Host ('─' * [Math]::Max(0, 58 - $text.Length)) -ForegroundColor DarkGray
}
function Write-Ok($t)   { Write-Host "  [ok]   $t" -ForegroundColor Green }
function Write-Skip($t) { Write-Host "  [skip] $t" -ForegroundColor DarkGray }
function Write-Warn($t) { Write-Host "  [warn] $t" -ForegroundColor Yellow }
function Write-Bad($t)  { Write-Host "  [fail] $t" -ForegroundColor Red }

function Get-DirStats($path) {
    $files = @(Get-ChildItem -LiteralPath $path -Recurse -File -Force -ErrorAction SilentlyContinue)
    $bytes = ($files | Measure-Object -Property Length -Sum).Sum
    if ($null -eq $bytes) { $bytes = 0 }
    [pscustomobject]@{ Count = $files.Count; Bytes = [double]$bytes }
}
function Format-Size($bytes) {
    if ($bytes -ge 1GB) { return '{0:N2} GB' -f ($bytes / 1GB) }
    if ($bytes -ge 1MB) { return '{0:N1} MB' -f ($bytes / 1MB) }
    return '{0:N0} KB' -f ($bytes / 1KB)
}

# ── 1. 定位数据目录 ─────────────────────────────────────────────────
Write-Section 'WebView2 数据目录'

$identifier = $IDENT_FALLBACK
if (Test-Path $CONF) {
    try {
        $conf = Get-Content -LiteralPath $CONF -Raw -Encoding utf8 | ConvertFrom-Json
        if ($conf.identifier) { $identifier = $conf.identifier }
    } catch {
        Write-Warn "读不了 $CONF，用默认 identifier：$IDENT_FALLBACK"
    }
} else {
    Write-Warn "找不到 $CONF，用默认 identifier：$IDENT_FALLBACK"
}

$DATA_ROOT = Join-Path $env:LOCALAPPDATA $identifier
$CACHE_DIR = Join-Path $DATA_ROOT 'EBWebView'
Write-Host "  $CACHE_DIR" -ForegroundColor DarkGray

if (-not (Test-Path $DATA_ROOT)) {
    Write-Bad "数据目录不存在（DShell 从没跑过？）：$DATA_ROOT"
    exit 1
}

# ── 2. 进程体检 ─────────────────────────────────────────────────────
Write-Section 'DShell 进程'

$shells = @(Get-Process dshell -ErrorAction SilentlyContinue)
if ($shells.Count -eq 0) {
    Write-Ok '没有运行中的 dshell.exe'
} else {
    foreach ($p in $shells) {
        $exePath = '(路径不可读)'
        try { $exePath = $p.Path } catch { }
        Write-Warn "dshell.exe pid=$($p.Id)  $exePath"
    }
    if (-not $KillProcesses) {
        Write-Host ''
        Write-Warn '目录被 WebView2 锁着，清不掉。'
        Write-Host '         手动关窗（托盘右键 → 完全退出），或加 -KillProcesses。' -ForegroundColor DarkGray
        exit 1
    }
    foreach ($p in $shells) {
        if ($PSCmdlet.ShouldProcess("pid $($p.Id)", 'Stop-Process')) {
            try {
                Stop-Process -Id $p.Id -Force -ErrorAction Stop
                Write-Ok "已结束 dshell.exe pid=$($p.Id)"
            } catch {
                Write-Bad "无法结束 pid=$($p.Id)：$($_.Exception.Message)"
            }
        }
    }
    # 给 Windows 一点时间释放句柄（WebView2 子进程也要退干净）
    Start-Sleep -Milliseconds 1200
}

# ── 3. 现状 ─────────────────────────────────────────────────────────
Write-Section '现状'

if (Test-Path $CACHE_DIR) {
    $s = Get-DirStats $CACHE_DIR
    Write-Host "  当前缓存：$(Format-Size $s.Bytes)（$($s.Count) 个文件）"
} else {
    Write-Skip '当前没有 EBWebView 目录（已经清过，或从未生成）'
}

$backups = @(Get-ChildItem -LiteralPath $DATA_ROOT -Directory -Filter 'EBWebView.bak*' -ErrorAction SilentlyContinue)
if ($backups.Count -gt 0) {
    $total = 0.0
    foreach ($b in $backups) { $total += (Get-DirStats $b.FullName).Bytes }
    Write-Host "  历史备份：$($backups.Count) 份，共 $(Format-Size $total)"
    foreach ($b in $backups) { Write-Host "           $($b.Name)" -ForegroundColor DarkGray }
} else {
    Write-Skip '没有历史备份'
}

# ── 4. 清理 ─────────────────────────────────────────────────────────
Write-Section '清理'

if ($PurgeBackups -and $backups.Count -gt 0) {
    foreach ($b in $backups) {
        if ($PSCmdlet.ShouldProcess($b.FullName, 'Remove-Item -Recurse -Force')) {
            try {
                Remove-Item -LiteralPath $b.FullName -Recurse -Force -ErrorAction Stop
                Write-Ok "已删除备份 $($b.Name)"
            } catch {
                Write-Bad "删除失败 $($b.Name)：$($_.Exception.Message)"
            }
        }
    }
} elseif ($PurgeBackups) {
    Write-Skip '没有备份可删'
}

if (Test-Path $CACHE_DIR) {
    $bakName = 'EBWebView.bak-{0}' -f (Get-Date -Format 'yyyyMMdd-HHmmss')
    if ($PSCmdlet.ShouldProcess($CACHE_DIR, "重命名为 $bakName")) {
        try {
            Rename-Item -LiteralPath $CACHE_DIR -NewName $bakName -ErrorAction Stop
            Write-Ok "已改名为 $bakName（可还原：把名字改回 EBWebView）"
        } catch [System.IO.IOException] {
            Write-Bad "改不动（多半还被占用）：$($_.Exception.Message)"
            Write-Host '         还有 dshell.exe / msedgewebview2.exe 活着？先完全退出再跑。' -ForegroundColor DarkGray
            exit 1
        } catch {
            Write-Bad "改名失败：$($_.Exception.Message)"
            exit 1
        }
    }
}

# ── 5. 收尾 ─────────────────────────────────────────────────────────
Write-Section '完成'
Write-Host ''
Write-Host '  下一步：' -ForegroundColor White
Write-Host '   1. 启动 DShell（目录会在启动时自动重建，约 20 MB）' -ForegroundColor DarkGray
Write-Host '   2. 界面应当正常进入，不再停在 Failed to load plugins' -ForegroundColor DarkGray
Write-Host ''
Write-Host '  想还原这次的清理：把改名后的目录改回 EBWebView 即可。' -ForegroundColor DarkGray
Write-Host '  想腾磁盘： .\clean-webview-cache.ps1 -PurgeBackups' -ForegroundColor Cyan
Write-Host ''
