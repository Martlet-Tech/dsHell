<#
.SYNOPSIS
    把「DShell 原生目录选择器」插件装进 dsh 的 web profile（或卸载）。

.DESCRIPTION
    **现在这一步通常不需要手动做了。** 新版 DShell 在启动体检的第 ⑥ 项
    （「原生目录选择器（自带插件）」）会**自己**把插件装进 profile —— 插件随
    exe 发布，复制过去即可（它零依赖，不需要 npm / pnpm）。用户双击即用。

    本脚本保留下来是作为**手动兜底**，适用于：
      * 用的是旧版 DShell（还没有自动安装那一步）；
      * 自动安装降级了（体检里那一项是黄字），想看详细过程或手工修；
      * 开发期想用 junction 链接到源码（改完插件不用重装）——自动安装**只复制、
        绝不覆盖已有安装**，而本脚本默认建 junction，这正是开发期想要的。

    自动安装的行为（对照着看，避免两边不一致）：
      * 目标：`%DSH_HOME%\profiles\web\node_modules\dshell-directory-picker`
      * 已经在且完整 → 只读不动（所以你建的 junction 会被保留）
      * profile 还不存在 → 本次跳过，下次启动再装（profile 模板归 dsh 所有）
      * 失败 → 体检里标黄字，**不拦启动**（插件只影响"选完目录要不要补点一下"）

    这个脚本解决的是一个**踩过的坑**：手工把插件塞进 profile 的 bundle 列表后，
    用**旧版 DShell** 启动时（旧版不设 DSHELL_PICKER_PORT 环境变量），插件会
    "礼貌地不注册"，而当时它同时还禁用了 dsh 自带的 picker —— 结果一个后端都
    没有，界面上「添加工作区」的按钮直接消失。

    现在插件已经改成**叠加式**（不动 dsh 自己的行，拿不到端口就完全不动作），
    所以那种失败模式已经不可能发生。

    装好之后必须用**新版 DShell**（会设置 DSHELL_PICKER_PORT 的那个）启动才
    走新路径；旧版 DShell 或直接在终端跑 dsh web 时，dsh 自己的选择器照常工作。

.PARAMETER Uninstall
    从 profile 里移除插件，恢复成 dsh 原样的行为。

.PARAMETER Profile
    profile 名，默认 web。

.EXAMPLE
    .\install-picker-plugin.ps1
    装进 web profile。

.EXAMPLE
    .\install-picker-plugin.ps1 -Uninstall
    卸掉，恢复原状。

.NOTES
    项目根目录 D:\Projects\dsh-shell **不是 git 仓库**，本脚本只做文件系统操作。
#>
[CmdletBinding()]
param(
    [switch]$Uninstall,
    [string]$Profile = 'web'
)

$ErrorActionPreference = 'Stop'

$PLUGIN_NAME = 'dshell-directory-picker'
# 本脚本住在 `<repo>/scripts/`，往上一层是仓库根，插件在 `<repo>/plugin/` 下。
$REPO_ROOT   = Split-Path $PSScriptRoot -Parent
$PLUGIN_DIR  = Join-Path $REPO_ROOT 'plugin\dshell-directory-picker'
$DSH_HOME    = if ($env:DSH_HOME) { $env:DSH_HOME } else { Join-Path $env:USERPROFILE '.dsh' }
$PROFILE_DIR = Join-Path $DSH_HOME "profiles\$Profile"
# 变量名统一用 $manifestPath，**不要**再用 $MANIFEST 这个别名：
# PowerShell 变量名不区分大小写，两个名字指向同一个变量，很容易在
# 后面某处给"另一个"赋值时把路径冲掉（这个坑真的踩过）。
$manifestPath = Join-Path $PROFILE_DIR 'package.json'
$LINK         = Join-Path $PROFILE_DIR "node_modules\$PLUGIN_NAME"

function Write-Section($text) {
    Write-Host ''
    Write-Host "── $text " -ForegroundColor Cyan -NoNewline
    Write-Host ('─' * [Math]::Max(0, 58 - $text.Length)) -ForegroundColor DarkGray
}
function Write-Ok($t)   { Write-Host "  [ok]   $t" -ForegroundColor Green }
function Write-Skip($t) { Write-Host "  [skip] $t" -ForegroundColor DarkGray }
function Write-Warn($t) { Write-Host "  [warn] $t" -ForegroundColor Yellow }
function Write-Bad($t)  { Write-Host "  [fail] $t" -ForegroundColor Red }

Write-Host ''
Write-Host "  DShell 原生目录选择器 —— profile 插件$(if ($Uninstall) { '卸载' } else { '安装' })" -ForegroundColor White
Write-Host "  profile：$PROFILE_DIR" -ForegroundColor DarkGray

# ── 前置检查 ────────────────────────────────────────────────────────
Write-Section '前置检查'
if (-not (Test-Path $PROFILE_DIR)) {
    Write-Bad "profile 目录不存在：$PROFILE_DIR"
    Write-Host '         先用 dsh 起一次该 profile，让它被创建。' -ForegroundColor DarkGray
    exit 1
}
if (-not (Test-Path $manifestPath)) {
    Write-Bad "找不到 profile 清单：$manifestPath"
    exit 1
}
Write-Ok 'profile 目录与清单都在'

# ── 正在运行的 DShell ───────────────────────────────────────────────
#
# 不强制要求先退出：实测**只有 `node_modules` 里的文件打开**会被 dsh 拒绝，
# 目录操作（建/删 junction）和 `package.json` 的读写都是好的。
#
# 所以本脚本在 DShell 运行时照样能装完，只是**装完必须重启 DShell 才生效**——
# dsh 只在启动时读一次 profile。这一点会明确提示，免得出现"装了却没变化"
# 这种最难查的情况（这坑真踩过）。
$running = @(Get-Process dshell -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) {
    Write-Warn "有 $($running.Count) 个 dshell.exe 正在运行"
    foreach ($p in $running) {
        $pth = try { $p.Path } catch { '(不可读)' }
        Write-Host "           pid=$($p.Id)  $pth" -ForegroundColor DarkGray
    }
    Write-Host '         可以继续安装，但**装完必须重启 DShell** 才会生效。' -ForegroundColor Yellow
} else {
    Write-Ok '没有运行中的 dshell.exe'
}

# ── 读清单 ──────────────────────────────────────────────────────────
$profileJson = Get-Content $manifestPath -Raw | ConvertFrom-Json
if (-not $profileJson.dsh -or -not $profileJson.dsh.profile) {
    Write-Bad 'profile 清单里没有 dsh.profile —— 这不是一个 dsh profile'
    exit 1
}

# 备份一次（只在还没有备份时做，避免把"已经改坏的"当成原始状态存下来）
$backup = "$manifestPath.bak-before-plugin"
if (-not (Test-Path $backup)) {
    Copy-Item $manifestPath $backup
    Write-Ok "已备份原始清单到 $(Split-Path $backup -Leaf)"
} else {
    Write-Skip '备份已存在，不覆盖'
}

# 写清单：统一走这里，避免 ConvertTo-Json 的参数在各处重复一遍
function Save-ProfileJson($json) {
    [System.IO.File]::WriteAllText($manifestPath, ($json | ConvertTo-Json -Depth 10) + "`n", (New-Object System.Text.UTF8Encoding($false)))
}

# ── 执行 ────────────────────────────────────────────────────────────
if ($Uninstall) {
    Write-Section '卸载'
    if (Test-Path $LINK) {
        try {
            Remove-Item $LINK -Recurse -Force
            Write-Ok '已移除 node_modules 里的插件链接'
        } catch {
            Write-Warn "链接删不掉（DShell 可能正占用）：$($_.Exception.Message)"
            Write-Host '         先完全退出 DShell 再重跑本脚本。' -ForegroundColor DarkGray
            exit 1
        }
    } else {
        Write-Skip '插件链接本就不存在'
    }

    $profileJson.dsh.profile.bundles = @($profileJson.dsh.profile.bundles | Where-Object { $_ -ne $PLUGIN_NAME })
    if ($profileJson.dependencies -and $profileJson.dependencies.PSObject.Properties.Name -contains $PLUGIN_NAME) {
        $profileJson.dependencies.PSObject.Properties.Remove($PLUGIN_NAME)
    }
    Save-ProfileJson $profileJson
    Write-Ok '已从 bundle 列表移除'
    Write-Host ''
    Write-Host '  现在 dsh 用的是它自带的选择器（会弹无主窗口，选完需要补点一下）。' -ForegroundColor DarkGray
    exit 0
}

Write-Section '安装'

if (-not (Test-Path $PLUGIN_DIR)) {
    Write-Bad "插件源码目录不存在：$PLUGIN_DIR"
    exit 1
}
Write-Ok '插件源码目录存在'

# 用 junction 而不是复制：改插件源码后不用重装。
# （这也是为什么发布版要复制而不是链接——开发期链接更省事。）
if (Test-Path $LINK) {
    Remove-Item $LINK -Recurse -Force
}
$nodeModules = Join-Path $PROFILE_DIR 'node_modules'
if (-not (Test-Path $nodeModules)) { New-Item -ItemType Directory -Force -Path $nodeModules | Out-Null }
New-Item -ItemType Junction -Path $LINK -Target $PLUGIN_DIR | Out-Null
Write-Ok "已链接插件到 node_modules\$PLUGIN_NAME"

if ($profileJson.dsh.profile.bundles -notcontains $PLUGIN_NAME) {
    $profileJson.dsh.profile.bundles = @($profileJson.dsh.profile.bundles) + $PLUGIN_NAME
    Write-Ok '已加入 bundle 列表（排最后 = patch 层最后应用）'
} else {
    Write-Skip 'bundle 列表里已有它'
}

if (-not $profileJson.dependencies) { $profileJson.dependencies = [pscustomobject]@{} }
if ($profileJson.dependencies.PSObject.Properties.Name -notcontains $PLUGIN_NAME) {
    $profileJson.dependencies | Add-Member -NotePropertyName $PLUGIN_NAME -NotePropertyValue "file:$PLUGIN_DIR"
}
Save-ProfileJson $profileJson
Write-Ok '清单已写入'

Write-Section '完成'
Write-Host ''
Write-Host '  接下来：' -ForegroundColor White
Write-Host '   1. 用**新版** DShell 启动（会设置 DSHELL_PICKER_PORT 的那个 exe）' -ForegroundColor DarkGray
Write-Host '   2. 点「添加工作区」→ 选目录 → 工作区应当立刻出现' -ForegroundColor DarkGray
Write-Host ''
Write-Host '  注意：旧版 DShell 下本插件完全不动作，dsh 自带选择器照常工作，' -ForegroundColor DarkGray
Write-Host '        所以不会出现「按钮消失」那类故障。' -ForegroundColor DarkGray
Write-Host ''
Write-Host '  想恢复原状：' -ForegroundColor White
Write-Host "   .\install-picker-plugin.ps1 -Uninstall" -ForegroundColor Cyan
Write-Host ''
