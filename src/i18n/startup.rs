//! 父进程在自己的交接文件里读取启动故障；日志本身始终是英文，旁观其他实例时
//! 不读启动规格。这里仅保留排版需要的数据，不携带启动参数、环境或配置内容。

use aproxy_ipc::{
    diag::Diagnostic,
    i18n::{Lang, diagnostic::Reason},
    message,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

message!(spec_read(path: &str, cause: &str), en: "cannot read the launch spec {path}: {cause}", zh: "无法读取启动规格 {path}：{cause}");
message!(missing(path: &str), en: "Configuration file not found: {path}", zh: "配置文件不存在: {path}");
message!(bad_settings(), en: "settings.json contains errors. The instance was not started:", zh: "settings.json 有错误，实例没有启动:");
message!(settings_hint(path: &str), en: "\n\nRun aproxy settings show to inspect the settings, or aproxy settings unset <key> to remove an invalid key. Delete {path} to return to the built-in defaults.", zh: "\n\n用 aproxy settings show 查看，aproxy settings unset <键> 删掉出问题的键；也可以删除 {path} 回到内置默认");
message!(read(path: &str, error: &str), en: "Cannot read the configuration file: {error}\nFile: {path}", zh: "读取配置文件失败: {error}\n文件: {path}");
message!(bad_config(), en: "Configuration errors:", zh: "配置错误:");
message!(config_hint(), en: "\n\nCorrect the configuration and try again. To set the upstream address:\n  aproxy config set base_url https://api.anthropic.com", zh: "\n\n请修改后重试。上游地址可以这样设置:\n  aproxy config set base_url https://api.anthropic.com");
message!(edit(path: &str), en: "\n  Or edit {path} manually", zh: "\n  或手动编辑 {path}");
message!(in_use(addr: &str), en: "Another program is using {addr}. The instance cannot start.", zh: "端口 {addr} 被其他程序占用，无法启动");
message!(denied(addr: &str), en: "Cannot bind {addr}: permission denied or the port is reserved. On Windows, check Hyper-V/WinNAT exclusions with netsh interface ipv4 show excludedportrange protocol=tcp.", zh: "端口 {addr} 无法绑定：无权限或端口被系统保留（如 Hyper-V/WinNAT 排除区间，可用 netsh interface ipv4 show excludedportrange protocol=tcp 查看）");
message!(bind(addr: &str, error: &str), en: "Cannot listen on {addr}: {error}", zh: "无法监听 {addr}: {error}");
message!(control(endpoint: &str, port: &str, error: &str), en: "Cannot create control channel {endpoint}: {error}. Another aProxy instance in this APROXY_HOME may be using port {port} on a different address. On Unix, check the length of the run directory path.", zh: "控制通道 {endpoint} 创建失败：{error}。同一 APROXY_HOME 里可能已有使用端口 {port} 的 aProxy 实例（监听地址不同）；unix 上 run 目录路径过长也会导致失败");
message!(exited(pid: u32), en: "The new instance (pid {pid}) exited immediately after startup", zh: "新实例（pid {pid}）启动后立即退出");
message!(spawn(error: &str), en: "Cannot start the process: {error}", zh: "spawn 失败: {error}");
message!(timeout(pid: u32, state: &str), en: "The new instance (pid {pid}) did not become ready in time ({state})", zh: "新实例（pid {pid}）未在预期时间内就绪（{state}）");
message!(alive(), en: "process is running", zh: "进程存活");
message!(unknown(), en: "process state unknown", zh: "进程状态不可判");
message!(look(path: std::path::Display<'_>, dir: std::path::Display<'_>), en: ". Check the logs for the cause:\n  {path}\n  {dir}", zh: "，原因通常记录在:\n  {path}\n  {dir}");
message!(excerpt(path: std::path::Display<'_>), en: ". New lines in {path}:", zh: "。{path} 本次新增内容:");

/// 启动时的一个解析错误；码与位置来自解析器，说明由读取交接文件的进程排版。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Issue {
    code: String,
    file: String,
    key: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
    reason: Reason,
}
impl From<&Diagnostic> for Issue {
    fn from(d: &Diagnostic) -> Self {
        Self {
            code: d.code.to_owned(),
            file: d.file.clone(),
            key: d.key.clone(),
            line: d.line,
            column: d.column,
            reason: d.message.clone(),
        }
    }
}

/// 只有本次启动的私有结果携带诊断，公共 `start_failed` 的码与 `details` 不由它决定。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "id", content = "args", rename_all = "snake_case")]
pub enum LaunchReason {
    /// 规格无法解析或读取。
    SpecRead { path: String, cause: Box<Reason> },
    /// 显式配置文件已经不在。
    Missing { path: String },
    /// 无效设置及其在原文件中的位置。
    Settings { path: String, issues: Vec<Issue> },
    /// 文件存在但无法读取；系统的原始证据不翻译。
    Read { path: String, error: String },
    /// 合并后配置无效；没有配置文件时仍可能是覆盖或默认设置的问题。
    Invalid {
        path: Option<String>,
        issues: Vec<Issue>,
    },
    /// 端口已经被占用。
    InUse { addr: String },
    /// 端口的访问权限或系统保留范围阻止监听。
    Denied { addr: String },
    /// 系统给出的其他监听失败原因。
    Bind { addr: String, error: String },
    /// TCP 已绑定但未发布控制通道；不把它误认为可用的实例。
    Control {
        endpoint: String,
        port: String,
        error: String,
    },
}
impl LaunchReason {
    /// 从本机绑定结果分类；不要通过错误说明猜测是否被别的程序占用。
    pub fn from_bind(addr: &str, error: &std::io::Error) -> Self {
        let addr = addr.to_owned();
        match error.kind() {
            std::io::ErrorKind::AddrInUse => Self::InUse { addr },
            std::io::ErrorKind::PermissionDenied => Self::Denied { addr },
            _ => Self::Bind {
                addr,
                error: error.to_string(),
            },
        }
    }
    /// 给接收端的语言渲染同一次启动留下的事实；文件、位置和错误码原样输出。
    pub fn render(&self, lang: Lang) -> String {
        let (mut lines, issues, hint) = match self {
            Self::SpecRead { path, cause } => return spec_read(lang, path, &cause.render(lang)),
            Self::Missing { path } => return missing(lang, path),
            Self::Settings { path, issues } => (
                bad_settings(lang).to_owned(),
                issues,
                settings_hint(lang, path),
            ),
            Self::Read { path, error } => return read(lang, path, error),
            Self::Invalid { path, issues } => {
                let mut hint = config_hint(lang).to_owned();
                if let Some(path) = path {
                    hint.push_str(&edit(lang, path));
                }
                (bad_config(lang).to_owned(), issues, hint)
            }
            Self::InUse { addr } => return in_use(lang, addr),
            Self::Denied { addr } => return denied(lang, addr),
            Self::Bind { addr, error } => return bind(lang, addr, error),
            Self::Control {
                endpoint,
                port,
                error,
            } => return control(lang, endpoint, port, error),
        };
        for d in issues {
            lines.push_str("\n  ");
            lines.push_str(&crate::ops::start::render_diagnostic_fields(
                &d.file,
                d.line,
                d.column,
                d.key.as_deref(),
                &d.reason.render(lang),
                &d.code,
            ));
        }
        lines.push_str(&hint);
        lines
    }
    fn code(&self) -> &'static str {
        match self {
            Self::Invalid { .. } | Self::Settings { .. } => "config_invalid",
            Self::Missing { .. } => "target_not_found",
            Self::InUse { .. } => "port_in_use",
            _ => "start_failed",
        }
    }
}

/// 时间戳只用来定位本次故障在英文摘录中的原始行，不独自用来确定实例身份。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchReport {
    code: String,
    details: ReportDetails,
    render: LaunchReason,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ReportDetails {
    stamp: String,
}
impl LaunchReport {
    // 交接槽不是额外的日志通道；丢弃放不进摘录、带控制字符或无法验证的排版参数。
    fn bounded_payload(&self) -> Option<Vec<u8>> {
        const MAX_BYTES: usize = 32 * 1024;
        let mut buf = vec![0; MAX_BYTES];
        let mut writer = std::io::Cursor::new(buf.as_mut_slice());
        serde_json::to_writer(&mut writer, self).ok()?;
        let len = writer.position() as usize;
        buf.truncate(len);
        fn clean(v: &serde_json::Value) -> bool {
            match v {
                serde_json::Value::String(s) => !s.contains(char::is_control),
                serde_json::Value::Object(m) => m.values().all(clean),
                serde_json::Value::Array(a) => a.iter().all(clean),
                _ => true,
            }
        }
        (self.code == self.render.code()
            && clean(&serde_json::from_slice(&buf).ok()?)
            && self
                .render
                .render(Lang::En)
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count()
                <= super::StartupLog::MAX_LINES)
            .then_some(buf)
    }
    /// 在启动者自己的交接槽内保存失败原因。只打开现有普通文件，不跟随符号链接，
    /// 不覆盖其他命名的用户文件，也不重新创建启动者超时后已删除的规格。
    pub fn publish(self, run_dir: &Path, slot: &Path) {
        let Some((nanos, pid)) = slot
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".spec.json"))
            .and_then(|n| n.split_once('-'))
        else {
            return;
        };
        if slot.parent() != Some(run_dir)
            || nanos.parse::<u128>().is_err()
            || pid.parse::<u32>().is_err()
        {
            return;
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(
                windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT,
            );
        }
        let Some(payload) = self.bounded_payload() else {
            return;
        };
        let Ok(mut file) = options.open(slot) else {
            return;
        };
        let Ok(metadata) = file.metadata() else {
            return;
        };
        if !metadata.is_file() {
            return;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes()
                & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                != 0
            {
                return;
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
            };
            let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
            // SAFETY: 只查已打开的句柄；成功时系统初始化完整结构体。硬链接不能作为交接槽。
            if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, info.as_mut_ptr()) }
                == 0
                || unsafe { info.assume_init() }.nNumberOfLinks != 1
            {
                return;
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                return;
            }
        }
        if file.set_len(0).is_ok() {
            use std::io::Write;
            let _ = file.write_all(&payload);
        }
    }
    /// 把日志写者实际用的时间戳与同一条原因配对，旧读者不认识这个文档便沿用日志摘录。
    pub fn new(render: LaunchReason, stamp: String) -> Self {
        Self {
            code: render.code().into(),
            details: ReportDetails { stamp },
            render,
        }
    }
    pub(super) fn localized_lines(&self, original: &[String], lang: Lang) -> Option<Vec<String>> {
        // 多个实例共用 startup.log；只有本进程选择的启动槽里的原因和完整的原始行均吻合，
        // 才在终端上显示译文。重名、截断或并发交错都保留英文证据，不猜哪一行来自本实例。
        self.bounded_payload()?;
        let block = |lang| {
            format!("[{}] {}", self.details.stamp, self.render.render(lang))
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let en = block(Lang::En);
        let mut matches = original
            .windows(en.len())
            .enumerate()
            .filter(|(_, lines)| *lines == en);
        let (i, _) = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        let mut lines = original.to_vec();
        lines.splice(i..i + en.len(), block(lang));
        Some(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_arguments_cannot_extend_the_excerpt() {
        for path in [
            "p".repeat(32 * 1024),
            "private\x1bfield".into(),
            "line\nbreak".into(),
        ] {
            let report = LaunchReport::new(LaunchReason::Missing { path }, "t".into());
            assert!(report.bounded_payload().is_none());
        }
        let report = LaunchReport::new(
            LaunchReason::Missing {
                path: "outside excerpt".into(),
            },
            "t".into(),
        );
        let err = super::super::SpawnError::Exited {
            pid: 1,
            format: None,
            report: Some(Box::new(report)),
            startup_log: super::super::StartupLog {
                path: "startup.log".into(),
                lines: vec!["different record".into()],
            },
        };
        assert!(serde_json::to_value(err.evidence()).unwrap()["report"].is_null());
    }

    #[test]
    fn translates_only_one_complete_matching_record() {
        let reason = LaunchReason::Read {
            path: "cfg".into(),
            error: "denied".into(),
        };
        let report = LaunchReport::new(reason, "t".into());
        let lines: Vec<_> = [
            "before",
            "[t] Cannot read the configuration file: denied",
            "File: cfg",
            "after",
        ]
        .map(str::to_owned)
        .into();
        assert_eq!(
            report.localized_lines(&lines, Lang::Zh).unwrap(),
            [
                "before",
                "[t] 读取配置文件失败: denied",
                "文件: cfg",
                "after"
            ]
        );
        assert!(report.localized_lines(&lines[..2], Lang::Zh).is_none());
        assert!(
            report
                .localized_lines(&[lines.clone(), lines].concat(), Lang::Zh)
                .is_none()
        );
    }
}
