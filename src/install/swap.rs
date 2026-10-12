//! 交换原语：把校验过的新二进制换进 `<home>/bin/`，普通安装与强制安装共用。
//!
//! 中间文件名是永久兼容契约 C2（见 [`OLD_PREFIX`]、[`TMP_PREFIX`]）。交换从不使用固定的
//! 中间文件名：短时间内连续换两次时，上一次让位的旧二进制可能还被进程占着（Windows 上
//! 运行中的镜像删不掉），固定名必然改名失败。所以旧二进制让位到唯一的
//! `aproxy.old-<纳秒>-<pid>[.exe]`，新文件先以唯一的 `aproxy.tmp-<纳秒>-<pid>[.exe]` 落进
//! bin，再一步改名到位。`bin/` 下以这两个前缀开头的文件都是残留，任何版本都可以在删得
//! 掉时删掉、删不掉就跳过——残留永远挡不住下一次安装。
//!
//! 两个平台的步骤相同，差别只在「旧二进制怎样让位」：
//!
//! - **Windows**：运行中的镜像禁止写与删、允许改名。旧二进制改名让位，新文件改名到位，
//!   两次改名之间 bin 里短暂没有 `aproxy.exe`（一次系统调用的窗口）；第二步失败立即
//!   改回。入口兜底脚本 `aproxy.bat` 在这个窗口里转调最近一次让位的旧二进制（见
//!   [`ensure_fallback_scripts_in`]）。
//! - **unix**：运行中的文件只锁 inode。先把旧二进制硬链接（失败则复制）成唯一的旧名，
//!   再把新文件 rename 覆盖到 `bin/aproxy`——一步原子替换，没有空窗。保留旧名不为防空窗，
//!   而为回滚：滚动重启时某实例在新版本下起不来，要用旧二进制按原参数把它拉回，而
//!   rename 覆盖之后旧 inode 只挂在运行中的进程上，文件系统里已没有路径可执行。
//!
//! 调用方负责在调用期间持有安装锁（永久兼容契约 C4，见 `super::lock_install_in`）。

use std::path::{Path, PathBuf};

/// 安装二进制的唯一管辖目录：`<home>/bin/`（永久兼容契约 C2 的位置部分）。
pub fn bin_dir_in(home: &Path) -> PathBuf {
    home.join("bin")
}

/// 规范位置的二进制全路径：`<home>/bin/aproxy`（Windows 为 `aproxy.exe`）。
pub fn bin_path_in(home: &Path) -> PathBuf {
    bin_dir_in(home).join(crate::install::staging::binary_name())
}

/// 旧二进制让位时改成的名字的前缀：完整名字是
/// `aproxy.old-<纳秒时间戳>-<pid>`，Windows 再加 `.exe`（让位的旧二进制要能被入口兜底
/// 脚本与回滚直接执行）。纳秒时间戳是 Unix 纪元以来的纳秒数（十进制，同一进程内严格
/// 递增），pid 是执行交换的进程，例如 `aproxy.old-1760256000123456789-4242.exe`。
///
/// 永久兼容契约 C2：前缀与「以它开头的文件随时可删」的语义无论如何都不得改动。任何
/// 版本都靠这个前缀认出别的版本留下的残留并清掉；改了前缀，旧版本留下的文件就再也没人
/// 认领，新版本留下的文件旧版本也清不掉。
pub const OLD_PREFIX: &str = "aproxy.old-";

/// 新二进制在 bin 里落盘时的临时名的前缀：完整名字是 `aproxy.tmp-<纳秒时间戳>-<pid>`，
/// Windows 再加 `.exe`。
///
/// 永久兼容契约 C2：无论如何都不得改动（理由同 [`OLD_PREFIX`]）。
pub const TMP_PREFIX: &str = "aproxy.tmp-";

/// 生成一个唯一的中间文件名：`<前缀><纳秒时间戳>-<pid>[.exe]`。时间戳取当前时刻，与
/// 本进程上一次取到的值相同时加一：系统时钟粒度可能比纳秒粗（Windows 是 100ns），连续
/// 两次取名不能撞上。不同进程靠 pid 区分。
pub fn unique_name(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut prev = LAST.load(Ordering::Relaxed);
    let nanos = loop {
        let next = now.max(prev + 1);
        match LAST.compare_exchange_weak(prev, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break next,
            Err(actual) => prev = actual,
        }
    };
    let exe = if cfg!(windows) { ".exe" } else { "" };
    format!("{prefix}{nanos}-{}{exe}", std::process::id())
}

/// 文件名是不是交换留下的中间文件（以 [`OLD_PREFIX`] 或 [`TMP_PREFIX`] 开头）。
pub fn is_residue_name(name: &str) -> bool {
    name.starts_with(OLD_PREFIX) || name.starts_with(TMP_PREFIX)
}

/// 尽力删掉 bin 里删得掉的中间文件残留，`keep` 除外（普通安装续作时，上一次尝试已
/// 记进安装状态的旧二进制是回滚要用的，不能删）。删不掉的（Windows 上仍被进程占着）
/// 直接跳过，绝不让残留挡住安装。返回删掉的个数。
pub fn sweep_residue_in(home: &Path, keep: Option<&Path>) -> usize {
    let Ok(dir) = std::fs::read_dir(bin_dir_in(home)) else {
        return 0;
    };
    let mut removed = 0;
    for item in dir.flatten() {
        let path = item.path();
        let is_residue = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(is_residue_name);
        if !is_residue || keep.is_some_and(|k| same_file_path(k, &path)) {
            continue;
        }
        if item.file_type().is_ok_and(|t| t.is_file()) && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// 两个路径是不是同一个文件名（Windows 文件系统不分大小写、分隔符两种写法都有）。
fn same_file_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let s = p.to_string_lossy().replace('\\', "/");
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    norm(a) == norm(b)
}

/// 交换结果：旧二进制让位到了哪里（bin 里原本没有二进制时为 None）。
#[derive(Debug, Clone, PartialEq)]
pub struct SwapOutcome {
    pub old_path: Option<PathBuf>,
}

/// 把 `source` 换进 `<home>/bin/`，并在新位置执行一次 `--version` 核对版本等于
/// `expected_version`。调用前置条件：`source` 已校验过（铁律 2）；调用方持有安装锁。
///
/// - `keep`：清残留时要保留的文件（普通安装续作时上一次尝试让位的旧二进制）。
/// - `before_retire`：旧二进制让位**之前**拿到它要去的唯一路径。普通安装在这里把它写进
///   安装状态——让位之后、写状态之前崩溃，续作就不知道回滚用的旧二进制在哪。它返回
///   Err 时什么都不动。
///
/// 返回 Err 时 bin 已恢复原状（新文件没放进去，或核对失败后已换回旧二进制）；只有恢复
/// 本身也失败时例外，错误信息会写明 bin 里现在是什么。
pub fn swap_in(
    home: &Path,
    source: &Path,
    expected_version: &str,
    keep: Option<&Path>,
    before_retire: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<SwapOutcome, String> {
    let dir = bin_dir_in(home);
    let bin = bin_path_in(home);
    std::fs::create_dir_all(&dir).map_err(|e| format!("bin 目录创建失败: {e}"))?;

    // 入口兜底脚本先于交换就位：只在交换之后才写，就保护不了这一次的空窗
    #[cfg(windows)]
    ensure_fallback_scripts_in(home).map_err(|e| format!("入口兜底脚本写入失败: {e}"))?;

    sweep_residue_in(home, keep);

    // 续作重入：上一次尝试已让旧二进制让位到 keep（让位之前就记进了安装状态，见
    // before_retire）。不能再让位一次：上一次若已经落位、只差推进相位（unix 的 rename、
    // Windows 的第二次改名之后中断），bin 里此刻是新二进制，再让位就会把它当成「旧的」
    // 记下来，之后的回滚拉起的仍是新版本。所以：bin 已经是新二进制 → 只核对；否则直接
    // 落位，旧二进制就是 keep——Windows 上 bin 此刻缺失，unix 上 bin 仍是旧二进制（keep
    // 是它的硬链接或副本），下面的 rename 原子地替换它
    let resumed = keep.filter(|k| k.is_file()).map(Path::to_path_buf);
    if let Some(old) = &resumed
        && bin.is_file()
        && same_contents(&bin, source)
    {
        verify_placed(&bin, &dir, expected_version, Some(old))?;
        return Ok(SwapOutcome { old_path: resumed });
    }

    // 1. 新文件以唯一临时名落进 bin：与 bin 同目录，最后一步改名必然在同一个卷上
    let tmp = copy_to_unique(source, &dir, TMP_PREFIX)?;

    // 2. 旧二进制让位（bin 里没有二进制：首次安装；续作重入：旧二进制已在 keep，见上）
    let old = if resumed.is_some() {
        resumed
    } else if bin.is_file() {
        match retire(&bin, &dir, before_retire) {
            Ok(old) => Some(old),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        }
    } else {
        None
    };

    // 3. 新二进制就位。unix 是一步原子替换；Windows 上 bin 此刻已空，失败就把旧的改回去
    if let Err(e) = with_retries(|| std::fs::rename(&tmp, &bin)) {
        let _ = std::fs::remove_file(&tmp);
        #[cfg(windows)]
        if let Some(old) = &old
            && let Err(back) = move_no_replace(old, &bin)
        {
            return Err(format!(
                "新二进制落位失败（{e}），把旧二进制改回 {} 也失败（{back}）：旧二进制现在在 {}",
                bin.display(),
                old.display()
            ));
        }
        return Err(format!("新二进制落位失败，bin 未改动: {e}"));
    }
    crate::install::staging::ensure_executable(&bin);

    // 4. 在新位置核对
    verify_placed(&bin, &dir, expected_version, old.as_deref())?;
    Ok(SwapOutcome { old_path: old })
}

/// 落位后在新位置执行一次 `--version`（有时限，见 `staging::run_version`），核对版本。
/// source 已校验过同样的内容，这里防的是复制、改名环节的意外（杀毒软件隔离了刚落位的
/// 文件等）。核对不过就换回旧二进制 `old`。
fn verify_placed(
    bin: &Path,
    dir: &Path,
    expected_version: &str,
    old: Option<&Path>,
) -> Result<(), String> {
    let verified = match crate::install::staging::probe_version(bin) {
        Ok(v) if v == expected_version => Ok(()),
        Ok(v) => Err(format!("自报版本 {v}，应为 {expected_version}")),
        Err(e) => Err(e),
    };
    verified.map_err(|e| match roll_back(bin, dir, old) {
        Ok(()) => format!("落位后核对失败，已换回原来的二进制: {e}"),
        Err(back) => format!("落位后核对失败（{e}），换回原来的二进制也失败: {back}"),
    })
}

/// 两个文件的内容是否相同（比 sha256；读不了的算不同）。
fn same_contents(a: &Path, b: &Path) -> bool {
    let hash = crate::install::staging::sha256_hex;
    matches!((hash(a), hash(b)), (Ok(x), Ok(y)) if x == y)
}

/// 把 `source` 复制成 `dir` 里的唯一中间文件（[`unique_name`]，目标已存在就换一个名字
/// 再试），补可执行位，返回它的路径。用 create_new 打开目标：std::fs::copy 会覆盖已存在
/// 的文件，唯一名要的是「已存在就失败」。
fn copy_to_unique(source: &Path, dir: &Path, prefix: &str) -> Result<PathBuf, String> {
    for _ in 0..8 {
        let path = dir.join(unique_name(prefix));
        let mut out = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(out) => out,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("{} 创建失败: {e}", path.display())),
        };
        // 改名到位之前先落盘（sync_all）：交换刚完成几秒内断电，没落盘的新文件可能只剩一个
        // 全零的空壳——它占着 aproxy(.exe) 的名字，入口兜底脚本也就不会去找让位的旧二进制
        let copied = std::fs::File::open(source)
            .and_then(|mut src| std::io::copy(&mut src, &mut out))
            .and_then(|_| out.sync_all());
        drop(out);
        if let Err(e) = copied {
            let _ = std::fs::remove_file(&path);
            return Err(format!(
                "{} 复制到 {} 失败: {e}",
                source.display(),
                path.display()
            ));
        }
        crate::install::staging::ensure_executable(&path);
        return Ok(path);
    }
    Err(format!("{} 里取不到不冲突的中间文件名", dir.display()))
}

/// 旧二进制让位到一个唯一的 `aproxy.old-…` 名字，返回它。先取名（已存在就换），交给
/// `before_retire`，再动文件。
fn retire(
    bin: &Path,
    dir: &Path,
    before_retire: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<PathBuf, String> {
    let old = loop {
        let candidate = dir.join(unique_name(OLD_PREFIX));
        if std::fs::symlink_metadata(&candidate).is_err() {
            break candidate;
        }
    };
    before_retire(&old)?;
    #[cfg(windows)]
    {
        // 改名不覆盖：取名之后若真有人抢先建了同名文件，这里失败而不是把它盖掉
        move_no_replace(bin, &old).map_err(|e| {
            format!("旧二进制让位失败（被不带共享删除权限的句柄占着？），bin 未改动: {e}")
        })?;
    }
    #[cfg(not(windows))]
    {
        // 硬链接零拷贝，且与 bin 同目录必然同卷；文件系统不支持硬链接时退回复制。
        // 两者都在目标已存在时失败，不会覆盖别的文件
        if std::fs::hard_link(bin, &old).is_err() {
            let copied = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&old)
                .and_then(|mut out| {
                    std::fs::File::open(bin).and_then(|mut src| std::io::copy(&mut src, &mut out))
                });
            if let Err(e) = copied {
                let _ = std::fs::remove_file(&old);
                return Err(format!(
                    "旧二进制保留失败（没有它，实例在新版本下起不来时无从回滚），bin 未改动: {e}"
                ));
            }
            crate::install::staging::ensure_executable(&old);
        }
    }
    Ok(old)
}

/// 核对失败后换回旧二进制：新文件改成一个唯一的临时名（成为残留，以后清掉），旧的
/// 改回 bin。bin 里原本没有二进制（首次安装）时只把新文件挪开。
fn roll_back(bin: &Path, dir: &Path, old: Option<&Path>) -> Result<(), String> {
    #[cfg(windows)]
    {
        let aside = dir.join(unique_name(TMP_PREFIX));
        move_no_replace(bin, &aside).map_err(|e| format!("挪开新二进制失败: {e}"))?;
        if let Some(old) = old {
            move_no_replace(old, bin).map_err(|e| {
                format!(
                    "旧二进制改回失败: {e}；旧二进制在 {}，新二进制在 {}",
                    old.display(),
                    aside.display()
                )
            })?;
        }
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        // unix：旧名是硬链接（或副本），一步 rename 原子地换回去
        match old {
            Some(old) => std::fs::rename(old, bin)
                .map_err(|e| format!("旧二进制改回失败: {e}；旧二进制在 {}", old.display()))?,
            None => std::fs::remove_file(bin).map_err(|e| format!("挪开新二进制失败: {e}"))?,
        }
    }
    Ok(())
}

/// 文件操作的短暂退避重试：杀毒软件扫描新落盘的文件时可能短暂地以不带共享删除权限的
/// 句柄占着它（Windows），改名会失败，几百毫秒后就好。共 5 次，间隔 100、200、400、
/// 800 毫秒。
fn with_retries(mut op: impl FnMut() -> std::io::Result<()>) -> std::io::Result<()> {
    let mut delay = std::time::Duration::from_millis(100);
    for _ in 0..4 {
        if op().is_ok() {
            return Ok(());
        }
        std::thread::sleep(delay);
        delay *= 2;
    }
    op()
}

/// Windows：改名且目标已存在时失败（MoveFileExW 不带 MOVEFILE_REPLACE_EXISTING），带
/// 短暂退避重试。std::fs::rename 在 Windows 上会替换已存在的目标，所以不用它。运行中的
/// 镜像允许改名。
#[cfg(windows)]
fn move_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain(Some(0)).collect() };
    let (from_w, to_w) = (wide(from), wide(to));
    with_retries(|| {
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(from_w.as_ptr(), to_w.as_ptr(), 0)
        };
        if ok == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    })
}

// ---------------------------------------------------------------------------
// 入口兜底脚本（Windows 专属：unix 的交换是一步原子替换，没有空窗）
// ---------------------------------------------------------------------------

/// 幂等确保入口兜底脚本在场：每次交换之前都写一遍（内容固定，重复写覆盖为同一内容）。
/// 这里是它们唯一的写者——一键安装脚本只做首装，首装之后 bin 里一直有 `aproxy.exe`，
/// 用不到兜底；第一次交换之前这里就会写好。`aproxy.ps1` 不做：PowerShell 的命令发现不
/// 含 .ps1 的这种用法，没有价值。
#[cfg(windows)]
pub fn ensure_fallback_scripts_in(home: &Path) -> std::io::Result<()> {
    let dir = bin_dir_in(home);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("aproxy.bat"), FALLBACK_BAT)?;
    std::fs::write(dir.join("aproxy"), FALLBACK_SH)?;
    Ok(())
}

/// cmd 与 PowerShell 用：PATHEXT 里 .EXE 排在 .BAT 前面，`aproxy.exe` 在场时这个脚本
/// 根本不参与。`aproxy.exe` 缺席（交换的两次改名之间、或交换在这一步被打断）时，按
/// 名字倒序取第一个 `aproxy.old-*.exe` 转调：名字里的纳秒时间戳在 2286 年之前都是 19 位，
/// 字典序就是先后顺序，第一个就是最近一次让位的旧二进制。`exit /b` 不带参数，退出码
/// 沿用被转调的进程的。
#[cfg(windows)]
const FALLBACK_BAT: &str = r#"@echo off
rem aProxy safe-entry: PATHEXT resolves .EXE before .BAT, so this script only
rem runs while aproxy.exe is missing (mid-upgrade) and falls back to the most
rem recently replaced binary, aproxy.old-<nanoseconds>-<pid>.exe.
if exist "%~dp0aproxy.exe" (
  "%~dp0aproxy.exe" %*
  exit /b
)
for /f "delims=" %%F in ('dir /b /a-d /o-n "%~dp0aproxy.old-*.exe" 2^>nul') do (
  "%~dp0%%F" %*
  exit /b
)
echo aproxy.exe not found in %~dp0 - upgrade in progress or broken install 1>&2
exit /b 1
"#;

/// Git Bash / MSYS 用（无扩展名：PATH 解析能命中；cmd 与 PowerShell 不会调它）。选旧
/// 二进制的规则同 [`FALLBACK_BAT`]。
#[cfg(windows)]
const FALLBACK_SH: &str = r#"#!/bin/sh
# aProxy safe-entry (Git Bash/MSYS): prefer the real binary, fall back to the
# most recently replaced one, aproxy.old-<nanoseconds>-<pid>.exe, mid-upgrade.
dir=$(dirname "$0")
if [ -x "$dir/aproxy.exe" ]; then
  exec "$dir/aproxy.exe" "$@"
fi
old=$(ls -1 "$dir"/aproxy.old-*.exe 2>/dev/null | sort | tail -n 1)
if [ -n "$old" ]; then
  exec "$old" "$@"
fi
echo "aproxy.exe not found in $dir - upgrade in progress or broken install" >&2
exit 1
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bin_layout_paths() {
        let home = Path::new("/tmp/fake-home");
        assert_eq!(bin_dir_in(home), home.join("bin"));
        if cfg!(windows) {
            assert_eq!(bin_path_in(home), home.join("bin").join("aproxy.exe"));
        } else {
            assert_eq!(bin_path_in(home), home.join("bin").join("aproxy"));
        }
    }

    #[test]
    fn unique_names_never_repeat_within_a_process() {
        let names: std::collections::HashSet<String> =
            (0..1000).map(|_| unique_name(OLD_PREFIX)).collect();
        assert_eq!(names.len(), 1000);
        assert!(names.iter().all(|n| is_residue_name(n)));
        assert!(!is_residue_name("aproxy.exe") && !is_residue_name("aproxy.bat"));
        // 0.1.x 的固定名不在约定之内
        assert!(!is_residue_name("aproxy.old.exe") && !is_residue_name("aproxy.exe.new"));
    }
}
