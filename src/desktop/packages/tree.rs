//! 对目录的读取相对于已验证的父句柄；不能让检查和打开之间替换的链接改变实际读到的文件。
use std::{ffi::OsString, fs::File, io, path::Path};
/// 分类原生打开失败，不把操作系统诊断或本机路径传给页面。
#[derive(Debug, Clone, Copy)]
pub(super) enum Why {
    Io,
    Link,
    FileType,
}
/// 打开的目录在遍历其子项期间保持有效；Windows 句柄不共享删除。
pub(super) struct Dir {
    file: File,
    #[cfg(windows)]
    path: std::path::PathBuf,
}
/// 只返回已打开的普通文件或目录，后续读取不重新解析名字。
pub(super) enum Node {
    Dir(Dir),
    File(File),
}
#[cfg(windows)]
fn windows_open(path: &Path, mutable: bool) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    File::options()
        .read(true)
        .share_mode(FILE_SHARE_READ | if mutable { FILE_SHARE_WRITE } else { 0 })
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}
impl Dir {
    /// 包资源只读，Windows 同时拒绝写入及改名共享。
    pub(super) fn open(path: &Path) -> Result<Self, Why> {
        Self::open_dir(path, false)
    }
    /// 可写数据目录仍不共享删除，允许同目录原子替换所需的写共享。
    pub(super) fn open_dir(path: &Path, mutable: bool) -> Result<Self, Why> {
        #[cfg(unix)]
        let _ = mutable;
        #[cfg(windows)]
        let f = windows_open(path, mutable);
        #[cfg(unix)]
        let f = {
            use std::os::unix::fs::OpenOptionsExt;
            File::options()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
                .open(path)
        };
        let file = f.map_err(|_| Why::Io)?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if file.metadata().map_err(|_| Why::Io)?.file_attributes() & 0x400 != 0 {
                return Err(Why::Link);
            }
        }
        if !file.metadata().map_err(|_| Why::Io)?.is_dir() {
            return Err(Why::FileType);
        }
        Ok(Self {
            file,
            #[cfg(windows)]
            path: path.to_owned(),
        })
    }
    /// 名字由上层先按单个路径分量核验，不能让带斜线的外部输入直接到达 openat/join。
    pub(super) fn node(&self, name: impl AsRef<std::ffi::OsStr>) -> Result<Option<Node>, Why> {
        self.open_child(name.as_ref(), false)
    }
    fn open_child(&self, name: &std::ffi::OsStr, mutable: bool) -> Result<Option<Node>, Why> {
        #[cfg(unix)]
        let _ = mutable;
        #[cfg(windows)]
        let opened = {
            let _keep_open = &self.file;
            windows_open(&self.path.join(name), mutable)
        };
        #[cfg(unix)]
        let opened = {
            use std::os::fd::{AsRawFd, FromRawFd};
            use std::os::unix::ffi::OsStrExt;
            let c = std::ffi::CString::new(name.as_bytes()).map_err(|_| Why::FileType)?;
            let mut meta = std::mem::MaybeUninit::uninit();
            let fd = self.file.as_raw_fd();
            if unsafe {
                libc::fstatat(fd, c.as_ptr(), meta.as_mut_ptr(), libc::AT_SYMLINK_NOFOLLOW)
            } != 0
            {
                return optional_error(io::Error::last_os_error());
            }
            let st = unsafe { meta.assume_init() };
            let kind = st.st_mode & libc::S_IFMT;
            if kind == libc::S_IFLNK {
                return Err(Why::Link);
            }
            if kind != libc::S_IFDIR && kind != libc::S_IFREG {
                return Err(Why::FileType);
            }
            let flags = libc::O_RDONLY
                | libc::O_NONBLOCK
                | libc::O_NOFOLLOW
                | libc::O_CLOEXEC
                | if kind == libc::S_IFDIR {
                    libc::O_DIRECTORY
                } else {
                    0
                };
            let raw = unsafe { libc::openat(fd, c.as_ptr(), flags) };
            if raw < 0 {
                Err(io::Error::last_os_error())
            } else {
                let file = unsafe { File::from_raw_fd(raw) };
                use std::os::unix::fs::MetadataExt;
                let m = file.metadata().map_err(|_| Why::Io)?;
                if u128::from(m.dev()) != st.st_dev as u128
                    || u128::from(m.ino()) != st.st_ino as u128
                {
                    return Err(Why::Io);
                }
                Ok(file)
            }
        };
        let f = match opened {
            Ok(v) => v,
            Err(e) => return optional_error(e),
        };
        let md = f.metadata().map_err(|_| Why::Io)?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if md.file_attributes() & 0x400 != 0 {
                return Err(Why::Link);
            }
        }
        if md.is_dir() {
            Ok(Some(Node::Dir(Self {
                file: f,
                #[cfg(windows)]
                path: self.path.join(name),
            })))
        } else if md.is_file() {
            Ok(Some(Node::File(f)))
        } else {
            Err(Why::FileType)
        }
    }
    /// 目录由父句柄逐级建立；读操作传 false，不会因读到缺项就写磁盘。
    pub(super) fn directory(&self, name: &str, create: bool) -> Result<Option<Dir>, Why> {
        match self.open_child(name.as_ref(), true)? {
            Some(Node::Dir(d)) => return Ok(Some(d)),
            Some(_) => return Err(Why::FileType),
            None if !create => return Ok(None),
            None => {}
        }
        {
            #[cfg(windows)]
            let result = std::fs::create_dir(self.path.join(name));
            #[cfg(unix)]
            let result = {
                use std::os::fd::AsRawFd;
                let c = std::ffi::CString::new(name).map_err(|_| Why::FileType)?;
                if unsafe { libc::mkdirat(self.file.as_raw_fd(), c.as_ptr(), 0o700) } == 0 {
                    Ok(())
                } else {
                    Err(io::Error::last_os_error())
                }
            };
            if let Err(e) = result
                && e.kind() != io::ErrorKind::AlreadyExists
            {
                return Err(Why::Io);
            }
        }
        match self.open_child(name.as_ref(), true)? {
            Some(Node::Dir(d)) => Ok(Some(d)),
            None => Err(Why::Io),
            _ => Err(Why::FileType),
        }
    }
    /// 创建同目录临时文件再原子换名。Unix 的目标相对于已经打开的目录 fd；Windows 的目录句柄不共享删除。
    pub(super) fn write(&self, name: &str, data: &[u8]) -> Result<(), Why> {
        if let Some(Node::Dir(_)) = self.node(name)? {
            return Err(Why::FileType);
        }
        #[cfg(windows)]
        {
            crate::fsx::write_atomic_private(&self.path.join(name), data).map_err(|_| Why::Io)
        }
        #[cfg(unix)]
        {
            use std::{
                ffi::CString,
                io::Write,
                os::fd::{AsRawFd, FromRawFd},
            };
            let fd = self.file.as_raw_fd();
            let dest = CString::new(name).map_err(|_| Why::FileType)?;
            let nonce = getrandom::u64().map_err(|_| Why::Io)?;
            let tmp = CString::new(format!(".tmp-{nonce:016x}")).unwrap();
            let raw = unsafe {
                libc::openat(
                    fd,
                    tmp.as_ptr(),
                    libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_WRONLY
                        | libc::O_CLOEXEC
                        | libc::O_NOFOLLOW,
                    0o600,
                )
            };
            if raw < 0 {
                return Err(Why::Io);
            }
            let mut f = unsafe { File::from_raw_fd(raw) };
            let result = f
                .write_all(data)
                .and_then(|()| f.sync_all())
                .and_then(|()| {
                    if unsafe { libc::renameat(fd, tmp.as_ptr(), fd, dest.as_ptr()) } == 0 {
                        Ok(())
                    } else {
                        Err(io::Error::last_os_error())
                    }
                });
            if result.is_err() {
                unsafe { libc::unlinkat(fd, tmp.as_ptr(), 0) };
            }
            result.map_err(|_| Why::Io)
        }
    }
    /// 只删已核验的普通文件；缺失表示期望状态已达成。
    pub(super) fn delete(&self, name: &str) -> Result<(), Why> {
        match self.node(name)? {
            None => return Ok(()),
            Some(Node::Dir(_)) => return Err(Why::FileType),
            _ => (),
        }
        #[cfg(windows)]
        {
            std::fs::remove_file(self.path.join(name)).map_err(|_| Why::Io)
        }
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let c = std::ffi::CString::new(name).map_err(|_| Why::FileType)?;
            if unsafe { libc::unlinkat(self.file.as_raw_fd(), c.as_ptr(), 0) } == 0 {
                Ok(())
            } else {
                Err(Why::Io)
            }
        }
    }
    /// 保留名称的原生编码，以便配额也计算无法显示的普通文件。
    pub(super) fn names(&self) -> Result<Vec<OsString>, Why> {
        let mut names = Vec::new();
        #[cfg(windows)]
        for entry in std::fs::read_dir(&self.path).map_err(|_| Why::Io)? {
            names.push(entry.map_err(|_| Why::Io)?.file_name());
        }
        #[cfg(unix)]
        {
            use std::os::{fd::AsRawFd, unix::ffi::OsStrExt};
            // dup 会共享原描述符的枚举游标；单独打开 "." 才能在扫描末尾从头核对。
            let fd = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    c".".as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(Why::Io);
            }
            let dir = unsafe { libc::fdopendir(fd) };
            if dir.is_null() {
                unsafe { libc::close(fd) };
                return Err(Why::Io);
            }
            struct Entries(*mut libc::DIR);
            impl Drop for Entries {
                fn drop(&mut self) {
                    unsafe { libc::closedir(self.0) };
                }
            }
            let dir = Entries(dir);
            loop {
                #[cfg(target_os = "macos")]
                let errno = unsafe { libc::__error() };
                #[cfg(not(target_os = "macos"))]
                let errno = unsafe { libc::__errno_location() };
                unsafe { *errno = 0 };
                let ent = unsafe { libc::readdir(dir.0) };
                if ent.is_null() {
                    if unsafe { *errno } != 0 {
                        return Err(Why::Io);
                    }
                    break;
                }
                let bytes = unsafe { std::ffi::CStr::from_ptr((*ent).d_name.as_ptr()) }.to_bytes();
                if matches!(bytes, b"." | b"..") {
                    continue;
                }
                names.push(std::ffi::OsStr::from_bytes(bytes).to_os_string());
            }
        }
        names.sort_unstable();
        Ok(names)
    }
}
fn optional_error<T>(e: io::Error) -> Result<Option<T>, Why> {
    if e.kind() == io::ErrorKind::NotFound {
        Ok(None)
    } else {
        Err(Why::Io)
    }
}
