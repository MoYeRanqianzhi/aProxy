//! clap 命令行定义：`Cli` 顶层参数、`Commands`/`AliasCmd` 子命令树、`ConfigArgs`。
//!
//! 只承载定义（结构体与帮助树），不承载业务逻辑；
//! 各子命令的处理函数在 `commands/` 下与命令同名的模块中。

use aproxy_ipc::i18n::{Lang, current};
use clap::{Parser, Subcommand};

#[path = "i18n/help.rs"]
mod help;
use std::path::PathBuf;

/// 修复命令要保留完整的路径或键名，不能让其中的引号结束外层 shell 引号。
pub fn quoted_arg(arg: &str) -> String {
    let mut text = String::from("'");
    for c in arg.chars() {
        // PowerShell 的弯单引号也能关闭字符串；加一个单引号便保留原字符。
        if cfg!(windows) && matches!(c, '\'' | '\u{2018}'..='\u{201b}') {
            text.push('\'');
        } else if !cfg!(windows) && c == '\'' {
            text.push_str("'\\'");
        }
        text.push(c);
    }
    text.push('\'');
    text
}

#[derive(Parser, Debug, Clone)]
// 不带任何参数时打印帮助（写到 stderr）并以 2 退出：那是一次没说要做什么的调用，按 clap 的
// 惯例算用法错误。启动实例要写 `aproxy start`
#[command(
    name = "aproxy",
    about = help::root(current()),
    long_about = None,
    arg_required_else_help = true
)]
pub struct Cli {
    // 不用 clap 自带的版本参数：输出的那一行是安装器认版本的永久格式
    // （aproxy::VERSION_LINE），由 main 自己打印；单独的 `aproxy --version` /
    // `aproxy -V` 在解析参数之前就已短路返回，这里只为帮助里列出它、并接住
    // 与别的参数写在一起的情形
    #[arg(help = help::version(current()))]
    #[arg(short = 'V', long)]
    pub version: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// `aproxy start` 的参数集：启动哪个配置，以及只对这次启动生效的覆盖。
#[derive(clap::Args, Debug, Clone, PartialEq, Default)]
pub struct StartArgs {
    #[arg(help = help::start_target(current()))]
    #[arg(value_name = "ALIAS|PATH", conflicts_with = "config")]
    pub target: Option<String>,

    #[arg(help = help::start_config(current()))]
    // 这个写法是永久的：强制安装（永久兼容契约 C7、C8）在实例没能恢复时，让用户执行的就是
    // `aproxy start --config "<文件>"`，而执行强制安装的可以是任何一个已经发布的版本——
    // 它们打印出来的命令，以后的每个版本都要认
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    #[arg(help = help::start_url(current()))]
    #[arg(long, value_name = "URL")]
    pub base_url: Option<String>,

    #[arg(help = help::start_addr(current()))]
    #[arg(long, value_name = "ADDR")]
    pub listen_addr: Option<String>,

    #[arg(help = help::start_proxy(current()))]
    #[arg(long, value_name = "URL")]
    pub proxy: Option<String>,

    #[arg(help = help::start_key(current()))]
    #[arg(long, value_name = "KEY|-", allow_hyphen_values = true)]
    pub api_key: Option<String>,

    #[arg(help = help::start_log(current()))]
    #[arg(long, value_name = "PATH")]
    pub log_file: Option<String>,

    #[arg(help = help::foreground(current()))]
    #[arg(long)]
    pub foreground: bool,

    #[arg(help = help::start_json(current()))]
    #[arg(long, conflicts_with = "foreground")]
    pub json: bool,
}

#[derive(Subcommand, Debug, Clone, PartialEq)]
pub enum Commands {
    #[command(about = help::status(current()))]
    Status {
        #[arg(help = help::idle(current()))]
        #[arg(long, conflicts_with = "busy")]
        idle: bool,
        #[arg(help = help::busy(current()))]
        #[arg(long)]
        busy: bool,
        #[arg(help = help::json(current()))]
        #[arg(long)]
        json: bool,
    },

    #[command(about = help::start(current()))]
    Start(StartArgs),

    #[command(about = help::stop(current()))]
    Stop {
        #[arg(help = help::target(current()))]
        #[arg(value_name = "PORT|ALIAS|all|idle")]
        target: Option<String>,
        #[arg(help = help::idle_threshold(current()))]
        #[arg(long = "idle-threshold-secs", value_name = "SECS")]
        threshold: Option<u64>,
        #[arg(help = help::stop_force(current()))]
        #[arg(long)]
        force: bool,
        #[arg(help = help::results_json(current()))]
        #[arg(long)]
        json: bool,
    },

    #[command(about = help::restart(current()))]
    Restart {
        #[arg(help = help::target(current()))]
        #[arg(value_name = "PORT|ALIAS|all|idle")]
        target: Option<String>,
        #[arg(help = help::idle_threshold(current()))]
        #[arg(long = "idle-threshold-secs", value_name = "SECS")]
        threshold: Option<u64>,
        #[arg(long="format", value_name="NAME|all", help=help::restart_formats(current()))]
        formats: Vec<String>,
        #[arg(help = help::restart_force(current()))]
        #[arg(long)]
        force: bool,
        #[arg(help = help::results_json(current()))]
        #[arg(long)]
        json: bool,
    },

    #[command(about = help::logs(current()))]
    Logs {
        #[arg(help = help::log_target(current()))]
        #[arg(value_name = "PORT|ALIAS")]
        target: Option<String>,
        #[arg(help = help::lines(current()))]
        #[arg(short = 'n', long, value_name = "N", default_value_t = 200)]
        lines: usize,
        #[arg(help = help::follow(current()))]
        #[arg(short, long)]
        follow: bool,
    },

    #[command(about = help::restore(current()))]
    // 永久兼容契约 C7：restore 子命令永远存在、名字不变——任何版本的强制安装换好二进制
    // 之后，都执行新二进制的 `restore` 把被结束的实例拉回来（见 aproxy::install::force）
    Restore {
        #[arg(help = help::restore_json(current()))]
        #[arg(long)]
        json: bool,
    },

    #[command(about = help::login_restore(current()))]
    #[command(name = "restore-at-login", hide = true)]
    RestoreAtLogin {
        #[arg(help = help::login_home(current()))]
        #[arg(long,value_name="DIR",value_parser=absolute_login_home)]
        home: PathBuf,
    },

    #[command(about = help::alias(current()))]
    Alias {
        #[command(subcommand)]
        cmd: AliasCmd,
    },

    #[command(about = help::migrate(current()))]
    Migrate {
        #[arg(value_name="PATH", help=help::migrate_paths(current()))]
        paths: Vec<PathBuf>,
        #[arg(long, help = help::migrate_preview(current()))]
        dry_run: bool,
        #[arg(long, help = help::json(current()))]
        json: bool,
    },

    #[command(about = help::doctor(current()))]
    Doctor {
        #[arg(help = help::json(current()))]
        #[arg(long)]
        json: bool,
    },

    #[command(about = help::find(current()))]
    Find {
        #[arg(help = help::aliased(current()))]
        #[arg(long, conflicts_with = "unaliased")]
        aliased: bool,
        #[arg(help = help::unaliased(current()))]
        #[arg(long)]
        unaliased: bool,
        #[arg(help = help::query(current()))]
        #[arg(value_name = "QUERY")]
        query: Option<String>,
        #[arg(help = help::port(current()))]
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,
        #[arg(help = help::find_json(current()))]
        #[arg(long)]
        json: bool,
    },

    #[command(about = help::install(current()))]
    // 永久兼容契约 C7：`upgrade --force …` 与 `install --force …` 是同一个命令
    #[command(visible_alias = "upgrade")]
    Install(InstallArgs),

    #[command(about = help::skill(current()))]
    Skill {
        /// 对 skill 文档执行的操作，不改动二进制。
        #[command(subcommand)]
        cmd: SkillCmd,
    },

    #[command(about = help::config(current()))]
    Config(ConfigArgs),

    #[command(about = help::serve(current()))]
    #[command(name = "__serve", hide = true)]
    Serve {
        #[arg(help = help::serve_home(current()))]
        #[arg(long, value_name = "DIR")]
        home: PathBuf,
        #[arg(help = help::spec(current()))]
        #[arg(long, value_name = "FILE")]
        spec: PathBuf,
    },

    #[command(about = help::watchdog(current()))]
    #[command(name = "__watchdog", hide = true)]
    Watchdog {
        #[arg(help = help::watchdog_home(current()))]
        #[arg(long, value_name = "DIR")]
        home: PathBuf,
    },

    #[command(about = help::settings(current()))]
    Settings {
        #[command(subcommand)]
        cmd: SettingsCmd,
    },

    #[cfg_attr(feature = "desktop", command(about = help::desktop(current())))]
    // 无界面的 headless 构建也认得这个子命令（帮助里隐藏），执行时说明这个版本没有界面、
    // 怎样换装，而不是让用户看到 clap 的「unrecognized subcommand」（见 commands::desktop）。
    // 那里的 `aproxy desktop --help` 也换一段说明：上面的文档注释说的是带界面的构建（显式的
    // about 排在文档注释之后，覆盖它）
    #[command(hide = cfg!(not(feature = "desktop")))]
    #[cfg_attr(
        not(feature = "desktop"),
        command(
            about = help::headless(current()),
            long_about = None
        )
    )]
    Desktop {
        /// 菜单项按可执行文件所在的 bin 选择 home，不依赖桌面会话继承的环境。
        #[arg(long, hide = true)]
        installed_home: bool,
    },
}

/// `aproxy install` 的参数集。
#[derive(clap::Args, Debug, Clone, PartialEq)]
pub struct InstallArgs {
    #[arg(help = help::install_version(current()))]
    #[arg(value_name = "VERSION", conflicts_with_all = ["from", "adopt", "abort"])]
    pub version: Option<String>,

    #[arg(help = help::from(current()))]
    #[arg(long, value_name = "PATH", conflicts_with_all = ["adopt", "abort", "version"])]
    pub from: Option<String>,

    #[arg(help = help::adopt(current()))]
    #[arg(long, conflicts_with_all = ["from", "abort", "version"])]
    pub adopt: bool,

    /// 只在收编时提供 PATH 集成，普通升级不改用户环境。
    #[arg(long, requires = "adopt", help = help::no_path(current()))]
    pub no_path: bool,

    /// 菜单项与 PATH 可以分别选择。
    #[arg(long, requires = "adopt", help = help::no_shortcut(current()))]
    pub no_shortcut: bool,

    /// 引导脚本已验证并落位镜像，只做收编的用户环境部分，不交换、不下载。
    #[arg(long, hide = true, requires = "adopt")]
    pub integrate_only: bool,

    #[arg(help = help::abort(current()))]
    #[arg(long, conflicts_with_all = ["from", "adopt", "version"])]
    pub abort: bool,

    #[arg(help = help::force_install(current()))]
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

    #[arg(help = help::yes(current()))]
    #[arg(long, requires = "force")]
    pub yes: bool,

    #[arg(help = help::variant(current()))]
    // 它在 --force 下的写法属于永久兼容契约 C7；普通安装也接受它是放宽，C7 允许放宽
    #[arg(
        long,
        value_name = "VARIANT",
        value_parser = ["desktop", "headless"],
        conflicts_with_all = ["adopt", "abort", "skills_only", "continue_"]
    )]
    pub variant: Option<String>,

    #[arg(help = help::cpu(current()))]
    #[arg(long, value_name = "CPU", value_parser = ["x86-64-v3", "baseline"], conflicts_with_all = ["from", "adopt", "abort"])]
    pub cpu: Option<String>,

    #[arg(help = help::downgrade(current()))]
    #[arg(long)]
    pub allow_downgrade: bool,

    #[arg(help = help::pre(current()))]
    #[arg(long, conflicts_with_all = ["from", "adopt", "abort"])]
    pub pre: bool,

    #[arg(help = help::download_proxy(current()))]
    #[arg(long, value_name = "URL")]
    pub download_proxy: Option<String>,

    #[arg(help = help::no_skills(current()))]
    #[arg(long)]
    pub no_skills: bool,

    #[arg(help = help::skills_only(current()))]
    #[arg(long, hide = true, conflicts_with_all = ["from", "adopt", "abort", "continue_", "handover_from"])]
    pub skills_only: bool,

    #[arg(help = help::check(current()))]
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

    #[arg(help = help::install_json(current()))]
    // 续作与交棒的进程没有读它输出的人；--force 是冻结的另一条路径（C7），不带这个参数
    #[arg(long, conflicts_with_all = ["force", "abort", "continue_", "skills_only"])]
    pub json: bool,

    #[arg(help = help::resume(current()))]
    #[arg(long, hide = true)]
    pub continue_: bool,

    #[arg(help = help::handover(current()))]
    #[arg(long, hide = true, value_name = "PID", requires = "continue_")]
    pub handover_from: Option<u32>,
}

/// 文档的版本随二进制，不提供独立选版或切换通道的入口。
#[derive(Subcommand, Debug, Clone, PartialEq)]
pub enum SkillCmd {
    #[command(about = help::verify(current()))]
    #[command(name = "__verify-package", hide = true)]
    VerifyPackage {
        #[arg(help = help::archive(current()))]
        archive: PathBuf,
        #[arg(help = help::packaged(current()))]
        #[arg(long)]
        packaged_crate: Option<PathBuf>,
        #[arg(help = help::exact_version(current()))]
        version: String,
    },
    #[command(about = help::skill_install(current()))]
    #[command(visible_alias = "upgrade")]
    Install {
        #[arg(help = help::skill_proxy(current()))]
        #[arg(long, value_name = "URL")]
        download_proxy: Option<String>,
        #[arg(help = help::json(current()))]
        #[arg(long)]
        json: bool,
    },
    #[command(about = help::skill_list(current()))]
    List {
        #[arg(help = help::json(current()))]
        #[arg(long)]
        json: bool,
    },
    #[command(about = help::skill_show(current()))]
    Show {
        #[arg(help = help::skill_name(current()))]
        name: String,
        #[arg(help = help::skill_file(current()))]
        file: Option<String>,
        #[arg(help = help::files(current()))]
        #[arg(long, conflicts_with = "file")]
        files: bool,
    },
}

#[derive(Subcommand, Debug, Clone, PartialEq)]
pub enum AliasCmd {
    #[command(about = help::alias_add(current()))]
    Add {
        #[arg(help = help::alias_new_name(current()))]
        name: String,
        #[arg(help = help::alias_path(current()))]
        #[arg(value_name = "PATH")]
        path: Option<String>,
    },
    #[command(about = help::alias_remove(current()))]
    Remove {
        #[arg(help = help::alias_name(current()))]
        name: String,
    },
    #[command(about = help::alias_list(current()))]
    List {
        #[arg(help = help::alias_json(current()))]
        #[arg(long)]
        json: bool,
    },
}

/// `aproxy config` 的参数集。独立成结构体以整体传递给处理函数，
/// 避免 main 与函数之间逐字段搬运十几个参数。
#[derive(Subcommand, Debug, Clone, PartialEq)]
pub enum SettingsCmd {
    #[command(about = help::settings_show(current()))]
    Show {
        #[arg(help = help::settings_json(current()))]
        #[arg(long)]
        json: bool,
    },
    #[command(about = help::settings_get(current()))]
    Get {
        #[arg(value_name = "KEY")]
        key: String,
        #[arg(help = help::get_json(current()))]
        #[arg(long)]
        json: bool,
    },
    #[command(about = help::settings_set(current()))]
    Set {
        #[arg(value_name = "KEY")]
        key: String,
        #[arg(value_name = "VALUE", allow_hyphen_values = true)]
        value: String,
        #[arg(help = help::changes_json(current()))]
        #[arg(long)]
        json: bool,
    },
    #[command(about = help::settings_unset(current()))]
    Unset {
        #[arg(value_name = "KEY")]
        key: String,
        #[arg(help = help::changes_json(current()))]
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Args, Debug, Clone, PartialEq)]
pub struct ConfigArgs {
    #[arg(help = help::config_file(current()))]
    #[arg(long, value_name = "ALIAS|PATH", global = true)]
    pub config: Option<String>,

    #[command(subcommand)]
    pub cmd: ConfigCmd,
}

#[derive(Subcommand, Debug, Clone, PartialEq)]
pub enum ConfigCmd {
    #[command(about = help::config_show(current()))]
    Show {
        #[arg(help = help::effective(current()))]
        #[arg(long)]
        effective: bool,
        #[arg(help = help::json(current()))]
        #[arg(long)]
        json: bool,
    },
    #[command(about = help::config_get(current()))]
    Get {
        #[arg(value_name = "KEY")]
        key: String,
        #[arg(help = help::source_json(current()))]
        #[arg(long)]
        json: bool,
    },
    #[command(about = help::config_set(current()))]
    Set {
        #[arg(value_name = "KEY")]
        key: String,
        #[arg(value_name = "VALUE|-", allow_hyphen_values = true)]
        value: String,
        #[arg(help = help::changes_json(current()))]
        #[arg(long)]
        json: bool,
    },
    #[command(about = help::config_unset(current()))]
    Unset {
        #[arg(value_name = "KEY")]
        key: String,
        #[arg(help = help::changes_json(current()))]
        #[arg(long)]
        json: bool,
    },
}

/// 构造实际解析用的完整命令树，包括 clap 自己生成的 help 子树，再统一附上文档入口。
pub fn command_tree() -> clap::Command {
    use clap::CommandFactory;
    /// 内部命令也可能被单独查询帮助，不能只修饰用户可见的第一层。
    fn annotate(mut cmd: clap::Command) -> clap::Command {
        let lang = current();
        if lang == Lang::Zh {
            // clap 不能单独设置 Usage 的标题。只改固定标题，正文、语法与注解仍由 clap 生成。
            cmd = cmd
                .help_template(help::usage_template(lang))
                .subcommand_help_heading(help::commands(lang))
                .mut_args(|arg| {
                    let heading = if arg.is_positional() {
                        help::arguments(lang)
                    } else {
                        help::options(lang)
                    };
                    let arg = if arg.get_id() == "help" && arg.get_long() == Some("help") {
                        if arg.get_long_help().is_some() {
                            arg.help(help::help_short(lang))
                                .long_help(help::help_long(lang))
                        } else {
                            arg.help(help::help_button(lang))
                        }
                    } else {
                        arg
                    };
                    arg.help_heading(heading)
                });
            if cmd.get_name() == "help" {
                cmd = cmd.about(help::help_command(lang));
            }
        }
        cmd.after_help(
            "Agents: run 'aproxy skill list' and 'aproxy skill show <name>' for full docs.",
        )
        .mut_subcommands(annotate)
    }
    let mut cmd = Cli::command();
    cmd.build();
    annotate(cmd)
}

fn absolute_login_home(raw: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(raw);
    if p.is_absolute() {
        Ok(p)
    } else {
        Err("login home must be an absolute path".into())
    }
}
