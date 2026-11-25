//! 收编完成后的用户环境集成。两个动作分别记录实际写入，后一处失败不撤销前一处，更不回滚二进制。
use super::{events, terminal_lang, text};
#[cfg(feature = "desktop")]
#[path = "../../i18n/install_shortcut.rs"]
mod menu;
use serde::Serialize;
use std::{io, path::Path};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

/// 结果属于发起收编的进程，不写入跨版本的交接文件。
#[derive(Serialize)]
pub(super) struct Steps {
    path: Step,
    shortcut: Step,
}

#[derive(Serialize)]
struct Step {
    status: &'static str,
    reason: String,
    touched: Vec<String>,
}

impl Step {
    fn skipped(reason: &str) -> Self {
        Self {
            status: "skipped",
            reason: reason.into(),
            touched: Vec::new(),
        }
    }

    fn run(action: impl FnOnce(&mut Self) -> io::Result<()>) -> Self {
        let mut step = Self::skipped("");
        match action(&mut step) {
            Ok(()) if step.touched.is_empty() => {
                step.status = "unchanged";
                step.reason = "already configured".into();
            }
            Ok(()) => {
                step.status = "changed";
                step.reason = "updated user environment".into();
            }
            Err(e) => {
                step.status = "failed";
                step.reason = e.to_string();
            }
        }
        step
    }

    /// 只更新内容不同的自管文件；原子替换，不截断可能正在被读取的入口。
    #[cfg(unix)]
    fn write(&mut self, file: &Path, bytes: &[u8]) -> io::Result<()> {
        match std::fs::read(file) {
            Ok(old) if old == bytes => return Ok(()),
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => (),
        }
        aproxy::fsx::write_atomic(file, bytes)?;
        self.touched.push(file.display().to_string());
        Ok(())
    }
}

#[cfg(feature = "desktop")]
fn suffix(home: &Path, profile: &Path) -> String {
    use sha2::{Digest, Sha256};
    if home == profile.join(".aproxy") {
        return String::new();
    }
    // 目录名可能相同，也可能含有文件名不准用的字符。摘要在不同进程间稳定。
    let digest = Sha256::digest(home.as_os_str().as_encoded_bytes());
    format!(
        "-{:016x}",
        u64::from_be_bytes(digest[..8].try_into().unwrap())
    )
}

/// PATH 与入口独立运行。headless 的机器结果仍说明为何跳过，但不向终端提窗口。
pub(super) fn apply(home: &Path, no_path: bool, no_shortcut: bool) -> Steps {
    #[cfg(unix)]
    use unix as platform;
    #[cfg(windows)]
    use windows as platform;
    let lang = terminal_lang();
    let bin = home.join("bin");
    let path = if no_path {
        Step::skipped("--no-path")
    } else {
        Step::run(|s| platform::path(s, home))
    };
    let bin = bin.display();
    match path.status {
        "changed" => events::say(text::path_changed(lang, &path.touched.join(", "))),
        "unchanged" => events::say(text::path_present(lang, bin)),
        "failed" => eprintln!("{}", text::path_failed(lang, &path.reason, bin)),
        _ => events::say(text::path_skipped(lang)),
    }
    #[cfg(feature = "desktop")]
    let shortcut = if no_shortcut {
        events::say(menu::shortcut_skipped(lang));
        Step::skipped("--no-shortcut")
    } else {
        let step = Step::run(|s| {
            let profile = aproxy_ipc::home::user_dir()
                .filter(|p| p.is_absolute())
                .ok_or(io::ErrorKind::NotFound)?;
            platform::shortcut(s, home, &profile)
        });
        match step.status {
            "changed" => events::say(menu::shortcut_changed(lang, &step.touched.join(", "))),
            "unchanged" => events::say(menu::shortcut_present(lang)),
            _ => eprintln!(
                "{}",
                menu::shortcut_failed(
                    lang,
                    &step.reason,
                    aproxy::install::swap::bin_path_in(home).display(),
                    home.display()
                )
            ),
        }
        step
    };
    #[cfg(not(feature = "desktop"))]
    let shortcut = {
        let _ = no_shortcut;
        Step::skipped("headless build")
    };
    Steps { path, shortcut }
}
