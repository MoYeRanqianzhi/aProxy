//! 命令树的说明和标题；选项、值的拼写与 clap 的注解在所有语言下相同。

use aproxy_ipc::message;

message!(version(), en: "Print the version as `aproxy v<version> (<variant>; <target>…)`", zh: "打印版本（`aproxy v<版本> (<变体>; <目标三元组>…)`）");

message!(start_target(), en: "Configuration alias from settings.json or path to a configuration file. Defaults to default_config in settings.json, otherwise ~/.aproxy/config.toml", zh: "配置别名（settings.json 中的别名表）或配置文件路径；不写时用默认配置文件 （settings.json 的 default_config，否则 ~/.aproxy/config.toml）");

message!(start_config(), en: "Path to the configuration file, equivalent to passing a path after start. Use a different listen_addr for each instance", zh: "配置文件路径，与把路径直接写在 start 后面等价。多开不同配置的实例时各自指定 （listen_addr 须互不相同）");

message!(start_url(), en: "Override base_url for this instance without editing the file. Restart, restore and the watchdog reuse the override", zh: "上游 API base URL，覆盖配置文件中的 base_url（只对这个实例生效，不写入配置； restart、restore 与看护者重拉时沿用）");

message!(start_addr(), en: "Override the listening address (listen_addr)", zh: "监听地址，覆盖配置文件中的 listen_addr");

message!(start_proxy(), en: "Override the upstream proxy URL (proxy)", zh: "上游代理 URL，覆盖配置文件中的 proxy");

message!(start_key(), en: "Override api_key (Authorization: Bearer <key>). Use - to read one line from stdin; keys on the command line are visible in shell history and to other processes", zh: "快捷 api_key（等效覆盖 Authorization: Bearer <key>），覆盖配置文件中的 api_key。 写 - 表示从 stdin 读一行：写在命令行上的密钥会留在 shell 历史里，本机其他进程也 看得到");

message!(start_log(), en: "Override log_file. By default the daemon chooses a random name under ~/.aproxy/logs/ at startup. Use aproxy logs to get its path from the instance over IPC", zh: "守护日志文件路径，覆盖配置文件中的 log_file。未指定时写入 ~/.aproxy/logs/ 下按 启动时刻随机命名的文件，实际路径以 aproxy logs 经 IPC 向实例询问到的为准");

message!(foreground(), en: "Run in the foreground and log to the console instead of starting a background instance", zh: "前台运行（日志输出到控制台，Ctrl+C 停止）；默认在后台运行");

message!(start_json(), en: "Write a JSON document with the start outcome, port, PID, log path and instance status. Not available with --foreground", zh: "输出一个 JSON 文档（给程序读）：启动了还是本来就在运行，端口、pid、日志路径与实例的 状态。前台运行没有「启动完成」这个时刻，不能与 --foreground 同用");

message!(status(), en: "List registered instances and check liveness over IPC. Use --idle or --busy to filter by activity (idle_timeout_secs; default 1800 seconds)", zh: "列出运行中的 aProxy 实例（来自实例注册表，存活以 IPC 探测为准）。 --idle 只列闲置实例，--busy 只列活跃实例（判定阈值 settings 的 idle_timeout_secs，默认 1800s）");

message!(idle(), en: "Show only instances idle longer than the threshold", zh: "只列闲置（空闲超过阈值）的实例");

message!(busy(), en: "Show only instances with requests within the threshold", zh: "只列活跃（阈值内有请求）的实例");

message!(json(), en: "Write a JSON document for programs to read", zh: "输出一个 JSON 文档（给程序读）");

message!(start(), en: "Start the proxy using a configuration alias or file path. Runs in the background by default. Manage aliases with `aproxy alias add`; for example, `aproxy start openrouter`", zh: "启动代理，可指定配置别名或配置文件路径。默认在后台运行。 别名用 `aproxy alias add` 管理，如 `aproxy start openrouter`。");

message!(stop(), en: "Stop a running instance. With one instance, use `aproxy stop`. Otherwise specify a port, configuration alias, `all` or `idle`. Use `aproxy stop idle --idle-threshold-secs 600` to override the default idle threshold of 1800 seconds", zh: "停止运行中的实例。 单个实例时可直接 `aproxy stop`；多实例必须指定端口号、`all`、配置别名 或 `idle`（可带秒数）： `aproxy stop 12345` / `aproxy stop all` / `aproxy stop openrouter` `aproxy stop idle`（闲置超 1800s 的全部实例）/ `aproxy stop idle --idle-threshold-secs 600`");

message!(target(), en: "Port, configuration alias, all or idle", zh: "端口号、配置别名、all 或 idle");

message!(idle_threshold(), en: "Idle threshold in seconds for the idle target. Overrides instances.idle_threshold_secs in settings.json", zh: "目标是 idle 时的空闲阈值（秒），覆盖 settings.json 的 instances.idle_threshold_secs");

message!(stop_force(), en: "Stop immediately without waiting for graceful shutdown. Check the PID and process creation time before terminating the process", zh: "立即强制终止（跳过优雅关闭，零等待；终止前按 pid + 进程创建时间核验身份防杀错）");

message!(results_json(), en: "Write a JSON document with each instance outcome and the totals", zh: "输出一个 JSON 文档（给程序读）：每个实例的结局与汇总");

message!(restart(), en: "Restart running instances using their original configuration file, overrides and environment. Does not start stopped instances. With multiple instances, specify a port, configuration alias, `all` or `idle`, as for stop", zh: "重启运行中的实例（停下它，再按它启动时的配置文件、覆盖参数与环境重新拉起）。 只负责重启已运行的实例：未启动的 target 会提示「未启动」而不会拉起。 单个实例时可直接 `aproxy restart`；多实例必须指定端口号、`all`、配置 别名或 `idle`（可加 --idle-threshold-secs），语义与 stop 一致。");

message!(restart_formats(), en: "Restart only the named format (repeatable), or all formats; do not reload config.toml", zh: "只重启指定的 format（可重复指定）或全部 format，不重读 config.toml");
message!(restart_force(), en: "Terminate the old process immediately without waiting for graceful shutdown, then restart it", zh: "立即强制终止旧进程（跳过优雅关闭，零等待）再重启");

message!(logs(), en: "Print the last lines of one instance log. Use -f to follow new lines until Ctrl+C. With multiple instances, select a port or alias; all is not supported", zh: "打印一个实例的守护日志的最近若干行然后退出；加 -f 接着实时输出新写进来的 （Ctrl+C 退出）。单个实例时可直接 `aproxy logs`；多实例必须指定端口号或配置别名。 不支持 all：一次只看一个实例。");

message!(log_target(), en: "Port or configuration alias", zh: "端口号或配置别名");

message!(lines(), en: "Number of recent lines to print", zh: "打印最近多少行");

message!(follow(), en: "Continue printing new lines until the instance stops or you press Ctrl+C", zh: "打印完最近的日志之后不退出，接着输出新写进来的，直到实例停止或 Ctrl+C");

message!(restore(), en: "Restore instances lost after a crash, power loss or system restart. Skip running instances. Keep records with missing configuration files for a later restore", zh: "一键恢复先前在运行、因崩溃/断电/系统重启而消失的实例。 无可恢复实例时静默结束（适合配置为开机自启）。 已在运行的实例自动跳过；配置文件此刻找不到的记录保留着，文件回来之后再恢复。");

message!(restore_json(), en: "Write a JSON document with each record's outcome and the totals", zh: "输出一个 JSON 文档（给程序读）：每条记录的结局与汇总");

message!(login_restore(), en: "Restore at login using the home in the system registration, not the login environment", zh: "登录登记专用的恢复入口，主目录来自系统登记项，不依赖登录环境中的变量。");

message!(login_home(), en: "Absolute path to the home registered for login", zh: "登录项所属的绝对主目录。");

message!(alias(), en: "Manage configuration aliases in ~/.aproxy/settings.json. An alias names a config.toml file for starting and stopping instances", zh: "管理配置别名（存于 ~/.aproxy/settings.json，内部配置不建议手改）。 别名指向某个 config.toml，让多开配置可以用名字快捷启停。");

message!(doctor(), en: "Check settings.json, configurations referenced by aliases, and TOML files in configuration directories. Includes port conflicts", zh: "全面体检全部配置（settings.json error 级检查 + 别名配置深入审查 + 配置目录 toml 扫描，均含端口冲突检测）");

message!(find(), en: "Find TOML configurations in config_dirs and the two default directories. Filter by alias, search text or port", zh: "从配置目录列表（settings.json 的 config_dirs + 默认两个）发现全部 config（toml），支持按别名归属与关键字/端口过滤");

message!(aliased(), en: "List only configurations with an alias; --unaliased lists those without one", zh: "别名归属过滤：--aliased 只列已配别名的，--unaliased 只列未配的");

message!(unaliased(), en: "List only configurations without an alias", zh: "只列未配置别名的");

message!(query(), en: "Match part of a path or alias, ignoring case", zh: "关键字过滤：匹配路径片段或别名（大小写不敏感）");

message!(port(), en: "Filter by listening port", zh: "按监听端口过滤");

message!(find_json(), en: "Write a JSON document with matching configurations, aliases and diagnostics", zh: "输出一个 JSON 文档（给程序读）：匹配的配置、别名与错误诊断");

message!(install(), en: "Install or upgrade aProxy under ~/.aproxy/bin/. Restart at most one instance at a time to minimize disruption to clients. `upgrade` is an alias. Online installs try github, npm, cargo-binstall and cargo in order. Configure download_chain and URL templates in settings, or use --download-proxy. Interrupted installs resume automatically. If an instance cannot start, restore it with the old binary and stop the upgrade with a nonzero exit code. If installation fails or stalls, `install --force` stops all aProxy processes for this home before replacing the binary", zh: "安装/升级 aProxy 到规范位置（~/.aproxy/bin/）：全程对客户端 ≈ 无感 （逐实例滚动重启，任一时刻至多一个实例在重启）。`upgrade` 是它的别名。 在线安装走下载链条（github → npm → cargo-binstall → cargo，可配 settings 的 download_chain/url 模板与 --download-proxy）；中断自动续作。 某实例在新版本下起不来时用旧二进制把它拉回并中止滚动（命令返回非零）。 普通安装失败或卡住时，`install --force` 结束全部 aproxy 进程后直接换二进制。");

message!(skill(), en: "Install, list or read the skill documents for aProxy", zh: "安装、列出或读取与 aProxy 配套的 skill 文档");

message!(config(), en: "View or edit a configuration file. Defaults to ~/.aproxy/config.toml; use --config to specify another file", zh: "查看或修改一份配置文件（默认是 ~/.aproxy/config.toml；--config 指定别的）");

message!(serve(), en: "Internal instance entry point. Read the launch specification written by a starter, watchdog or installer, including other aProxy versions. Do not run manually", zh: "[内部] 后台实例的入口：按交接文件里的拉起规格承载服务。由 start、restart、restore、 看护者与安装器拉起，也会被别的版本的 aproxy 拉起（安装时新旧版本互相拉起对方）， 所以名字与两个参数不能改。勿手动使用");

message!(serve_home(), en: "Absolute path to the instance home", zh: "实例所属的 home（绝对路径）");

message!(spec(), en: "Launch specification file", zh: "拉起规格的交接文件");

message!(watchdog(), en: "Internal watchdog entry point. Monitor all instances in one home. Other aProxy versions may invoke this command. Do not run manually", zh: "[内部] 看护者的入口：看护这个 home 里的全部实例。与 `__serve` 一样会被别的版本 拉起，名字与参数不能改。勿手动使用");

message!(watchdog_home(), en: "Absolute path to the monitored home", zh: "看护的 home（绝对路径）");

message!(settings(), en: "View or edit ~/.aproxy/settings.json, including aliases, configuration defaults, watchdog, logging and update settings. Also works with invalid settings so you can inspect and repair them", zh: "查看或修改全局设置（~/.aproxy/settings.json）：别名、config.toml 键的全局默认、 看护者、日志、安装更新。settings.json 有错误时也能用，用来查看与修复它");

#[cfg(feature = "desktop")]
message!(desktop(), en: "Open the desktop interface in a separate process. Closing it does not stop proxy instances. This preview displays sample data, not live instance data", zh: "打开桌面界面：独立的窗口进程，关掉窗口就退出，不影响运行中的代理实例。 这个版本的界面是预览版，显示的是演示数据，不是本机实例的真实状态。");

message!(install_version(), en: "Install `latest` (default) from the selected update channel (see --pre) or specify a version such as 0.2.0-alpha.1. Stable excludes pre-releases; pre includes them. Skip latest if it is not newer than the running version. Use --allow-downgrade for older versions; versions below 0.2.0 are not supported. With --force, reinstall even if already current. Cannot be combined with --from or --adopt", zh: "安装目标版本：`latest`（默认，按更新通道取最大版本——stable 通道只取 正式版，pre 通道含预发布；通道见 --pre）或具体版本号（如 0.2.0-alpha.1）。latest 不高于当前版本时提示「已是最新」不重装。 默认拒绝降级（--allow-downgrade 放行）；低于 0.2.0 的版本一律拒绝。 加 --force 时不与当前版本比较，总是安装。与 --from/--adopt 互斥。");

message!(from(), en: "Install from a local binary: copy, verify, swap atomically and restart instances one at a time. Its --version output selects the version (at least 0.2.0). Cannot be combined with --adopt or VERSION", zh: "从本地二进制文件安装（复制 → 校验 → 原子落位 → 滚动重启实例）。 二进制的 `--version` 自报版本即安装目标版本（低于 0.2.0 的拒绝）。");

message!(adopt(), en: "Install the running aProxy binary (for example, one installed by npm) in ~/.aproxy/bin/ using the normal installation procedure. Adoption is explicit, never automatic", zh: "收编：把当前运行的 aProxy（如包管理器/npm 安装的）迁移到标准位置 `~/.aproxy/bin/`——当前进程镜像作为安装源，走完整标准流水线。 显式执行，绝不自动。");

message!(abort(), en: "Abort installation before the swap and remove staging files and state. After the swap, installation can only continue", zh: "中止进行中的安装：仅 swapping 前可完全回滚（此后只进不退）。 清理状态文件与 staging 现场。");

message!(force_install(), en: "Stop the current user's aProxy processes for this home using the process ledger (instances, watchdog, installers and desktop), replace the binary, then restore instances from their launch specifications. Use if a normal install fails or stalls or the installation is damaged. This bypasses handover and interrupts all in-flight requests through those instances; confirm first. Use --yes in noninteractive mode. Specify VERSION or --from. The default, latest, cannot be older than the running version. An explicit version can downgrade, but never below 0.2.0. Does not update skills. Exit codes: 0 complete; 10 no process stopped; 11 processes stopped but binary not replaced; 12 binary replaced but not all instances restored", zh: "强制安装：不走交接，按进程台账结束这个 home 下当前用户的全部 aproxy 进程（实例、 看护者、安装者、桌面界面），换上新二进制，再用新版本的 restore 按原来的拉起规格恢复实例。 普通安装失败、卡住，或装着的版本坏了时用；会中断所有经 aproxy 的在途请求，先确认 （非交互环境加 --yes）。目标取 VERSION（默认 latest，不低于当前版本）或 --from； 写明的版本号可以降级，低于 0.2.0 的版本仍不装；不更新 skill 文档。退出码：0 完成； 10 一个进程都没结束；11 结束过进程、二进制没换；12 二进制已换、有实例没恢复");

message!(yes(), en: "Skip the force-install confirmation (required for noninteractive use). First confirm that interrupting requests through aProxy is acceptable", zh: "强制安装不再询问确认（非交互环境必须加）。加之前先向用户确认可以中断经 aproxy 的在途请求");

message!(variant(), en: "Select desktop or headless; defaults to the running variant. An online install can fall back to headless on Linux without the required libraries such as WebKitGTK; it explains the fallback or fails if desktop was requested explicitly. With --from, verify that the file matches the specified variant", zh: "要装的变体：desktop（带桌面界面）或 headless（无界面）；默认与正在运行的这个 aproxy 相同。带界面的版本在缺 WebKitGTK 等系统库的 Linux 上跑不起来：没写这个 参数时，在线安装下载的带界面版本跑不起来就改装 headless 并说明原因，写了 desktop 就报错。与 --from 同用时是核对：文件必须正是这个变体，否则不装");

message!(cpu(), en: "Select x86-64-v3 or baseline. Defaults to x86-64-v3 on Windows or GNU/Linux x86-64 when the CPU supports AVX2 and a build is available; otherwise baseline", zh: "指令集档：x86-64-v3 或 baseline（默认按本机 CPU：支持 AVX2、且这个目标发布了 x86-64-v3 档（x86_64 的 Windows 与 Linux gnu）→ x86-64-v3，否则 baseline）");

message!(downgrade(), en: "Allow an older version than the installed one. Versions below 0.2.0 are not supported", zh: "允许降级安装（目标版本 < 当前已安装版本时默认拒绝）；低于 0.2.0 的版本 仍然拒绝");

message!(pre(), en: "Select latest across stable and alpha, beta and release-candidate versions. Without this flag a pre-release follows the pre-release channel and a stable release follows stable. Does not affect explicit versions", zh: "latest 走预发布通道：在全部版本（含正式版与 alpha/beta/rc 预发布） 中取最大。不加时通道跟随当前版本——当前是预发布则默认即为预发布 通道，当前是正式版则只取正式版（不会被带到预发布）。只影响 latest 解析，指定具体版本号时无作用。");

message!(download_proxy(), en: "Proxy URL for this download only, independent of the request proxy in config.toml. Defaults to download_proxy in settings, then the system proxy. With --force, only this option or the system proxy is used", zh: "仅本次下载使用的代理 URL（与 config.toml 的请求代理绝对分离）； 未指定时用 settings 的 download_proxy，再未配置则走系统环境代理 （--force 不读 settings：只用这个参数，没给就走系统环境代理）");

message!(no_skills(), en: "Skip the skill update for this install. The global setting is skill_auto_update in settings.json", zh: "本次安装跳过 skill 文档更新（全局开关在 settings 的 skill_auto_update）");

message!(skills_only(), en: "Install only skills matching the running binary. With this installer entry point, --json is a usage error (exit 2); use skill install --json instead", zh: "只安装与当前二进制同版本的 skill；保留已发布安装器给用户的命令行。 旧入口与 --json 同用仍按参数错误退出（2）；文档结果请用 skill install --json。");

message!(check(), en: "Check which version would be installed: report current and latest versions and whether an update is available. Does not download or change anything. Accepts --pre, --variant, --download-proxy and --json", zh: "只查询 install 会装哪个版本：打印当前版本、通道里的最新版本与有没有更新， 不下载、不改动任何东西。可与 --pre、--variant、--download-proxy、--json 同用");

message!(install_json(), en: "Write JSON only to stdout (one event per line during installation; one document with --check). Human-readable messages go to stderr", zh: "机器可读的输出：标准输出上只有 JSON（安装时每行一个事件，--check 时一个文档）， 给人看的文字改写到标准错误");

message!(resume(), en: "Internal: resume idempotently from the saved install phase without prompting. Invoked by the watchdog, CLI or previous installer. Do not run manually", zh: "[内部] 续作模式：从 install.state 残留的 phase 幂等推进（看门狗/ CLI 入口/交棒自动拉起，全自动无人工询问）。勿手动使用。");

message!(handover(), en: "Internal: PID of the installer handing over ownership. Allows continuation while that PID is alive only if it still owns the installation state. This is a permanent cross-version interface; see install::flow. Do not run manually", zh: "[内部] 交棒：拉起本进程的安装者的 pid。带着它（且它仍是 install.state 的主人）的续作进程可以接手一个安装者健在的安装。旧版本的安装者会这样 拉起新版本，是永久接口，见 install::flow 模块文档。勿手动使用。");

message!(verify(), en: "Validate the archive and its documents before publication without modifying the user's installation", zh: "发布前用实际解包和正文读取规则校验文档，不改用户的安装。");

message!(archive(), en: "Release archive to verify", zh: "待核验的总包。");

message!(packaged(), en: "Also verify the packaged crate against the release archive without reading the working tree", zh: "同时核对 cargo 打好的包与发布总包，而不读取工作副本的正文。");

message!(exact_version(), en: "Exact version requested by the publisher", zh: "发布者请求的精确版本。");

message!(skill_install(), en: "Install the skills for the running version. The upgrade alias performs the same operation", zh: "安装当前版本的 skill 文档；upgrade 执行同一操作");

message!(skill_proxy(), en: "Proxy URL for this download only; overrides the download proxy in settings", zh: "仅本次下载使用的代理 URL，优先于设置中的下载代理");

message!(skill_list(), en: "List installed skills, versions, locations and files", zh: "列出已安装的 skill、版本、位置和文件");

message!(skill_show(), en: "Read and verify a file from the manifest. Defaults to SKILL.md", zh: "读取并核对清单中的文件，默认读取 SKILL.md");

message!(skill_name(), en: "Exact skill name from the manifest", zh: "清单中精确的 skill 名");

message!(skill_file(), en: "File name relative to the skill directory, with forward slashes", zh: "相对 skill 目录的文件名，使用正斜杠");

message!(files(), en: "List only this skill's file names", zh: "只列出这个 skill 的文件名");

message!(alias_add(), en: "Add or replace an alias. If PATH is omitted, use ~/.aproxy/config.toml", zh: "添加/覆盖别名（path 省略时指向默认配置 ~/.aproxy/config.toml）");

message!(alias_new_name(), en: "Alias name. Cannot be all or a number: start and stop interpret these as a reserved word or port", zh: "别名（不能为 all 或纯数字——会被 start/stop 当作端口/保留字解析）");

message!(alias_path(), en: "Path to config.toml; supports ~ expansion", zh: "指向的 config.toml 路径（支持 ~ 展开）");

message!(alias_remove(), en: "Remove an alias", zh: "删除别名");

message!(alias_name(), en: "Alias name", zh: "别名");

message!(alias_list(), en: "List all aliases", zh: "列出全部别名");

message!(alias_json(), en: "Write a JSON document with each alias path and whether the file exists", zh: "输出一个 JSON 文档（给程序读）：每个别名的路径与文件是否存在");

message!(settings_show(), en: "Print effective settings, including built-in defaults. Credentials are shown as placeholders", zh: "打印生效的全部设置（文件里的键盖在内置默认上）。密钥显示成占位对象");

message!(settings_json(), en: "Write a JSON document with effective settings and diagnostics", zh: "输出一个 JSON 文档：生效的设置与诊断");

message!(settings_get(), en: "Print the effective value of a dotted key, such as watchdog.enabled, aliases.work or defaults.timeouts.connect_secs", zh: "打印一个键的生效值。键是点号路径，例如 watchdog.enabled、aliases.work、 defaults.timeouts.connect_secs");

message!(get_json(), en: "Write a JSON document with the key and its effective value", zh: "输出一个 JSON 文档：键与它的生效值");

message!(settings_set(), en: "Set a recognized key to a JSON value (true, 30, [\"a\"], {{\"k\":1}}). String keys also accept plain text. Use - to read one line from stdin for credentials", zh: "设置一个键。值写成 JSON（true、30、[\"a\"]、{{\"k\":1}}）；字符串类型的键可以直接写 原文。值写 - 表示从 stdin 读一行（传密钥用）。只能设置认识的键");

message!(changes_json(), en: "Write a JSON document listing the instances that need a restart", zh: "输出一个 JSON 文档，里面列出要重启才生效的实例");

message!(settings_unset(), en: "Remove a key and return to the built-in default. Unknown keys can also be removed", zh: "删除一个键（回到内置默认）。任何键都能删，包括不认识的键");

message!(config_file(), en: "Configuration alias or path to inspect or edit. Defaults to default_config in settings.json, otherwise ~/.aproxy/config.toml", zh: "要查看或修改的配置文件：别名或路径。不写时用默认配置文件（settings.json 的 default_config，否则 ~/.aproxy/config.toml）");

message!(config_show(), en: "Print the values in the file. Credentials are shown as placeholders. Invalid files are shown with diagnostics and exit code 3", zh: "打印配置文件里写了的值。密钥显示成占位，不给明文。文件有错误时照样显示，附上 诊断，退出码 3");

message!(effective(), en: "Include settings.json and built-in defaults for missing keys. Show each value's source and list default credentials that were not inherited", zh: "打印生效的值：文件里没写的键补上 settings.json 的全局默认与内置默认，每个值 注明来自哪一层；全局默认里没有继承的凭据单独列出");

message!(config_get(), en: "Print the effective value of a dotted key such as base_url, timeouts.connect_secs or extra_headers.\"x-foo\"", zh: "打印一个键的生效值。键是点号路径，例如 base_url、timeouts.connect_secs、 extra_headers.\"x-foo\"");

message!(source_json(), en: "Write a JSON document with the value and its source", zh: "输出一个 JSON 文档：值与它来自哪一层");

message!(config_set(), en: "Set one recognized key, keeping comments and other contents. Use TOML (true, 30, [\"a\"], {{ k = 1 }}) or plain text for a string. Use - to read one line from stdin for credentials. Invalid changes are not written. Restart running instances for the change to take effect", zh: "设置一个键，只改这一个键：文件里的注释与其余内容原样保留。值写成 TOML（true、30、 [\"a\"]、{{ k = 1 }}）；字符串类型的键可以直接写原文。值写 - 表示从 stdin 读一行（传密钥 用）。只能设置认识的键；会让配置无效的改动不写入。运行中的实例要重启才生效");

message!(config_unset(), en: "Remove a key and return to the settings or built-in default. Unknown keys can also be removed", zh: "删除一个键（回到全局默认或内置默认）。任何键都能删，包括不认识的键");

message!(root(), en: "Local API proxy with infinite retries", zh: "支持无限重试的本地 API 代理");

#[cfg(not(feature = "desktop"))]
message!(headless(), en: "This headless build has no desktop interface. Run this command for installation instructions", zh: "这是无界面的 headless 版本，没有桌面界面：运行它会说明怎样换装带界面的版本");

message!(commands(), en: "Commands", zh: "命令");

message!(arguments(), en: "Arguments", zh: "参数");

message!(options(), en: "Options", zh: "选项");

message!(help_button(), en: "Print help", zh: "显示帮助");

message!(help_short(), en: "Print help (see more with '--help')", zh: "显示帮助（用 --help 查看详情）");

message!(help_long(), en: "Print help (see a summary with '-h')", zh: "显示帮助（用 -h 查看摘要）");

message!(help_command(), en: "Print this message or the help of the given subcommand(s)", zh: "显示本页或指定子命令的帮助");

message!(usage_template(), en: "{{before-help}}{{about-with-newline}}\nUsage: {{usage}}\n\n{{all-args}}{{after-help}}", zh: "{{before-help}}{{about-with-newline}}\n用法: {{usage}}\n\n{{all-args}}{{after-help}}");
