@echo off
rem aProxy bootstrap installer (cmd fallback, English-only output).
rem
rem WHY ASCII: cmd parses batch files with the active OEM code page; UTF-8
rem Chinese comments/strings get mis-tokenized even after `chcp 65001`
rem (byte-level line-splitting artifacts, observed on Win11 26200). The
rem primary installers with localized output are scripts/install.ps1 and
rem scripts/install.sh - use those unless PowerShell is unavailable.
rem
rem Usage: scripts\install.cmd [tag]
rem   tag like v0.1.0-alpha.7; omitted = resolve latest release.
rem Env:
rem   APROXY_HOME       root dir (default %USERPROFILE%\.aproxy)
rem   APROXY_NO_SKILLS  1 = skip skill docs
rem
rem First install only - upgrades are managed by `aproxy install`.
rem cmd has no built-in HTTP client, so downloads delegate to PowerShell's
rem Invoke-WebRequest (system built-in); all logic stays here in cmd.

setlocal enabledelayedexpansion

set "REPO=MoYeRanqianzhi/aProxy"
set "HOME_DIR=%USERPROFILE%"
if defined APROXY_HOME set "HOME_DIR=%APROXY_HOME%"
set "BIN_DIR=%HOME_DIR%\bin"
set "SKILLS_DIR=%HOME_DIR%\skills"
set "TMP_DIR=%HOME_DIR%\staging\bootstrap"
set "UA=aproxy-install-script"

rem ---- Resolve latest tag: /releases/latest 404s when only prereleases
rem ---- exist (alpha line) - list endpoint, first entry (newest first) ----
set "TAG=%~1"
if not defined TAG (
    for /f "delims=" %%T in ('powershell -NoProfile -Command "(Invoke-RestMethod -Uri 'https://api.github.com/repos/%REPO%/releases?per_page=1' -Headers @{/'User-Agent/'='aproxy-install-script'})[0].tag_name"') do set "TAG=%%T"
)
if not defined TAG (
    echo Cannot resolve latest release ^(no release yet, or network unavailable^) 1>&2
    exit /b 1
)
echo Installing aProxy %TAG%

rem ---- Already-installed check: upgrades belong to `aproxy install` ----
if exist "%BIN_DIR%\aproxy.exe" (
    echo aProxy is already installed at %BIN_DIR%\aproxy.exe - this script will not overwrite it.
    echo To upgrade, run: aproxy install
    exit /b 0
)

set "ASSET=aproxy-x86_64-pc-windows-msvc.exe"
set "BASE=https://github.com/%REPO%/releases/download/%TAG%"

mkdir "%BIN_DIR%" 2>nul
mkdir "%SKILLS_DIR%" 2>nul
mkdir "%TMP_DIR%" 2>nul

rem ---- Download binary + verify + place ----
echo Downloading %ASSET% ...
powershell -NoProfile -Command "try { Invoke-WebRequest -Uri '%BASE%/%ASSET%' -OutFile '%TMP_DIR%\aproxy.exe' -UserAgent '%UA%' } catch { exit 1 }"
if errorlevel 1 (
    echo Download failed 1>&2
    exit /b 1
)
powershell -NoProfile -Command "try { Invoke-WebRequest -Uri '%BASE%/%ASSET%.sha256' -OutFile '%TMP_DIR%\aproxy.exe.sha256' -UserAgent '%UA%' } catch { exit 1 }"
if errorlevel 1 (
    echo Checksum download failed 1>&2
    exit /b 1
)
rem SHA256 via certutil (cmd-native, immune to PSModulePath pollution in
rem Git Bash -> cmd -> powershell chains; Get-FileHash breaks there)
set "ACTUAL="
for /f "delims=" %%H in ('certutil -hashfile "%TMP_DIR%\aproxy.exe" SHA256 ^| findstr /r /i "^[0-9a-f]*$"') do set "ACTUAL=%%H"
set "EXPECTED="
for /f %%X in (%TMP_DIR%\aproxy.exe.sha256) do if not defined EXPECTED set "EXPECTED=%%X"
if /i not "%ACTUAL%"=="%EXPECTED%" (
    echo SHA256 mismatch: expected %EXPECTED% got %ACTUAL% 1>&2
    exit /b 1
)

move /y "%TMP_DIR%\aproxy.exe" "%BIN_DIR%\aproxy.exe" >nul

rem ---- Fallback entry script (aproxy.bat redirects to old binary when the
rem ---- new exe is absent mid-swap; PATHEXT mechanism, see install plan) ----
> "%BIN_DIR%\aproxy.bat" (
    echo @echo off
    echo if exist "%%~dp0aproxy.exe" ^(
    echo   "%%~dp0aproxy.exe" %%*
    echo ^) else if exist "%%~dp0aproxy.old.exe" ^(
    echo   "%%~dp0aproxy.old.exe" %%*
    echo ^)
)

rem ---- Skill docs (optional: failure does not affect the install) ----
if "%APROXY_NO_SKILLS%"=="1" goto :done
echo Downloading skills aproxy-skills.zip ...
powershell -NoProfile -Command "try { Invoke-WebRequest -Uri '%BASE%/aproxy-skills.zip' -OutFile '%TMP_DIR%\aproxy-skills.zip' -UserAgent '%UA%'; Invoke-WebRequest -Uri '%BASE%/aproxy-skills.zip.sha256' -OutFile '%TMP_DIR%\aproxy-skills.zip.sha256' -UserAgent '%UA%' } catch { exit 1 }"
if errorlevel 1 (
    echo Skill docs download failed - skipped ^(does not affect the install; retry later via aproxy install^)
    goto :done
)
powershell -NoProfile -Command "try { $e = (Get-Content '%TMP_DIR%\aproxy-skills.zip.sha256' -Raw).Trim().Split(' ')[0].ToLower(); $ok = $false; try { $h = certutil -hashfile '%TMP_DIR%\aproxy-skills.zip' SHA256 | Select-String -Pattern '^[0-9a-f]+$'; if ($h -and $h.Matches[0].Value.ToLower() -eq $e) { $ok = $true } } catch {}; if (-not $ok) { exit 1 }; Expand-Archive '%TMP_DIR%\aproxy-skills.zip' '%SKILLS_DIR%' -Force; Write-Host ('Skill docs placed at ' + '%SKILLS_DIR%') } catch { exit 1 }"
if errorlevel 1 (
    echo Skill docs verify/extract failed - skipped ^(does not affect the install^)
    goto :done
)

:done
rd /s /q "%TMP_DIR%" 2>nul

echo.
echo aProxy installed at: %BIN_DIR%\aproxy.exe
echo %PATH% | findstr /i /c:"%BIN_DIR%" >nul
if errorlevel 1 (
    echo NOTE: %BIN_DIR% is not in PATH. Add it to use the aproxy command directly:
    echo   setx PATH "%%PATH%%;%BIN_DIR%"
)
echo Start:  aproxy    ^(or full path "%BIN_DIR%\aproxy.exe"^)
echo Status: aproxy status    Upgrade: aproxy install
endlocal
exit /b 0
