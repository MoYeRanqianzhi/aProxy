//! 强制安装 `aproxy install --force`：永久兼容契约 C6–C8 的执行体。其余几项各在
//! 一处：台账 C3 在 `crate::ledger`，唯一中间名 C2 在 `super::swap`，安装锁 C4 在
//! `super::lock_install_in`，`--version` 的格式 C5 在 `crate::VERSION_LINE`。设计全文见
//! `.agents/plan/release-0.2.0.md` 的「W2b 强制安装：永久兼容契约」一节。
//!
//! **永久兼容契约：本文件里标明冻结的东西——发布清单的资产名与 JSON 格式、退出码 10/11/12、
//! 命令行写法、流程的先后顺序——无论如何都不得改动，只许追加可选字段。**
//!
//! 为什么：强制安装是普通安装失败之后的兜底，它保证从 0.2.0 起，任意版本 X 的
//! `install --force` 都能把任意版本 Y 装进同一个 home，并停掉这个 home 下由任意版本启动
//! 的全部 aproxy 进程；这也覆盖「装着的版本坏到跑不起来」：手动下载新版 Y，执行
//! `Y install --force --from Y`。X 与 Y 之间唯一的共同语言就是 C1–C8 这几样，所以 X 不
//! 依赖除此之外的任何东西——不走 IPC、不读注册表与 install.state 的内容、不读写配置、
//! settings.json 与日志（损坏的 settings.json 不能挡住修复它的手段，以后界面语言的判定
//! 也不能让强制安装去读它）。agent 与脚本按退出码决定下一步，改了退出码，写在各处的
//! 处置就会指向错误的方向。
//!
//! 流程（C8）：先验证、后破坏——新二进制取到 `<home>/install/force/` 下、校验、在本机
//! 跑通 `--version` 之前，什么都不停；每一步可重入，中途被打断就再跑一次，最终收敛。
//!
//! 1. 取得目标（发布清单 C6 或 `--from`），校验 sha256，在本机执行 `--version` 核对；
//!    失败 → 退出码 10，什么都没动。
//! 2. 确认：会中断所有经 aproxy 的在途请求，包括 agent 自己经 aproxy 的会话。`--yes`
//!    跳过；标准输入不是终端又没有 `--yes` → 10，不卡着等输入。
//! 3. 按台账结束安装者（本进程除外）→ 拿安装锁 → 结束看护者、实例、桌面界面与不认识
//!    的角色；反复扫描直到连续两轮没有存活条目（有上限）。从这一步起，二进制换好之前的
//!    任何失败 → 11。
//! 4. 清空 `<home>/install/`（保留锁与本次的目标文件）：留下的 install.state 会让续作在
//!    强制安装之后接着推进一个旧的安装。
//! 5. 换 `bin/` 里的二进制（唯一中间名，见 `super::swap`），落位后在新位置再跑一次
//!    `--version` 核对。
//! 6. 尽力调用新版本的 `restore`。被强制结束的实例留下了 `.restore` 记录，restore 正是
//!    凭它把实例拉回来，所以强制安装不删 `.restore`（`stop --force` 会删，两者不同）。
//!    是否拉回只按台账判断：被结束实例的端口上出现了新的 instance 条目才算；没回来的按
//!    端口与说明列出 → 12。
//! 7. 报告不在台账里、却从 `<home>/bin/` 运行的进程（例如 0.1.x 留下的），只报告、
//!    不结束。
//!
//! 强制安装不更新 skill 文档，结束时提示怎样更新。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::download::{Channel, DownloadCtx};
use super::swap;
use crate::ledger::{
    self, LedgerEntry, ROLE_DESKTOP, ROLE_INSTALLER, ROLE_INSTANCE, ROLE_WATCHDOG,
};

/// 退出码 0：完成——二进制已换好，被结束的实例都已按台账确认回来。
///
/// 永久兼容契约 C7：无论如何都不得改动（理由见文件头）。
pub const EXIT_DONE: i32 = 0;

/// 退出码 10：在动任何东西之前就中止了——目标取不到、校验或 `--version` 不过、版本低于
/// 0.2.0、没有得到确认。进程、二进制、`<home>/install/` 都没动，可以排除原因后重来。
///
/// 永久兼容契约 C7：无论如何都不得改动。通用退出码只用 0–5（2 是命令行用法错误），
/// 10 起留给强制安装，互不重叠。
pub const EXIT_UNCHANGED: i32 = 10;

/// 退出码 11：已经开始结束进程（第 3 步起），但 `bin/` 里的二进制没有换成。被结束的
/// 进程不会自己回来；再执行一次同一条命令即可（每一步都可重入），放弃的话
/// `aproxy restore` 会用原来的二进制把实例拉回来。
///
/// 永久兼容契约 C7：无论如何都不得改动。
pub const EXIT_STOPPED: i32 = 11;

/// 退出码 12：二进制已经换好，但有被结束的实例没有回来（按台账判断），输出里按端口与
/// 说明列出，要手动启动。
///
/// 永久兼容契约 C7：无论如何都不得改动。
pub const EXIT_NOT_RESTORED: i32 = 12;

/// 发布清单的资产名：每个主线 GitHub Release（tag `v<semver>`）都附这个文件。
///
/// 永久兼容契约 C6：无论如何都不得改动。强制安装只靠它找资产——所以以后资产怎么改名，
/// 都不影响旧版本的强制安装；换掉它的名字，旧版本就再也找不到清单。
pub const MANIFEST_ASSET: &str = "aproxy-manifest.json";

/// 清单 `manifest` 字段的取值。写者恒写 1；读者要求这个字段存在，但不按它的值分支。
///
/// 永久兼容契约 C6：无论如何都不得改动（只许追加可选字段，所以永远是 1）。
pub const MANIFEST_FORMAT: u32 = 1;

/// 清单与 `--version` 变体段的取值：带桌面界面的版本。
///
/// 永久兼容契约 C5、C6、C7：无论如何都不得改动——它同时出现在 `--version` 的变体段、清单
/// 资产的 `variant` 字段与 `--variant` 的取值里，三处必须是同一个词。
pub const VARIANT_DESKTOP: &str = "desktop";
/// 清单与 `--version` 变体段的取值：无界面版本。永久兼容契约 C5、C6、C7，同上。
pub const VARIANT_HEADLESS: &str = "headless";

/// 清单资产 `cpu` 字段的取值：不要求 x86-64-v3 指令集的基线产物（每个目标都有）。
///
/// 永久兼容契约 C6：无论如何都不得改动，只许追加新的取值（读者跳过不认识的取值）。
pub const CPU_BASELINE: &str = "baseline";
/// 清单资产 `cpu` 字段的取值：以 `-C target-cpu=x86-64-v3` 构建的产物（与 `--version`
/// 末段 `x86-64-v3` 是同一个词）。永久兼容契约 C5、C6，同上。
pub const CPU_X86_64_V3: &str = "x86-64-v3";

/// 等安装锁的上限：会持有它的只有正在改 bin/ 的安装者，而它们在第 3 步已被结束
/// （锁随进程死亡释放）；还拿不到，说明有不在台账里的进程占着它。
const LOCK_WAIT: Duration = Duration::from_secs(30);
/// 结束一个进程后等它真正退出的上限。
const KILL_WAIT: Duration = Duration::from_secs(10);
/// 两轮台账扫描之间的间隔：给刚被拉起的进程留出登记台账的时间。
const STOP_ROUND_GAP: Duration = Duration::from_millis(500);
/// 台账扫描的轮数上限：一直有新进程冒出来（例如有人在反复启动实例）就放弃，退出码 11。
const MAX_STOP_ROUNDS: usize = 20;
/// 执行目标二进制 `--version` 的上限，余量留给杀毒软件对新文件的首次扫描。
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);
/// 等新版本的 `restore` 结束的上限：基数加每个被结束的实例一份（restore 逐个等实例就绪，
/// 每个最多 8 秒）。
const RESTORE_WAIT_BASE: Duration = Duration::from_secs(30);
const RESTORE_WAIT_PER_INSTANCE: Duration = Duration::from_secs(10);
/// restore 结束之后，再等被结束的实例在台账里出现的上限。
const RETURN_WAIT: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// C5：`--version` 首行
// ---------------------------------------------------------------------------

/// `--version` 首行（永久兼容契约 C5）拆开的各段：
/// `aproxy v<semver> (<变体>; <目标三元组>[; x86-64-v3])`。
///
/// 版本就用普通安装的取法（`staging::parse_version_line`：`aproxy v` 之后、到空白为止，
/// 必须是合法 semver），两条路认版本的口径因此不会分叉；括号段缺失或不是这个形状时，
/// 变体与目标为 None——只靠版本也认得出它。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionLine {
    pub version: String,
    pub variant: Option<String>,
    pub target: Option<String>,
    /// 末段是 `x86-64-v3`
    pub x86_64_v3: bool,
}

impl VersionLine {
    pub fn parse(output: &str) -> Option<Self> {
        let version = super::staging::parse_version_line(output)?;
        // 版本是首行 `aproxy v` 之后的第一段（semver 只含 ASCII，按字节长度切不会切坏字符）
        let tail = output.lines().next()?["aproxy v".len() + version.len()..].trim();
        let mut line = Self {
            version,
            variant: None,
            target: None,
            x86_64_v3: false,
        };
        if let Some(inner) = tail.strip_prefix('(').and_then(|t| t.strip_suffix(')')) {
            let mut parts = inner.split("; ");
            line.variant = parts.next().map(str::to_string);
            line.target = parts.next().map(str::to_string);
            line.x86_64_v3 = parts.next() == Some(CPU_X86_64_V3);
        }
        Some(line)
    }

    /// 正在运行的这个二进制自己的版本行：强制安装默认沿用它的变体与目标三元组。
    pub fn own() -> Self {
        Self::parse(crate::VERSION_LINE)
            .expect("VERSION_LINE 恒为永久格式（lib.rs 的单元测试钉死）")
    }
}

// ---------------------------------------------------------------------------
// C6：发布清单
// ---------------------------------------------------------------------------

/// 发布清单 `aproxy-manifest.json`，例如：
///
/// ```text
/// {"manifest":1,"version":"0.2.0","assets":[{"target":"x86_64-pc-windows-msvc","variant":"desktop","cpu":"x86-64-v3","file":"aproxy-x86_64-pc-windows-msvc-v3.exe","sha256":"<64 位小写十六进制>"}]}
/// ```
///
/// 永久兼容契约 C6：字段的名字、类型、含义无论如何都不得改动，只许追加可选字段；读者
/// 忽略不认识的字段，资产里不认识的 `variant`、`cpu` 取值不参与选择（所以以后加新的
/// 变体与指令集档，旧版本照样能读、只是选不到它们）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// 恒为 [`MANIFEST_FORMAT`]
    pub manifest: u32,
    /// 这份清单所属的版本（不带 v），必须等于 release 的 tag 去掉 v
    pub version: String,
    pub assets: Vec<ManifestAsset>,
}

/// 清单里的一个二进制资产。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestAsset {
    /// Rust 目标三元组，与 `--version` 的目标段同一个词，例如 `x86_64-unknown-linux-gnu`
    pub target: String,
    /// [`VARIANT_DESKTOP`] 或 [`VARIANT_HEADLESS`]
    pub variant: String,
    /// [`CPU_BASELINE`] 或 [`CPU_X86_64_V3`]
    pub cpu: String,
    /// 同一个 release 里的资产文件名
    pub file: String,
    /// 文件的 sha256（十六进制，比对时不分大小写）
    pub sha256: String,
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("{MANIFEST_ASSET} 解析失败: {e}"))
    }

    /// 精确查找：目标三元组、变体、指令集档三者都对上的资产（显式 `--cpu` 走这里）。
    pub fn find(&self, target: &str, variant: &str, cpu: &str) -> Option<&ManifestAsset> {
        self.assets
            .iter()
            .find(|a| a.target == target && a.variant == variant && a.cpu == cpu)
    }

    /// 按本机自动选档：目标三元组与变体都得对上；`prefer_v3` 时先找 x86-64-v3 档、没有
    /// 就退回基线档（不是每个目标都发布 v3）。不认识的 cpu 取值不参与。
    pub fn select(&self, target: &str, variant: &str, prefer_v3: bool) -> Option<&ManifestAsset> {
        prefer_v3
            .then(|| self.find(target, variant, CPU_X86_64_V3))
            .flatten()
            .or_else(|| self.find(target, variant, CPU_BASELINE))
    }

    /// 清单为 `target` 实际提供的变体与指令集档，给人看，按清单里出现的先后列出，例如
    /// `headless（baseline）` 或 `desktop（baseline、x86-64-v3）；headless（baseline）`。
    /// 清单里没有这个目标时为 None。
    ///
    /// 找不到要的组合时据此如实报告，而不是为某个目标写特例：例如 musl 目标只发布无界面
    /// 版本，`--variant desktop` 碰上它，用户看到的就是「这个目标只有 headless」。
    pub fn offered(&self, target: &str) -> Option<String> {
        let mut variants: Vec<(&str, Vec<&str>)> = Vec::new();
        for asset in self.assets.iter().filter(|a| a.target == target) {
            match variants.iter_mut().find(|(v, _)| *v == asset.variant) {
                Some((_, cpus)) if !cpus.contains(&asset.cpu.as_str()) => {
                    cpus.push(asset.cpu.as_str())
                }
                Some(_) => {}
                None => variants.push((asset.variant.as_str(), vec![asset.cpu.as_str()])),
            }
        }
        (!variants.is_empty()).then(|| {
            variants
                .iter()
                .map(|(variant, cpus)| format!("{variant}（{}）", cpus.join("、")))
                .collect::<Vec<_>>()
                .join("；")
        })
    }

    /// 清单里找不到「目标三元组 + 变体 [+ 指令集档]」时的说明：列出这个目标实际有哪些
    /// 变体与指令集档；清单里根本没有这个目标时，列出清单里有的目标。
    fn missing(&self, target: &str, variant: &str, cpu: Option<&str>) -> String {
        // 全角括号后面不再空格：「desktop 版本」「headless（x86-64-v3）版本」
        let wanted = match cpu {
            Some(cpu) => format!("{variant}（{cpu}）"),
            None => format!("{variant} "),
        };
        match self.offered(target) {
            Some(offered) => format!(
                "release v{} 的发布清单里没有 {target} 的 {wanted}版本，这个目标只有 {offered}",
                self.version
            ),
            None => {
                let mut targets: Vec<&str> =
                    self.assets.iter().map(|a| a.target.as_str()).collect();
                targets.sort_unstable();
                targets.dedup();
                format!(
                    "release v{} 的发布清单里没有 {target} 的资产（清单里有的目标：{}）",
                    self.version,
                    targets.join("、")
                )
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 第 1 步：取得目标
// ---------------------------------------------------------------------------

/// 取到并核对过的目标二进制。
#[derive(Debug, Clone)]
pub struct Acquired {
    /// `<home>/install/force/<本次>/` 下的副本
    pub path: PathBuf,
    /// 它自己 `--version` 报出的版本行
    pub line: VersionLine,
    /// 给人看的来源说明
    pub origin: String,
}

/// 在本机执行 `bin --version`（最多 [`PROBE_TIMEOUT`]），按 C5 拆开首行。跑不起来、
/// 超时、输出不是永久格式都算失败，错误里带上它的标准错误（缺系统库时，动态链接器的
/// 报错就在那里）。与 `staging::probe_version` 的区别只在有时限、带标准错误：强制安装是
/// 最后的手段，一个卡住的 `--version` 不能把它也卡住。
pub fn probe(bin: &Path) -> Result<VersionLine, String> {
    use std::io::Read;
    let mut child = std::process::Command::new(bin)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{} 无法执行: {e}", bin.display()))?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    // --version 只输出一行，管道写不满，等它退出再读不会互相卡住
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{} --version 在 {} 秒内没有结束",
                    bin.display(),
                    PROBE_TIMEOUT.as_secs()
                ));
            }
        }
    };
    let (mut stdout, mut stderr) = (String::new(), String::new());
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }
    if !status.success() {
        return Err(format!(
            "{} --version 失败（{status}）: {}",
            bin.display(),
            stderr.trim()
        ));
    }
    // 认不出版本时用普通安装的说明（0.1.x 的二进制会得到「低于 0.2.0、怎样回到 0.1.x」）
    super::staging::version_of(bin, &stdout)?;
    Ok(VersionLine::parse(&stdout).expect("version_of 认得出版本，首行就是永久格式"))
}

/// 版本下限 0.2.0（[`super::INSTALL_FLOOR`]）照样生效：强制安装隐含允许降级，但装回
/// 0.1.x 只会让 0.2.0 起的命令再也管不了它。
fn check_target_version(version: &str) -> Result<(), String> {
    let parsed = semver::Version::parse(version)
        .map_err(|e| format!("版本号 {version} 非法（semver 解析失败: {e}）"))?;
    super::check_floor(&parsed)
}

/// `--from` 来源：先在原处执行一次 `--version`——认不出的文件（0.1.x 的二进制、别的
/// 程序）在这里就得到说明，不必复制；再复制进 `dest`，对副本再执行一次：装进去的是
/// 副本，复制出错、杀毒软件隔离了新文件，都要在结束任何进程之前发现。给了 `variant`
/// 就要求文件的变体与之一致（防止把带界面的版本当成无界面的装进缺库的机器）。
pub fn acquire_file(from: &Path, variant: Option<&str>, dest: &Path) -> Result<Acquired, String> {
    if !from.is_file() {
        return Err(format!("源二进制不存在: {}", from.display()));
    }
    let line = probe(from)?;
    check_target_version(&line.version)?;
    if let Some(variant) = variant
        && line.variant.as_deref() != Some(variant)
    {
        return Err(format!(
            "{} 是 {} 版本，与 --variant {variant} 不符",
            from.display(),
            line.variant.as_deref().unwrap_or("未知变体")
        ));
    }
    std::fs::copy(from, dest)
        .map_err(|e| format!("{} 复制到 {} 失败: {e}", from.display(), dest.display()))?;
    super::staging::ensure_executable(dest);
    let copied = probe(dest)?;
    if copied != line {
        return Err(format!(
            "{} 的副本 {} 自报的版本行与原文件不同（复制出错？）",
            from.display(),
            dest.display()
        ));
    }
    Ok(Acquired {
        path: dest.to_path_buf(),
        line,
        origin: format!("本地文件 {}", from.display()),
    })
}

/// GitHub 上本仓库的 API 基址（`<基址>/releases…`）。测试换成本地假服务器的地址。
pub fn github_api() -> String {
    format!(
        "https://api.github.com/repos/{}",
        super::download::github::REPO
    )
}

/// 一个主线 release：版本与资产（文件名 → 下载地址）。
#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    assets: Vec<(String, String)>,
}

impl Release {
    fn asset_url(&self, name: &str) -> Option<&str> {
        self.assets
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, url)| url.as_str())
    }
}

/// GET 一个地址，非 2xx 算失败。`api` 时带上 GitHub API 要求的 Accept 头；下载 release
/// 资产（`browser_download_url`）不带，与普通安装下载资产的请求一致。
async fn get(client: &reqwest::Client, url: &str, api: bool) -> Result<reqwest::Response, String> {
    let mut request = client.get(url);
    if api {
        request = request.header("Accept", "application/vnd.github+json");
    }
    request
        .send()
        .await
        .map_err(|e| format!("请求 {url} 失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("请求 {url} 失败: {e}"))
}

/// 找要装的 release。`version` 为 None 时取通道内最新、且附带发布清单的主线版本——
/// 复用普通安装的选版规则（只认 `v*` 语法，通道规则相同，见 `download::github::pick_latest`），
/// 只是把「资产已齐」的判据换成清单。0.2.0 之前的 release 都没有清单，不会被选中。
pub async fn find_release(
    client: &reqwest::Client,
    api: &str,
    version: Option<&str>,
    channel: Channel,
) -> Result<Release, String> {
    let version = match version {
        Some(v) => v.to_string(),
        None => {
            let list: serde_json::Value =
                get(client, &format!("{api}/releases?per_page=100"), true)
                    .await?
                    .json()
                    .await
                    .map_err(|e| format!("GitHub Releases 列表解析失败: {e}"))?;
            if !list.is_array() {
                return Err("GitHub Releases 列表格式异常（不是数组）".to_string());
            }
            super::download::github::pick_latest(&list, channel, MANIFEST_ASSET).ok_or_else(|| {
                format!(
                    "{} 通道里没有附带 {MANIFEST_ASSET} 的版本（0.2.0 之前的发布都没有发布清单）；可以手动下载二进制后用 --from 安装",
                    channel.label()
                )
            })?
        }
    };
    check_target_version(&version)?;
    let item: serde_json::Value = get(client, &format!("{api}/releases/tags/v{version}"), true)
        .await?
        .json()
        .await
        .map_err(|e| format!("release v{version} 的信息解析失败: {e}"))?;
    let assets = item["assets"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|a| {
                    Some((
                        a["name"].as_str()?.to_string(),
                        a["browser_download_url"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Release { version, assets })
}

/// 下载并解析 release 附带的发布清单，核对它的版本就是这个 release 的版本。
pub async fn fetch_manifest(
    client: &reqwest::Client,
    release: &Release,
) -> Result<Manifest, String> {
    let url = release.asset_url(MANIFEST_ASSET).ok_or_else(|| {
        format!(
            "release v{} 没有附带 {MANIFEST_ASSET}（0.2.0 之前的发布都没有发布清单）；可以手动下载二进制后用 --from 安装",
            release.version
        )
    })?;
    let text = get(client, url, false)
        .await?
        .text()
        .await
        .map_err(|e| format!("{MANIFEST_ASSET} 下载失败: {e}"))?;
    let manifest = Manifest::parse(&text)?;
    if manifest.version != release.version {
        return Err(format!(
            "release v{} 的 {MANIFEST_ASSET} 写的版本是 {}，两者不符",
            release.version, manifest.version
        ));
    }
    Ok(manifest)
}

/// 下载清单里的一个资产到 `dest`，比对清单写的 sha256。不符就删掉并报错。
pub async fn download_asset(
    client: &reqwest::Client,
    release: &Release,
    asset: &ManifestAsset,
    dest: &Path,
) -> Result<(), String> {
    let url = release.asset_url(&asset.file).ok_or_else(|| {
        format!(
            "{MANIFEST_ASSET} 列出的 {} 不在 release v{} 的资产里",
            asset.file, release.version
        )
    })?;
    super::download::download_to(client, url, dest)
        .await
        .map_err(|e| format!("{} 下载失败: {e}", asset.file))?;
    let actual = super::staging::sha256_hex(dest)?;
    if !actual.eq_ignore_ascii_case(&asset.sha256) {
        let _ = std::fs::remove_file(dest);
        return Err(format!(
            "{} 的 sha256 与清单不符（清单 {}，实际 {actual}）",
            asset.file, asset.sha256
        ));
    }
    super::staging::ensure_executable(dest);
    Ok(())
}

/// 在线取目标的参数。
#[derive(Debug, Clone)]
pub struct OnlineSpec {
    /// 指定版本（不带 v）；None = 通道内最新
    pub version: Option<String>,
    pub channel: Channel,
    /// 目标三元组，默认是正在运行的这个二进制的
    pub target: String,
    /// 变体，默认是正在运行的这个二进制的
    pub variant: String,
    /// `--cpu` 显式指定的指令集档：清单里必须正好有这一档，没有就报告这个目标有什么
    pub cpu: Option<String>,
    /// 没有显式指定指令集档时按本机 CPU 选：true = 先找 x86-64-v3 档，没有就退回基线档
    pub prefer_v3: bool,
}

/// 在线取目标：找 release → 读发布清单 → 选资产 → 下载并比对 sha256 → 在本机执行
/// `--version`，核对版本与变体。linux-gnu 的产物跑不起来时（它按构建机的 glibc 链接，
/// 老发行版上跑不了；带界面的版本还要 WebKitGTK 等系统库），改用同一份清单里同变体的
/// musl 基线产物再试一次——普通在线安装也是这样回退的。清单里找不到要的组合时（例如
/// musl 目标只发布 headless），错误里列出这个目标实际有的变体与指令集档。
pub async fn acquire_online(
    client: &reqwest::Client,
    api: &str,
    spec: &OnlineSpec,
    dest: &Path,
) -> Result<Acquired, String> {
    let release = find_release(client, api, spec.version.as_deref(), spec.channel).await?;
    let manifest = fetch_manifest(client, &release).await?;
    let asset = match &spec.cpu {
        Some(cpu) => manifest.find(&spec.target, &spec.variant, cpu),
        None => manifest.select(&spec.target, &spec.variant, spec.prefer_v3),
    }
    .ok_or_else(|| manifest.missing(&spec.target, &spec.variant, spec.cpu.as_deref()))?;
    download_asset(client, &release, asset, dest).await?;
    let (asset, line) = match probe(dest) {
        Ok(line) => (asset, line),
        Err(gnu_err) => {
            let Some(musl_target) = spec
                .target
                .strip_suffix("-linux-gnu")
                .map(|arch| format!("{arch}-linux-musl"))
            else {
                return Err(gnu_err);
            };
            // musl 产物恒为基线档（不发布 x86-64-v3）
            let Some(musl) = manifest.find(&musl_target, &spec.variant, CPU_BASELINE) else {
                return Err(format!(
                    "{gnu_err}；改用 musl 产物也不行：{}",
                    manifest.missing(&musl_target, &spec.variant, Some(CPU_BASELINE))
                ));
            };
            download_asset(client, &release, musl, dest).await?;
            let line = probe(dest)
                .map_err(|musl_err| format!("{gnu_err}；改用 musl 产物也不行：{musl_err}"))?;
            (musl, line)
        }
    };
    if line.version != release.version {
        return Err(format!(
            "{} 自报版本 {}，与 release v{} 不符",
            asset.file, line.version, release.version
        ));
    }
    if line.variant.as_deref() != Some(spec.variant.as_str()) {
        return Err(format!(
            "{} 自报变体 {}，与要装的 {} 不符",
            asset.file,
            line.variant.as_deref().unwrap_or("未知"),
            spec.variant
        ));
    }
    Ok(Acquired {
        path: dest.to_path_buf(),
        line,
        origin: format!("GitHub Release v{} 的 {}", release.version, asset.file),
    })
}

// ---------------------------------------------------------------------------
// 第 3–7 步
// ---------------------------------------------------------------------------

fn role_label(role: &str) -> &str {
    match role {
        ROLE_INSTALLER => "安装者",
        ROLE_WATCHDOG => "看护者",
        ROLE_INSTANCE => "实例",
        ROLE_DESKTOP => "桌面界面",
        other => other,
    }
}

/// 一条条目给人看的说明：角色、端口、pid、说明。
fn describe(entry: &LedgerEntry) -> String {
    let mut text = format!("{} pid {}", role_label(&entry.role), entry.pid);
    if let Some(port) = entry.port {
        text.push_str(&format!(" 端口 {port}"));
    }
    if let Some(label) = &entry.label {
        text.push_str(&format!("（{label}）"));
    }
    text.push_str(&format!(" v{}", entry.version));
    text
}

/// 按台账结束进程，本进程除外：`only_installers` 时只结束安装者（第 3 步拿锁之前），
/// 否则全部角色，先后见 `LedgerEntry::stop_order`（不认识的角色排在最后）。条目对应的
/// 进程已经不在就删掉条目；结束之后也删掉。反复扫描，直到连续两轮没有存活的条目：
/// 结束看护者与实例的同时，别的进程可能正在拉起新的（实例的自检补种看护者、正在
/// 启动的实例），它们都在占端口或拉子进程之前登记，下一轮就会被看到。
fn stop_by_ledger(
    home: &Path,
    only_installers: bool,
    stopped: &mut Vec<LedgerEntry>,
) -> Result<(), String> {
    let me = std::process::id();
    let mut quiet_rounds = 0;
    for _ in 0..MAX_STOP_ROUNDS {
        let mut stopped_any = false;
        for found in ledger::read_all_in(home) {
            let entry = &found.entry;
            if entry.pid == me || (only_installers && entry.role != ROLE_INSTALLER) {
                continue;
            }
            match ledger::terminate(entry, KILL_WAIT) {
                ledger::Termination::Terminated => {
                    println!("  已结束 {}", describe(entry));
                    let _ = std::fs::remove_file(&found.path);
                    stopped_any = true;
                    stopped.push(entry.clone());
                }
                ledger::Termination::AlreadyGone => {
                    let _ = std::fs::remove_file(&found.path);
                }
                ledger::Termination::Failed(e) => {
                    return Err(format!("无法结束 {}：{e}", describe(entry)));
                }
            }
        }
        if stopped_any {
            quiet_rounds = 0;
        } else {
            quiet_rounds += 1;
            if quiet_rounds >= 2 {
                return Ok(());
            }
        }
        std::thread::sleep(STOP_ROUND_GAP);
    }
    Err(format!(
        "扫描 {MAX_STOP_ROUNDS} 轮之后仍不断有新的 aproxy 进程出现（有别的程序在反复启动它们？）"
    ))
}

/// 第 4 步：清空 `<home>/install/`，保留锁与本次的目标目录。删不掉的跳过，但
/// install.state 必须删掉——留着它，任何 aproxy 命令都会拉起续作，在强制安装之后接着
/// 推进那个旧的安装（把它备好的旧版本换进 bin、把实例滚到它的目标版本）。
fn clear_install_dir(home: &Path, keep_work: &Path) -> Result<(), String> {
    let dir = super::install_dir_in(home);
    let remove = |path: &Path| {
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        if let Err(e) = removed {
            tracing::warn!(path = %path.display(), error = %e, "install --force could not remove a leftover");
        }
    };
    if let Ok(items) = std::fs::read_dir(&dir) {
        for item in items.flatten() {
            let path = item.path();
            let name = item.file_name();
            if name == super::LOCK_FILE {
                continue;
            }
            if name == "force" {
                for sub in std::fs::read_dir(&path).into_iter().flatten().flatten() {
                    if sub.path() != keep_work {
                        remove(&sub.path());
                    }
                }
                continue;
            }
            remove(&path);
        }
    }
    let state = super::state::state_path_in(home);
    if state.exists() {
        return Err(format!(
            "{} 删不掉：留着它，之后的 aproxy 命令会接着推进那个未完成的普通安装",
            state.display()
        ));
    }
    Ok(())
}

/// 第 6 步：用新位置的二进制执行 `restore`（继承本进程的环境与输出，有上限），然后按
/// 台账核对被结束的实例有没有回来：同一个端口上出现了 pid 不同、仍在运行的 instance
/// 条目才算。返回没回来的那些。
fn restore_and_check(home: &Path, stopped: &[LedgerEntry]) -> Vec<LedgerEntry> {
    let killed: Vec<&LedgerEntry> = stopped.iter().filter(|e| e.role == ROLE_INSTANCE).collect();
    let bin = swap::bin_path_in(home);
    println!("用新版本恢复实例（{} restore）……", bin.display());
    let wait = RESTORE_WAIT_BASE + RESTORE_WAIT_PER_INSTANCE * killed.len() as u32;
    match std::process::Command::new(&bin)
        .arg("restore")
        .stdin(std::process::Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            let deadline = Instant::now() + wait;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(100))
                    }
                    _ => {
                        eprintln!(
                            "[警告] restore 在 {} 秒内没有结束，已终止它",
                            wait.as_secs()
                        );
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                }
            }
        }
        Err(e) => eprintln!("[ERROR] 无法执行 {} restore: {e}", bin.display()),
    }
    if killed.is_empty() {
        return Vec::new();
    }
    let deadline = Instant::now() + RETURN_WAIT;
    loop {
        let back: Vec<LedgerEntry> = ledger::read_all_in(home)
            .into_iter()
            .map(|found| found.entry)
            .filter(|e| e.role == ROLE_INSTANCE && ledger::is_alive(e))
            .collect();
        let missing: Vec<LedgerEntry> = killed
            .iter()
            .filter(|k| {
                !k.port
                    .is_some_and(|port| back.iter().any(|b| b.port == Some(port) && b.pid != k.pid))
            })
            .map(|k| (*k).clone())
            .collect();
        if missing.is_empty() || Instant::now() >= deadline {
            return missing;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// 从 `dir` 里（或其下）的文件运行的进程（pid，镜像路径）：枚举全部进程（见
/// [`process_ids`]），逐个查镜像路径（`watchdog::process_image_path`），用
/// [`super::file_is_within`] 比较所在目录——镜像路径保留启动时的写法（例如 8.3 短名），
/// 要解析成真实路径再比。查不到镜像路径的进程（别的用户的、刚退出的）跳过；macOS 上
/// 镜像路径查不到，结果恒为空。只供第 7 步报告，不参与结束任何进程——结束只认台账。
pub fn processes_running_from(dir: &Path) -> Vec<(u32, PathBuf)> {
    process_ids()
        .into_iter()
        .filter_map(|pid| {
            let image = crate::watchdog::process_image_path(pid)?;
            // Linux：文件被 rename 换掉之后，还在运行的旧镜像显示为「<路径> (deleted)」
            let text = image.to_string_lossy();
            let image = PathBuf::from(text.strip_suffix(" (deleted)").unwrap_or(&text));
            super::file_is_within(&image, dir).then_some((pid, image))
        })
        .collect()
}

/// 全部进程的 pid。Windows：Toolhelp 快照。
#[cfg(windows)]
fn process_ids() -> Vec<u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    // PROCESSENTRY32W 是纯数据结构体，全零是合法的初值；dwSize 必须先填好
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut pids = Vec::new();
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) };
    while more != 0 {
        pids.push(entry.th32ProcessID);
        more = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    unsafe {
        CloseHandle(snapshot);
    }
    pids
}

/// 全部进程的 pid。Linux：`/proc` 下的数字目录。
#[cfg(target_os = "linux")]
fn process_ids() -> Vec<u32> {
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|item| item.file_name().to_str()?.parse().ok())
        .collect()
}

/// 全部进程的 pid。其余平台不枚举（`watchdog::process_image_path` 在这些平台上也查不到
/// 镜像路径），第 7 步的报告因此为空。
#[cfg(not(any(windows, target_os = "linux")))]
fn process_ids() -> Vec<u32> {
    Vec::new()
}

/// 第 7 步：报告不在台账里、却从 `<home>/bin/` 运行的进程（例如 0.1.x 留下的——0.1.x
/// 不写台账）。只报告，不结束：结束进程只认台账里核验过的条目。
fn report_strays(home: &Path) {
    let me = std::process::id();
    let known: Vec<u32> = ledger::read_all_in(home)
        .into_iter()
        .filter(|found| ledger::is_alive(&found.entry))
        .map(|found| found.entry.pid)
        .collect();
    let bin_dir = swap::bin_dir_in(home);
    let strays: Vec<(u32, PathBuf)> = processes_running_from(&bin_dir)
        .into_iter()
        .filter(|(pid, _)| *pid != me && !known.contains(pid))
        .collect();
    if strays.is_empty() {
        return;
    }
    println!(
        "注意：下面这些进程从 {} 运行，但不在台账里（多半是 0.1.x 留下的，0.1.x 不写台账），强制安装没有结束它们：",
        bin_dir.display()
    );
    for (pid, image) in &strays {
        println!("  pid {pid}  {}", image.display());
    }
    println!("  确认它们属于这个 home 之后，用当初启动它们的那个 aproxy 版本执行 stop 停掉。");
}

/// 第 2 步：问一句。标准输入不是终端时不卡着等输入，直接返回 Err（退出码 10）。
fn confirm() -> Result<bool, String> {
    use std::io::{BufRead, IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return Err(
            "需要确认才能继续，但标准输入不是终端（非交互环境）。先向用户确认可以中断上面列出的进程与经它们转发的在途请求，再加 --yes 重新执行。"
                .to_string(),
        );
    }
    print!("确认强制安装？输入 y 继续 [y/N]: ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(|e| format!("读取确认失败: {e}"))?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes" | "YES"))
}

/// 强制安装的输入（命令行归一之后）。
#[derive(Debug, Clone)]
pub struct ForceRequest {
    pub source: ForceSource,
    /// `--variant`：None = 与正在运行的这个二进制相同
    pub variant: Option<String>,
    /// `--cpu`：None = 按本机 CPU（见 `download::prefers_x86_64_v3`）
    pub cpu: Option<String>,
    /// `--pre`：latest 走预发布通道（通道规则与普通安装相同）
    pub pre: bool,
    /// `--yes`：不再询问确认
    pub yes: bool,
    /// `--download-proxy`：只用命令行给的；强制安装不读 settings.json 的 download_proxy
    pub download_proxy: Option<String>,
}

/// 目标从哪里来。
#[derive(Debug, Clone)]
pub enum ForceSource {
    /// 在线（GitHub Release 的发布清单）：版本号，None 或 "latest" = 通道内最新
    Online(Option<String>),
    /// `--from <文件>`
    File(PathBuf),
}

/// 执行一次强制安装，返回退出码（[`EXIT_DONE`]、[`EXIT_UNCHANGED`]、[`EXIT_STOPPED`]、
/// [`EXIT_NOT_RESTORED`]）。调用方先把本进程以 installer 登记进台账（第 3 步结束安装者
/// 时跳过自己的 pid；两个强制安装并发时，后来的会结束先来的）。
pub async fn run(home: &Path, request: &ForceRequest) -> i32 {
    let variant = request.variant.clone().unwrap_or_else(|| {
        VersionLine::own()
            .variant
            .expect("VERSION_LINE 恒带变体段（lib.rs 的单元测试钉死）")
    });
    // 本次的工作目录按 pid 命名：同一时刻一个 pid 只属于一个进程；同名目录只可能是
    // 早先一个同 pid、已经死掉的强制安装留下的，里面的东西直接覆盖
    let work = super::install_dir_in(home)
        .join("force")
        .join(std::process::id().to_string());
    let dest = work.join(super::staging::binary_name());
    // 用完丢掉工作目录。force/ 与 install/ 因此变空时（多半是本次才建的）一并删掉：退出码
    // 10 说「什么都没动」，就连空目录也不留；install/ 里还有别的东西（安装锁、进行中的
    // 普通安装）时 remove_dir 失败，什么都不删
    let discard_work = || {
        let _ = std::fs::remove_dir_all(&work);
        if let Some(force_dir) = work.parent() {
            let _ = std::fs::remove_dir(force_dir);
            let _ = std::fs::remove_dir(super::install_dir_in(home));
        }
    };

    // ---- 第 1 步：取得目标、核对。失败 = 什么都没动
    let acquired = match std::fs::create_dir_all(&work) {
        Err(e) => Err(format!("{} 创建失败: {e}", work.display())),
        Ok(()) => match &request.source {
            ForceSource::File(from) => acquire_file(from, request.variant.as_deref(), &dest),
            ForceSource::Online(version) => {
                acquire_from_github(request, version.as_deref(), &variant, &dest).await
            }
        },
    };
    let target = match acquired {
        Ok(target) => target,
        Err(e) => {
            eprintln!("[ERROR] {e}");
            if variant == VARIANT_DESKTOP && cfg!(target_os = "linux") {
                eprintln!(
                    "带桌面界面的版本需要 WebKitGTK 等系统库；本机缺这些库时，加 --variant headless 改装无界面版本。"
                );
            }
            if matches!(request.source, ForceSource::Online(_)) {
                eprintln!(
                    "连不上 GitHub 时可加 --download-proxy <URL>，或手动下载二进制后用 --from <文件> 安装。"
                );
            }
            eprintln!("什么都没有改动（退出码 {EXIT_UNCHANGED}）。");
            discard_work();
            return EXIT_UNCHANGED;
        }
    };
    println!(
        "目标：aproxy v{}（{}；{}{}），来自{}",
        target.line.version,
        target.line.variant.as_deref().unwrap_or("未知变体"),
        target.line.target.as_deref().unwrap_or("未知目标"),
        if target.line.x86_64_v3 {
            "；x86-64-v3"
        } else {
            ""
        },
        target.origin
    );

    // ---- 第 2 步：确认
    let me = std::process::id();
    let live: Vec<LedgerEntry> = ledger::read_all_in(home)
        .into_iter()
        .map(|found| found.entry)
        .filter(|e| e.pid != me && ledger::is_alive(e))
        .collect();
    if live.is_empty() {
        println!(
            "台账里没有正在运行的 aproxy 进程（home：{}）。",
            home.display()
        );
    } else {
        println!(
            "将结束这个 home（{}）下台账里的全部 aproxy 进程：",
            home.display()
        );
        for entry in &live {
            println!("  {}", describe(entry));
        }
    }
    println!(
        "这会立即中断经这些实例转发的全部在途请求——如果你（或正在执行本命令的 agent 会话）正经 aproxy 访问模型，这次请求也会断开。换好二进制之后会用新版本按原参数恢复实例。"
    );
    if !request.yes {
        match confirm() {
            Ok(true) => {}
            Ok(false) => {
                println!("已取消，什么都没有改动（退出码 {EXIT_UNCHANGED}）。");
                discard_work();
                return EXIT_UNCHANGED;
            }
            Err(e) => {
                eprintln!("[ERROR] {e}");
                discard_work();
                return EXIT_UNCHANGED;
            }
        }
    }

    // ---- 第 3–7 步
    let code = replace(home, &target, &work);
    discard_work();
    code
}

/// 在线来源：组装下载客户端（代理只用命令行给的），按通道与本机挑资产。目标三元组取
/// 正在运行的这个二进制的（构建时的 TARGET，也就是它 `--version` 的目标段）。
async fn acquire_from_github(
    request: &ForceRequest,
    version: Option<&str>,
    variant: &str,
    dest: &Path,
) -> Result<Acquired, String> {
    let version = match version {
        None | Some("latest") => None,
        Some(v) => Some(v.to_string()),
    };
    let ctx = DownloadCtx::build(
        env!("CARGO_PKG_VERSION"),
        request.download_proxy.as_deref(),
        None,
    )?;
    let current =
        semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("内置版本号必为合法 semver");
    let spec = OnlineSpec {
        version,
        channel: Channel::resolve(request.pre, &current),
        target: env!("APROXY_BUILD_TARGET").to_string(),
        variant: variant.to_string(),
        cpu: request.cpu.clone(),
        prefer_v3: super::download::prefers_x86_64_v3(),
    };
    match &spec.version {
        Some(v) => println!("从 GitHub 取 v{v} 的发布清单……"),
        None => println!(
            "查询 {} 通道最新的、带发布清单的版本……",
            spec.channel.label()
        ),
    }
    acquire_online(&ctx.client, &github_api(), &spec, dest).await
}

/// 第 3–7 步。从这里起出错就不再是「什么都没动」：二进制换好之前的失败是 11，换好之后
/// 有实例没回来是 12。
fn replace(home: &Path, target: &Acquired, work: &Path) -> i32 {
    let mut stopped = Vec::new();
    let fail_stopped = |stopped: &[LedgerEntry], e: &str| {
        eprintln!("[ERROR] {e}");
        if stopped.is_empty() {
            eprintln!("bin 里的二进制没有换（退出码 {EXIT_STOPPED}）。");
        } else {
            eprintln!(
                "已经结束了上面列出的进程，但 bin 里的二进制没有换（退出码 {EXIT_STOPPED}）。"
            );
        }
        eprintln!(
            "排除原因后再执行一次同一条命令即可（每一步都可重入）。暂时不再重试的话，`aproxy restore` 会用原来的二进制把被结束的实例拉回来。"
        );
        EXIT_STOPPED
    };

    // ---- 第 3 步：先结束安装者（它们可能持有安装锁、正在改 bin/），再拿锁，再结束其余
    println!("结束 aproxy 进程……");
    if let Err(e) = stop_by_ledger(home, true, &mut stopped) {
        return fail_stopped(&stopped, &e);
    }
    let lock = match super::lock_install_in(home, LOCK_WAIT) {
        Ok(lock) => lock,
        Err(e) => return fail_stopped(&stopped, &e),
    };
    if let Err(e) = stop_by_ledger(home, false, &mut stopped) {
        return fail_stopped(&stopped, &e);
    }

    // ---- 第 4 步
    if let Err(e) = clear_install_dir(home, work) {
        return fail_stopped(&stopped, &e);
    }

    // ---- 第 5 步：换二进制。落位后在新位置核对版本，不符就换回原来的
    let outcome = match swap::swap_in(home, &target.path, &target.line.version, None, |_| Ok(())) {
        Ok(outcome) => outcome,
        Err(e) => return fail_stopped(&stopped, &format!("二进制交换失败: {e}")),
    };
    println!(
        "二进制已换好：{}（v{}）",
        swap::bin_path_in(home).display(),
        target.line.version
    );
    if let Some(old) = &outcome.old_path {
        println!("  原来的二进制让位到 {}（下次安装时清理）", old.display());
    }

    // ---- 第 6 步
    let missing = restore_and_check(home, &stopped);

    // ---- 第 7 步
    report_strays(home);
    drop(lock);

    if stopped.iter().any(|e| e.role == ROLE_WATCHDOG) {
        println!(
            "看护者已被结束：实例在 5 分钟内的自检里会用新版本把它拉起来，下次 aproxy start 也会。"
        );
    }
    if stopped.iter().any(|e| e.role == ROLE_DESKTOP) {
        println!("桌面界面已被结束，需要时重新打开。");
    }
    println!(
        "skill 文档没有更新（强制安装不碰它们）：之后执行 `aproxy install --skills-only` 更新到与新版本一致。"
    );
    if missing.is_empty() {
        println!("强制安装完成（退出码 {EXIT_DONE}）。");
        return EXIT_DONE;
    }
    eprintln!(
        "[ERROR] 二进制已换好，但下面这些被结束的实例没有回来（退出码 {EXIT_NOT_RESTORED}），请手动启动（aproxy start <别名|配置文件路径>）："
    );
    for entry in &missing {
        match (entry.port, &entry.label) {
            (Some(port), Some(label)) => eprintln!("  端口 {port}（{label}）"),
            (Some(port), None) => eprintln!("  端口 {port}"),
            (None, Some(label)) => eprintln!("  {label}（pid {}）", entry.pid),
            (None, None) => eprintln!("  pid {}", entry.pid),
        }
    }
    eprintln!(
        "没回来的原因多半记在 {} 里。",
        home.join("logs").join("startup.log").display()
    );
    EXIT_NOT_RESTORED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_version_line_parses_with_variant_and_target() {
        let own = VersionLine::own();
        assert_eq!(own.version, env!("CARGO_PKG_VERSION"));
        assert!(matches!(
            own.variant.as_deref(),
            Some(VARIANT_DESKTOP) | Some(VARIANT_HEADLESS)
        ));
        assert_eq!(own.target.as_deref(), Some(env!("APROXY_BUILD_TARGET")));
    }

    #[test]
    fn manifest_selection_prefers_v3_then_falls_back_to_baseline() {
        let asset = |target: &str, variant: &str, cpu: &str, file: &str| ManifestAsset {
            target: target.into(),
            variant: variant.into(),
            cpu: cpu.into(),
            file: file.into(),
            sha256: "00".into(),
        };
        let manifest = Manifest {
            manifest: 1,
            version: "0.2.0".into(),
            assets: vec![
                asset("t", VARIANT_DESKTOP, CPU_BASELINE, "d-base"),
                asset("t", VARIANT_DESKTOP, CPU_X86_64_V3, "d-v3"),
                asset("t", VARIANT_HEADLESS, CPU_BASELINE, "h-base"),
                asset("t", VARIANT_HEADLESS, "x86-64-v9", "h-future"),
            ],
        };
        let pick =
            |variant: &str, v3: bool| manifest.select("t", variant, v3).map(|a| a.file.as_str());
        assert_eq!(pick(VARIANT_DESKTOP, true), Some("d-v3"));
        assert_eq!(pick(VARIANT_DESKTOP, false), Some("d-base"));
        // 没有 v3 档就退回基线档；不认识的 cpu 取值不参与
        assert_eq!(pick(VARIANT_HEADLESS, true), Some("h-base"));
        assert_eq!(manifest.select("other", VARIANT_DESKTOP, true), None);
        // 精确查找不退回别的档
        assert_eq!(manifest.find("t", VARIANT_HEADLESS, CPU_X86_64_V3), None);
        assert_eq!(
            manifest
                .find("t", VARIANT_DESKTOP, CPU_X86_64_V3)
                .map(|a| a.file.as_str()),
            Some("d-v3")
        );
    }

    #[test]
    fn a_missing_combination_lists_what_the_target_offers() {
        let asset = |target: &str, variant: &str, cpu: &str| ManifestAsset {
            target: target.into(),
            variant: variant.into(),
            cpu: cpu.into(),
            file: format!("{target}-{variant}-{cpu}"),
            sha256: "00".into(),
        };
        let gnu = "x86_64-unknown-linux-gnu";
        let musl = "x86_64-unknown-linux-musl";
        let manifest = Manifest {
            manifest: 1,
            version: "0.2.0".into(),
            assets: vec![
                asset(gnu, VARIANT_DESKTOP, CPU_BASELINE),
                asset(gnu, VARIANT_DESKTOP, CPU_X86_64_V3),
                asset(gnu, VARIANT_HEADLESS, CPU_BASELINE),
                asset(musl, VARIANT_HEADLESS, CPU_BASELINE),
            ],
        };
        assert_eq!(
            manifest.offered(gnu).as_deref(),
            Some("desktop（baseline、x86-64-v3）；headless（baseline）")
        );
        assert_eq!(
            manifest.missing(musl, VARIANT_DESKTOP, None),
            "release v0.2.0 的发布清单里没有 x86_64-unknown-linux-musl 的 desktop 版本，这个目标只有 headless（baseline）"
        );
        assert_eq!(
            manifest.missing(gnu, VARIANT_HEADLESS, Some(CPU_X86_64_V3)),
            "release v0.2.0 的发布清单里没有 x86_64-unknown-linux-gnu 的 headless（x86-64-v3）版本，这个目标只有 desktop（baseline、x86-64-v3）；headless（baseline）"
        );
        assert_eq!(
            manifest.missing("riscv64gc-unknown-linux-gnu", VARIANT_HEADLESS, None),
            "release v0.2.0 的发布清单里没有 riscv64gc-unknown-linux-gnu 的资产（清单里有的目标：x86_64-unknown-linux-gnu、x86_64-unknown-linux-musl）"
        );
    }
}
