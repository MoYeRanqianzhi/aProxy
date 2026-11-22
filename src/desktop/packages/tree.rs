//! 对目录的读取相对于已验证的父句柄；不能让检查和打开之间替换的链接改变实际读到的文件。
use std::{ffi::OsString, fs::File, io, path::Path};
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
pub(super) enum Node {
    Dir(Dir),
    File(File),
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
    pub(super) fn open(path: &Path) -> Result<Self, Why> {
        #[cfg(windows)]
        let f = windows_open(path);
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
    // 名字由上层先按单个路径分量核验，不能让带斜线的外部输入直接到达 openat/join。
    pub(super) fn node(&self, name: &str) -> Result<Option<Node>, Why> {
        #[cfg(windows)]
        let opened = {
            let _keep_open = &self.file;
            windows_open(&self.path.join(name))
        };
        #[cfg(unix)]
        let opened = {
            use std::os::fd::{AsRawFd, FromRawFd};
            let c = std::ffi::CString::new(name).map_err(|_| Why::FileType)?;
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
