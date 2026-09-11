//! clap 命令行定义：`Cli` 顶层参数、`Commands`/`AliasCmd` 子命令树、`ConfigArgs`。
//!
//! 只承载定义（结构体与 doc comment 帮助文本），不承载业务逻辑；
//! 各子命令的处理函数在 `commands/` 下与命令同名的模块中。

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(name = "aproxy", version, about = "Local API proxy with infinite retries", long_about = None)]
pub(crate) struct Cli {
    /// 配置文件路径（默认 ~/.aproxy/config.toml）。
    /// 多开不同配置的进程时各自指定，例如：
    /// aproxy --config ~/.aproxy/work.toml 与 aproxy --config ~/.aproxy/personal.toml
    /// （listen_addr 须互不相同）
    #[arg(long, value_name = "PATH", global = true)]
    pub(crate) config: Option<PathBuf>,

    /// 前台运行（日志输出到控制台，Ctrl+C 停止）；默认在后台运行，
    /// 日志写 ~/.aproxy/logs/<端口>.log，用 `aproxy status`/`aproxy stop` 管理
    #[arg(long)]
    pub(crate) foreground: bool,

    /// 上游 API base URL，覆盖配置文件中的 base_url
    #[arg(long, value_name = "URL")]
    pub(crate) baseurl: Option<String>,

    /// 监听地址，覆盖配置文件中的 listen_addr
    #[arg(long, value_name = "ADDR")]
    pub(crate) listen: Option<String>,

    /// 上游代理 URL（仅本次运行生效，不写入配置），覆盖配置文件中的 proxy
    #[arg(long, value_name = "URL")]
    pub(crate) proxy: Option<String>,

    /// 快捷 api_key（等效覆盖 Authorization: Bearer <key>，仅本次运行生效），覆盖配置文件中的 api_key
    /// （建议改用配置文件，命令行参数可被本机其他进程枚举）
    #[arg(long, value_name = "KEY")]
    pub(crate) api_key: Option<String>,

    /// [内部] 守护子进程标记：后台启动时父进程把它附加到子进程命令行，
    /// 子进程据此以守护模式运行（无控制台、日志写文件）。勿手动使用。
    /// 必须是 global：start 子命令转发时它出现在 `start` 之后。
    #[arg(long, hide = true, global = true)]
    pub(crate) daemon_child: bool,

    /// [内部] 看护进程标记：守护/CLI 拉起看护者时附加到命令行，
    /// 本进程据此进入看护主循环。勿手动使用。
    #[arg(long, hide = true, global = true)]
    pub(crate) daemon_watchdog: bool,

    #[command(subcommand)]
    pub(crate) command: Option<Commands>,
}

#[derive(Subcommand, Debug, Clone, PartialEq)]
pub(crate) enum Commands {
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
    },

    /// 启动代理（与直接运行 `aproxy` 相同），可指定配置别名或配置文件路径。
    /// 别名用 `aproxy alias add` 管理，如 `aproxy start openrouter`。
    Start {
        /// 配置别名（settings.json 中的别名表）或配置文件路径
        #[arg(value_name = "ALIAS|PATH")]
        target: Option<String>,
    },

    /// 停止运行中的实例。
    /// 单个实例时可直接 `aproxy stop`；多实例必须指定端口号、`all`、配置别名
    /// 或 `idle`（可带秒数）：
    /// `aproxy stop 12345` / `aproxy stop all` / `aproxy stop openrouter`
    /// `aproxy stop idle`（闲置超 1800s 的全部实例）/ `aproxy stop idle 600`
    Stop {
        /// 端口号、all、配置别名或 idle
        #[arg(value_name = "PORT|all|ALIAS|idle")]
        target: Option<String>,
        /// idle 模式的空闲阈值（秒），覆盖 settings.json 的 idle_timeout_secs
        #[arg(value_name = "SECS", requires = "target")]
        threshold: Option<u64>,
        /// 立即强制终止（跳过优雅关闭，零等待；进程终止前验证镜像名防杀错）
        #[arg(long)]
        force: bool,
    },

    /// 重启运行中的实例（等价于对该实例执行 stop 后用原参数立即 start）。
    /// 只负责重启已运行的实例：未启动的 target 会提示「未启动」而不会拉起。
    /// 单个实例时可直接 `aproxy restart`；多实例必须指定端口号、`all`、配置
    /// 别名或 `idle`（可带秒数），语义与 stop 一致。
    Restart {
        /// 端口号、all、配置别名或 idle
        #[arg(value_name = "PORT|all|ALIAS|idle")]
        target: Option<String>,
        /// idle 模式的空闲阈值（秒），覆盖 settings.json 的 idle_timeout_secs
        #[arg(value_name = "SECS", requires = "target")]
        threshold: Option<u64>,
        /// 立即强制终止旧进程（跳过优雅关闭，零等待）再重启
        #[arg(long)]
        force: bool,
    },

    /// 连接到运行中的实例并实时输出其守护日志（Ctrl+C 退出）。
    /// 单个实例时可直接 `aproxy logs`；多实例必须指定端口号。
    /// 不支持 all：一次只能连接一个实例。
    Logs {
        /// 端口号
        #[arg(value_name = "PORT")]
        target: Option<String>,
    },

    /// 一键恢复先前在运行、因崩溃/断电/系统重启而消失的实例。
    /// 无可恢复实例时静默结束（适合配置为开机自启）。
    /// 已在运行的实例自动跳过；配置文件已不存在的记录被清理。
    Restore,

    /// 管理配置别名（存于 ~/.aproxy/settings.json，内部配置不建议手改）。
    /// 别名指向某个 config.toml，让多开配置可以用名字快捷启停。
    Alias {
        #[command(subcommand)]
        cmd: AliasCmd,
    },

    /// 全面体检全部配置（settings.json error 级检查 + 别名配置深入审查 +
    /// 配置目录 toml 扫描，均含端口冲突检测）
    Doctor,

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
    },

    /// 安装/升级 aProxy 到规范位置（~/.aproxy/bin/）：全程对客户端 ≈ 无感
    /// （逐实例滚动重启，任一时刻至多一个实例在重启）。`upgrade` 为其别名。
    /// 指定版本/在线渠道（github/npm/cargo）随后续版本提供；当前支持
    /// `--from` 本地路径与 `--adopt` 收编。
    Install(InstallArgs),

    /// `aproxy install` 的别名（可发现性）
    Upgrade(InstallArgs),

    /// 查看或修改配置（配置文件位于 ~/.aproxy/config.toml）
    Config(ConfigArgs),
}

/// `aproxy install` 的参数集。
#[derive(clap::Args, Debug, Clone, PartialEq)]
pub(crate) struct InstallArgs {
    /// 从本地二进制文件安装（复制 → 校验 → 原子落位 → 滚动重启实例）。
    /// 二进制的 `--version` 自报版本即安装目标版本。
    #[arg(long, value_name = "PATH", conflicts_with_all = ["adopt", "abort"])]
    pub(crate) from: Option<String>,

    /// 收编：把当前运行的 aProxy（如包管理器/npm 安装的）迁移到标准位置
    /// `~/.aproxy/bin/`——当前进程镜像作为安装源，走完整标准流水线。
    /// 显式执行，绝不自动。
    #[arg(long, conflicts_with_all = ["from", "abort"])]
    pub(crate) adopt: bool,

    /// 中止进行中的安装：仅 swapping 前可完全回滚（此后只进不退）。
    /// 清理状态文件与 staging 现场。
    #[arg(long, conflicts_with_all = ["from", "adopt"])]
    pub(crate) abort: bool,

    /// [内部] 续作模式：从 install.state 残留的 phase 幂等推进（看门狗/
    /// CLI 入口/接力自动拉起，全自动无人工询问）。勿手动使用。
    #[arg(long, hide = true)]
    pub(crate) continue_: bool,
}

#[derive(Subcommand, Debug, Clone, PartialEq)]
pub(crate) enum AliasCmd {
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
    List,
}

/// `aproxy config` 的参数集。独立成结构体以整体传递给处理函数，
/// 避免 main 与函数之间逐字段搬运十几个参数。
#[derive(clap::Args, Debug, Clone, PartialEq)]
pub(crate) struct ConfigArgs {
    /// 设置上游 base URL，例如 https://api.anthropic.com
    #[arg(long, value_name = "URL")]
    pub(crate) baseurl: Option<String>,
    /// 设置监听地址，例如 127.0.0.1:12345
    #[arg(long, value_name = "ADDR")]
    pub(crate) listen: Option<String>,
    /// 快捷设置 api_key（等效覆盖 Authorization: Bearer <key>）
    #[arg(long, value_name = "KEY")]
    pub(crate) api_key: Option<String>,
    /// 额外请求头（仅当未携带时追加），格式 key=value，可重复
    #[arg(long = "extra-header", value_name = "KEY=VALUE")]
    pub(crate) extra_headers: Vec<String>,
    /// 覆盖请求头（无条件覆盖），格式 key=value，可重复
    #[arg(long = "override-header", value_name = "KEY=VALUE")]
    pub(crate) override_headers: Vec<String>,
    /// 保活心跳间隔秒数，0 表示关闭
    #[arg(long, value_name = "SECS")]
    pub(crate) keepalive_secs: Option<u64>,
    /// 设置上游代理 URL，例如 http://127.0.0.1:7890 或 socks5://user:pass@127.0.0.1:7890
    #[arg(long, value_name = "URL")]
    pub(crate) proxy: Option<String>,
    /// 设置代理用户名（可选，优先于 URL 内嵌的用户名）
    #[arg(long, value_name = "USER")]
    pub(crate) proxy_username: Option<String>,
    /// 设置代理密码（可选，优先于 URL 内嵌的密码）
    #[arg(long, value_name = "PASS")]
    pub(crate) proxy_password: Option<String>,
    /// 清空已配置的 api_key
    #[arg(long)]
    pub(crate) clear_api_key: bool,
    /// 清空 extra/override 头
    #[arg(long)]
    pub(crate) clear_headers: bool,
    /// 清空代理配置（URL、用户名、密码）
    #[arg(long)]
    pub(crate) clear_proxy: bool,
    /// 把指定路径设为默认配置文件（写入 settings.json，不指定时默认
    /// ~/.aproxy/config.toml；支持 ~ 展开）
    #[arg(long, value_name = "PATH")]
    pub(crate) set_default: Option<String>,
    /// 取消默认配置文件设置（恢复用 ~/.aproxy/config.toml）
    #[arg(long)]
    pub(crate) clear_default: bool,
    /// 打印当前配置及文件路径
    #[arg(long)]
    pub(crate) show: bool,
}
