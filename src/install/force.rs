//! 强制安装 `aproxy install --force`：永久兼容契约 C6–C8 的执行体。其余几项各在
//! 一处：台账 C3 在 `crate::ledger`，唯一中间名 C2 在 `super::swap`，安装锁 C4 在
//! `super::lock_install_in`，`--version` 的格式 C5 在 `crate::VERSION_LINE`。设计全文见
//! `.agents/plan/release-0.2.0.md` 的「W2b 强制安装：永久兼容契约」一节。
//!
//! **永久兼容契约：本文件里标明冻结的东西——发布清单的资产名与 JSON 格式、退出码 10/11/12、
//! 命令行写法、流程的先后顺序——无论如何都不得改动，只许追加可选字段。** 写输出失败（例如
//! 管道已关闭）不得改变退出码，所以这里的一切输出都经 `outln!`、`errln!`。
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
//! 1. home 不存在 → 10（多半是 APROXY_HOME 写错了，首次安装用一键脚本，见
//!    [`locate_home`]）。清掉 pid 已经不在的旧 `install/force/<pid>/`。取得目标（发布清单
//!    C6 或 `--from`），校验 sha256，在本机执行 `--version` 核对；失败 → 退出码 10，什么
//!    都没动。
//! 2. 确认（提示写到标准错误）：会中断所有经 aproxy 的在途请求，包括 agent 自己经 aproxy
//!    的会话。`--yes` 跳过；标准输入不是终端又没有 `--yes` → 10，不卡着等输入。
//! 3. 按台账结束安装者（本进程除外）→ 拿安装锁 → 结束看护者、实例、桌面界面与不认识
//!    的角色；反复扫描直到连续两轮没有存活条目（有上限）。结束前核验启动标记与属主（只
//!    结束当前用户的进程，见 `ledger::terminate`）。从这一步起，二进制换好之前的任何失败
//!    按实际发生的事给退出码：一个进程都没结束是 10，否则 11。
//! 4. 先删 `install/state.json`，再清空 `<home>/install/`（保留锁与本次的工作目录；unix
//!    的安装宣告也在这里，随之清掉）：留下的状态文件会让续作在强制安装之后接着推进一个
//!    旧的安装，过期的宣告会让看护者反复拉起续作。
//! 5. 换 `bin/` 里的二进制（先尽力清掉删得掉的旧残留；唯一中间名，见 `super::swap`），
//!    落位后在新位置再跑一次 `--version` 核对（有时限），不符就换回原来的。
//! 6. 尽力调用新版本的 `restore`。被强制结束的实例留下了 `.restore` 记录，restore 正是
//!    凭它把实例拉回来，所以强制安装不删 `.restore`（`stop --force` 会删，两者不同）。
//!    是否拉回只按台账判断：被结束实例的端口上出现了新的、版本等于目标版本的 instance
//!    条目才算；没回来的按端口与配置文件（条目的 `config`）列出怎样启动 → 12。
//! 7. 报告不在台账里、却从 `<home>/bin/` 运行的进程（例如 0.1.x 留下的，但不一定是），
//!    只报告、不结束。macOS 上查不到别的进程的镜像路径，这一步不做。
//!
//! 强制安装不更新 skill 文档，结束时提示怎样更新。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::download::Channel;
use super::swap;
use crate::ledger::{
    self, LedgerEntry, ROLE_DESKTOP, ROLE_INSTALLER, ROLE_INSTANCE, ROLE_WATCHDOG,
};

/// 写一行到标准输出，写失败（管道已关闭、终端已断开）一律忽略。强制安装的一切输出都经它
/// 与 `errln!`，不用 `println!`/`eprintln!`。
///
/// 永久兼容契约 C7：写输出失败不得改变退出码。`println!` 写失败会 panic：退出码变成 101、
/// 跳过清理（留下 `install/force/<pid>/`），接了 `| head` 时 shell 报的甚至是 head 的 0；
/// panic 若发生在第 3 步之后，进程已停、二进制没换、实例没拉回，调用方却拿不到 11。输出
/// 接到提前退出的读者是常态而不是意外：`| head`、agent 的工具超时后只杀了 shell、将来
/// 桌面界面读着输出就按规格先退出了。
macro_rules! outln {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

/// 写一行到标准错误，写失败一律忽略。理由同 `outln!`。
macro_rules! errln {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

/// 退出码 0：完成——二进制已换好，被结束的实例都已按台账确认回来。
///
/// 永久兼容契约 C7：无论如何都不得改动（理由见文件头）。
pub const EXIT_DONE: i32 = 0;

/// 退出码 10：什么都没动——一个进程都没结束。在第 1、2 步中止（home 不存在、目标取不到、
/// 校验或 `--version` 不过、版本低于 0.2.0、没有得到确认），或者第 3 步起失败时还没有
/// 结束任何进程（锁等不到、第一个进程就结束不了）。二进制没换，可以排除原因后重来。
///
/// 永久兼容契约 C7：无论如何都不得改动。通用退出码只用 0–5（2 是命令行用法错误），
/// 10 起留给强制安装，互不重叠。
pub const EXIT_UNCHANGED: i32 = 10;

/// 退出码 11：至少结束了一个进程，但 `bin/` 里的二进制没有换成。被结束的进程不会自己
/// 回来；再执行一次同一条命令即可（每一步都可重入），放弃的话 `aproxy restore` 会用
/// 原来的二进制把实例拉回来。
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
/// 下载时两次收到数据之间的最长间隔：超过就算这条连接卡死了。按空闲而不是按总长算：
/// 慢链路上下载带界面的版本可能要好几分钟，只要数据还在来就不该放弃。
const DOWNLOAD_IDLE: Duration = Duration::from_secs(60);
/// 一次请求（含读完整个资产）的总时限，防的是数据一直在来、却慢到没有尽头。
const DOWNLOAD_TOTAL: Duration = Duration::from_secs(30 * 60);
/// 连接的时限。
const DOWNLOAD_CONNECT: Duration = Duration::from_secs(15);
/// 发布清单的大小上限（C6 读者）：一份清单只有几十条资产，几 KB；超过就不是清单。
const MANIFEST_LIMIT: u64 = 1024 * 1024;
/// 资产的大小上限（C6 读者）：一个 aproxy 二进制几十 MB，留足余量；超过就不是它。
const ASSET_LIMIT: u64 = 256 * 1024 * 1024;
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
/// 必须是合法 semver），两条路认版本的口径因此不会分叉。
///
/// 读者容错（C5）：只取版本号之后的第一个括号组，`)` 之后的任何内容都不看；组内按 `;`
/// 分段、去掉两端空白。写者在 `)` 之后不得追加任何内容——这里的容忍是容错，不是许可：
/// 冻结的读者过严，以后哪个版本在 `)` 后多写一点东西，所有旧版本就都认不出它的变体，
/// 在线路径与 `--variant` 核对都会判成「变体不符」而退出 10。没有括号组时变体与目标为
/// None——只靠版本也认得出它。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionLine {
    pub version: String,
    pub variant: Option<String>,
    pub target: Option<String>,
    /// 括号组的第三段起有 `x86-64-v3`
    pub x86_64_v3: bool,
}

impl VersionLine {
    pub fn parse(output: &str) -> Option<Self> {
        let version = super::staging::parse_version_line(output)?;
        // 版本是首行 `aproxy v` 之后的第一段（semver 只含 ASCII，按字节长度切不会切坏字符）
        let tail = &output.lines().next()?["aproxy v".len() + version.len()..];
        let mut line = Self {
            version,
            variant: None,
            target: None,
            x86_64_v3: false,
        };
        let group = tail
            .split_once('(')
            .and_then(|(_, rest)| rest.split_once(')'))
            .map(|(inner, _)| inner);
        if let Some(inner) = group {
            let mut parts = inner.split(';').map(str::trim);
            let mut next_word = || parts.next().filter(|p| !p.is_empty()).map(str::to_string);
            line.variant = next_word();
            line.target = next_word();
            line.x86_64_v3 = parts.any(|p| p == CPU_X86_64_V3);
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
/// 永久兼容契约 C6：字段的名字、类型、含义无论如何都不得改动，只许追加可选字段。
///
/// 写者：不带 BOM 的 UTF-8；`version` 等于 tag 去掉 `v`；`file` 永远是同一 release 里的裸
/// 可执行文件（不是压缩包），每个 release 都为它发布的每个目标保留旧版本能运行的裸二进制
/// 条目；assets 的每个元素都带齐五个字段。
///
/// 读者（这里）比写者宽：忽略不认识的字段；容忍开头的 BOM；assets 逐个元素解析，缺字段、
/// 类型不对、不是对象的元素跳过；不认识的 `variant`、`cpu` 取值不参与选择。宽是为了
/// 「只增」真能成立——以后清单里加一种非二进制的资产、换一种写法，旧版本的强制安装照样
/// 读得懂其余部分，只是选不到新东西；读者严一点，所有旧版本就会因为一个元素读不了整份清单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// 恒为 [`MANIFEST_FORMAT`]
    pub manifest: u32,
    /// 这份清单所属的版本（不带 v），必须等于 release 的 tag 去掉 v
    pub version: String,
    /// 读得懂的资产；读不懂的元素在解析时已跳过（见 [`assets_skipping_bad`]）
    #[serde(deserialize_with = "assets_skipping_bad")]
    pub assets: Vec<ManifestAsset>,
}

/// assets 先按任意 JSON 值的数组读，再逐个元素解析成 [`ManifestAsset`]，解析不了的跳过。
/// assets 本身不是数组时整份清单不成立。
fn assets_skipping_bad<'de, D>(deserializer: D) -> Result<Vec<ManifestAsset>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Vec<serde_json::Value> = Vec::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .filter_map(|item| serde_json::from_value(item).ok())
        .collect())
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
    /// 解析清单文本；开头的 UTF-8 BOM 先去掉（读者容错，见上）。
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
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

/// 在本机执行 `bin --version`（有时限，见 `staging::run_version`），按 C5 拆开首行。跑不
/// 起来、超时、输出不是永久格式都算失败，错误里带上它的标准错误。
pub fn probe(bin: &Path) -> Result<VersionLine, String> {
    let stdout = super::staging::run_version(bin)?;
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
///
/// 在线路径依赖 GitHub API 的形状与仓库名，是尽力而为；永久保证的是 `--from`（C6）：
/// 哪天 API 变了、仓库搬了，手动下载二进制再 `--from` 照样能装。
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
    let too_big = || format!("{MANIFEST_ASSET} 超过 {} KiB 的上限", MANIFEST_LIMIT >> 10);
    let mut response = get(client, url, false).await?;
    if response
        .content_length()
        .is_some_and(|n| n > MANIFEST_LIMIT)
    {
        return Err(too_big());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("{MANIFEST_ASSET} 下载失败: {e}"))?
    {
        body.extend_from_slice(&chunk);
        if body.len() as u64 > MANIFEST_LIMIT {
            return Err(too_big());
        }
    }
    let text = String::from_utf8(body).map_err(|_| format!("{MANIFEST_ASSET} 不是 UTF-8"))?;
    let manifest = Manifest::parse(&text)?;
    if manifest.version != release.version {
        return Err(format!(
            "release v{} 的 {MANIFEST_ASSET} 写的版本是 {}，两者不符",
            release.version, manifest.version
        ));
    }
    Ok(manifest)
}

/// 下载清单里的一个资产到 `dest`（超过 [`ASSET_LIMIT`] 就放弃），比对清单写的 sha256。
/// 不符就删掉并报错。
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
    let streamed = async {
        use std::io::Write;
        let too_big = || format!("超过 {} MiB 的上限", ASSET_LIMIT >> 20);
        let mut response = get(client, url, false).await?;
        if response.content_length().is_some_and(|n| n > ASSET_LIMIT) {
            return Err(too_big());
        }
        let mut file =
            std::fs::File::create(dest).map_err(|e| format!("{} 创建失败: {e}", dest.display()))?;
        let mut total = 0u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("下载中断: {e}"))?
        {
            total += chunk.len() as u64;
            if total > ASSET_LIMIT {
                return Err(too_big());
            }
            file.write_all(&chunk)
                .map_err(|e| format!("写入失败: {e}"))?;
        }
        file.flush().map_err(|e| format!("写入失败: {e}"))
    }
    .await;
    if let Err(e) = streamed {
        let _ = std::fs::remove_file(dest);
        return Err(format!("{} 下载失败: {e}", asset.file));
    }
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
    /// 正在运行的这个二进制的版本：`latest` 解析出更低的版本时不装（见 [`acquire_online`]）
    pub running: String,
}

/// 在线取目标：找 release → 读发布清单 → 选资产 → 下载并比对 sha256 → 在本机执行
/// `--version`，核对版本与变体。
///
/// `latest`（没有指定版本）解析出的版本低于正在运行的版本时不装，报错（退出码 10）：
/// latest 只认带发布清单的 release，新版本若漏传了清单，latest 就会落到更旧的版本上，
/// `--force --yes` 会悄悄降级。明确写出版本号的不受影响——那是用户自己选的。
///
/// linux-gnu 的产物跑不起来时（它按构建机的 glibc 链接，
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
    if spec.version.is_none() {
        let picked = semver::Version::parse(&release.version);
        let running = semver::Version::parse(&spec.running);
        if let (Ok(picked), Ok(running)) = (picked, running)
            && picked < running
        {
            return Err(format!(
                "{} 通道里最新的、带发布清单的版本是 v{picked}，低于正在运行的 v{running}（新版本的发布可能漏了 {MANIFEST_ASSET}）。强制安装不会悄悄降级；确实要装 v{picked}，就写明版本号：aproxy install --force {picked}",
                spec.channel.label()
            ));
        }
    }
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

/// 第 3 步的进度：结束了哪些、因属主不同跳过了哪些（两次扫描共用）。
#[derive(Default)]
struct Stops {
    stopped: Vec<LedgerEntry>,
    /// 属主不是当前用户的进程：不结束、条目不删，每个只报告一次
    skipped: Vec<LedgerEntry>,
}

/// 按台账结束进程，本进程除外：`only_installers` 时只结束安装者（第 3 步拿锁之前），
/// 否则全部角色，先后见 `LedgerEntry::stop_order`（不认识的角色排在最后）。条目对应的
/// 进程已经不在就删掉条目；结束之后也删掉——删之前都重读一遍，确认还是那一条
/// （`ledger::remove_if_unchanged`）。属主不是当前用户的跳过并报告。反复扫描，直到连续
/// 两轮没有新结束的进程：结束看护者与实例的同时，别的进程可能正在拉起新的（实例的
/// 自检补种看护者、正在启动的实例），它们都在占端口或拉子进程之前登记，下一轮就会被
/// 看到。
fn stop_by_ledger(home: &Path, only_installers: bool, stops: &mut Stops) -> Result<(), String> {
    let me = std::process::id();
    let mut quiet_rounds = 0;
    for _ in 0..MAX_STOP_ROUNDS {
        let mut stopped_any = false;
        for found in ledger::read_all_in(home) {
            let entry = &found.entry;
            let skipped = stops
                .skipped
                .iter()
                .any(|s| s.pid == entry.pid && s.start == entry.start);
            if entry.pid == me || skipped || (only_installers && entry.role != ROLE_INSTALLER) {
                continue;
            }
            match ledger::terminate(entry, KILL_WAIT) {
                ledger::Termination::Terminated => {
                    outln!("  已结束 {}", describe(entry));
                    ledger::remove_if_unchanged(&found);
                    stopped_any = true;
                    stops.stopped.push(entry.clone());
                }
                ledger::Termination::AlreadyGone => {
                    ledger::remove_if_unchanged(&found);
                }
                ledger::Termination::NotOwned(e) => {
                    outln!("  跳过 {}：{e}（只结束当前用户的进程）", describe(entry));
                    stops.skipped.push(entry.clone());
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

/// 第 4 步：先删 install/state.json，再清空 `<home>/install/`（保留锁与本次的目标
/// 目录）。状态文件必须删掉——留着它，任何 aproxy 命令都会拉起续作，在强制安装之后接着
/// 推进那个旧的安装（把它备好的旧版本换进 bin、把实例滚到它的目标版本）；先删它，是
/// 因为清空到一半被打断时，最要紧的这一件已经做完。其余删不掉的跳过。
fn clear_install_dir(home: &Path, keep_work: &Path) -> Result<(), String> {
    let state = super::state::state_path_in(home);
    if let Err(e) = std::fs::remove_file(&state)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        return Err(format!(
            "{} 删不掉（{e}）：留着它，之后的 aproxy 命令会接着推进那个未完成的普通安装",
            state.display()
        ));
    }
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
    Ok(())
}

/// 第 1 步之前：清掉 pid 已经不在的旧 `install/force/<pid>/`——在第 1、2 步被打断
/// （Ctrl+C、确认时选了否之外的退出）的强制安装留下的已下载二进制。pid 还在的不动：
/// 那可能是另一个正在进行的强制安装（它会在第 3 步被本次结束，工作目录在第 4 步清掉）。
fn clear_stale_work_dirs(home: &Path) {
    let force_dir = super::install_dir_in(home).join("force");
    for item in std::fs::read_dir(&force_dir)
        .into_iter()
        .flatten()
        .flatten()
    {
        let gone = item
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
            .is_some_and(|pid| crate::watchdog::process_start_time(pid).is_none());
        if gone {
            let _ = std::fs::remove_dir_all(item.path());
        }
    }
}

/// 解析并确认 home（永久兼容契约 C1）：APROXY_HOME 不是绝对路径、或 home 不存在 → Err，
/// 调用方以 10 结束（什么都没动）。强制安装只修已经装着的 home：home 不存在多半是
/// APROXY_HOME 写错了，照样装下去只会在一个新目录里「成功」，真正要修的那个原封不动。
pub fn locate_home() -> Result<PathBuf, String> {
    let home = crate::settings::resolve_home()?;
    if !home.is_dir() {
        return Err(format!(
            "aproxy 的主目录 {} 不存在：强制安装只修已经装着的 aproxy，多半是 APROXY_HOME 写错了；首次安装请用一键安装脚本。",
            home.display()
        ));
    }
    Ok(home)
}

/// 子进程的一路输出：子进程写进工作目录里的一个文件，本进程边等边读出新增的部分，转写到
/// 自己的标准输出或标准错误（写失败忽略）。
///
/// 不让子进程直接继承本进程的输出：输出端已关闭时，子进程自己会因写失败而 panic——
/// restore 死在半路，实例就拉不回来，写输出失败因此改变了结果（违反 C7）。也不用管道：
/// 读管道要等写端全部关闭，而写端可能被子进程再拉起的进程继承下去，一直等不到。文件
/// 没有这两个问题，被继承了也只是多一个句柄。
struct Relay {
    reader: std::fs::File,
    to_stderr: bool,
}

impl Relay {
    /// 子进程这一路输出的去处：`path` 处新建的文件，配一个读它的 Relay。文件建不起来就
    /// 丢弃这一路输出（Stdio::null），子进程照样运行。
    fn capture(path: &Path, to_stderr: bool) -> (std::process::Stdio, Option<Self>) {
        let opened =
            std::fs::File::create(path).and_then(|writer| Ok((writer, std::fs::File::open(path)?)));
        match opened {
            Ok((writer, reader)) => (writer.into(), Some(Self { reader, to_stderr })),
            Err(_) => (std::process::Stdio::null(), None),
        }
    }

    /// 把文件里自上次以来新增的内容转写出去。普通文件读到末尾之后，再读会接着读到新写入的
    /// 部分，所以反复调用就能跟上子进程的输出。
    fn pump(&mut self) {
        use std::io::{Read, Write};
        let mut buf = Vec::new();
        let _ = self.reader.read_to_end(&mut buf);
        if buf.is_empty() {
            return;
        }
        let _ = if self.to_stderr {
            std::io::stderr().write_all(&buf)
        } else {
            std::io::stdout().write_all(&buf)
        };
    }
}

/// 被结束的实例 `killed` 有没有回来：`live`（仍在运行的条目）里有同一个端口、pid 不同、
/// 版本等于目标版本的 instance 条目。版本也要对：同一个端口上若是别人用旧二进制拉起的
/// 实例，它不是这次恢复的结果，算「回来了」就会掩盖新版本起不来的问题。
fn came_back(killed: &LedgerEntry, live: &[LedgerEntry], target_version: &str) -> bool {
    killed.port.is_some_and(|port| {
        live.iter().any(|e| {
            e.role == ROLE_INSTANCE
                && e.port == Some(port)
                && e.pid != killed.pid
                && e.version == target_version
        })
    })
}

/// 第 6 步：用新位置的二进制执行 `restore`（继承本进程的环境，输出经 [`Relay`] 转写，有
/// 上限），然后按台账核对被结束的实例有没有回来（[`came_back`]）。返回没回来的那些。
fn restore_and_check(
    home: &Path,
    work: &Path,
    stopped: &[LedgerEntry],
    target_version: &str,
) -> Vec<LedgerEntry> {
    let killed: Vec<&LedgerEntry> = stopped.iter().filter(|e| e.role == ROLE_INSTANCE).collect();
    let bin = swap::bin_path_in(home);
    outln!("用新版本恢复实例（{} restore）……", bin.display());
    let wait = RESTORE_WAIT_BASE + RESTORE_WAIT_PER_INSTANCE * killed.len() as u32;
    let (stdout, out_relay) = Relay::capture(&work.join("restore.out"), false);
    let (stderr, err_relay) = Relay::capture(&work.join("restore.err"), true);
    let mut relays: Vec<Relay> = out_relay.into_iter().chain(err_relay).collect();
    match std::process::Command::new(&bin)
        .arg("restore")
        .stdin(std::process::Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
    {
        Ok(mut child) => {
            let deadline = Instant::now() + wait;
            let finished = loop {
                relays.iter_mut().for_each(Relay::pump);
                match child.try_wait() {
                    Ok(Some(_)) => break true,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(100))
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break false;
                    }
                }
            };
            // 子进程退出之前写下的最后一段
            relays.iter_mut().for_each(Relay::pump);
            if !finished {
                errln!(
                    "[警告] restore 在 {} 秒内没有结束，已终止它",
                    wait.as_secs()
                );
            }
        }
        Err(e) => errln!("[ERROR] 无法执行 {} restore: {e}", bin.display()),
    }
    if killed.is_empty() {
        return Vec::new();
    }
    let deadline = Instant::now() + RETURN_WAIT;
    loop {
        let live: Vec<LedgerEntry> = ledger::read_all_in(home)
            .into_iter()
            .map(|found| found.entry)
            .filter(ledger::is_alive)
            .collect();
        let missing: Vec<LedgerEntry> = killed
            .iter()
            .filter(|k| !came_back(k, &live, target_version))
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

/// 第 7 步：报告不在台账里、却从 `<home>/bin/` 运行的进程。它们可能是 0.1.x 留下的（0.1.x
/// 不写台账），也可能是放在 bin 里的别的程序，所以措辞不下结论。只报告，不结束：结束
/// 进程只认台账里核验过的条目。macOS 上查不到别的进程的镜像路径，结果恒为空。
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
    outln!(
        "注意：下面这些进程从 {} 运行，但不在台账里，强制安装没有结束它们（例如 0.1.x 启动的 aproxy——0.1.x 不写台账；也可能是放在 bin 里的别的程序）：",
        bin_dir.display()
    );
    for (pid, image) in &strays {
        outln!("  pid {pid}  {}", image.display());
    }
    outln!(
        "  先确认它们是什么再处理：0.1.x 的 aproxy 用当初启动它们的 0.1.x 执行 aproxy stop 停掉。"
    );
}

/// 第 2 步：问一句。标准输入不是终端时不卡着等输入，直接返回 Err（退出码 10）。提示写到
/// 标准错误（C8）：标准输出被重定向进文件、标准输入仍是终端时，写到标准输出的提示用户
/// 看不见，命令就像卡住了一样。
fn confirm() -> Result<bool, String> {
    use std::io::{BufRead, IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return Err(
            "需要确认才能继续，但标准输入不是终端（非交互环境）。先向用户确认可以中断上面列出的进程与经它们转发的在途请求，再加 --yes 重新执行。"
                .to_string(),
        );
    }
    let mut stderr = std::io::stderr();
    let _ = write!(stderr, "确认强制安装？输入 y 继续 [y/N]: ");
    let _ = stderr.flush();
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
    clear_stale_work_dirs(home);
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
            errln!("[ERROR] {e}");
            if variant == VARIANT_DESKTOP && cfg!(target_os = "linux") {
                errln!(
                    "带桌面界面的版本需要 WebKitGTK 等系统库；本机缺这些库时，加 --variant headless 改装无界面版本。"
                );
            }
            if matches!(request.source, ForceSource::Online(_)) {
                errln!(
                    "连不上 GitHub 时可加 --download-proxy <URL>，或手动下载二进制后用 --from <文件> 安装。"
                );
            }
            errln!("什么都没有改动（退出码 {EXIT_UNCHANGED}）。");
            discard_work();
            return EXIT_UNCHANGED;
        }
    };
    outln!(
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
        outln!(
            "台账里没有正在运行的 aproxy 进程（home：{}）。",
            home.display()
        );
    } else {
        outln!(
            "将结束这个 home（{}）下台账里的全部 aproxy 进程：",
            home.display()
        );
        for entry in &live {
            outln!("  {}", describe(entry));
        }
    }
    outln!(
        "这会立即中断经这些实例转发的全部在途请求——如果你（或正在执行本命令的 agent 会话）正经 aproxy 访问模型，这次请求也会断开。换好二进制之后会用新版本按原参数恢复实例。"
    );
    if !request.yes {
        match confirm() {
            Ok(true) => {}
            Ok(false) => {
                outln!("已取消，什么都没有改动（退出码 {EXIT_UNCHANGED}）。");
                discard_work();
                return EXIT_UNCHANGED;
            }
            Err(e) => {
                errln!("[ERROR] {e}");
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

/// 强制安装下载用的 HTTP 客户端：只走 HTTPS（重定向也一样，`https_only`）；连接
/// [`DOWNLOAD_CONNECT`]、空闲 [`DOWNLOAD_IDLE`]、总长 [`DOWNLOAD_TOTAL`]；代理只用命令行
/// 给的 `--download-proxy`（外加 reqwest 默认认的代理环境变量），不读 settings.json。
pub fn http_client(download_proxy: Option<&str>) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .user_agent(format!("aproxy-install/{}", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .connect_timeout(DOWNLOAD_CONNECT)
        .read_timeout(DOWNLOAD_IDLE)
        .timeout(DOWNLOAD_TOTAL);
    if let Some(proxy) = download_proxy {
        // 代理 URL 可能内嵌凭据（user:pass@host），错误里不复述它
        let proxy = reqwest::Proxy::all(proxy)
            .map_err(|_| "下载代理 URL 非法（凭据已隐去）".to_string())?;
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(|e| format!("HTTP 客户端构建失败: {e}"))
}

/// 在线来源：组装下载客户端（[`http_client`]），按通道与本机挑资产。目标三元组取正在
/// 运行的这个二进制的（构建时的 TARGET，也就是它 `--version` 的目标段）。
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
    let client = http_client(request.download_proxy.as_deref())?;
    let current =
        semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("内置版本号必为合法 semver");
    let spec = OnlineSpec {
        version,
        channel: Channel::resolve(request.pre, &current),
        target: env!("APROXY_BUILD_TARGET").to_string(),
        variant: variant.to_string(),
        cpu: request.cpu.clone(),
        prefer_v3: super::download::prefers_x86_64_v3(),
        running: env!("CARGO_PKG_VERSION").to_string(),
    };
    match &spec.version {
        Some(v) => outln!("从 GitHub 取 v{v} 的发布清单……"),
        None => outln!(
            "查询 {} 通道最新的、带发布清单的版本……",
            spec.channel.label()
        ),
    }
    acquire_online(&client, &github_api(), &spec, dest).await
}

/// 第 3–7 步。从这里起出错就不再是「什么都没动」：二进制换好之前的失败是 11，换好之后
/// 有实例没回来是 12。
fn replace(home: &Path, target: &Acquired, work: &Path) -> i32 {
    let mut stops = Stops::default();
    // 退出码按实际发生的事给（C7）：一个进程都没结束是 10——即便 install/ 已被清掉一部分，
    // 也没有任何东西需要恢复；结束过进程才是 11，它们不会自己回来
    let fail_stopped = |stopped: &[LedgerEntry], e: &str| {
        errln!("[ERROR] {e}");
        if stopped.is_empty() {
            errln!(
                "一个进程都没有结束，bin 里的二进制也没有换（退出码 {EXIT_UNCHANGED}）。排除原因后再执行一次即可。"
            );
            return EXIT_UNCHANGED;
        }
        errln!("已经结束了上面列出的进程，但 bin 里的二进制没有换（退出码 {EXIT_STOPPED}）。");
        errln!(
            "排除原因后再执行一次同一条命令即可（每一步都可重入）。暂时不再重试的话，`aproxy restore` 会用原来的二进制把被结束的实例拉回来。"
        );
        EXIT_STOPPED
    };

    // ---- 第 3 步：先结束安装者（它们可能持有安装锁、正在改 bin/），再拿锁，再结束其余
    outln!("结束 aproxy 进程……");
    if let Err(e) = stop_by_ledger(home, true, &mut stops) {
        return fail_stopped(&stops.stopped, &e);
    }
    let lock = match super::lock_install_in(home, LOCK_WAIT) {
        Ok(lock) => lock,
        Err(e) => return fail_stopped(&stops.stopped, &e),
    };
    if let Err(e) = stop_by_ledger(home, false, &mut stops) {
        return fail_stopped(&stops.stopped, &e);
    }
    let Stops { stopped, skipped } = stops;

    // ---- 第 4 步
    if let Err(e) = clear_install_dir(home, work) {
        return fail_stopped(&stopped, &e);
    }

    // ---- 第 5 步：换二进制。落位后在新位置核对版本，不符就换回原来的
    let outcome = match swap::swap_in(home, &target.path, &target.line.version, None, |_| Ok(())) {
        Ok(outcome) => outcome,
        Err(e) => return fail_stopped(&stopped, &format!("二进制交换失败: {e}")),
    };
    outln!(
        "二进制已换好：{}（v{}）",
        swap::bin_path_in(home).display(),
        target.line.version
    );
    if let Some(old) = &outcome.old_path {
        outln!("  原来的二进制让位到 {}（下次安装时清理）", old.display());
    }

    // ---- 第 6 步
    let missing = restore_and_check(home, work, &stopped, &target.line.version);

    // ---- 第 7 步
    report_strays(home);
    drop(lock);

    if stopped.iter().any(|e| e.role == ROLE_WATCHDOG) {
        outln!(
            "看护者已被结束：实例在 5 分钟内的自检里会用新版本把它拉起来，下次 aproxy start 也会。"
        );
    }
    if stopped.iter().any(|e| e.role == ROLE_DESKTOP) {
        outln!("桌面界面已被结束，需要时重新打开。");
    }
    if !skipped.is_empty() {
        outln!(
            "注意：上面跳过的 {} 个进程不属于当前用户，强制安装没有结束它们，它们仍在运行原来的版本。",
            skipped.len()
        );
    }
    outln!(
        "skill 文档没有更新（强制安装不碰它们）：之后执行 `aproxy install --skills-only` 更新到与新版本一致。"
    );
    if missing.is_empty() {
        outln!("强制安装完成（退出码 {EXIT_DONE}）。");
        return EXIT_DONE;
    }
    errln!(
        "[ERROR] 二进制已换好，但下面这些被结束的实例没有回来（退出码 {EXIT_NOT_RESTORED}），请手动启动："
    );
    for entry in &missing {
        let which = match entry.port {
            Some(port) => format!("端口 {port}"),
            None => format!("pid {}", entry.pid),
        };
        // 条目的 config 是配置文件的绝对路径，能直接拼出启动命令；label 是自由文本，只
        // 能照抄给人看（没有 config 的条目来自更早的版本）
        match (&entry.config, &entry.label) {
            (Some(config), _) => errln!("  {which}：aproxy start --config \"{config}\""),
            (None, Some(label)) => errln!("  {which}（{label}）：aproxy start <别名|配置文件路径>"),
            (None, None) => errln!("  {which}：aproxy start <别名|配置文件路径>"),
        }
    }
    errln!(
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
    fn an_instance_comes_back_only_on_its_port_with_the_target_version() {
        let entry = |pid: u32, role: &str, port: Option<u16>, version: &str| LedgerEntry {
            ledger: ledger::LEDGER_FORMAT,
            pid,
            start: format!("w:{pid}"),
            role: role.into(),
            version: version.into(),
            port,
            label: None,
            exe: None,
            config: None,
        };
        let killed = entry(10, ROLE_INSTANCE, Some(12345), "0.2.0");
        let back = |live: Vec<LedgerEntry>| came_back(&killed, &live, "0.3.0");
        assert!(back(vec![entry(11, ROLE_INSTANCE, Some(12345), "0.3.0")]));
        // 同一端口上是旧版本的实例：别人拉起的，不是这次恢复的结果
        assert!(!back(vec![entry(11, ROLE_INSTANCE, Some(12345), "0.2.0")]));
        // 别的端口、同一个 pid（还是被结束的那个）、不是实例：都不算
        assert!(!back(vec![entry(11, ROLE_INSTANCE, Some(12346), "0.3.0")]));
        assert!(!back(vec![entry(10, ROLE_INSTANCE, Some(12345), "0.3.0")]));
        assert!(!back(vec![entry(11, ROLE_WATCHDOG, Some(12345), "0.3.0")]));
        // 没有端口的被结束实例无从核对，算没回来
        let portless = entry(10, ROLE_INSTANCE, None, "0.2.0");
        assert!(!came_back(
            &portless,
            &[entry(11, ROLE_INSTANCE, None, "0.3.0")],
            "0.3.0"
        ));
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
