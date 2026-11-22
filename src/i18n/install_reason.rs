//! 普通安装失败只携带排版所需的事实。私有记录的读者不认识某种原因时，仍可使用原始说明。
use super::state::InstallPhase;
use aproxy_ipc::{
    i18n::{Lang, diagnostic::Reason},
    message,
};
use serde::{Deserialize, Serialize};
message!(unmanaged(pids: &str), en: "live instances started by an older version are in this home (pid {pids}); stop them with the aproxy that started them (aproxy stop all), or run aproxy install --force", zh: "这个 home 里有旧版本启动、本次安装管不了的实例还在运行（pid {pids}）。先用启动它们的那个版本执行 aproxy stop all，再重新安装；或者改用 aproxy install --force（会结束全部 aproxy 进程）。");

message!(foreground(p: u16), en: "The instance on port {p} is running in the foreground. Nothing was changed. Stop it in its terminal with Ctrl+C, with aproxy stop {p}, or with its service manager. Run aproxy install, then start it as before.", zh: "端口 {p} 上的实例在前台运行，安装无法替它重启。什么都没有改动：实例照常运行，二进制没换。先停掉它（在启动它的终端里按 Ctrl+C，或执行 aproxy stop {p}；交给服务管理器运行的，用服务管理器停止），再执行 aproxy install，装好之后照原来的方式启动它。");

message!(foreground_many(ports: &str), en: "The instances on ports {ports} are running in the foreground. Nothing was changed. Stop each instance in its terminal with Ctrl+C, with aproxy stop <port>, or with its service manager. Run aproxy install, then start them as before.", zh: "端口 {ports} 上的实例在前台运行，安装无法替它们重启。什么都没有改动：实例照常运行，二进制没换。先停掉它们（在各自的终端里按 Ctrl+C，或逐个执行 aproxy stop <端口>；交给服务管理器运行的，用服务管理器停止），再执行 aproxy install，装好之后照原来的方式启动它们。");

message!(precheck(port: u16), en: "Precheck failed for the instance on port {port}: the target version cannot load its configuration. Nothing was changed. The instance is still running and the binary was not replaced.", zh: "安装前的预检没有通过：端口 {port} 的实例按新版本解析配置时出错。什么都没有改动：实例照常运行，二进制没换。");

message!(no_reason(), en: "reason not recorded", zh: "原因未记录");

message!(successor_missing(), en: "The successor reported a failure without a reason.", zh: "接手进程报告失败（原因未记录）");

message!(aborted(), en: "Installation was aborted (--abort).", zh: "安装已被中止（--abort）");

message!(halted(cause: &str), en: "The last installation stopped after an instance failed and will not retry automatically ({cause}). Correct the problem and run aproxy install again.", zh: "上次安装因实例级失败已中止，不自动重试（{cause}）。排除原因后重新执行 aproxy install");

message!(owner(pid: u32, phase: InstallPhase, reason: &str), en: "The successor (pid {pid}) exited during {phase:?}: {reason}. Run aproxy install again to resume.", zh: "接手进程（pid {pid}）已退出但安装停在 {phase:?}：{reason}；重新执行 aproxy install 即可接着装完");

message!(timeout(secs: u64), en: "Installation did not finish within {secs} seconds. It may still be running. Use aproxy status to check instance versions.", zh: "{secs} 秒内未确认安装完成（可能仍在后台进行）；可用 aproxy status 查看实例版本");

message!(panic_at(location: &str), en: "installer panic at {location}", zh: "安装进程在 {location} 意外终止");

message!(no_terminal(status: &str), en: "no terminal state; {status}", zh: "未记录终态；{status}");

message!(saving(code: &str, cause: &str, error: &str), en: "{code}: {cause}; saving install/state.json: {error}", zh: "{code}：{cause}；保存 install/state.json 失败：{error}");

message!(exit_status(code: u32), en: "exit status {code} (0x{code:08x})", zh: "退出状态 {code} (0x{code:08x})");

message!(no_status(), en: "exit status unavailable", zh: "无法取得退出状态");

message!(local_variant(path:&str, got:&str, want:&str), en: "{path} is a {got} build; --variant requires {want}.", zh: "{path} 是 {got} 版本，与 --variant {want} 不符");

message!(unknown_variant(), en: "unknown variant", zh: "未知变体");

message!(self_exe(e:&str), en: "Cannot locate the current executable: {e}", zh: "无法定位自身可执行文件: {e}");

message!(cannot_run(e:&str), en: "The downloaded file cannot run: {e}", zh: "下载产物不可运行: {e}");

message!(bad_download(e:&str), en: "Downloaded file verification failed: {e}", zh: "下载产物校验失败: {e}");

message!(wrong_version(got:&str,target:&str), en: "The downloaded binary reports version {got}; expected {target}.", zh: "下载产物自报 {got}，与目标版本 {target} 不符");

message!(download_variant(asset:&str,got:&str,want:&str), en: "Downloaded {asset} reports variant {got}; expected {want}.", zh: "下载的 {asset} 自报变体 {got}，不是要装的 {want}");

message!(unknown(), en: "unknown", zh: "未知");

message!(asset_failed(asset:&str,e:&str), en: "{asset} cannot run on this system ({e})", zh: "{asset} 在本机不可运行（{e}）");

message!(cannot_abort(phase:InstallPhase), en: "The installation has reached binary replacement (phase {phase:?}) and cannot roll back. Some instances may already run the target version. Interrupted installations resume automatically. For manual recovery, follow the skill’s manual update instructions.", zh: "安装已进入交换阶段（phase {phase:?}），无法回滚——实例可能已开始滚动到新版本。\n中断的安装会自动续作完成；如需手动干预，按 skill 的手动更新兜底路径操作。");

/// 安装接手者留下的私有排版参数，不是事件码，也不进入公开的 details。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "id", content = "args", rename_all = "snake_case")]
pub enum Cause {
    /// 备料、版本核对或二进制交换失败。
    Files(super::files::FileIssue),
    /// 原始渠道的失败分类。
    Download(super::download::issue::Issue),
    /// 尝试顺序也是诊断的一部分；每个渠道只保留最后一次失败的原因。
    Sources {
        asset: String,
        failures: Vec<(String, Cause)>,
    },
    /// 独立 skill 命令及普通安装支线保留同一组故障事实。
    Skill(super::skills::issue::Issue),
    /// 显式本地文件与要安装的变体不一致。
    LocalVariant {
        path: String,
        got: Option<String>,
        want: String,
    },
    /// 收编时无法取得本进程的镜像路径。
    SelfExe(String),
    /// 下载文件执行探测失败。
    CannotRun(Box<Cause>),
    /// 下载的文件不是可验证的 aProxy 发布产物。
    BadDownload(Box<Cause>),
    /// 候选文件报告了另一个版本。
    WrongVersion { got: String, target: String },
    /// 下载的变体与文件名对应不上。
    DownloadVariant {
        asset: String,
        got: Option<String>,
        want: String,
    },
    /// 候选产物无法在本机运行。
    AssetFailed { asset: String, cause: Box<Cause> },
    /// 交换之后可能已有部分实例升级，不能回滚。
    CannotAbort(InstallPhase),
    /// 旧版本或系统提供的证据不按文字反推类别。
    Evidence(String),
    /// 旧版启动、当前无法管理的实例，临终显示不重新枚举进程。
    Unmanaged(Vec<u32>),
    /// 前台实例由用户或服务管理器负责重新拉起。
    Foreground(Vec<u16>),
    /// 各诊断按公开数组的原顺序排列，码、位置及详情仍来自原数组。
    Precheck { port: u16, issues: Vec<Reason> },
    /// 失败现场的原因缺失。
    NoReason,
    /// 接手者报告了失败但没有留下原因。
    Missing,
    /// 用户已中止安装。
    Aborted,
    /// 上一次的实例级失败不自动重试。
    Halted(Box<Cause>),
    /// 未确认接手者成功，退出说明是已核实的该事务的证据。
    Owner {
        pid: u32,
        phase: InstallPhase,
        reason: Box<Cause>,
    },
    /// 等待超过上限，不能据此断言安装失败或已经停止。
    Timeout(u64),
    /// 接手者 panic 的位置，不带 panic 内容。
    Panic(String),
    /// 只记录了退出状态；句柄不能提供退出码时为 None。
    NoTerminal(Option<u32>),
    /// 保存主状态失败，因此附上原始失败的原因以及保存错误。
    Saving {
        code: String,
        cause: Box<Cause>,
        error: String,
    },
}
message!(sources(asset:&str,errors:&str),en:"All download sources failed (asset {asset}):\n  {errors}",zh:"下载链条全失败（asset {asset}）:\n  {errors}");
impl Cause {
    /// 只保留可安全记录的文档下载参数；未知的嵌套类型回退到已打码的英文证据。
    pub(crate) fn scrub_skill_urls(&mut self, clean: &impl Fn(&mut String)) {
        match self {
            Self::Sources { asset, failures } => {
                clean(asset);
                for (_, e) in failures {
                    e.scrub_skill_urls(clean);
                }
            }
            Self::Download(e) => e.scrub(clean),
            Self::Evidence(s) => clean(s),
            other => {
                let mut s = other.render(Lang::En);
                let before = s.clone();
                clean(&mut s);
                if before != s {
                    *other = Self::Evidence(s);
                }
            }
        }
    }
    /// 状态文件、事件和日志选英文，终端可以对相同的事实另选语言。
    pub fn render(&self, lang: Lang) -> String {
        match self {
            Self::Files(e) => e.render(lang),
            Self::Download(e) => e.render(lang),
            Self::Sources { asset, failures } => sources(
                lang,
                asset,
                &failures
                    .iter()
                    .map(|(s, e)| format!("[{s}] {}", e.render(lang)))
                    .collect::<Vec<_>>()
                    .join("\n  "),
            ),
            Self::Skill(e) => e.render(lang),
            Self::LocalVariant { path, got, want } => local_variant(
                lang,
                path,
                got.as_deref().unwrap_or_else(|| unknown_variant(lang)),
                want,
            ),
            Self::SelfExe(e) => self_exe(lang, e),
            Self::CannotRun(e) => cannot_run(lang, &e.render(lang)),
            Self::BadDownload(e) => bad_download(lang, &e.render(lang)),
            Self::WrongVersion { got, target } => wrong_version(lang, got, target),
            Self::DownloadVariant { asset, got, want } => download_variant(
                lang,
                asset,
                got.as_deref().unwrap_or_else(|| unknown(lang)),
                want,
            ),
            Self::AssetFailed { asset, cause } => asset_failed(lang, asset, &cause.render(lang)),
            Self::CannotAbort(phase) => cannot_abort(lang, *phase),
            Self::Evidence(s) => s.clone(),
            Self::Unmanaged(pids) => unmanaged(
                lang,
                &pids
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            Self::Foreground(ports) => match ports.as_slice() {
                [p] => foreground(lang, *p),
                _ => foreground_many(
                    lang,
                    &ports
                        .iter()
                        .map(u16::to_string)
                        .collect::<Vec<_>>()
                        .join(if lang == Lang::Zh { "、" } else { ", " }),
                ),
            },
            Self::Precheck { port, .. } => precheck(lang, *port),
            Self::NoReason => no_reason(lang).into(),
            Self::Missing => successor_missing(lang).into(),
            Self::Aborted => aborted(lang).into(),
            Self::Halted(e) => halted(lang, &e.render(lang)),
            Self::Owner { pid, phase, reason } => owner(lang, *pid, *phase, &reason.render(lang)),
            Self::Timeout(s) => timeout(lang, *s),
            Self::Panic(l) => panic_at(lang, l),
            Self::NoTerminal(code) => no_terminal(
                lang,
                &code
                    .map(|n| exit_status(lang, n))
                    .unwrap_or_else(|| no_status(lang).to_owned()),
            ),
            Self::Saving { code, cause, error } => saving(lang, code, &cause.render(lang), error),
        }
    }
}
