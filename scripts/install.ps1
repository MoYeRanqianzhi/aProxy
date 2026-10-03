# aProxy 引导安装脚本（Windows，PowerShell 5.1 / pwsh 7+）。
#
# 职责 = bootstrap 首装：从 GitHub Releases 下载二进制与 skill 文档，落位到
# $APROXY_HOME（默认 ~/.aproxy）。已安装则不重复安装——装机后的升级一律
# `aproxy install` 自管（本脚本指路）。
#
# 用法：
#   irm https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/scripts/install.ps1 | iex
#   或本地执行：pwsh -File scripts/install.ps1 [-Version v0.1.0] [-Pre] [-NoSkills]
#   irm | iex 无法传参，用环境变量代替：
#     $env:APROXY_PRE = "1"; irm https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/scripts/install.ps1 | iex
#
# 选版规则（未指定 -Version 时）：
#   1. 只认 v<数字> 开头的 tag（同仓库的 format-v* 是另一条发版线，必须排除）并跳过 draft；
#   2. 默认取最新的稳定版（非 prerelease，按版本号取最大）；
#   3. 仓库里一个 v* 稳定版都没有时（0.1.0 发布前）回退到最新的 v* 预发布并打印说明；
#   4. -Pre（或 APROXY_PRE=1）：取最新创建的 v* release，预发布也可。
#
# 环境变量：
#   APROXY_HOME       根目录（默认 ~\.aproxy）——bin/skills/run 全目录的根
#   APROXY_NO_SKILLS=1  跳过 skill 文档（等效 -NoSkills）
#   APROXY_PRE=1      等效 -Pre
#   APROXY_DL_PROXY   下载代理（等效 -DownloadProxy；仅本次，与上游请求代理完全无关）

param(
    [string]$Version = "",      # 指定 tag（如 v0.1.0）；空 = 按上面的选版规则
    [switch]$NoSkills,          # 跳过 skill 文档下载
    [string]$DownloadProxy = "", # 下载代理（仅本次；与上游请求代理完全无关）
    [switch]$Pre                # 允许安装预发布（取最新创建的 v* release）
)

# 主体全部放进函数：`irm | iex` 在用户自己的会话里执行，顶层 exit 会直接关掉
# 用户的 PowerShell 窗口，顶层设置的 $ErrorActionPreference 也会泄漏到会话里。
# 函数内用 return/throw 收场，$ErrorActionPreference 随函数作用域结束自动还原。
function Install-AProxy {
    param([string]$Version, [bool]$NoSkills, [string]$DownloadProxy, [bool]$Pre)

    $ErrorActionPreference = "Stop"
    # Windows PowerShell 5.1 在较老的系统上默认只启用 TLS 1.0/1.1，访问 GitHub 会
    # 握手失败。只在「显式列出了协议且不含 TLS 1.2」时追加 Tls12（-bor，不覆盖其它
    # 协议）；值为 0 表示 SystemDefault（由系统决定，现代系统已含 1.2/1.3），不动它。
    $tls12 = 3072
    $proto = [int][Net.ServicePointManager]::SecurityProtocol
    if ($proto -ne 0 -and -not ($proto -band $tls12)) {
        [Net.ServicePointManager]::SecurityProtocol = $proto -bor $tls12
    }
    # 输出统一 UTF-8：中文提示在重定向/管道场景不再按系统 OEM 码页转码乱码
    # （与 aproxy 进程入口的 SetConsoleOutputCP(65001) 同款语义）
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8

    $Repo = "MoYeRanqianzhi/aProxy"
    $Home_ = if ($env:APROXY_HOME) { $env:APROXY_HOME } else { Join-Path $HOME ".aproxy" }
    $BinDir = Join-Path $Home_ "bin"
    $SkillsDir = Join-Path $Home_ "skills"
    $TmpDir = Join-Path $Home_ "staging\bootstrap"

    function Get-ReleaseTag([bool]$AllowPre) {
        # 取一整页（上限 100）自己过滤：/releases/latest 可能指向 format-v*，列表首项
        # 也不可靠（按创建时间排，并列时次序偶然）。
        $headers = @{ "User-Agent" = "aproxy-install-script" }
        try {
            $raw = @(Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases?per_page=100" -Headers $headers)
        } catch {
            throw "无法获取 release 列表（网络不可达或 GitHub API 限流）: $_"
        }
        # JSON 顶层是数组：PowerShell 7 会把整个数组当作单个对象输出（5.1 则逐项展开），
        # 逐层展平后两个版本得到同样的 release 对象列表
        $all = @(foreach ($r in $raw) { foreach ($i in @($r)) { $i } })
        # 只留 v<数字> 开头且非 draft 的条目（排除 format-v*）；保持列表（创建时间倒序）顺序
        $cands = @($all | Where-Object { $_.tag_name -and -not $_.draft -and $_.tag_name -match '^v[0-9]' })
        if ($cands.Count -eq 0) { throw "仓库里没有可安装的 v* release" }
        if ($AllowPre) { return $cands[0].tag_name }

        # 稳定版之间按版本号取最大；只接受 vX.Y.Z（带后缀的不参与比较）
        $stable = @($cands | Where-Object { -not $_.prerelease -and $_.tag_name -match '^v[0-9]+\.[0-9]+\.[0-9]+$' })
        if ($stable.Count -gt 0) {
            return ($stable | Sort-Object { [version]$_.tag_name.Substring(1) } -Descending | Select-Object -First 1).tag_name
        }
        $pres = @($cands | Where-Object { $_.prerelease })
        if ($pres.Count -eq 0) { throw "仓库里没有可安装的 v* release" }
        Write-Host "说明: 仓库暂无 v* 稳定版，回退到最新预发布 $($pres[0].tag_name)（正式版发布后默认装稳定版）"
        return $pres[0].tag_name
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
        return
    }

    # ---- 解析版本与平台资产 ----
    if ($Version) {
        # tag 会拼进下载 URL，限定字符集
        if ($Version -notmatch '^[A-Za-z0-9._-]+$') { throw "非法的 tag: $Version" }
        $tag = $Version
    } else {
        $tag = Get-ReleaseTag $Pre
    }
    Write-Host "安装 aProxy $tag"

    # 本脚本不探测 AVX2（保守拉 baseline）；指令集变体选择由 `aproxy install` 做
    $asset = "aproxy-x86_64-pc-windows-msvc.exe"
    $base = "https://github.com/$Repo/releases/download/$tag"

    New-Item -ItemType Directory -Force $BinDir, $SkillsDir, $TmpDir | Out-Null

    try {
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
    } finally {
        # 成功与失败都清暂存目录，不留残骸
        Remove-Item $TmpDir -Recurse -Force -ErrorAction SilentlyContinue
    }

    Write-Host ""
    Write-Host "aProxy 已安装: $exePath"
    if ($env:PATH -notlike "*$BinDir*") {
        Write-Host "注意: $BinDir 不在 PATH 中——将其加入 PATH 后即可直接使用 aproxy 命令："
        Write-Host ('  [Environment]::SetEnvironmentVariable(''Path'', $env:Path + '';' + $BinDir + ''', ''User'')')
    }
    Write-Host "下一步: 先配置上游（新装机没有 base_url，直接启动会失败），再启动："
    Write-Host "  aproxy config --baseurl <上游地址> --api-key <密钥>"
    Write-Host "  aproxy        （或完整路径 `"$exePath`"）"
    Write-Host "状态: aproxy status    升级: aproxy install"
}

# 环境变量对应 irm | iex 场景（无法传 param）；显式 param 优先
Install-AProxy `
    -Version $Version `
    -NoSkills ([bool]($NoSkills -or $env:APROXY_NO_SKILLS -eq "1")) `
    -DownloadProxy $(if ($DownloadProxy) { $DownloadProxy } else { $env:APROXY_DL_PROXY }) `
    -Pre ([bool]($Pre -or $env:APROXY_PRE -eq "1"))
