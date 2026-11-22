//! 目录只从已核验的父目录向下打开。Windows 持有不共享删除或写入的父句柄；Unix 的查找和枚举均相对于目录 fd。
use aproxy_ipc::desktop::InvalidReason as Why;
use std::{
    ffi::OsString,
    fs::{File, Metadata},
    io,
    path::Path,
    time::SystemTime,
};

pub(super) struct Dir {
    file: File,
    #[cfg(windows)]
    path: std::path::PathBuf,
}
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(super) struct Stamp {
    identity: (u64, u64),
    pub(super) bytes: u64,
    modified: SystemTime,
    #[cfg(unix)]
    change: (i64, i64),
}
fn stamp(f: &File, md: &Metadata) -> Result<Stamp, Why> {
    #[cfg(unix)]
    let (dev, ino, links, change) = {
        use std::os::unix::fs::MetadataExt;
        (
            md.dev(),
            md.ino(),
            md.nlink(),
            (md.ctime(), md.ctime_nsec()),
        )
    };
    #[cfg(windows)]
    let (dev, ino, links) = {
        use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_TYPE_DISK,
            GetFileInformationByHandle, GetFileType,
        };
        if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(Why::Link);
        }
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // 信息来自此后实际读取的同一个句柄，不能先核验路径再按路径打开文件。
        let raw = f.as_raw_handle() as isize;
        if unsafe { GetFileType(raw) } != FILE_TYPE_DISK {
            return Err(Why::FileType);
        }
        if unsafe { GetFileInformationByHandle(raw, info.as_mut_ptr()) } == 0 {
            return Err(Why::Io);
        }
        let i = unsafe { info.assume_init() };
        (
            u64::from(i.dwVolumeSerialNumber),
            (u64::from(i.nFileIndexHigh) << 32) | u64::from(i.nFileIndexLow),
            u64::from(i.nNumberOfLinks),
        )
    };
    #[cfg(unix)]
    let _ = f;
    if md.is_file() && links != 1 {
        return Err(Why::Link);
    }
    if !(md.is_dir() || md.is_file()) {
        return Err(Why::FileType);
    }
    Ok(Stamp {
        identity: (dev, ino),
        bytes: md.len(),
        modified: md.modified().map_err(|_| Why::Io)?,
        #[cfg(unix)]
        change,
    })
}
pub(super) fn observed(f: &File) -> Result<Stamp, Why> {
    stamp(f, &f.metadata().map_err(|_| Why::Io)?)
}

#[cfg(windows)]
fn windows_open(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
    };
    File::options()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}
impl Dir {
    pub(super) fn home(path: &Path) -> Result<Self, Why> {
        Self::open_dir(path, false)
    }
    // Windows 的同目录原子替换需要父目录允许共享写；仍不共享删除，且拒绝重解析点。
    pub(super) fn state_dir(path: &Path) -> Result<Self, Why> {
        Self::open_dir(path, true)
    }
    fn open_dir(path: &Path, writable: bool) -> Result<Self, Why> {
        #[cfg(unix)]
        let _ = writable;
        #[cfg(windows)]
        let f = if writable {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
                FILE_SHARE_WRITE,
            };
            File::options()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
                .open(path)
        } else {
            windows_open(path)
        };
        #[cfg(unix)]
        let f = {
            use std::os::unix::fs::OpenOptionsExt;
            File::options()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
                .open(path)
        };
        let file = f.map_err(|_| Why::Io)?;
        observed(&file)?;
        if !file.metadata().map_err(|_| Why::Io)?.is_dir() {
            return Err(Why::FileType);
        }
        Ok(Self {
            file,
            #[cfg(windows)]
            path: path.to_owned(),
        })
    }
    pub(super) fn still_child(&self, name: &str, child: &Self) -> Result<bool, Why> {
        Ok(self.child(name)?.is_some_and(|current| {
            observed(&current.file)
                .and_then(|now| Ok(now.identity == observed(&child.file)?.identity))
                == Ok(true)
        }))
    }
    // 名字由上层先按固定 ASCII 词表核验，不能让带斜线的外部输入直接到达 openat/join。
    pub(super) fn node(&self, name: &str) -> Result<Option<(File, Stamp, bool)>, Why> {
        #[cfg(windows)]
        let opened = windows_open(&self.path.join(name));
        #[cfg(unix)]
        let opened = {
            use std::os::fd::{AsRawFd, FromRawFd};
            let c = std::ffi::CString::new(name).map_err(|_| Why::Path)?;
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
        let item = stamp(&f, &md)?;
        if item.identity.0 != observed(&self.file)?.identity.0 {
            return Err(Why::Link);
        }
        Ok(Some((f, item, md.is_dir())))
    }
    pub(super) fn child(&self, name: &str) -> Result<Option<Self>, Why> {
        match self.node(name)? {
            None => Ok(None),
            Some((file, _, true)) => Ok(Some(Self {
                file,
                #[cfg(windows)]
                path: self.path.join(name),
            })),
            _ => Err(Why::FileType),
        }
    }
    pub(super) fn file(&self, name: &str) -> Result<(File, Stamp), Why> {
        match self.node(name)? {
            Some((file, stamp, false)) => Ok((file, stamp)),
            Some(_) => Err(Why::Depth),
            None => Err(Why::Io),
        }
    }
    pub(super) fn names(&self, cap: usize, overflow: Why) -> Result<Vec<OsString>, Why> {
        let mut names = Vec::new();
        #[cfg(windows)]
        for entry in std::fs::read_dir(&self.path).map_err(|_| Why::Io)? {
            if names.len() == cap {
                return Err(overflow);
            }
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
                if names.len() == cap {
                    return Err(overflow);
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
