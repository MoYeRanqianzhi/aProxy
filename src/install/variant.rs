//! 选变体：带桌面界面的 desktop 还是无界面的 headless（`--version` 的变体段，永久兼容契约 C5，
//! 取值见 force.rs 的 [`VARIANT_DESKTOP`](super::force::VARIANT_DESKTOP) 与
//! [`VARIANT_HEADLESS`](super::force::VARIANT_HEADLESS)）。
//!
//! Windows 与 macOS 上带界面的版本随处能跑（缺 WebView2 只影响 `aproxy desktop` 本身）。Linux 上
//! 不行：带界面的 gnu 版本动态链接 GTK 与 WebKitGTK，缺库或库太旧时动态加载器在 main 之前就失败，
//! 连 `--version` 与代理都起不来；musl 目标只发布 headless（带界面的 musl 版本只能动态链接、只能
//! 跑在装了 WebKitGTK 的 musl 发行版上，失去 musl 版的意义，用户 2026-10-12 定）。所以在线安装
//! 装带界面的版本之前先看本机够不够（[`desktop_blocker`]）：不够时，用户没写 `--variant` 就改装
//! headless 并说明原因与之后怎样换回，写了 `--variant desktop` 就报错。一键安装脚本
//! scripts/install.sh 与 npm 启动器用的是同样的两个下限。

/// 带界面的 Linux gnu 产物能运行的最低 glibc（主、次版本）。它在 ubuntu:22.04 容器里链接系统的
/// GTK 与 WebKitGTK，下限由构建系统决定，发布时断言产物不需要更高的版本（release.yml 的
/// DESKTOP_GLIBC_FLOOR）；scripts/install.sh 的 DESKTOP_MIN_GLIBC 与 npm 启动器的
/// DESKTOP_MIN_GLIBC 必须是同一个数。
pub const DESKTOP_GLIBC_FLOOR: (u32, u32) = (2, 35);

/// 带界面的版本要的最低 WebKitGTK（4.1 API）：tauri 打开了 webkit2gtk 的 v2_40 特性，Rust 在
/// Linux 上默认完整 RELRO，启动时就要解析到这些符号，库太旧同样起不来。
pub const WEBKIT_FLOOR: (u32, u32) = (2, 40);

/// 带界面的版本在本机跑不起来的原因（给人看）；能跑、或者不需要看的平台为 None。`target` 是要装
/// 的产物的目标三元组，也就是正在运行的这个二进制的：musl 目标没有带界面的版本；gnu 目标要本机
/// 的 glibc 与 WebKitGTK 都不低于下限。
///
/// 判断不了的情形（读不出 glibc 版本）按能跑处理：下载之后还要在本机执行 `--version` 核对，
/// 真跑不起来的会在那里挡住。
pub fn desktop_blocker(target: &str) -> Option<String> {
    if target.ends_with("-linux-musl") {
        return Some("musl 目标只发布无界面的 headless 版本".to_string());
    }
    if !target.ends_with("-linux-gnu") {
        return None;
    }
    linux_gnu_blocker()
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn linux_gnu_blocker() -> Option<String> {
    // SAFETY: gnu_get_libc_version 返回指向 glibc 内静态字符串的指针，进程存活期间一直有效
    let glibc = unsafe { std::ffi::CStr::from_ptr(libc::gnu_get_libc_version()) }
        .to_string_lossy()
        .into_owned();
    if let Some(found) = major_minor(&glibc)
        && found < DESKTOP_GLIBC_FLOOR
    {
        return Some(format!(
            "本机 glibc {glibc} 低于带界面版本要求的 {}.{}",
            DESKTOP_GLIBC_FLOOR.0, DESKTOP_GLIBC_FLOOR.1
        ));
    }
    match webkit_version() {
        Err(e) => Some(format!(
            "没有可用的 WebKitGTK 4.1（{e}）；Debian / Ubuntu 上它在 libwebkit2gtk-4.1-0 包里"
        )),
        Ok(found) if found < WEBKIT_FLOOR => Some(format!(
            "本机 WebKitGTK {}.{} 低于要求的 {}.{}",
            found.0, found.1, WEBKIT_FLOOR.0, WEBKIT_FLOOR.1
        )),
        Ok(_) => None,
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn linux_gnu_blocker() -> Option<String> {
    None
}

/// 本机 WebKitGTK（4.1 API）的主、次版本：按动态加载器的搜索规则加载 libwebkit2gtk-4.1.so.0，
/// 调它的 webkit_get_major_version / webkit_get_minor_version。两个函数只返回编译期常量，不初始化
/// GTK、不建任何对象。库装进来就留在进程里（GLib 一族本来也卸不掉）；安装器做完就退出。
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn webkit_version() -> Result<(u32, u32), String> {
    let dlerror = || {
        // SAFETY: dlerror 返回 NULL 或指向线程局部错误文本的指针，下一次 dl* 调用之前有效
        let text = unsafe { libc::dlerror() };
        if text.is_null() {
            "原因未知".to_string()
        } else {
            unsafe { std::ffi::CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned()
        }
    };
    // SAFETY: 参数是以 NUL 结尾的字面量；加载失败返回 NULL，下面检查
    let handle = unsafe {
        libc::dlopen(
            c"libwebkit2gtk-4.1.so.0".as_ptr(),
            libc::RTLD_LAZY | libc::RTLD_LOCAL,
        )
    };
    if handle.is_null() {
        return Err(dlerror());
    }
    let version_fn = |name: &std::ffi::CStr| -> Result<extern "C" fn() -> u32, String> {
        // SAFETY: handle 有效；找不到符号返回 NULL，下面检查
        let symbol = unsafe { libc::dlsym(handle, name.as_ptr()) };
        if symbol.is_null() {
            return Err(dlerror());
        }
        // SAFETY: 两个函数在 WebKitGTK 的公开头文件里都声明为 `guint f (void)`，guint 即 u32
        Ok(unsafe { std::mem::transmute::<*mut libc::c_void, extern "C" fn() -> u32>(symbol) })
    };
    let major = version_fn(c"webkit_get_major_version")?;
    let minor = version_fn(c"webkit_get_minor_version")?;
    Ok((major(), minor()))
}

/// `2.35`、`2.39.0` 这类版本号的主、次版本；读不出两段数字为 None。
#[cfg(any(test, all(target_os = "linux", target_env = "gnu")))]
fn major_minor(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts
        .next()?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()?;
    Some((major, minor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn major_minor_reads_glibc_style_versions() {
        assert_eq!(major_minor("2.35"), Some((2, 35)));
        assert_eq!(major_minor("2.39.0"), Some((2, 39)));
        assert_eq!(major_minor(" 2.28\n"), Some((2, 28)));
        assert_eq!(major_minor("2.36-9+deb12u14"), Some((2, 36)));
        assert_eq!(major_minor("2.40rc1"), Some((2, 40)));
        assert_eq!(major_minor("2"), None);
        assert_eq!(major_minor("x.y"), None);
        // 比较按数字而不是按字符串：2.4 < 2.35 < 2.40
        assert!(major_minor("2.4").unwrap() < (2, 35));
    }

    #[test]
    fn only_linux_targets_are_checked() {
        for target in [
            "x86_64-pc-windows-msvc",
            "i686-pc-windows-msvc",
            "aarch64-pc-windows-msvc",
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
        ] {
            assert_eq!(desktop_blocker(target), None, "{target}");
        }
        for target in ["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"] {
            assert!(desktop_blocker(target).is_some(), "{target}");
        }
    }
}
