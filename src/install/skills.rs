//! skill 更新支线（非强制）：install 时随二进制**并行下载更新**
//! `~/.aproxy/skills/aproxy-cli/`。
//!
//! 非强制语义（用户定调）：skill 不是二进制的依赖——下载失败重试后放弃，
//! **安装照常成功**，主流程任何阶段都不等待/不受影响（唯一同步点：install
//! 进程结束前 join 收尾并落终态）。未完成的下载重取；目录切换由持锁的下一次安装恢复。
//! `--continue` 续作时 failed 不再自动重试
//! （避免每次续作都拖一遍下载），`aproxy skill install` 手动重试。
//!
//! 落位 = 目录原子替换：解压到 staging 子目录 → 旧目录 rename 走 → 新目录
//! rename 进 → 删旧；Windows 上 agent 正读 skill 文件的冲突短重试。

pub mod archive;
pub mod installed;
pub mod manifest;

use super::download::{self, Artifact, DownloadCtx};
use std::path::{Path, PathBuf};
use std::{
    fs,
    io::{self, Read, Write},
};

/// 同卷独占的临时目录。Unix 不依赖用户的 umask，Windows 继承私有 home 的 ACL。
fn workspace(parent: &Path) -> std::io::Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(".staging-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir_in(parent)
}

/// 锁文件还保存工作区凭据，写入前要排除硬链接，否则空的外部文件也会被改写。
fn single_link(file: &fs::File) -> io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(file.metadata()?.nlink() == 1)
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // 成功返回时系统已填好整份结构，句柄在调用期间由 file 持有。
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as isize, info.as_mut_ptr()) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { info.assume_init() }.nNumberOfLinks == 1)
    }
}

/// 从下载开始直到发布完成一直持有独立的文件锁；进程退出后内核释放，不按 pid 猜测过期。
fn skill_lock(home: &Path) -> Result<super::InstallLock, String> {
    let root = home.join("skills");
    crate::fsx::create_dir_all(&root)
        .and_then(|()| installed::directory(&root))
        .map_err(|e| e.to_string())?;
    let lock = || -> std::io::Result<Option<super::InstallLock>> {
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
            opts.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
        }
        let file = installed::open_regular(&root.join(".lock"), &mut opts)?;
        if !single_link(&file)? {
            return Err(io::Error::other("skill 锁文件有其他硬链接"));
        }
        Ok(super::try_lock_exclusive(&file)?.then_some(super::InstallLock { file }))
    };
    lock()
        .map_err(|e| format!("skill 加锁失败：{e}"))?
        .ok_or_else(|| "另一个进程正在安装 skill，请稍后重试".into())
}

/// 锁内的定长令牌指向独占创建的私有目录。另半段令牌留在目录中作凭据，
/// 不从目录名前缀猜所有权，不遍历 skills/。不确定归属（包括初始化中断）时停下而不清目录。
fn private_root(root: &Path, lock: &mut super::InstallLock) -> io::Result<PathBuf> {
    use std::hash::{BuildHasher, RandomState};
    let file = &mut lock.file;
    let mut token = String::new();
    file.take(33).read_to_string(&mut token)?;
    let invalid =
        || io::Error::other("skill 工作区凭据无效；请移走 skills/ 后执行 aproxy skill install");
    if token.is_empty() {
        let rng = RandomState::new();
        token = format!("{:016x}{:016x}", rng.hash_one(0u8), rng.hash_one(1u8));
        let path = root.join(format!(".staging-{}", &token[..16]));
        match fs::symlink_metadata(&path) {
            Ok(_) => return Err(invalid()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        file.write_all(token.as_bytes())?;
        file.sync_data()?;
    }
    if token.len() != 32 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    let path = root.join(format!(".staging-{}", &token[..16]));
    match installed::directory(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let mut options = fs::DirBuilder::new();
            let builder = &mut options;
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&path)?;
            fs::File::create_new(path.join("owner"))?.write_all(&token.as_bytes()[16..])?;
        }
        Err(e) => return Err(e),
    }
    let mut proof = Vec::new();
    installed::open_regular(&path.join("owner"), fs::OpenOptions::new().read(true))
        .map_err(|_| invalid())?
        .take(17)
        .read_to_end(&mut proof)?;
    if proof != token.as_bytes()[16..] {
        return Err(invalid());
    }
    Ok(path)
}

/// 私有目录里最多一份活动事务、一份待清理事务。先整体改名再删除；删除中断不再触发回滚。
fn recover(root: &Path, private: &Path) -> io::Result<()> {
    let active = private.join("active");
    let spent = private.join("spent");
    if spent.try_exists()? {
        installed::directory(&spent)?;
        fs::remove_dir_all(&spent)?;
    }
    if !active.try_exists()? {
        return Ok(());
    }
    installed::directory(&active)?;
    let mut entries = fs::read_dir(&active)?;
    if let Some(entry) = entries.next().transpose()? {
        if entries.next().is_some() || !entry.file_name().to_string_lossy().starts_with(".staging-")
        {
            return Err(io::Error::other("skill 工作区内容异常"));
        }
        let stage = entry.path();
        installed::directory(&stage)?;
        archive::restore_interrupted(root, &stage)?;
    }
    fs::rename(active, &spent)?;
    fs::remove_dir_all(spent)
}

/// 在同一把锁里恢复上一次切换，再下载、核验与发布；失败也在释放锁之前收尾。
pub async fn install(
    ctx: &DownloadCtx,
    chain: &[download::ChainStep],
    home: &Path,
) -> Result<usize, super::InstallFailure> {
    let mut lock = skill_lock(home)?;
    let root = home.join("skills");
    let private = private_root(&root, &mut lock).map_err(|e| e.to_string())?;
    recover(&root, &private).map_err(|e| e.to_string())?;
    let active = private.join("active");
    fs::create_dir(&active).map_err(|e| e.to_string())?;
    let stage = workspace(&active)
        .map_err(|e| format!("skill 下载目录创建失败：{e}"))?
        .keep();
    let result = async {
        let fetched = download::fetch_artifact(ctx, chain, Artifact::Skills, &stage)
            .await
            .map_err(|e| {
                // 链条的文字可能已经拼接了多个来源。整段地址去掉，保留渠道与失败原因；
                // 同一错误也可能写入普通安装的状态文件，不能只在 CLI 打印时做遮盖。
                static URL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
                    regex::Regex::new(r"(?i)[a-z][a-z0-9+.\-]*://\S+").unwrap()
                });
                let message = URL.replace_all(&e.message, "[URL 已隐去]").into_owned();
                e.with_message(message)
            })?;
        let (manifest, ignored) = archive::publish(&root, &stage, &fetched.path, &ctx.version)
            .map_err(|e| format!("skill 安装失败：{e}"))?;
        if ignored != 0 {
            tracing::warn!(ignored, "已跳过 skill 包中未登记的条目");
        }
        tracing::info!(skills=?manifest.names(),"skill 文档已随版本落位");
        Ok(manifest.files.len())
    }
    .await;
    recover(&root, &private).map_err(|e| e.to_string())?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 硬链接虽然是普通文件，也不能借工作区登记过程给另一处空文件写入内容。
    #[test]
    fn skill_lock_refuses_hard_link() {
        let h = tempfile::tempdir().unwrap();
        let root = h.path().join("skills");
        fs::create_dir(&root).unwrap();
        let outside = h.path().join("note");
        fs::write(&outside, b"").unwrap();
        fs::hard_link(&outside, root.join(".lock")).unwrap();
        assert!(super::skill_lock(h.path()).is_err());
        assert!(fs::read(outside).unwrap().is_empty());
    }
    /// 即使锁链接的目标尚不存在也不能在 skill 目录外建文件；已有目标也不能充当锁。
    #[test]
    fn skill_lock_refuses_link_and_non_file() {
        #[cfg(unix)]
        use std::os::unix::fs::symlink as link;
        #[cfg(windows)]
        use std::os::windows::fs::symlink_file as link;
        let h = tempfile::tempdir().unwrap();
        let lock = h.path().join("skills/.lock");
        fs::create_dir_all(&lock).unwrap();
        assert!(super::skill_lock(h.path()).is_err());
        fs::remove_dir(&lock).unwrap();
        let target = h.path().join("other");
        if let Err(e) = link(&target, &lock) {
            eprintln!("跳过符号链接检查：{e}");
            return;
        }
        assert!(super::skill_lock(h.path()).is_err());
        assert!(!target.exists());
        fs::write(&target, b"keep").unwrap();
        assert!(super::skill_lock(h.path()).is_err());
    }

    /// 随发布出去的每个 skill（仓库的 `.claude/skills/*/SKILL.md`），description 都不超过
    /// 1024 个字符：这是 Agent Skills 规范与 skill-creator 校验的上限，按上限校验的客户端
    /// 会拒收或截断，整份 skill 可能加载失败。description 只写成单行（`description: …`）。
    #[test]
    fn shipped_skill_descriptions_fit_the_1024_character_limit() {
        let skills = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".claude")
            .join("skills");
        let mut checked = 0;
        for dir in std::fs::read_dir(&skills).unwrap().flatten() {
            if !dir.file_type().unwrap().is_dir() {
                continue;
            }
            let doc_path = dir.path().join("SKILL.md");
            let doc = std::fs::read_to_string(&doc_path).unwrap();
            let description = doc
                .lines()
                .find_map(|line| line.strip_prefix("description:"))
                .unwrap_or_else(|| panic!("{} 没有单行的 description", doc_path.display()))
                .trim();
            let chars = description.chars().count();
            assert!(
                chars <= 1024,
                "{} 的 description 有 {chars} 个字符，超过 1024",
                doc_path.display()
            );
            checked += 1;
        }
        assert!(checked >= 2, "应至少检查 aproxy-cli 与 aproxy-format");
    }
}

#[cfg(test)]
mod archive_tests;
