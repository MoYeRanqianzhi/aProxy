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
//!
//! 宣告属于一个 home：节名带 run 目录的标识（与控制端点同一套命名空间规则），
//! 一个 home 的 install 不会让别的 home 的看护者进入安装模式；unix 上也不再
//! 与别的用户的 /dev/shm 文件撞名。0.1.0 用的是全局名字，升级窗口内两边都要
//! 照顾到，见 compat_0_1_0 的 S5。

use std::path::Path;
use std::time::Duration;

/// Windows 命名节名 / unix 文件名（/dev/shm 下）。
pub fn announcement_name(run_dir: &Path) -> String {
    let home_id = crate::daemon::home_id(run_dir);
    #[cfg(windows)]
    {
        format!(r"Local\aproxy-{home_id}-install")
    }
    #[cfg(not(windows))]
    {
        format!("aproxy-{home_id}-install")
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
/// 句柄消失；unix 主动 unlink，在 Drop 里做）。持有本 home 的节，以及 0.1.0
/// 读的全局节（S5，尽力而为）。
pub struct Announcer {
    sections: Vec<Section>,
}

/// 一个宣告节
struct Section {
    #[cfg(windows)]
    mapping: isize,
    #[cfg(windows)]
    view: *mut std::ffi::c_void,
    #[cfg(not(windows))]
    file: std::sync::Mutex<std::fs::File>,
    #[cfg(not(windows))]
    path: String,
}

// Windows 视图指针跨线程（beat 是原子 store）；unix 无共享字段
#[cfg(windows)]
unsafe impl Send for Section {}
#[cfg(windows)]
unsafe impl Sync for Section {}

impl Announcer {
    /// 创建本 home 的宣告节并写入初始内容。失败只降级（看护者/守护按「无宣告」
    /// 处理，常态行为），绝不阻断安装。
    pub fn create(run_dir: &Path) -> Option<Self> {
        let ann = Announcement {
            installer_pid: std::process::id(),
            heartbeat_ms: crate::watchdog::now_millis(),
        };
        let mut sections = vec![imp::create_section(&announcement_name(run_dir), ann)?];
        sections.extend(imp::create_section(
            crate::compat_0_1_0::LEGACY_ANNOUNCEMENT,
            ann,
        ));
        Some(Self { sections })
    }

    /// 刷新心跳（install 侧独立 ticker 周期调用）。
    pub fn beat(&self) {
        let ms = crate::watchdog::now_millis();
        for section in &self.sections {
            imp::store_section(section, ms);
        }
    }
}

impl Drop for Announcer {
    fn drop(&mut self) {
        // Windows：unmap + close → 节消失；unix：unlink 持久文件（done/abort
        // 的主动解除路径；崩溃残留靠心跳过期自然失效）
        for section in &mut self.sections {
            imp::destroy_section(section);
        }
    }
}

/// 读本 home 的宣告。节不存在 → None（无安装宣告——常态）。本 home 没有
/// 宣告时再看 0.1.0 的全局节：升级窗口里驱动安装的可能是 0.1.0 的安装器。
pub fn read(run_dir: &Path) -> Option<Announcement> {
    read_own(run_dir).or_else(|| imp::load_section(crate::compat_0_1_0::LEGACY_ANNOUNCEMENT))
}

/// 只读本 home 的宣告，不看 0.1.0 的全局节。会据此动手的判断（拉起续作）用它：
/// 全局节可能是别的 home 的安装留下的残留。
pub fn read_own(run_dir: &Path) -> Option<Announcement> {
    imp::load_section(&announcement_name(run_dir))
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
    use super::{Announcement, Section};

    fn wide(name: &str) -> Vec<u16> {
        name.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn create_section(name: &str, ann: Announcement) -> Option<Section> {
        use windows_sys::Win32::System::Memory::{
            CreateFileMappingW, FILE_MAP_WRITE, MapViewOfFile, PAGE_READWRITE,
        };
        let name = wide(name);
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
            store_view(view.Value, ann.installer_pid, ann.heartbeat_ms);
            Some(Section {
                mapping,
                view: view.Value,
            })
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

    pub fn store_section(section: &Section, ms: u64) {
        unsafe {
            store_view(section.view, std::process::id(), ms);
        }
    }

    pub fn load_section(name: &str) -> Option<Announcement> {
        use windows_sys::Win32::System::Memory::{FILE_MAP_READ, MapViewOfFile, OpenFileMappingW};
        let name = wide(name);
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

    pub fn destroy_section(section: &mut Section) {
        let view = windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS {
            Value: section.view,
        };
        unsafe {
            let _ = windows_sys::Win32::System::Memory::UnmapViewOfFile(view);
            let _ = windows_sys::Win32::Foundation::CloseHandle(section.mapping);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{Announcement, Section};

    fn shm_path(name: &str) -> String {
        format!("/dev/shm/{name}")
    }

    pub fn create_section(name: &str, ann: Announcement) -> Option<Section> {
        let path = shm_path(name);
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .ok()?;
        write_content(&mut f, &ann);
        Some(Section {
            file: std::sync::Mutex::new(f),
            path,
        })
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

    pub fn store_section(section: &Section, ms: u64) {
        // 心跳写已打开的 fd（锁内短临界区；&Section 共享引用不可变借用）
        if let Ok(mut f) = section.file.lock() {
            write_content(
                &mut f,
                &Announcement {
                    installer_pid: std::process::id(),
                    heartbeat_ms: ms,
                },
            );
        }
    }

    pub fn load_section(name: &str) -> Option<Announcement> {
        let data = std::fs::read(shm_path(name)).ok()?;
        if data.len() < 16 {
            return None;
        }
        Some(Announcement {
            installer_pid: u32::from_le_bytes(data[0..4].try_into().ok()?),
            heartbeat_ms: u64::from_le_bytes(data[8..16].try_into().ok()?),
        })
    }

    pub fn destroy_section(section: &mut Section) {
        // unix 主动 unlink：done/abort 正常退出路径的宣告解除；崩溃残留由
        // 心跳过期自然失效（与 Windows「节消失」殊途同归）
        let _ = std::fs::remove_file(&section.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announce_roundtrip_and_active_judgement() {
        // create → beat → read：同进程内内容一致。只建本 home 的节——
        // `Announcer::create` 还会发布 0.1.0 的全局节，测试不该让机器上别的
        // home（包括正在用的 0.1.0 实例的看护者）看到一场并不存在的安装
        let run = tempfile::tempdir().unwrap();
        let section = imp::create_section(
            &announcement_name(run.path()),
            Announcement {
                installer_pid: std::process::id(),
                heartbeat_ms: crate::watchdog::now_millis(),
            },
        )
        .expect("宣告节创建失败");
        let a = Announcer {
            sections: vec![section],
        };
        a.beat();
        let ann = read(run.path()).expect("宣告节应可读");
        assert_eq!(ann.installer_pid, std::process::id());
        // 心跳新鲜 + pid 存活 = 有效
        assert!(is_active(&ann, crate::watchdog::now_millis()));
        // 心跳过期 = 无效（挂死/unix 残留的判定路径）——即使 pid 活着
        assert!(!is_active(&ann, ann.heartbeat_ms + FRESH_MS + 1));
        drop(a);
        // Windows：进程内节句柄关闭 → 节消失（读不到）；unix：Drop unlink
        // （测试进程未崩，主动解除路径生效）
        assert!(
            imp::load_section(&announcement_name(run.path())).is_none(),
            "Drop 后宣告应解除（Windows 节消失 / unix unlink）"
        );
    }

    #[test]
    fn announcement_belongs_to_its_home() {
        // 另一个 home 的 install 不得让本 home 的看护者进入安装模式
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        assert_ne!(announcement_name(a.path()), announcement_name(b.path()));
        let section = imp::create_section(
            &announcement_name(a.path()),
            Announcement {
                installer_pid: std::process::id(),
                heartbeat_ms: crate::watchdog::now_millis(),
            },
        )
        .expect("宣告节创建失败");
        assert!(imp::load_section(&announcement_name(b.path())).is_none());
        let mut section = section;
        imp::destroy_section(&mut section);
    }
}
