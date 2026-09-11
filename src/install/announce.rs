//! 安装态宣告：install 进程「我在安装」的显式宣告（易失介质）。
//!
//! 双介质分离（用户定调的分工原则落点）：
//! - **持久介质 = install.state**：install 自己的进度账本，只有 install 读。
//!   残留 ≠ 在安装——看护者/守护不做语义解读。
//! - **易失介质 = 本模块的宣告节**：install 启动时创建并持有，内容
//!   `{installer_pid, 心跳毫秒}`（独立 ticker 周期 beat）。**install 进程
//!   退出（done/abort/崩溃）节即消失 = 宣告自然解除**，零清理逻辑。
//!   这是运行时「安装进行中」的唯一真相源。
//!
//! 看护者/守护读节判定：节存在 + 心跳新鲜 + pid 存活 = 宣告有效。挂死与
//! unix 残留区分**正靠心跳过期**（Windows 节随进程死消失、只有挂死路径；
//! unix /dev/shm 持久文件崩溃残留也节在心跳旧）——同一判定路径，介质差异
//! 不影响语义。

use std::time::Duration;

/// Windows 命名节名 / unix 文件名（/dev/shm 下）。
pub fn announcement_name() -> &'static str {
    #[cfg(windows)]
    {
        r"Local\aproxy-install"
    }
    #[cfg(not(windows))]
    {
        "aproxy-install"
    }
}

/// 宣告心跳 beat 间隔。远小于新鲜阈值，任何一次扫描都能读到新鲜值。
pub const BEAT_INTERVAL: Duration = Duration::from_secs(1);

/// 心跳新鲜阈值：超时 = install 挂死（runtime 无法调度）或 unix 崩溃残留。
pub const FRESH_MS: u64 = 10_000;

/// 宣告内容：installer_pid + 心跳毫秒（16 字节节）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Announcement {
    pub installer_pid: u32,
    pub heartbeat_ms: u64,
}

/// install 侧宣告句柄：创建即宣告，Drop/进程退出即解除（Windows 节随最后
/// 句柄消失；unix 主动 unlink——见 [`remove`]，Drop 里做）。
pub struct Announcer {
    #[cfg(windows)]
    mapping: isize,
    #[cfg(windows)]
    view: *mut std::ffi::c_void,
    #[cfg(not(windows))]
    _keepalive: std::fs::File,
}

// Windows 视图指针跨线程（beat 是原子 store）；unix 无共享字段
#[cfg(windows)]
unsafe impl Send for Announcer {}
#[cfg(windows)]
unsafe impl Sync for Announcer {}

impl Announcer {
    /// 创建宣告节并写入初始内容。失败只降级（看护者/守护按「无宣告」处理，
    /// 常态行为），绝不阻断安装。
    pub fn create() -> Option<Self> {
        imp::create_announcement(Announcement {
            installer_pid: std::process::id(),
            heartbeat_ms: crate::watchdog::now_millis(),
        })
    }

    /// 刷新心跳（install 侧独立 ticker 周期调用）。
    pub fn beat(&self) {
        imp::store_announcement(self, crate::watchdog::now_millis());
    }
}

impl Drop for Announcer {
    fn drop(&mut self) {
        // Windows：unmap + close → 节消失；unix：unlink 持久文件（done/abort
        // 的主动解除路径；崩溃残留靠心跳过期自然失效）
        imp::destroy_announcement(self);
    }
}

/// 读宣告节。节不存在 → None（无安装宣告——常态）。
pub fn read() -> Option<Announcement> {
    imp::load_announcement()
}

/// 宣告有效性判定（看护者/守护的唯一入口）：节存在 + 心跳新鲜 + pid 存活。
/// 心跳旧 = install 挂死/unix 残留 → 无效（走保活续作路径，不是「在安装」）。
pub fn is_active(ann: &Announcement, now_ms: u64) -> bool {
    now_ms.saturating_sub(ann.heartbeat_ms) <= FRESH_MS
        && crate::watchdog::process_start_time(ann.installer_pid).is_some()
}

// ---------------------------------------------------------------------------
// 平台实现：Windows 命名节 / unix /dev/shm 文件
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod imp {
    use super::Announcement;

    fn wide_name() -> Vec<u16> {
        super::announcement_name()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    }

    pub fn create_announcement(ann: Announcement) -> Option<super::Announcer> {
        use windows_sys::Win32::System::Memory::{
            CreateFileMappingW, FILE_MAP_WRITE, MapViewOfFile, PAGE_READWRITE,
        };
        let name = wide_name();
        unsafe {
            // 16 字节节：pid(u32) + 心跳(u64)。页面文件支撑，随最后句柄消失
            let mapping = CreateFileMappingW(
                windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE,
                0,
                16,
                name.as_ptr(),
            );
            if mapping == 0 {
                return None;
            }
            let view = MapViewOfFile(mapping, FILE_MAP_WRITE, 0, 0, 16);
            if view.Value.is_null() {
                let _ = windows_sys::Win32::Foundation::CloseHandle(mapping);
                return None;
            }
            let a = super::Announcer {
                mapping,
                view: view.Value,
            };
            store_view(view.Value, ann.installer_pid, ann.heartbeat_ms);
            Some(a)
        }
    }

    unsafe fn store_view(view: *mut std::ffi::c_void, pid: u32, ms: u64) {
        // edition 2024：unsafe fn 体内的 unsafe 操作须显式块
        unsafe {
            let base = view as *mut u8;
            std::ptr::copy_nonoverlapping(pid.to_le_bytes().as_ptr(), base, 4);
            std::ptr::copy_nonoverlapping(ms.to_le_bytes().as_ptr(), base.add(8), 8);
        }
    }

    pub fn store_announcement(a: &super::Announcer, ms: u64) {
        unsafe {
            store_view(a.view, std::process::id(), ms);
        }
    }

    pub fn load_announcement() -> Option<Announcement> {
        use windows_sys::Win32::System::Memory::{FILE_MAP_READ, MapViewOfFile, OpenFileMappingW};
        let name = wide_name();
        unsafe {
            let mapping = OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr());
            if mapping == 0 {
                return None;
            }
            let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 16);
            let _ = windows_sys::Win32::Foundation::CloseHandle(mapping);
            if view.Value.is_null() {
                return None;
            }
            let (pid_b, ms_b) = {
                let base = view.Value as *const u8;
                let mut pid_b = [0u8; 4];
                let mut ms_b = [0u8; 8];
                std::ptr::copy_nonoverlapping(base, pid_b.as_mut_ptr(), 4);
                std::ptr::copy_nonoverlapping(base.add(8), ms_b.as_mut_ptr(), 8);
                (pid_b, ms_b)
            };
            let _ = windows_sys::Win32::System::Memory::UnmapViewOfFile(view);
            Some(Announcement {
                installer_pid: u32::from_le_bytes(pid_b),
                heartbeat_ms: u64::from_le_bytes(ms_b),
            })
        }
    }

    pub fn destroy_announcement(a: &mut super::Announcer) {
        let view = windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS { Value: a.view };
        unsafe {
            let _ = windows_sys::Win32::System::Memory::UnmapViewOfFile(view);
            let _ = windows_sys::Win32::Foundation::CloseHandle(a.mapping);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Announcement;

    fn shm_path() -> String {
        format!("/dev/shm/{}", super::announcement_name())
    }

    pub fn create_announcement(ann: Announcement) -> Option<super::Announcer> {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(shm_path())
            .ok()?;
        write_content(&mut f, &ann);
        Some(super::Announcer { _keepalive: f })
    }

    fn write_content(f: &mut std::fs::File, ann: &Announcement) {
        use std::io::{Seek, SeekFrom, Write};
        let _ = f.seek(SeekFrom::Start(0));
        let mut buf = Vec::with_capacity(16);
        buf.extend_from_slice(&ann.installer_pid.to_le_bytes());
        buf.extend_from_slice(&[0u8; 4]);
        buf.extend_from_slice(&ann.heartbeat_ms.to_le_bytes());
        let _ = f.write_all(&buf);
    }

    pub fn store_announcement(a: &super::Announcer, ms: u64) {
        write_content(
            &mut &a._keepalive,
            &Announcement {
                installer_pid: std::process::id(),
                heartbeat_ms: ms,
            },
        );
    }

    pub fn load_announcement() -> Option<Announcement> {
        let data = std::fs::read(shm_path()).ok()?;
        if data.len() < 16 {
            return None;
        }
        Some(Announcement {
            installer_pid: u32::from_le_bytes(data[0..4].try_into().ok()?),
            heartbeat_ms: u64::from_le_bytes(data[8..16].try_into().ok()?),
        })
    }

    pub fn destroy_announcement(_a: &mut super::Announcer) {
        // unix 主动 unlink：done/abort 正常退出路径的宣告解除；崩溃残留由
        // 心跳过期自然失效（与 Windows「节消失」殊途同归）
        let _ = std::fs::remove_file(shm_path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announce_roundtrip_and_active_judgement() {
        // create → beat → read：同进程内内容一致
        let Some(a) = Announcer::create() else {
            panic!("宣告节创建失败");
        };
        a.beat();
        let ann = read().expect("宣告节应可读");
        assert_eq!(ann.installer_pid, std::process::id());
        // 心跳新鲜 + pid 存活 = 有效
        assert!(is_active(&ann, crate::watchdog::now_millis()));
        // 心跳过期 = 无效（挂死/unix 残留的判定路径）——即使 pid 活着
        assert!(!is_active(&ann, ann.heartbeat_ms + FRESH_MS + 1));
        drop(a);
        // Windows：进程内节句柄关闭 → 节消失（读不到）；unix：Drop unlink
        // （测试进程未崩，主动解除路径生效）
        assert!(
            read().is_none(),
            "Drop 后宣告应解除（Windows 节消失 / unix unlink）"
        );
    }
}
