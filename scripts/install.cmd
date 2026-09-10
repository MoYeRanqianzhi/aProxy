@echo off
rem aProxy 引导安装脚本（cmd 版，无 PowerShell 环境时的兜底）。
rem
rem 职责 = bootstrap 首装：从 GitHub Releases 下载二进制，落位到
rem %APROXY_HOME%（默认 %USERPROFILE%\.aproxy）。已安装则不重复安装——
rem 装机后的升级一律 `aproxy install` 自管（本脚本指路）。
rem
rem 用法：scripts\install.cmd [tag]
rem   tag 形如 v0.1.0-alpha.7；省略 = 自动取最新 release。
rem 环境变量：
rem   APROXY_HOME       目录根（默认 %USERPROFILE%\.aproxy）
rem   APROXY_NO_SKILLS  1 = 跳过 skill 文档
rem
rem 说明：cmd 无内置 HTTP 客户端，下载走 PowerShell 的 Invoke-WebRequest
rem（system 自带 pwsh/powershell，仅用作下载原语，脚本逻辑本身全在 cmd）。
rem skill 解压同理用 powershell Expand-Archive。

setlocal enabledelayedexpansion

set "REPO=MoYeRanqianzhi/aProxy"
set "HOME_DIR=%USERPROFILE%"
if defined APROXY_HOME set "HOME_DIR=%APROXY_HOME%"
set "BIN_DIR=%HOME_DIR%\bin"
set "SKILLS_DIR=%HOME_DIR%\skills"
set "TMP_DIR=%HOME_DIR%\staging\bootstrap"
set "UA=aproxy-install-script"

rem ---- 最新 tag：releases/latest 在仅有 prerelease 时 404（alpha 时代整线
rem ---- 都是 prerelease）——用列表接口取第一个 ----
set "TAG=%~1"
if not defined TAG (
    for /f "delims=" %%T in ('powershell -NoProfile -Command "(Invoke-RestMethod -Uri 'https://api.github.com/repos/%REPO%/releases?per_page=1' -Headers @{/'User-Agent/'='aproxy-install-script'})[0].tag_name"') do set "TAG=%%T"
)
if not defined TAG (
    echo 无法解析最新 release（仓库无 release 或网络不可达^） 1>&2
    exit /b 1
)
echo 安装 aProxy %TAG%

rem ---- 已安装检测：装机后的升级归 install 管，本脚本只做首装 ----
if exist "%BIN_DIR%\aproxy.exe" (
    echo aProxy 已安装于 %BIN_DIR%\aproxy.exe，本脚本不做覆盖。
    echo 升级请使用: aproxy install
    exit /b 0
)

set "ASSET=aproxy-x86_64-pc-windows-msvc.exe"
set "BASE=https://github.com/%REPO%/releases/download/%TAG%"

mkdir "%BIN_DIR%" 2>nul
mkdir "%SKILLS_DIR%" 2>nul
mkdir "%TMP_DIR%" 2>nul

rem ---- 下载二进制 + 校验 + 落位 ----
echo 下载二进制 %ASSET% ...
powershell -NoProfile -Command "Invoke-WebRequest -Uri '%BASE%/%ASSET%' -OutFile '%TMP_DIR%\aproxy.exe' -UserAgent '%UA%'; if ($LASTEXITCODE) { exit 1 }"
if errorlevel 1 (
    echo 下载失败 1>&2
    exit /b 1
)
powershell -NoProfile -Command "Invoke-WebRequest -Uri '%BASE%/%ASSET%.sha256' -OutFile '%TMP_DIR%\aproxy.exe.sha256' -UserAgent '%UA%'"
if errorlevel 1 (
    echo 校验文件下载失败 1>&2
    exit /b 1
)
rem ---- SHA256 校验：ReadAllText 去掉可能的 BOM/尾空白，比对前 64 位十六进制 ----
powershell -NoProfile -Command "$e = (Get-Content '%TMP_DIR%\aproxy.exe.sha256' -Raw).Trim().Split(' ')[0].ToLower(); $a = (Get-FileHash '%TMP_DIR%\aproxy.exe' -Algorithm SHA256).Hash.ToLower(); if ($e -ne $a) { Write-Host ('SHA256 校验失败: 期望 ' + $e + ' 实际 ' + $a); exit 1 }"
if errorlevel 1 exit /b 1

move /y "%TMP_DIR%\aproxy.exe" "%BIN_DIR%\aproxy.exe" >nul

rem ---- fallback 入口脚本（aproxy.bat：exe 缺席时重定向旧二进制，见 install 计划）----
> "%BIN_DIR%\aproxy.bat" (
    echo @echo off
    echo if exist "%%~dp0aproxy.exe" ^(
    echo   "%%~dp0aproxy.exe" %%*
    echo ^) else if exist "%%~dp0aproxy.old.exe" ^(
    echo   "%%~dp0aproxy.old.exe" %%*
    echo ^)
)

rem ---- skill 文档（非强制：失败不影响安装）----
if "%APROXY_NO_SKILLS%"=="1" goto :done
echo 下载 skill 文档 aproxy-skills.zip ...
powershell -NoProfile -Command "try { Invoke-WebRequest -Uri '%BASE%/aproxy-skills.zip' -OutFile '%TMP_DIR%\aproxy-skills.zip' -UserAgent '%UA%'; Invoke-WebRequest -Uri '%BASE%/aproxy-skills.zip.sha256' -OutFile '%TMP_DIR%\aproxy-skills.zip.sha256' -UserAgent '%UA%' } catch { exit 1 }"
if errorlevel 1 (
    echo skill 文档下载失败，跳过（不影响安装，可稍后用 aproxy install 重试^）
    goto :done
)
powershell -NoProfile -Command "$e = (Get-Content '%TMP_DIR%\aproxy-skills.zip.sha256' -Raw).Trim().Split(' ')[0].ToLower(); $a = (Get-FileHash '%TMP_DIR%\aproxy-skills.zip' -Algorithm SHA256).Hash.ToLower(); if ($e -ne $a) { exit 1 }; Expand-Archive '%TMP_DIR%\aproxy-skills.zip' '%SKILLS_DIR%' -Force; Write-Host ('skill 文档已就位 ' + '%SKILLS_DIR%')"
if errorlevel 1 (
    echo skill 文档校验/解压失败，跳过（不影响安装^）
    goto :done
)

:done
rd /s /q "%TMP_DIR%" 2>nul

echo.
echo aProxy 已安装: %BIN_DIR%\aproxy.exe
echo %PATH% | findstr /i /c:"%BIN_DIR%" >nul
if errorlevel 1 (
    echo 注意: %BIN_DIR% 不在 PATH 中——将其加入 PATH 后即可直接使用 aproxy 命令：
    echo   setx PATH "%%PATH%%;%BIN_DIR%"
)
echo 启动: aproxy   （或完整路径 "%BIN_DIR%\aproxy.exe"^）
echo 状态: aproxy status    升级: aproxy install
endlocal
