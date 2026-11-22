//! 终端文案在显示处选择语言；写日志、文件或协议的代码明确传 English，不受启动者影响。

use std::{ffi::OsStr, path::Path, sync::OnceLock};

/// 由最终读者决定的文字语言；不序列化，也不跟随工作进程一起传递。
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Lang {
    /// 英文，同时也是程序间交换文字时使用的语言。
    En,
    /// 简体中文终端文字，适用于以 zh 开头的语言标签。
    Zh,
}

static TERMINAL: OnceLock<Lang> = OnceLock::new();

/// 只供写本进程终端的代码调用。未初始化的库默认英文，但查询本身不占用初始化机会。
pub fn current() -> Lang {
    TERMINAL.get().copied().unwrap_or(Lang::En)
}

/// 在构造帮助树前发布终端语言。闭包只在确实需要设置时求值，强制安装可完全绕过 home。
pub fn init_terminal(settings_home: impl FnOnce() -> Option<std::path::PathBuf>) {
    TERMINAL.get_or_init(|| {
        resolve(
            std::env::var_os("APROXY_LANG").as_deref(),
            || settings_language(&settings_home()?),
            system_language,
        )
    });
}

fn tag(value: &str) -> Lang {
    if value
        .as_bytes()
        .get(..2)
        .is_some_and(|p| p.eq_ignore_ascii_case(b"zh"))
    {
        Lang::Zh
    } else {
        Lang::En
    }
}

fn resolve(
    env: Option<&OsStr>,
    settings: impl FnOnce() -> Option<Lang>,
    system: impl FnOnce() -> Option<Lang>,
) -> Lang {
    env.and_then(OsStr::to_str)
        .filter(|s| !s.trim().is_empty() && *s != "system")
        .map(tag)
        .or_else(settings)
        .or_else(system)
        .unwrap_or(Lang::En)
}

fn settings_language(home: &Path) -> Option<Lang> {
    // 这里只读取语言，不校验或修复其他设置；普通校验还要用选好的语言报告问题。
    #[derive(serde::Deserialize)]
    struct Preference {
        language: String,
    }
    let bytes = std::fs::read(home.join("settings.json")).ok()?;
    match serde_json::from_slice::<Preference>(&bytes)
        .ok()?
        .language
        .as_str()
    {
        "en" => Some(Lang::En),
        "zh-CN" => Some(Lang::Zh),
        _ => None,
    }
}

#[cfg(any(test, unix))]
fn unix_preference(mut read: impl FnMut(&str) -> Option<std::ffi::OsString>) -> Option<Lang> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|key| {
            let value = read(key)?;
            let text = value.to_str()?;
            (!text.trim().is_empty()).then(|| tag(text))
        })
}

#[cfg(unix)]
fn system_language() -> Option<Lang> {
    unix_preference(|k| std::env::var_os(k))
}

#[cfg(any(test, windows))]
fn first_preference(names: &[u16]) -> Option<Lang> {
    let end = names.iter().position(|unit| *unit == 0)?;
    (end > 0)
        .then(|| String::from_utf16(&names[..end]).ok())
        .flatten()
        .map(|s| tag(&s))
}

#[cfg(windows)]
fn system_language() -> Option<Lang> {
    use windows_sys::Win32::{
        Foundation::{ERROR_INSUFFICIENT_BUFFER, GetLastError},
        Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME},
    };
    for _ in 0..2 {
        let (mut count, mut len) = (0, 0);
        // 长度单位为 UTF-16 码元，包含分隔以及列表末尾的 NUL；两次调用之间用户可改变首选项。
        if unsafe {
            GetUserPreferredUILanguages(
                MUI_LANGUAGE_NAME,
                &mut count,
                std::ptr::null_mut(),
                &mut len,
            )
        } == 0
            || len == 0
        {
            return None;
        }
        let mut buf = Vec::new();
        buf.try_reserve_exact(len as usize).ok()?;
        buf.resize(len as usize, 0);
        if unsafe {
            GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, buf.as_mut_ptr(), &mut len)
        } != 0
        {
            return (count > 0)
                .then(|| first_preference(buf.get(..len as usize)?))
                .flatten();
        }
        if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
            break;
        }
    }
    None
}

#[cfg(not(any(windows, unix)))]
fn system_language() -> Option<Lang> {
    None
}

/// 定义一条带双语文字和参数类型的消息。每个格式串显式绑定相同参数，不能偷读外围变量。
/// 不带参数的消息返回静态切片，可直接用于 clap；带参数的消息仅格式化被选中的一种语言。
///
/// ```compile_fail
/// aproxy_ipc::message!(missing(), en: "hello");
/// ```
/// ```compile_fail
/// aproxy_ipc::message!(unused(port: u16), en: "port {port}", zh: "端口");
/// ```
/// ```compile_fail
/// aproxy_ipc::message!(extra(port: u16), en: "port {port} {other}", zh: "端口 {port}");
/// ```
/// ```compile_fail
/// aproxy_ipc::message!(count(n: usize), en_one: "one", en_other: "{n} items", zh: "{n} 项");
/// ```
#[macro_export]
macro_rules! message {
    ($id:ident (), en: $en:literal, zh: $zh:literal $(,)?) => {
        #[doc = $en]
        pub(crate) fn $id(lang: $crate::i18n::Lang) -> &'static str {
            match lang {
                $crate::i18n::Lang::En => const { format_args!(concat!($en)).as_str().unwrap() },
                $crate::i18n::Lang::Zh => const { format_args!(concat!($zh)).as_str().unwrap() },
            }
        }
    };
    ($id:ident ($($p:ident : $t:ty),+), en: $en:literal, zh: $zh:literal $(,)?) => {
        #[doc = $en]
        pub(crate) fn $id(lang: $crate::i18n::Lang, $($p: $t),+) -> String {
            match lang {
                $crate::i18n::Lang::En => format!(concat!($en), $($p = $p),+),
                $crate::i18n::Lang::Zh => format!(concat!($zh), $($p = $p),+),
            }
        }
    };
    ($id:ident ($n:ident : usize), en_one: $one:literal, en_other: $many:literal, zh: $zh:literal $(,)?) => {
        #[doc = $many]
        pub(crate) fn $id(lang: $crate::i18n::Lang, $n: usize) -> String {
            match (lang, $n) {
                ($crate::i18n::Lang::En, 1) => format!(concat!($one), $n = $n),
                ($crate::i18n::Lang::En, _) => format!(concat!($many), $n = $n),
                ($crate::i18n::Lang::Zh, _) => format!(concat!($zh), $n = $n),
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn higher_priority_sources_short_circuit() {
        for (input, expected) in [("ZH_hant", Lang::Zh), ("C", Lang::En), ("other", Lang::En)] {
            assert_eq!(
                resolve(Some(input.as_ref()), || panic!(), || panic!()),
                expected
            );
        }
        for env in [None, Some(OsStr::new("")), Some(OsStr::new("system"))] {
            assert_eq!(resolve(env, || Some(Lang::Zh), || panic!()), Lang::Zh);
            assert_eq!(resolve(env, || None, || Some(Lang::Zh)), Lang::Zh);
            assert_eq!(resolve(env, || None, || None), Lang::En);
        }
    }

    #[test]
    fn non_unicode_override_falls_through() {
        #[cfg(windows)]
        let invalid =
            <std::ffi::OsString as std::os::windows::ffi::OsStringExt>::from_wide(&[0xd800]);
        #[cfg(unix)]
        let invalid = <std::ffi::OsString as std::os::unix::ffi::OsStringExt>::from_vec(vec![0xff]);
        #[cfg(any(windows, unix))]
        assert_eq!(
            resolve(Some(&invalid), || Some(Lang::Zh), || panic!()),
            Lang::Zh
        );
    }

    #[test]
    fn only_a_recognized_preference_in_valid_json_counts() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(settings_language(home.path()), None);
        for (json, expected) in [
            ("{}", None),
            ("{", None),
            (r#"{"language":null}"#, None),
            (r#"{"language":"system"}"#, None),
            (r#"{"language":"fr"}"#, None),
            (r#"{"language":"zh-CN","bad":0}"#, Some(Lang::Zh)),
            (r#"{"language":"en"}"#, Some(Lang::En)),
        ] {
            std::fs::write(home.path().join("settings.json"), json).unwrap();
            assert_eq!(settings_language(home.path()), expected);
        }
    }

    #[test]
    fn system_precedence() {
        let locales = |all: &str, messages: &str, lang: &str| {
            unix_preference(|key| {
                Some(
                    match key {
                        "LC_ALL" => all,
                        "LC_MESSAGES" => messages,
                        "LANG" => lang,
                        _ => unreachable!(),
                    }
                    .into(),
                )
            })
        };
        assert_eq!(locales("C", "zh", "zh"), Some(Lang::En));
        assert_eq!(locales("", "zh_TW.UTF-8", "POSIX"), Some(Lang::Zh));
        assert_eq!(locales("", "", "POSIX"), Some(Lang::En));
        assert_eq!(locales("", "", ""), None);
        for (names, expected) in [
            ("en-US\0zh-CN\0\0", Some(Lang::En)),
            ("zh-Hant\0en-US\0\0", Some(Lang::Zh)),
            ("\0\0", None),
            ("en-US", None),
        ] {
            assert_eq!(
                first_preference(&names.encode_utf16().collect::<Vec<_>>()),
                expected
            );
        }
        assert_eq!(first_preference(&[0xd800, 0, 0]), None);
    }

    #[test]
    fn render_forms_and_parameters() {
        message!(empty(), en: "No instances to restore.", zh: "没有需要恢复的实例。");
        message!(running(port: &str), en: "Port {port} is already running; skipping.", zh: "端口 {port} 已在运行，跳过。");
        message!(count(n: usize), en_one: "Found {n} configuration:", en_other: "Found {n} configurations:", zh: "找到 {n} 个配置:");
        assert_eq!(empty(Lang::En), "No instances to restore.");
        assert_eq!(running(Lang::Zh, "测试"), "端口 测试 已在运行，跳过。");
        assert_eq!(count(Lang::En, 1), "Found 1 configuration:");
        assert_eq!(count(Lang::En, 0), "Found 0 configurations:");
        assert_eq!(count(Lang::Zh, 2), "找到 2 个配置:");
    }
}
