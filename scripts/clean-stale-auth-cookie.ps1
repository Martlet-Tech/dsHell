<#
.SYNOPSIS
    只删「非本代端口」的 dsh-auth-* cookie，修 HTTP 431（请求头过大）。

.DESCRIPTION
    为什么要有这个脚本：DShell 用 `dsh web --port 0`，每次启动换随机端口。dsh 的
    浏览器会话 cookie 名 = `dsh-auth-` + base64url(sha256(authority))，而 authority
    带端口（`127.0.0.1:64618`）→ 每次启动 mint 一个**全新名字**的 cookie，Max-Age
    默认 30 天，一条都不失效。

    浏览器却只按 host 存 cookie（Domain 不含端口），这几十条在它眼里互不相干，于是
    请求时**全部一起发**。攒到约 68 条（每条序列化 236 字节）就越过 dsh 那个 Node
    `http` 服务器的 `maxHeaderSize`（16384 字节），由**服务端**直接回 431，界面进不去。
    实测（2026-09-21）：72 条 × 236 ≈ 17.5 KB > 16 KB，超线仅约 1.2 KB。

    与 clean-webview-cache.ps1 的区别：那个清**整个** EBWebView（连 HTTP 缓存一起，
    界面设置全丢）；本脚本只动 cookie，且**保留本代端口的 cookie**，其余 cookie
    （非 dsh-auth）一律不碰。

    保留端口怎么定：
      1. 显式给 -KeepPort；
      2. 不给就取日志 %USERPROFILE%\.dshell\dshell-poc.log 里**最后一次** `dsh web:` 的端口；
      3. 都拿不到就删掉所有 dsh-auth-*（等价于 cookie 全清，但仍然只动这些）。

    行为：
      - 先做进程体检：有 dshell.exe 在跑就报告并退出（SQLite 被 WebView2 锁着，
        改了也会被回写覆盖）。本脚本**不做 -KillProcesses**：只删 cookie 不值得中断会话，
        要强清请用 clean-webview-cache.ps1。
      - 改的是 SQLite 表，改前把 Cookies/-wal/-shm 三件套复制到临时目录留档。
      - 支持 -DryRun：只报告将删多少条，不写回。

.PARAMETER KeepPort
    要保留的端口（当前 dsh 后端的端口）。不给就从日志猜。

.PARAMETER DataRoot
    WebView2 数据根目录。默认从 tauri.conf.json 的 identifier 推
    `%LOCALAPPDATA%\<identifier>`。

.PARAMETER DryRun
    只报告，不写回。

.EXAMPLE
    # 完全退出 DShell 后，删掉除「日志里最后那个端口」以外的全部 dsh-auth-* cookie
    pwsh -File .\scripts\clean-stale-auth-cookie.ps1

.EXAMPLE
    pwsh -File .\scripts\clean-stale-auth-cookie.ps1 -DryRun

.NOTES
    脚本是 UTF-8 无 BOM，Windows PowerShell 5.1 按 GBK 读会报解析错，请用 PowerShell 7（pwsh）。
    外层 D:\Projects\dshell 不是 git 仓库，本脚本只做文件系统与进程操作。
#>
[CmdletBinding(SupportsShouldProcess)]
param(
    [int]$KeepPort = 0,
    [string]$DataRoot,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'

$HOST_KEY   = '127.0.0.1'
$PREFIX     = 'dsh-auth-'
$SIZE_LIMIT = 16384          # Node http.maxHeaderSize 默认值
$SINGLE     = 236            # 单条 cookie 序列化字节（实测 encrypted_value 字符长度）

function Write-Section($text) {
    Write-Host ''
    Write-Host "-- $text " -ForegroundColor Cyan -NoNewline
    Write-Host ('-' * [Math]::Max(0, 58 - $text.Length)) -ForegroundColor DarkGray
}
function Write-Ok($t)   { Write-Host "  [ok]   $t" -ForegroundColor Green }
function Write-Skip($t) { Write-Host "  [skip] $t" -ForegroundColor DarkGray }
function Write-Warn($t) { Write-Host "  [warn] $t" -ForegroundColor Yellow }
function Write-Bad($t)  { Write-Host "  [fail] $t" -ForegroundColor Red }

# ---- 1. 定位数据目录 --------------------------------------------------
Write-Section 'WebView2 数据目录'

if (-not $DataRoot) {
    $repo = Split-Path $PSScriptRoot -Parent
    $conf = Join-Path $repo 'src-tauri\tauri.conf.json'
    $identifier = 'com.dshell.poc'
    if (Test-Path $conf) {
        try {
            $j = Get-Content -LiteralPath $conf -Raw -Encoding utf8 | ConvertFrom-Json
            if ($j.identifier) { $identifier = $j.identifier }
        } catch { Write-Warn "读不了 $conf，用默认 identifier" }
    }
    $DataRoot = Join-Path $env:LOCALAPPDATA $identifier
}

$DB = Join-Path $DataRoot 'EBWebView\Default\Network\Cookies'
Write-Host "  $DB" -ForegroundColor DarkGray
if (-not (Test-Path $DB)) {
    Write-Bad "找不到 cookie 库（DShell 从没跑过？）：$DB"
    exit 1
}

$node = Get-Command node -ErrorAction SilentlyContinue
if (-not $node) {
    Write-Bad 'PATH 上没有 node，无法计算 cookie 名（本脚本内嵌 Node 只为免维护 sha256 实现）'
    exit 1
}

# ---- 2. 进程体检 ------------------------------------------------------
Write-Section 'DShell 进程'

$shells = @(Get-Process dshell -ErrorAction SilentlyContinue)
if ($shells.Count -gt 0) {
    foreach ($p in $shells) { Write-Warn "dshell.exe pid=$($p.Id) 还在跑" }
    Write-Bad 'cookie 库被 WebView2 占着，改了也会被回写覆盖。'
    Write-Host '         请托盘右键 → 完全退出 DShell，再跑本脚本。' -ForegroundColor DarkGray
    exit 1
}
Write-Ok '没有运行中的 dshell.exe'

# ---- 3. 定保留端口 ----------------------------------------------------
Write-Section '保留端口'

if ($KeepPort -eq 0) {
    $log = Join-Path $env:USERPROFILE '.dshell\dshell-poc.log'
    if (Test-Path $log) {
        $last = Select-String -LiteralPath $log -Pattern 'dsh web: http://127\.0\.0\.1:(\d+)/' |
                Select-Object -Last 1
        if ($last) {
            $KeepPort = [int]$last.Matches[0].Groups[1].Value
            Write-Ok "从日志取到最后一次启动端口：$KeepPort"
        }
    }
}
if ($KeepPort -eq 0) {
    Write-Warn '拿不到端口，将删除**所有** dsh-auth-*（不影响其它 cookie）'
    $keepName = $null
    $keepDesc = '(无)'
} else {
    $keepName = & node -e "process.stdout.write('dsh-auth-'+require('node:crypto').createHash('sha256').update('$HOST_KEY'+':'+$KeepPort).digest('base64url'))"
    $keepDesc = "$keepName  (authority=$HOST_KEY`:$KeepPort)"
}
Write-Host "  保留：$keepDesc" -ForegroundColor DarkGray

# ---- 4. 现状 ----------------------------------------------------------
Write-Section '现状'

# 注：cookie 明文在 value 列（通常为空），真正序列化的是 encrypted_value（BLOB）
$sqlite = Get-Command sqlite3 -ErrorAction SilentlyContinue
$tmp = Join-Path $env:TEMP ('dshell-cookie-{0}' -f (Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Path $tmp -Force | Out-Null

$files = @($DB) + @("$DB-wal", "$DB-shm") | Where-Object { Test-Path $_ }
foreach ($f in $files) { Copy-Item -LiteralPath $f -Destination $tmp -Force }
$work = Join-Path $tmp 'Cookies'

function Invoke-Sql($db, $sql) {
    if ($sqlite) { & $sqlite.Source $db $sql } else { & node -e "const{DatabaseSync}=require('node:sqlite');const d=new DatabaseSync(process.argv[1]);for(const r of d.prepare(process.argv[2]).all())console.log(JSON.stringify(r));d.close()" $db $sql }
}
function Invoke-SqlScalar($db, $sql) {
    if ($sqlite) { return (& $sqlite.Source $db $sql) }
    return (& node -e "const{DatabaseSync}=require('node:sqlite');const d=new DatabaseSync(process.argv[1]);console.log(d.prepare(process.argv[2]).get().v);d.close()" $db $sql)
}

if (-not $sqlite) { Write-Host '  (无 sqlite3，改用 Node 内建 node:sqlite)' -ForegroundColor DarkGray }

$totalSql = "select count(*) v from cookies where host_key='$HOST_KEY' and name like '$PREFIX%'"
$total = [int](Invoke-SqlScalar $work $totalSql)
$bytes = $total * $SINGLE
$pct   = [Math]::Round(100.0 * $bytes / $SIZE_LIMIT, 1)
Write-Host ("  dsh-auth-* 共 {0} 条，约 {1:N0} 字节（431 上限的 {2}%）" -f $total, $bytes, $pct)

if ($keepName) {
    $staleSql = "select count(*) v from cookies where host_key='$HOST_KEY' and name like '$PREFIX%' and name<>'$keepName'"
} else {
    $staleSql = "select count(*) v from cookies where host_key='$HOST_KEY' and name like '$PREFIX%'"
}
$stale = [int](Invoke-SqlScalar $work $staleSql)
if ($stale -eq 0) {
    Write-Skip '没有可删的陈旧 cookie'
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    exit 0
}
Write-Host ("  将删除 {0} 条，删后约 {1:N0} 字节" -f $stale, (($total - $stale) * $SINGLE))

# ---- 5. 删除 ----------------------------------------------------------
Write-Section '清理'

if ($DryRun) {
    Write-Skip "DryRun：没有写回。临时副本在 $tmp"
    exit 0
}

if (-not $PSCmdlet.ShouldProcess($work, "删除 $stale 条陈旧 auth cookie")) {
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    exit 0
}

try {
    Invoke-Sql $work $staleSql.Replace('select count(*) v', 'delete') | Out-Null
    $left = [int](Invoke-SqlScalar $work $totalSql)
    if ($left -ne ($total - $stale)) { throw "删后条数 $left 与预期 $($total - $stale) 不符" }
    Write-Ok "已删除 $stale 条，剩余 $left 条"

    # 写回：直接把临时副本覆盖回数据目录（没有 -wal 参与，_journal 为 0 长度）
    Copy-Item -LiteralPath $work -Destination $DB -Force
    foreach ($extra in @("$DB-wal", "$DB-shm")) { if (Test-Path $extra) { Remove-Item -LiteralPath $extra -Force } }
    Write-Ok '已写回 cookie 库'
} catch {
    Write-Bad "清理失败：$($_.Exception.Message)"
    Write-Host "         原库未被改动；临时副本留档在 $tmp" -ForegroundColor DarkGray
    exit 1
}

Write-Section '完成'
Write-Host ''
Write-Host '  下一步：启动 DShell，界面应当能进。' -ForegroundColor White
Write-Host "  恢复：把 $tmp\Cookies 覆盖回 $DB 即可。" -ForegroundColor DarkGray
Write-Host "  临时副本确认无用后可删：Remove-Item -Recurse -Force `"$tmp`"" -ForegroundColor DarkGray
Write-Host ''
