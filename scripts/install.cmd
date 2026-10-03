@echo off
rem aProxy bootstrap installer (cmd fallback, English-only output).
rem
rem WHY ASCII: cmd parses batch files with the active OEM code page; UTF-8
rem Chinese comments/strings get mis-tokenized even after `chcp 65001`
rem (byte-level line-splitting artifacts, observed on Win11 26200). The
rem primary installers with localized output are scripts/install.ps1 and
rem scripts/install.sh - use those unless PowerShell is unavailable.
rem
rem Usage: scripts\install.cmd [--pre] [tag]
rem   tag like v0.1.0; omitted = pick a release by the rules below.
rem   --pre (or APROXY_PRE=1) also allows prereleases.
rem Release selection (no explicit tag): only tags starting with v<digit> are
rem   considered (the repo also hosts format-v* releases, which are excluded),
rem   drafts skipped. Default = newest stable (highest version number). If the
rem   repo has no v* stable release yet, falls back to the newest v* prerelease
rem   with a note. --pre = the most recently created v* release, prerelease or not.
rem Env:
rem   APROXY_HOME       root dir (default %USERPROFILE%\.aproxy)
rem   APROXY_NO_SKILLS  1 = skip skill docs
rem   APROXY_PRE        1 = same as --pre
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

rem ---- Arguments: [--pre] [tag] ----
set "TAG="
set "WANT_PRE=0"
if "%APROXY_PRE%"=="1" set "WANT_PRE=1"
:parse
if "%~1"=="" goto :parsed
if /i "%~1"=="--pre" (
    set "WANT_PRE=1"
) else if /i "%~1"=="-Pre" (
    set "WANT_PRE=1"
) else (
    set "TAG=%~1"
)
shift
goto :parse
:parsed

rem ---- The tag ends up inside URLs and PowerShell command strings: restrict
rem ---- the character set (delayed expansion keeps metacharacters inert) ----
if defined TAG (
    echo "!TAG!"| findstr /r /c:"^\"[A-Za-z0-9._-][A-Za-z0-9._-]*\"$" >nul
    if errorlevel 1 (
        echo Invalid tag 1>&2
        exit /b 1
    )
)

rem ---- Already-installed check: upgrades belong to `aproxy install` ----
if exist "%BIN_DIR%\aproxy.exe" (
    echo aProxy is already installed at %BIN_DIR%\aproxy.exe - this script will not overwrite it.
    echo To upgrade, run: aproxy install
    exit /b 0
)

rem ---- Resolve the release tag when none was given. One PowerShell command:
rem ---- fetch up to 100 releases, drop drafts and non-v<digit> tags (format-v*),
rem ---- then apply the selection rules above. usebackq lets the PowerShell code
rem ---- use single quotes freely. Keep this free of exclamation marks (delayed
rem ---- expansion is on). The fallback note goes to stderr so only the tag is
rem ---- captured. ----
if not defined TAG (
    for /f "usebackq delims=" %%T in (`powershell -NoProfile -Command "$ErrorActionPreference='Stop'; try { $raw=@(Invoke-RestMethod -Uri 'https://api.github.com/repos/%REPO%/releases?per_page=100' -Headers @{'User-Agent'='%UA%'}) } catch { [Console]::Error.WriteLine('Cannot list releases: ' + $_); exit 1 }; $a=@(@(foreach($r in $raw){foreach($i in @($r)){$i}}) | Where-Object { $_.tag_name -and -not $_.draft -and $_.tag_name -match '^v[0-9]' }); if ('%WANT_PRE%' -eq '1') { $t=$a | Select-Object -First 1 } else { $t=$a | Where-Object { -not $_.prerelease -and $_.tag_name -match '^v[0-9]+\.[0-9]+\.[0-9]+$' } | Sort-Object { [version]$_.tag_name.Substring(1) } -Descending | Select-Object -First 1; if (-not $t) { $t=$a | Where-Object { $_.prerelease } | Select-Object -First 1; if ($t) { [Console]::Error.WriteLine('NOTE: no stable v* release yet, falling back to the newest prerelease ' + $t.tag_name) } } }; if ($t) { $t.tag_name }"`) do set "TAG=%%T"
)
if not defined TAG (
    echo Cannot resolve a release ^(no installable v* release, or network unavailable^) 1>&2
    exit /b 1
)
echo Installing aProxy %TAG%

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
    rd /s /q "%TMP_DIR%" 2>nul
    exit /b 1
)
powershell -NoProfile -Command "try { Invoke-WebRequest -Uri '%BASE%/%ASSET%.sha256' -OutFile '%TMP_DIR%\aproxy.exe.sha256' -UserAgent '%UA%' } catch { exit 1 }"
if errorlevel 1 (
    echo Checksum download failed 1>&2
    rd /s /q "%TMP_DIR%" 2>nul
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
    rd /s /q "%TMP_DIR%" 2>nul
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
echo Next: configure the upstream first ^(a fresh install has no base_url, so a plain start fails^), then start:
echo   aproxy config --baseurl ^<upstream-url^> --api-key ^<key^>
echo   aproxy    ^(or full path "%BIN_DIR%\aproxy.exe"^)
echo Status: aproxy status    Upgrade: aproxy install
endlocal
exit /b 0
