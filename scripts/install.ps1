# aProxy 引导安装脚本（Windows，pwsh）。
#
# 职责 = bootstrap 首装：从 GitHub Releases 下载二进制与 skill 文档，落位到
# $APROXY_HOME（默认 ~/.aproxy）。已安装则不重复安装——装机后的升级一律
# `aproxy install` 自管（本脚本指路）。
#
# 用法：
#   irm https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/master/scripts/install.ps1 | iex
#   或本地执行：pwsh -File scripts/install.ps1 [-Version v0.1.0-alpha.7] [-NoSkills]
#
# 环境变量：APROXY_HOME（默认 ~\.aproxy）——bin/skills/run 全目录的根。

param(
    [string]$Version = "",     # 指定 tag（如 v0.1.0-alpha.7）；空 = 最新版（含 prerelease）
    [switch]$NoSkills,         # 跳过 skill 文档下载
    [string]$DownloadProxy = "" # 下载代理（仅本次；与上游请求代理完全无关）
)

$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repo = "MoYeRanqianzhi/aProxy"
$Home_ = if ($env:APROXY_HOME) { $env:APROXY_HOME } else { Join-Path $HOME ".aproxy" }
$BinDir = Join-Path $Home_ "bin"
$SkillsDir = Join-Path $Home_ "skills"
$TmpDir = Join-Path $Home_ "staging\bootstrap"

function Get-LatestTag {
    # releases/latest 在「仅有 prerelease」时会 404（alpha 时代整线都是
    # prerelease）——用列表接口取第一个（GitHub 按创建时间倒序）。
    $headers = @{ "User-Agent" = "aproxy-install-script" }
    $list = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases?per_page=1" -Headers $headers
    if (-not $list -or $list.Count -eq 0) { throw "仓库无任何 release" }
    return $list[0].tag_name
}

function Get-File([string]$Url, [string]$Out) {
    if ($DownloadProxy) {
        Invoke-WebRequest -Uri $Url -OutFile $Out -Proxy $DownloadProxy -UserAgent "aproxy-install-script"
    } else {
        Invoke-WebRequest -Uri $Url -OutFile $Out -UserAgent "aproxy-install-script"
    }
}

function Test-Sha256([string]$File, [string]$ShaFile) {
    # 校验文件格式：<hash>  <filename>（sha256sum 语义）；下载时目标文件名
    # 与校验文件记录名一致
    $expected = (Get-Content $ShaFile -Raw).Split(" ")[0].Trim().ToLower()
    $actual = (Get-FileHash $File -Algorithm SHA256).Hash.ToLower()
    if ($expected -ne $actual) { throw "SHA256 校验失败: $File`n  期望 $expected`n  实际 $actual" }
}

# ---- 已安装检测：装机后的升级归 install 管，本脚本只做首装 ----
$exePath = Join-Path $BinDir "aproxy.exe"
if (Test-Path $exePath) {
    Write-Host "aProxy 已安装于 $exePath，本脚本不做覆盖。"
    Write-Host "升级请使用: aproxy install"
    exit 0
}

# ---- 解析版本与平台资产 ----
$tag = if ($Version) { $Version } else { Get-LatestTag }
Write-Host "安装 aProxy $tag"

# 本脚本不探测 AVX2（保守拉 baseline）；指令集变体选择由 `aproxy install` 做
$asset = "aproxy-x86_64-pc-windows-msvc.exe"
$base = "https://github.com/$Repo/releases/download/$tag"

New-Item -ItemType Directory -Force $BinDir, $SkillsDir, $TmpDir | Out-Null

# ---- 下载二进制 + 校验 + 落位 ----
Write-Host "下载二进制 $asset ..."
$binTmp = Join-Path $TmpDir "aproxy.exe"
$shaTmp = Join-Path $TmpDir "aproxy.exe.sha256"
Get-File "$base/$asset" $binTmp
Get-File "$base/$asset.sha256" $shaTmp
Test-Sha256 $binTmp $shaTmp
Move-Item $binTmp $exePath -Force

# ---- fallback 入口脚本（aproxy.bat：exe 缺席时重定向旧二进制，见 install 计划）----
$batPath = Join-Path $BinDir "aproxy.bat"
@"
@echo off
if exist "%~dp0aproxy.exe" (
  "%~dp0aproxy.exe" %*
) else if exist "%~dp0aproxy.old.exe" (
  "%~dp0aproxy.old.exe" %*
)
"@ | Out-File -Encoding ascii $batPath

# ---- skill 文档（非强制：失败不影响安装）----
if (-not $NoSkills) {
    try {
        Write-Host "下载 skill 文档 aproxy-skills.zip ..."
        $zipTmp = Join-Path $TmpDir "aproxy-skills.zip"
        $zipSha = Join-Path $TmpDir "aproxy-skills.zip.sha256"
        Get-File "$base/aproxy-skills.zip" $zipTmp
        Get-File "$base/aproxy-skills.zip.sha256" $zipSha
        Test-Sha256 $zipTmp $zipSha
        Expand-Archive $zipTmp $SkillsDir -Force
        Write-Host "skill 文档已就位 $SkillsDir（按所用 agent 的方式链接/复制到其 skills 目录）"
    } catch {
        Write-Host "skill 文档下载失败（不影响安装，可稍后用 aproxy install 重试）: $_"
    }
}

# ---- 清理与提示 ----
Remove-Item $TmpDir -Recurse -Force -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "aProxy 已安装: $exePath"
if ($env:PATH -notlike "*$BinDir*") {
    Write-Host "注意: $BinDir 不在 PATH 中——将其加入 PATH 后即可直接使用 aproxy 命令："
    Write-Host "  [Environment]::SetEnvironmentVariable('Path', \`$env:Path + ';$BinDir', 'User')"
}
Write-Host "启动: aproxy   （或完整路径 `"$exePath`"）"
Write-Host "状态: aproxy status    升级: aproxy install"
