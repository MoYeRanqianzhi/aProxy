//! 文件系统的小工具：原子写。注册表（.pid）、恢复记录（.restore）、安装状态、settings.json、
//! 看护者的 claim 都经这一处写盘。台账（永久兼容契约 C3）有自己冻结的写者，不走这里。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// 原子地把 `bytes` 写到 `path`：在同目录下 create_new 一个临时文件，写完 `sync_all`，再 rename
/// 覆盖目标。父目录不存在时先建好。
///
/// 为什么要原子：这些文件都有并发的读者（status、看护者、安装器……）。原地写时读者可能读到
/// 写到一半的 JSON，把记录当损坏处理——注册表被误删、claim 被当成「无人在任」而补种出第二个
/// 看护者。rename 在同一卷内是原子的（Windows 上经 MoveFileEx 的替换语义覆盖已存在的文件）。
/// rename 之前先 `sync_all`：不然断电后可能留下一个名字已换、内容却还没落盘的空文件。
///
/// 临时文件名是 `<目标文件名>.tmp-<pid>-<纳秒>`（W2 设计稿 §4.1 的命名），纳秒取当前时间、在
/// 本进程内严格递增：同一目标的并发写者（两个进程、或同一进程的两个线程）各写各的临时文件，
/// 不会把对方写了一半的内容截断后再 rename 出去。create_new 保证万一撞名也只会失败、不会
/// 覆盖别人的临时文件。rename 或写入失败时删掉临时文件；进程恰好在写临时文件与 rename 之间
/// 崩溃时会留下一个临时文件，读者只认目标文件名，不受影响。
///
/// unix 上临时文件一创建就是 0600：这些记录可能含密钥（恢复记录里有启动参数，其中可能有
/// `--api-key`），rename 之前也不能被别的用户读到。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = temp_path_for(path);
    let written = create_new_private(&tmp).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn create_new_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// `<目标文件名>.tmp-<pid>-<纳秒>`，与目标同目录（rename 只在同一卷内原子）
fn temp_path_for(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp-{}-{}", std::process::id(), unique_nanos()));
    path.with_file_name(name)
}

/// 当前时间的纳秒数，在本进程内严格递增（同一纳秒内的两次调用拿到相邻的两个值）
fn unique_nanos() -> u64 {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut prev = LAST.load(Ordering::Relaxed);
    loop {
        let next = now.max(prev + 1);
        match LAST.compare_exchange_weak(prev, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(actual) => prev = actual,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_creates_parents_and_replaces_existing_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a").join("b").join("record.json");
        write_atomic(&path, b"first").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        write_atomic(&path, b"second, longer than the first").unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"second, longer than the first"
        );
        // 不留临时文件
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("record.json")]);
    }

    #[test]
    fn concurrent_writers_never_leave_a_torn_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.json");
        write_atomic(&path, b"initial").unwrap();
        let contents: Vec<Vec<u8>> = (0..8u8).map(|i| vec![b'a' + i; 64 * 1024]).collect();
        std::thread::scope(|scope| {
            for content in &contents {
                let path = &path;
                scope.spawn(move || {
                    for _ in 0..20 {
                        // Windows 上 rename 覆盖一个正被另一次 rename 替换的文件可能短暂失败
                        // （共享冲突）；这里只关心「读到的永远是某一次完整的写入」
                        let _ = write_atomic(path, content);
                    }
                });
            }
        });
        let last = std::fs::read(&path).unwrap();
        assert!(
            last == b"initial" || contents.contains(&last),
            "读到的必须是某一次完整写入的内容"
        );
    }

    #[test]
    fn temp_names_are_unique_and_next_to_the_target() {
        let path = Path::new("dir").join("x.json");
        let (a, b) = (temp_path_for(&path), temp_path_for(&path));
        assert_ne!(a, b);
        assert_eq!(a.parent(), path.parent());
        assert!(
            a.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("x.json.tmp-{}-", std::process::id()))
        );
    }

    #[cfg(unix)]
    #[test]
    fn written_files_are_private_to_the_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.json");
        write_atomic(&path, b"{}").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
