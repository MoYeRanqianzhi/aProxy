//! clap 命令行定义：`Cli` 顶层参数、`Commands`/`AliasCmd` 子命令树、`ConfigArgs`。
//!
//! 只承载定义（结构体与 doc comment 帮助文本），不承载业务逻辑；
//! 各子命令的处理函数在 `commands/` 下与命令同名的模块中。

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
// 不带任何参数时打印帮助（写到 stderr）并以 2 退出：那是一次没说要做什么的调用，按 clap 的
// 惯例算用法错误。启动实例要写 `aproxy start`
#[command(
    name = "aproxy",
    about = "Local API proxy with infinite retries",
    long_about = None,
    arg_required_else_help = true
)]
pub struct Cli {
    // 不用 clap 自带的版本参数：输出的那一行是安装器认版本的永久格式
    // （aproxy::VERSION_LINE），由 main 自己打印；单独的 `aproxy --version` /
    // `aproxy -V` 在解析参数之前就已短路返回，这里只为帮助里列出它、并接住
    // 与别的参数写在一起的情形
    /// 打印版本（`aproxy v<版本> (<变体>; <目标三元组>…)`）
    #[arg(short = 'V', long)]
    pub version: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// `aproxy start` 的参数集：启动哪个配置，以及只对这次启动生效的覆盖。
#[derive(clap::Args, Debug, Clone, PartialEq, Default)]
pub struct StartArgs {
    /// 配置别名（settings.json 中的别名表）或配置文件路径；不写时用默认配置文件
    /// （settings.json 的 default_config，否则 ~/.aproxy/config.toml）
    #[arg(value_name = "ALIAS|PATH", conflicts_with = "config")]
    pub target: Option<String>,

    /// 配置文件路径，与把路径直接写在 start 后面等价。多开不同配置的实例时各自指定
    /// （listen_addr 须互不相同）
    // 这个写法是永久的：强制安装（永久兼容契约 C7、C8）在实例没能恢复时，让用户执行的就是
    // `aproxy start --config "<文件>"`，而执行强制安装的可以是任何一个已经发布的版本——
    // 它们打印出来的命令，以后的每个版本都要认
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// 上游 API base URL，覆盖配置文件中的 base_url（只对这个实例生效，不写入配置；
    /// restart、restore 与看护者重拉时沿用）
    #[arg(long, value_name = "URL")]
    pub base_url: Option<String>,

    /// 监听地址，覆盖配置文件中的 listen_addr
    #[arg(long, value_name = "ADDR")]
    pub listen_addr: Option<String>,

    /// 上游代理 URL，覆盖配置文件中的 proxy
    #[arg(long, value_name = "URL")]
    pub proxy: Option<String>,

    /// 快捷 api_key（等效覆盖 Authorization: Bearer <key>），覆盖配置文件中的 api_key。
    /// 写 - 表示从 stdin 读一行：写在命令行上的密钥会留在 shell 历史里，本机其他进程也
    /// 看得到
    #[arg(long, value_name = "KEY|-", allow_hyphen_values = true)]
    pub api_key: Option<String>,

    /// 守护日志文件路径，覆盖配置文件中的 log_file。未指定时写入 ~/.aproxy/logs/ 下按
    /// 启动时刻随机命名的文件，实际路径以 aproxy logs 经 IPC 向实例询问到的为准
    #[arg(long, value_name = "PATH")]
    pub log_file: Option<String>,

    /// 前台运行（日志输出到控制台，Ctrl+C 停止）；默认在后台运行
    #[arg(long)]
    pub foreground: bool,

    /// 输出一个 JSON 文档（给程序读）：启动了还是本来就在运行，端口、pid、日志路径与实例的
    /// 状态。前台运行没有「启动完成」这个时刻，不能与 --foreground 同用
    #[arg(long, conflicts_with = "foreground")]
    pub json: bool,
}

#[derive(Subcommand, Debug, Clone, PartialEq)]
pub enum Commands {
    /// 列出运行中的 aProxy 实例（来自实例注册表，存活以 IPC 探测为准）。
    /// --idle 只列闲置实例，--busy 只列活跃实例（判定阈值 settings 的
    /// idle_timeout_secs，默认 1800s）
    Status {
        /// 只列闲置（空闲超过阈值）的实例
        #[arg(long, conflicts_with = "busy")]
        idle: bool,
        /// 只列活跃（阈值内有请求）的实例
        #[arg(long)]
        busy: bool,
        /// 输出一个 JSON 文档（给程序读）
        #[arg(long)]
        json: bool,
    },

    /// 启动代理，可指定配置别名或配置文件路径。默认在后台运行。
    /// 别名用 `aproxy alias add` 管理，如 `aproxy start openrouter`。
    Start(StartArgs),

    /// 停止运行中的实例。
    /// 单个实例时可直接 `aproxy stop`；多实例必须指定端口号、`all`、配置别名
    /// 或 `idle`（可带秒数）：
    /// `aproxy stop 12345` / `aproxy stop all` / `aproxy stop openrouter`
    /// `aproxy stop idle`（闲置超 1800s 的全部实例）/
    /// `aproxy stop idle --idle-threshold-secs 600`
    Stop {
        /// 端口号、配置别名、all 或 idle
        #[arg(value_name = "PORT|ALIAS|all|idle")]
        target: Option<String>,
        /// 目标是 idle 时的空闲阈值（秒），覆盖 settings.json 的
        /// instances.idle_threshold_secs
        #[arg(long = "idle-threshold-secs", value_name = "SECS")]
        threshold: Option<u64>,
        /// 立即强制终止（跳过优雅关闭，零等待；终止前按 pid + 进程创建时间核验身份防杀错）
        #[arg(long)]
        force: bool,
        /// 输出一个 JSON 文档（给程序读）：每个实例的结局与汇总
        #[arg(long)]
        json: bool,
    },

    /// 重启运行中的实例（停下它，再按它启动时的配置文件、覆盖参数与环境重新拉起）。
    /// 只负责重启已运行的实例：未启动的 target 会提示「未启动」而不会拉起。
    /// 单个实例时可直接 `aproxy restart`；多实例必须指定端口号、`all`、配置
    /// 别名或 `idle`（可加 --idle-threshold-secs），语义与 stop 一致。
    Restart {
        /// 端口号、配置别名、all 或 idle
        #[arg(value_name = "PORT|ALIAS|all|idle")]
        target: Option<String>,
        /// 目标是 idle 时的空闲阈值（秒），覆盖 settings.json 的
        /// instances.idle_threshold_secs
        #[arg(long = "idle-threshold-secs", value_name = "SECS")]
        threshold: Option<u64>,
        /// 立即强制终止旧进程（跳过优雅关闭，零等待）再重启
        #[arg(long)]
        force: bool,
        /// 输出一个 JSON 文档（给程序读）：每个实例的结局与汇总
        #[arg(long)]
        json: bool,
    },

    /// 打印一个实例的守护日志的最近若干行然后退出；加 -f 接着实时输出新写进来的
    /// （Ctrl+C 退出）。单个实例时可直接 `aproxy logs`；多实例必须指定端口号或配置别名。
    /// 不支持 all：一次只看一个实例。
    Logs {
        /// 端口号或配置别名
        #[arg(value_name = "PORT|ALIAS")]
        target: Option<String>,
        /// 打印最近多少行
        #[arg(short = 'n', long, value_name = "N", default_value_t = 200)]
        lines: usize,
        /// 打印完最近的日志之后不退出，接着输出新写进来的，直到实例停止或 Ctrl+C
        #[arg(short, long)]
        follow: bool,
    },

    /// 一键恢复先前在运行、因崩溃/断电/系统重启而消失的实例。
    /// 无可恢复实例时静默结束（适合配置为开机自启）。
    /// 已在运行的实例自动跳过；配置文件此刻找不到的记录保留着，文件回来之后再恢复。
    // 永久兼容契约 C7：restore 子命令永远存在、名字不变——任何版本的强制安装换好二进制
    // 之后，都执行新二进制的 `restore` 把被结束的实例拉回来（见 aproxy::install::force）
    Restore {
        /// 输出一个 JSON 文档（给程序读）：每条记录的结局与汇总
        #[arg(long)]
        json: bool,
    },

    /// 管理配置别名（存于 ~/.aproxy/settings.json，内部配置不建议手改）。
    /// 别名指向某个 config.toml，让多开配置可以用名字快捷启停。
    Alias {
        #[command(subcommand)]
        cmd: AliasCmd,
    },

    /// 全面体检全部配置（settings.json error 级检查 + 别名配置深入审查 +
    /// 配置目录 toml 扫描，均含端口冲突检测）
    Doctor {
        /// 输出一个 JSON 文档（给程序读）
        #[arg(long)]
        json: bool,
    },

    /// 从配置目录列表（settings.json 的 config_dirs + 默认两个）发现全部
    /// config（toml），支持按别名归属与关键字/端口过滤
    Find {
        /// 别名归属过滤：--aliased 只列已配别名的，--unaliased 只列未配的
        #[arg(long, conflicts_with = "unaliased")]
        aliased: bool,
        /// 只列未配置别名的
        #[arg(long)]
        unaliased: bool,
        /// 关键字过滤：匹配路径片段或别名（大小写不敏感）
        #[arg(value_name = "QUERY")]
        query: Option<String>,
        /// 按监听端口过滤
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,
        /// 输出一个 JSON 文档（给程序读）：匹配的配置、别名与错误诊断
        #[arg(long)]
        json: bool,
    },

    /// 安装/升级 aProxy 到规范位置（~/.aproxy/bin/）：全程对客户端 ≈ 无感
    /// （逐实例滚动重启，任一时刻至多一个实例在重启）。`upgrade` 是它的别名。
    /// 在线安装走下载链条（github → npm → cargo-binstall → cargo，可配
    /// settings 的 download_chain/url 模板与 --download-proxy）；中断自动续作。
    /// 某实例在新版本下起不来时用旧二进制把它拉回并中止滚动（命令返回非零）。
    /// 普通安装失败或卡住时，`install --force` 结束全部 aproxy 进程后直接换二进制。
    // 永久兼容契约 C7：`upgrade --force …` 与 `install --force …` 是同一个命令
    #[command(visible_alias = "upgrade")]
    Install(InstallArgs),

    /// 安装、列出或读取与 aProxy 配套的 skill 文档
    Skill {
        /// 对 skill 文档执行的操作，不改动二进制。
        #[command(subcommand)]
        cmd: SkillCmd,
    },

    /// 查看或修改一份配置文件（默认是 ~/.aproxy/config.toml；--config 指定别的）
    Config(ConfigArgs),

    /// [内部] 后台实例的入口：按交接文件里的拉起规格承载服务。由 start、restart、restore、
    /// 看护者与安装器拉起，也会被别的版本的 aproxy 拉起（安装时新旧版本互相拉起对方），
    /// 所以名字与两个参数不能改。勿手动使用
    #[command(name = "__serve", hide = true)]
    Serve {
        /// 实例所属的 home（绝对路径）
        #[arg(long, value_name = "DIR")]
        home: PathBuf,
        /// 拉起规格的交接文件
        #[arg(long, value_name = "FILE")]
        spec: PathBuf,
    },

    /// [内部] 看护者的入口：看护这个 home 里的全部实例。与 `__serve` 一样会被别的版本
    /// 拉起，名字与参数不能改。勿手动使用
    #[command(name = "__watchdog", hide = true)]
    Watchdog {
        /// 看护的 home（绝对路径）
        #[arg(long, value_name = "DIR")]
        home: PathBuf,
    },

    /// 查看或修改全局设置（~/.aproxy/settings.json）：别名、config.toml 键的全局默认、
    /// 看护者、日志、安装更新。settings.json 有错误时也能用，用来查看与修复它
    Settings {
        #[command(subcommand)]
        cmd: SettingsCmd,
    },

    /// 打开桌面界面：独立的窗口进程，关掉窗口就退出，不影响运行中的代理实例。
    /// 这个版本的界面是预览版，显示的是演示数据，不是本机实例的真实状态。
    // 无界面的 headless 构建也认得这个子命令（帮助里隐藏），执行时说明这个版本没有界面、
    // 怎样换装，而不是让用户看到 clap 的「unrecognized subcommand」（见 commands::desktop）。
    // 那里的 `aproxy desktop --help` 也换一段说明：上面的文档注释说的是带界面的构建（显式的
    // about 排在文档注释之后，覆盖它）
    #[command(hide = cfg!(not(feature = "desktop")))]
    #[cfg_attr(
        not(feature = "desktop"),
        command(
            about = "这是无界面的 headless 版本，没有桌面界面：运行它会说明怎样换装带界面的版本",
            long_about = None
        )
    )]
    Desktop,
}

/// `aproxy install` 的参数集。
#[derive(clap::Args, Debug, Clone, PartialEq)]
pub struct InstallArgs {
    /// 安装目标版本：`latest`（默认，按更新通道取最大版本——stable 通道只取
    /// 正式版，pre 通道含预发布；通道见 --pre）或具体版本号（如
    /// 0.2.0-alpha.1）。latest 不高于当前版本时提示「已是最新」不重装。
    /// 默认拒绝降级（--allow-downgrade 放行）；低于 0.2.0 的版本一律拒绝。
    /// 加 --force 时不与当前版本比较，总是安装。与 --from/--adopt 互斥。
    #[arg(value_name = "VERSION", conflicts_with_all = ["from", "adopt", "abort"])]
    pub version: Option<String>,

    /// 从本地二进制文件安装（复制 → 校验 → 原子落位 → 滚动重启实例）。
    /// 二进制的 `--version` 自报版本即安装目标版本（低于 0.2.0 的拒绝）。
    #[arg(long, value_name = "PATH", conflicts_with_all = ["adopt", "abort", "version"])]
    pub from: Option<String>,

    /// 收编：把当前运行的 aProxy（如包管理器/npm 安装的）迁移到标准位置
    /// `~/.aproxy/bin/`——当前进程镜像作为安装源，走完整标准流水线。
    /// 显式执行，绝不自动。
    #[arg(long, conflicts_with_all = ["from", "abort", "version"])]
    pub adopt: bool,

    /// 中止进行中的安装：仅 swapping 前可完全回滚（此后只进不退）。
    /// 清理状态文件与 staging 现场。
    #[arg(long, conflicts_with_all = ["from", "adopt", "version"])]
    pub abort: bool,

    /// 强制安装：不走交接，按进程台账结束这个 home 下当前用户的全部 aproxy 进程（实例、
    /// 看护者、安装者、桌面界面），换上新二进制，再用新版本的 restore 按原来的拉起规格恢复实例。
    /// 普通安装失败、卡住，或装着的版本坏了时用；会中断所有经 aproxy 的在途请求，先确认
    /// （非交互环境加 --yes）。目标取 VERSION（默认 latest，不低于当前版本）或 --from；
    /// 写明的版本号可以降级，低于 0.2.0 的版本仍不装；不更新 skill 文档。退出码：0 完成；
    /// 10 一个进程都没结束；11 结束过进程、二进制没换；12 二进制已换、有实例没恢复
    // 永久兼容契约 C7：`install --force [<版本>|latest] [--from <文件>] [--variant
    // desktop|headless] [--cpu baseline|x86-64-v3] [--pre] [--download-proxy <地址>]
    // [--yes]` 与 `upgrade --force`（等同 `install --force latest`）的写法、退出码
    // 0/10/11/12 无论如何都不得改动，只许追加可选参数——agent 与脚本照着写在各处的命令行
    // 修复坏掉的安装，任何版本都要认得（样本钉在 tests/force_contract.rs）。与 --force
    // 冲突的选项由参数解析拒绝（用法错误，退出码 2，什么都不做），而不是「接受但不起
    // 作用」：以后要放宽不破坏调用方，收紧才会
    #[arg(
        long,
        conflicts_with_all = ["adopt", "abort", "skills_only", "continue_", "no_skills", "allow_downgrade"]
    )]
    pub force: bool,

    /// 强制安装不再询问确认（非交互环境必须加）。加之前先向用户确认可以中断经
    /// aproxy 的在途请求
    #[arg(long, requires = "force")]
    pub yes: bool,

    /// 要装的变体：desktop（带桌面界面）或 headless（无界面）；默认与正在运行的这个
    /// aproxy 相同。带界面的版本在缺 WebKitGTK 等系统库的 Linux 上跑不起来：没写这个
    /// 参数时，在线安装下载的带界面版本跑不起来就改装 headless 并说明原因，写了 desktop
    /// 就报错。与 --from 同用时是核对：文件必须正是这个变体，否则不装
    // 它在 --force 下的写法属于永久兼容契约 C7；普通安装也接受它是放宽，C7 允许放宽
    #[arg(
        long,
        value_name = "VARIANT",
        value_parser = ["desktop", "headless"],
        conflicts_with_all = ["adopt", "abort", "skills_only", "continue_"]
    )]
    pub variant: Option<String>,

    /// 指令集档：x86-64-v3 或 baseline（默认按本机 CPU：支持 AVX2、且这个目标发布了
    /// x86-64-v3 档（x86_64 的 Windows 与 Linux gnu）→ x86-64-v3，否则 baseline）
    #[arg(long, value_name = "CPU", value_parser = ["x86-64-v3", "baseline"], conflicts_with_all = ["from", "adopt", "abort"])]
    pub cpu: Option<String>,

    /// 允许降级安装（目标版本 < 当前已安装版本时默认拒绝）；低于 0.2.0 的版本
    /// 仍然拒绝
    #[arg(long)]
    pub allow_downgrade: bool,

    /// latest 走预发布通道：在全部版本（含正式版与 alpha/beta/rc 预发布）
    /// 中取最大。不加时通道跟随当前版本——当前是预发布则默认即为预发布
    /// 通道，当前是正式版则只取正式版（不会被带到预发布）。只影响 latest
    /// 解析，指定具体版本号时无作用。
    #[arg(long, conflicts_with_all = ["from", "adopt", "abort"])]
    pub pre: bool,

    /// 仅本次下载使用的代理 URL（与 config.toml 的请求代理绝对分离）；
    /// 未指定时用 settings 的 download_proxy，再未配置则走系统环境代理
    /// （--force 不读 settings：只用这个参数，没给就走系统环境代理）
    #[arg(long, value_name = "URL")]
    pub download_proxy: Option<String>,

    /// 本次安装跳过 skill 文档更新（全局开关在 settings 的
    /// skill_auto_update）
    #[arg(long)]
    pub no_skills: bool,

    /// 只安装与当前二进制同版本的 skill；保留已发布安装器给用户的命令行。
    /// 旧入口与 --json 同用仍按参数错误退出（2）；文档结果请用 skill install --json。
    #[arg(long, hide = true, conflicts_with_all = ["from", "adopt", "abort", "continue_", "handover_from"])]
    pub skills_only: bool,

    /// 只查询 install 会装哪个版本：打印当前版本、通道里的最新版本与有没有更新，
    /// 不下载、不改动任何东西。可与 --pre、--variant、--download-proxy、--json 同用
    // 不登记台账、不建安装状态，所以随时可以执行；能与之同用的只有影响「会装哪个版本」
    // 与怎样查询的那几个参数，其余一律按用法错误拒绝
    #[arg(
        long,
        conflicts_with_all = [
            "version", "from", "adopt", "abort", "continue_", "handover_from", "no_skills",
            "allow_downgrade", "skills_only", "yes", "force"
        ]
    )]
    pub check: bool,

    /// 机器可读的输出：标准输出上只有 JSON（安装时每行一个事件，--check 时一个文档），
    /// 给人看的文字改写到标准错误
    // 续作与交棒的进程没有读它输出的人；--force 是冻结的另一条路径（C7），不带这个参数
    #[arg(long, conflicts_with_all = ["force", "abort", "continue_", "skills_only"])]
    pub json: bool,

    /// [内部] 续作模式：从 install.state 残留的 phase 幂等推进（看门狗/
    /// CLI 入口/交棒自动拉起，全自动无人工询问）。勿手动使用。
    #[arg(long, hide = true)]
    pub continue_: bool,

    /// [内部] 交棒：拉起本进程的安装者的 pid。带着它（且它仍是 install.state
    /// 的主人）的续作进程可以接手一个安装者健在的安装。旧版本的安装者会这样
    /// 拉起新版本，是永久接口，见 install::flow 模块文档。勿手动使用。
    #[arg(long, hide = true, value_name = "PID", requires = "continue_")]
    pub handover_from: Option<u32>,
}

/// 文档的版本随二进制，不提供独立选版或切换通道的入口。
#[derive(Subcommand, Debug, Clone, PartialEq)]
pub enum SkillCmd {
    /// 发布前用实际解包和正文读取规则校验文档，不改用户的安装。
    #[command(name = "__verify-package", hide = true)]
    VerifyPackage {
        /// 待核验的总包。
        archive: PathBuf,
        /// 同时核对 cargo 打好的包与发布总包，而不读取工作副本的正文。
        #[arg(long)]
        packaged_crate: Option<PathBuf>,
        /// 发布者请求的精确版本。
        version: String,
    },
    /// 安装当前版本的 skill 文档；upgrade 执行同一操作
    #[command(visible_alias = "upgrade")]
    Install {
        /// 仅本次下载使用的代理 URL，优先于设置中的下载代理
        #[arg(long, value_name = "URL")]
        download_proxy: Option<String>,
        /// 输出一个 JSON 文档（给程序读）
        #[arg(long)]
        json: bool,
    },
    /// 列出已安装的 skill、版本、位置和文件
    List {
        /// 输出一个 JSON 文档（给程序读）
        #[arg(long)]
        json: bool,
    },
    /// 读取并核对清单中的文件，默认读取 SKILL.md
    Show {
        /// 清单中精确的 skill 名
        name: String,
        /// 相对 skill 目录的文件名，使用正斜杠
        file: Option<String>,
        /// 只列出这个 skill 的文件名
        #[arg(long, conflicts_with = "file")]
        files: bool,
    },
}

#[derive(Subcommand, Debug, Clone, PartialEq)]
pub enum AliasCmd {
    /// 添加/覆盖别名（path 省略时指向默认配置 ~/.aproxy/config.toml）
    Add {
        /// 别名（不能为 all 或纯数字——会被 start/stop 当作端口/保留字解析）
        name: String,
        /// 指向的 config.toml 路径（支持 ~ 展开）
        #[arg(value_name = "PATH")]
        path: Option<String>,
    },
    /// 删除别名
    Remove {
        /// 别名
        name: String,
    },
    /// 列出全部别名
    List {
        /// 输出一个 JSON 文档（给程序读）：每个别名的路径与文件是否存在
        #[arg(long)]
        json: bool,
    },
}

/// `aproxy config` 的参数集。独立成结构体以整体传递给处理函数，
/// 避免 main 与函数之间逐字段搬运十几个参数。
#[derive(Subcommand, Debug, Clone, PartialEq)]
pub enum SettingsCmd {
    /// 打印生效的全部设置（文件里的键盖在内置默认上）。密钥显示成占位对象
    Show {
        /// 输出一个 JSON 文档：生效的设置与诊断
        #[arg(long)]
        json: bool,
    },
    /// 打印一个键的生效值。键是点号路径，例如 watchdog.enabled、aliases.work、
    /// defaults.timeouts.connect_secs
    Get {
        #[arg(value_name = "KEY")]
        key: String,
        /// 输出一个 JSON 文档：键与它的生效值
        #[arg(long)]
        json: bool,
    },
    /// 设置一个键。值写成 JSON（true、30、["a"]、{"k":1}）；字符串类型的键可以直接写
    /// 原文。值写 - 表示从 stdin 读一行（传密钥用）。只能设置认识的键
    Set {
        #[arg(value_name = "KEY")]
        key: String,
        #[arg(value_name = "VALUE", allow_hyphen_values = true)]
        value: String,
        /// 输出一个 JSON 文档，里面列出要重启才生效的实例
        #[arg(long)]
        json: bool,
    },
    /// 删除一个键（回到内置默认）。任何键都能删，包括不认识的键
    Unset {
        #[arg(value_name = "KEY")]
        key: String,
        /// 输出一个 JSON 文档，里面列出要重启才生效的实例
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Args, Debug, Clone, PartialEq)]
pub struct ConfigArgs {
    /// 要查看或修改的配置文件：别名或路径。不写时用默认配置文件（settings.json 的
    /// default_config，否则 ~/.aproxy/config.toml）
    #[arg(long, value_name = "ALIAS|PATH", global = true)]
    pub config: Option<String>,

    #[command(subcommand)]
    pub cmd: ConfigCmd,
}

#[derive(Subcommand, Debug, Clone, PartialEq)]
pub enum ConfigCmd {
    /// 打印配置文件里写了的值。密钥显示成占位，不给明文。文件有错误时照样显示，附上
    /// 诊断，退出码 3
    Show {
        /// 打印生效的值：文件里没写的键补上 settings.json 的全局默认与内置默认，每个值
        /// 注明来自哪一层；全局默认里没有继承的凭据单独列出
        #[arg(long)]
        effective: bool,
        /// 输出一个 JSON 文档（给程序读）
        #[arg(long)]
        json: bool,
    },
    /// 打印一个键的生效值。键是点号路径，例如 base_url、timeouts.connect_secs、
    /// extra_headers."x-foo"
    Get {
        #[arg(value_name = "KEY")]
        key: String,
        /// 输出一个 JSON 文档：值与它来自哪一层
        #[arg(long)]
        json: bool,
    },
    /// 设置一个键，只改这一个键：文件里的注释与其余内容原样保留。值写成 TOML（true、30、
    /// ["a"]、{ k = 1 }）；字符串类型的键可以直接写原文。值写 - 表示从 stdin 读一行（传密钥
    /// 用）。只能设置认识的键；会让配置无效的改动不写入。运行中的实例要重启才生效
    Set {
        #[arg(value_name = "KEY")]
        key: String,
        #[arg(value_name = "VALUE|-", allow_hyphen_values = true)]
        value: String,
        /// 输出一个 JSON 文档，里面列出要重启才生效的实例
        #[arg(long)]
        json: bool,
    },
    /// 删除一个键（回到全局默认或内置默认）。任何键都能删，包括不认识的键
    Unset {
        #[arg(value_name = "KEY")]
        key: String,
        /// 输出一个 JSON 文档，里面列出要重启才生效的实例
        #[arg(long)]
        json: bool,
    },
}

/// 构造实际解析用的完整命令树，包括 clap 自己生成的 help 子树，再统一附上文档入口。
pub fn command_tree() -> clap::Command {
    use clap::CommandFactory;
    /// 内部命令也可能被单独查询帮助，不能只修饰用户可见的第一层。
    fn annotate(cmd: clap::Command) -> clap::Command {
        cmd.after_help(
            "Agents: run 'aproxy skill list' and 'aproxy skill show <name>' for full docs.",
        )
        .mut_subcommands(annotate)
    }
    let mut cmd = Cli::command();
    cmd.build();
    annotate(cmd)
}
