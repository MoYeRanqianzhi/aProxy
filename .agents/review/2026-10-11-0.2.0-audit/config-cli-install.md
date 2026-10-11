# 0.2.0 审查：配置、命令行界面与安装/升级（2026-10-11）

> 只读审查：未改任何已有文件，未运行 cargo，未启动 aproxy 进程，未读写 `~/.aproxy/`。基线 master `c1d4b96`；`v0.1.0` = `d376275`（2026-10-04）。
> 范围：src/config.rs、src/settings.rs、src/cli.rs、src/main.rs、src/lib.rs、src/commands/{config,doctor,install,mod,start}.rs（为核对 CLI 契约另读了 alias/restore/restart 的片段）、src/doctor.rs、src/find.rs、src/install/**，tests/install_flow*.rs、tests/compat_v0_1_0.rs、tests/upgrade_from_0_1_0.rs；0.1.0 一侧用 `git show v0.1.0:<路径>` 读。
> 记号：【事实】= 读代码或 git 可复核，附 `file:line`（不带前缀的是当前 master，带 `v0.1.0:` 的是 tag 里的文件）；【推断】= 由代码推演、未经运行验证。
> 级别：P0 = 0.2.0 前必须改（多为之后再改就是破坏性变更的契约）；P1 = 0.2.0 应改，其中 ⚑ 表示「只有 0.2.0 能低成本改」；P2 = 之后。成本 S/M/L。
> 已知正在进行、本报告不重复建议的工作：删除 `src/compat_0_1_0.rs` 与 `continue_install` 的 relaying 豁免。

---

## 0. 总体判断

1. 上一轮（0.1.0 前）在本范围列的阻断与发布前项基本都落地了：更新通道与选版、滚动失败用旧二进制拉回、在线安装的管辖检查、URL 脱敏出口、非回环告警与 Host/Origin 白名单、settings 损坏时写命令拒写、早交接、一键脚本修复。install 的状态机、断电恢复、滚动与回滚彼此自洽，测试面广。
2. 风险转到了「契约」上。几处跨版本接口当初图省事，做成了「原样 argv 落盘」「整份结构体重写」「每个字段加一个专用旗标」。0.2.0 一发出，这些就冻结了。本轮的 P0 大多属于这一类。
3. 0.1.0 → 0.2.0：如果按现在的计划删掉 compat、别的不动，0.1.0 用户执行 `aproxy upgrade` 会在**交换二进制之后**才失败，属于半途坏掉（§3）。0.1.0 在交换之前只会以 `--version` 运行一次新二进制，用来自证版本。这是不写兼容代码就能让它干净失败的唯一检查点，所有下载渠道都要经过它。
4. i18n：本范围约 400 处要处理的中文字符串（121 处直接打印，约 66 个帮助项，210 个向外传的消息串），另有 30 处日志与 panic 文案要改成英文。其中 169 个消息串既打印给用户，又进 startup.log、install.state 或日志。要做到「用户看本地化、日志固定英文」，前提是消息类型化、在输出边界渲染（§5）。
5. skill 命令（§8）：
   - show 的安全不靠「把发布清单当白名单」本身，而是靠安装时写下的本地记录，加上在同一块内存上先核对哈希、再输出。
   - `skill install` 严格取二进制自己的版本，包格式因此不再是跨版本契约。
   - 三个渠道改用 CI 打出的同一个 zip。借着换资产名、换 npm 包内路径、让 crate 不再带 skill 目录，可以挡住 0.1.0 的安装者把 0.2.0 的 skill 装到 0.1.0 上（CI-01 的门只拦得住二进制）。

---

## 1. 发现

### P0

#### CI-01 0.1.0 的 `aproxy upgrade` 会在交换二进制之后才失败；用 `--version` 输出格式把它拦在交换之前
- 级别 P0｜类别 install / 跨版本契约｜成本 S
- 证据：
  - 【事实】早交接（`9e91a95`，2026-10-06）晚于 v0.1.0，0.1.0 的安装器自己驱动广播、交换与滚动：`v0.1.0:src/install/flow.rs:637-698`。
  - 【事实】交换之前，0.1.0 只在一处执行新二进制：`probe_version` 取 `--version` 输出里第一个以数字开头的 token，要求它等于目标版本（`v0.1.0:src/install/staging.rs:42-58`）。在线路径里这一步发生在建 install.state 之前（`v0.1.0:src/commands/install.rs:307-371`），`--from` 在入口就做（`v0.1.0:src/commands/install.rs:41-47`）。
  - 【事实】交换之后，0.1.0 要靠新版本的 S1/S3 应答终验（`v0.1.0:src/install/flow.rs:386-410`），Windows 上还要靠 relaying 豁免接力（`v0.1.0:src/install/flow.rs:686-695`）。这三样正是 0.2.0 要删的（`src/compat_0_1_0.rs`、`src/install/flow.rs:742-748`）。
  - 逐步推演与结局矩阵见 §3。【推断】有实例在跑时，install 以 1 退出，文案还会说实例已下线；最坏情况是 bin 已是 0.2.0、实例却跑在旧二进制上，新 CLI 联系不上它们。
- 为什么重要：「0.1.0 用户只能重新安装」只有在 0.1.0 的升级命令**改动任何东西之前**失败才成立。0.1.0 的 aproxy-cli skill 让 agent 用 `aproxy install` 升级，没读发布说明的用户和 agent 会直接撞上。
- 建议：
  1. 0.2.0 起 `--version` 输出改为 `aproxy v0.2.0`：`#[command(version = concat!("v", env!("CARGO_PKG_VERSION")))]`。这样输出里没有以数字开头的 token，0.1.0 的 `probe_version` 报「--version 输出无版本号」并以 1 退出：不建状态、不广播、不交换（Linux gnu 先回退 musl 重试一次，同样失败）。唯一残留是 `~/.aproxy/staging/0.2.0/` 里下载的文件。github、npm、binstall、cargo、url 模板与 `--from` 都要经过这一步。
  2. 0.2.0 的 `probe_version`（`src/install/staging.rs:45-58`）接受可选的 `v` 前缀。仍要读得出 0.1.x 的裸版本号，CI-06 的版本下限要用它。同步测试替身（`tests/install_flow.rs:612,716,780` 输出 `aproxy {版本}`）与文档。
  3. 必须在**第一个 0.2.0 预发布 tag 之前**合入：0.1.0 用户加 `--pre` 或写明版本号就能装到预发布。
  4. 把 CI 的 upgrade-from-0-1-0 任务（`.github/workflows/ci.yml:70-96`）与 `tests/upgrade_from_0_1_0.rs` 改成「拒绝」用例：用真实 v0.1.0 起实例，再用它的 `install --from <当前构建>`，断言非零退出、bin 的哈希不变、实例仍应答、没有 install.state。可选：用 `download_chain = [{"url": "http://127.0.0.1:<端口>/{asset}"}]` 指向本地文件服务，覆盖在线路径。
  5. `--version` 的新格式写进永久接口清单（记忆 ipc-v1-upgrade-contract 的继任版本）。备选方案的取舍见 §3.4。

#### CI-02 `.restore` 与守护拉起：原样 argv 落盘与转发，把整套启动语法和 `--api-key` 明文都变成了跨版本契约
- 级别 P0｜类别 跨版本契约 / 安全｜成本 M
- 证据：
  - 【事实】start 父进程把自己的整条命令行转发给守护子进程，只改写 `--config` 并追加 `--daemon-child`（`src/commands/start.rs:233-249`）。守护把「命令行去掉两个旗标」原样写进 `.restore`（`src/server.rs:152-157`），注释承认其中可能有 `--api-key` 明文（`src/daemon.rs:1028-1029`）。所以 `.restore` 里存的是 `["start","openrouter","--config","…"]` 这样的用户原始语法。
  - 【事实】看护者的命令行同样继承用户原始参数：`watchdog_spawn_args()` = 当前 argv 去掉两个旗标 + `--daemon-watchdog`（`src/watchdog.rs:338-342`），被 `start` 父进程调用（`src/commands/start.rs:342`）。用 `aproxy start --api-key sk-… ` 启动的话，守护进程与看护者的命令行在整个生命周期里都带着这把 key。
  - 【事实】`--api-key` 的帮助写着「仅本次运行生效」（`src/cli.rs:43-46`），实际会进 `.restore`。unix 上文件按默认 umask 落盘（CI-14）。
  - 【事实】看护者重拉、`aproxy restore`、install 滚动重启都用**自己的**二进制重新执行**别的版本**写下的 argv。`src/commands/restart.rs:175` 的注释已经在防「旧版本留下的参数本版本已不认识」。记忆 ipc-v1-upgrade-contract 也把「`.restore` 里可能出现的每个启动参数」列为永久接口。
- 为什么重要：raw argv 让「启动时接受过的每个旗标、每种子命令写法」永远不能改，帮助文案还与事实相反。0.2.0 之后要改，就得写兼容代码。
- 建议：
  - `.restore` 改为带版本的结构化记录：`{"v":1,"config_path":…,"overrides":{"base_url"?,"listen_addr"?,"proxy"?,"log_file"?},"log_path":…}`。字段只增不改，未知字段忽略。
  - 父进程把同一份结构写成规格文件，子进程用内部入口读取（见 CI-03）。看护者、restore 与 install 都消费同一种记录；看护者的命令行不带任何用户参数。
  - 机密：最好去掉顶层 `--api-key`（异议 D2）。如果保留，只能放进规格文件（unix 0600），永不进 argv。
  - 解析不了的 `.restore` 要报出来，不要删。
  - 测试：断言守护进程、看护者的命令行与 `.restore` 里都不出现 key。

#### CI-03 进程间内部调用面要在 0.2.0 定型（`--daemon-child`、`--daemon-watchdog`、`install --continue --handover-from`）
- 级别 P0｜类别 CLI 契约｜成本 S–M
- 证据：
  - 【事实】两个内部模式做成用户 CLI 上的隐藏**全局**旗标（`src/cli.rs:48-57`），而且必须是 global：start 转发 argv 时它们出现在子命令后面（记忆 alias-settings 的教训）。`main.rs` 见到它们就绕开子命令分派（`src/main.rs:60-74`），所以 `aproxy status --daemon-child` 会直接起一个代理服务。
  - 【事实】续作与交棒挂在用户命令 `install` 底下：`--continue`/`--handover-from`（`src/cli.rs:232-241`；拉起处 `src/install/flow.rs:370-376`、`src/main.rs:97-100`）。`--handover-from` 是 0.1.0 之后才加的，还没有发布过（与 `v0.1.0:src/cli.rs` 的差异只有这一项）。
  - 【事实】0.2.0 不再与 0.1.0 互通，这些名字现在都可以改。0.2.0 之后它们就是永久接口：较旧的安装者用这些参数拉起较新的二进制，较新的看护者用它们重拉较旧版本写下的记录。
- 为什么重要：内部协议寄生在用户语法里，等于用户语法（InstallArgs、全局旗标）也一起被冻结。
- 建议：
  - 改用隐藏的内部子命令：`aproxy __serve --spec <文件>`、`aproxy __watchdog`、`aproxy __install-resume [--handover-from <pid>]`（名字可议）。它们不需要 global，也不出现在用户帮助里。
  - install.state 增加可选字段 `installer_version`（写入者版本）。以后的目标版本在接手时可以据此拒绝不再支持的起点，失败点就在交换之前（§3.7）。
  - 定稿后写进永久接口清单。

#### CI-04 配置写回会毁掉未知键、固化默认值：0.2.0 起必须「只写改动的键、保留其余」
- 级别 P0｜类别 契约 / 数据安全｜成本 M
- 证据：
  - 【事实】`config::save_to` 是 `toml::to_string_pretty(整个 Config)` + `std::fs::write`（`src/config.rs:939-946`）：先截断再写，不是原子写；注释、键序、未知键全部丢掉。非 Option 字段的默认值也被写进文件：`listen_addr`、`keepalive_interval_secs`、`max_retry_backoff_secs`、`spool_limit_mb`、`connect_timeout_secs`、`read_timeout_secs`（`src/config.rs:191-386`），连带空表 `[extra_headers]`/`[override_headers]`。唯一写入口是 `src/commands/config.rs:232`。
  - 【事实】`settings::save_to` 是原子写，但整份序列化（`src/settings.rs:301-320`）；大多数字段是带默认值的非 Option（`src/settings.rs:43-151`），没有 `flatten` 捕获未知字段。全仓 config/settings/state 都没有 `skip_serializing_if` 或 `flatten`（grep）。
  - 【事实】settings 的字段集合与 v0.1.0 完全相同（diff `v0.1.0:src/settings.rs`）。0.1.0 用户只要执行过一次 `alias add` 或 `config --set-default`，settings.json 里就写死了全部默认值。
  - 【事实】`toml_edit 0.22.27` 已经在依赖树里，是 toml 0.8.23 的依赖（`Cargo.lock:1781-1803`）。直接使用它不增加体积。
- 为什么重要：(1) 0.2.0 二进制只要会整份重写，以后版本新增的键就会被它删掉（降级或多份二进制混用时）。这个性质只能在 0.2.0 里修好，事后补不回来。(2) 固化进文件的默认值以后跟不上新默认值，而且无法区分「用户显式写了 15」和「写回时顺带写进去的 15」。(3) 0.2.0 的 UI 会频繁写这两个文件。
- 建议：
  - config.toml：用 `toml_edit::DocumentMut` 就地只改目标键，tmp+rename 原子写；新建文件只写显式给出的键。
  - settings.json：可调字段改为 `Option<T>` + `skip_serializing_if = "Option::is_none"`，默认值由访问器给；加 `#[serde(flatten)] extra: serde_json::Map<String, Value>` 保留未知字段。
  - 测试：往返之后注释与未知键都在；写一个键不会带出其他默认值。
  - 发布说明里提示删掉 0.1.0 写进来的默认值（§3.6）。

#### CI-05 `aproxy config` 的写入面：要在 0.2.0 前决定专用旗标还是通用 get/set
- 级别 P0（决策）｜类别 CLI 契约｜成本 M（含文档与 skill）
- 证据：
  - 【事实】`ConfigArgs` 有 15 个专用旗标（`src/cli.rs:263-313`），只覆盖 config.toml 26 个字段里的 base_url、listen_addr、api_key、两类头、keepalive_interval_secs 与代理三项（字段见 `src/config.rs:191-386`）。keepalive_trigger、keepalive_heartbeat、超时、max_body_mb、disk_cache、forward_only、bounded_retry_paths、allowed_hosts/origins、三个 transform、log_file 都没有命令可设。
  - 【事实】旗标名与键名不一一对应：`--baseurl`↔`base_url`、`--listen`↔`listen_addr`、`--keepalive-secs`↔`keepalive_interval_secs`。
  - 【事实】settings.json 的 20 个字段里，只有别名和 default_config 有命令（docs-12 未修）。`--set-default/--clear-default` 写的是 settings，却挂在 `config` 底下（`src/commands/config.rs:85-117`）。
  - 【事实】`config --listen`/`--proxy` 存盘前不校验（`src/commands/config.rs:131,180`；只校验 base_url，`:225`）。
- 为什么重要：CLI 是永久接口。保留专用旗标，就意味着以后每加一个字段都要加一个旗标、一份翻译，再写一遍校验；0.2.0 的 UI 和 agent 又都需要一个通用后端。
- 建议（需用户拍板）：
  - 用 `aproxy config show [--effective] [--json]`、`config get <键>`、`config set <键> <值>`、`config unset <键>` 取代专用旗标。键就是文件里的键，表用点号（`request_transform.command`、`extra_headers.X-Foo`）；值按 schema 解析；`-` 表示从 stdin 读，用于机密。
  - settings 同构：`aproxy settings get/set/unset`。
  - 写入走 CI-04 的写回层，写前走 CI-08 的统一诊断。0.2.0 删除专用旗标，不留别名。

#### CI-06 版本下限：0.2.0 必须拒绝装回 0.1.x，包括自动续作带进来的 0.1.x 残留安装
- 级别 P0｜类别 install｜成本 S
- 证据：
  - 【事实】`check_downgrade` 有 `--allow-downgrade` 就放行任何版本（`src/install/download/mod.rs:199-212`）。`--from`/`--adopt` 路径**完全不检查降级**（`src/commands/install.rs:38-96`），与帮助「默认拒绝降级」矛盾（`src/cli.rs:180`）。
  - 【事实】降级由较新的一方驱动（`src/install/flow.rs:324-333`），驱动者要能与较旧的实例对话。0.2.0 删掉 compat 后，它与 0.1.x 实例不通。
  - 【事实】任何命令只要看到 install.state 就会拉起 `install --continue`（`src/main.rs:89-101`），看护者每天也扫一次。续作遇到未 halted 的 failed 且有 `from_path`，就从备料重来（`src/install/flow.rs:781-788`）。
  - 【推断】0.1.0 时代残留的一份 failed `--from` 安装状态（目标 0.1.x）带进 0.2.0 的 home 后，会被**自动**续作成「0.2.0 驱动的降级」，交换之后坏掉，与 CI-01 同一种结局。
  - 【事实】halted 的失败指引建议 `aproxy install <旧版本> --allow-downgrade`（`src/commands/install.rs:488-491`）。
- 建议：
  - 定义常量 `INSTALL_FLOOR = 0.2.0`，在四处拒绝低于它的目标：`check_downgrade`（不论有没有 `--allow-downgrade`）、`--from`/`--adopt` 自证之后、`continue_install`（目标低于下限的残留状态直接清掉，并写一行英文日志）、指引文案。`--skills-only` 可以放行。
  - `--from` 补上降级检查（需要 `--allow-downgrade`）。

### P1

#### CI-07 failed 状态丢失了失败时的阶段：`--abort` 拒绝一切失败的安装；在线来源失败后永远续不了
- 级别 P1｜类别 install bug｜成本 S–M
- 证据：
  - 【事实】`fail_in` 把 phase 直接改成 Failed，不记录失败前的阶段（`src/install/state.rs:374-381`）。枚举序里 Failed 排在 Done 之后（`src/install/state.rs:34-50`）。
  - 【事实】`--abort` 的守卫是 `state.phase >= Swapping`（`src/commands/install.rs:529`），所以对**任何** failed 都拒绝，包括交换前就失败、bin 未动的情况。可失败指引恰恰叫用户用 `--abort`（`src/commands/install.rs:494`），`broadcast.rs:10-11` 也写「终失败 → 调用方走 abort」。
  - 【事实】测试只覆盖 Downloaded 与 Swapped 两种 abort（`tests/install_flow.rs:412-462`），没有 failed 的用例。
  - 【事实】续作对 Failed 一律要 `from_path`（`src/install/flow.rs:781-788`）。在线来源永远没有这个字段，于是报「续作缺少 --from 源记录（状态文件损坏？）」，哪怕 staging 完好、哪怕失败发生在交换之后、新 bin 已就位。上一轮的 install-07 至今未修。
  - 【事实】每条命令和看护者都会拉起这种注定失败的续作（`src/main.rs:89-101`）。
- 建议：
  - install.state 加可选字段 `failed_at: InstallPhase`（较旧的读者会忽略它）。`--abort` 按 `failed_at < Swapping` 判断。
  - 续作按 `failed_at` 与现有来源（`staged_path`、`from_path`、已就位的 bin）决定从哪里接着做，都没有时给出准确的提示「请重新执行 aproxy install」。
  - `main.rs` 只在状态可接管、未 halted、可以续作时才拉起续作。补 failed 场景的 abort 与续作测试。

#### CI-08 配置的解析与校验分成三条管线（start、doctor、config --show），彼此不一致
- 级别 P1｜类别 config｜成本 M
- 证据：
  - 【事实】start：严格加载 → CLI 覆盖 → 注入 settings 全局默认 → 检查端口 → validate，并对消息打码（`src/commands/start.rs:18-87`）。
  - 【事实】doctor：宽松解析，丢掉错误详情，只报「无法解析为有效 aProxy 配置」（`src/doctor.rs:347-351`）。默认配置只查非回环暴露，注释自述「默认配置的全面校验不在本检查的职责内」（`src/doctor.rs:120-145`，第 126 行）。别名与目录配置校验时不注入 settings，所以 settings 里打开的 `forward_only` 与 transform 互斥这类问题查不出来（`src/doctor.rs:206-210,293-297`）。别名配置的校验失败算 Warn，可它会让 `start <别名>` 失败。退出码恒为 0（`src/commands/doctor.rs:8-22`）。
  - 【事实】config --show：宽松加载，对于 settings 提供默认值的字段只显示「运行时取 settings.json 全局默认」，不给生效值（`src/commands/config.rs:241-470`）。
  - 【事实】未修的旧条目：非法头名、头值与 api_key 在运行期被静默跳过（`src/proxy.rs:1203-1240`，`src/proxy.rs:4286` 的测试把它固化成了预期，config-05）。`start <别名>` 指向不存在的文件时报「base_url 不能为空」（`src/commands/start.rs:22` 只检查显式 `--config`，config-10）。config 存盘前不校验 listen/proxy（config-12）。
  - 【事实】消息质量：`base_url 不能为空，请在 ~/.aproxy/config.toml 中配置` 写死了路径（`src/config.rs:518`）；任何配置错误都附一句 `aproxy config --baseurl https://api.anthropic.com`（`src/commands/start.rs:80-84`）。
- 建议：
  - 提供一个 `config::resolve(path, &Overrides, &Settings) -> Resolved { config, sources, diagnostics }`。`Diagnostic` 包含严重度、稳定 code、字段路径和参数。start、doctor、`config show --effective`、find 与 UI 共用它。
  - validate 补上头名、头值、api_key 与 listen_addr 的检查；transform command 能否解析只在 doctor 里告警。
  - doctor 深检默认配置，有 error 时以非零退出，报语法错误的行列。别名指向缺失文件时启动报准确的错误。

#### CI-09 未知键静默忽略（包括 0.1.0 配置里可能残留的 `upstream_url`）
- 级别 P1｜类别 config｜成本 S
- 证据：【事实】Config、Settings 都没有未知键捕获（`src/config.rs:189-386`、`src/settings.rs:43`）；上一轮的 config-03 未修。`535efaa` 删掉了 `upstream_url` 别名，0.1.0 还接受这个旧写法。所以用旧写法的配置到了 0.2.0 会静默丢掉上游地址，启动时只看到「base_url 不能为空」。【推断】这种配置很少，只有从 alpha 时代起没再执行过 `aproxy config` 的文件才会有。
- 建议：用 `serde_ignored` 或 `toml::Table` 与已知键集求差（含三个 transform 子表），在 start（stderr 与日志）、doctor、`config show` 里告警并给出近似建议。不报错，保留前向兼容。settings.json 同样处理。

#### CI-10 settings.json 健壮性：一个字段错，运行期整份静默回退默认
- 级别 P1｜类别 config｜成本 M
- 证据：
  - 【事实】任何 serde 错误都让整份回退默认，提示由库层 `eprintln!` 打印（`src/settings.rs:240-256`）。start 用这份宽松结果注入全局默认（`src/commands/start.rs:40-52`）。所以只要某个字段写错了类型，`watchdog:false` 会悄悄变回 true，max_body_mb 回到 128，只留一行 `[ERROR]`。
  - 【事实】`download_chain` 的 `ChainStep` 遇到未知渠道名直接报错（与 v0.1.0 同一份实现：`src/install/download/mod.rs` 的手写 Deserialize）。以后版本加了新渠道，旧二进制读到时整份 settings 作废。
  - 【事实】类型错误也报成「JSON 语法错误」（`src/settings.rs:467`）。
  - 【事实】没有 schema 版本；读改写没有并发保护（`src/commands/alias.rs:25-31`）。【推断】UI 与 CLI 同时写时会丢更新。
- 建议：按字段容错解码，坏字段告警并用默认值，好字段保留；未知渠道跳过并告警；加 `"schema": 1`，写入者遇到更高的 schema 就拒绝写；写前比较文件哈希（CAS）或加锁文件；错误分「语法」与「类型」两种措辞。

#### CI-11 ⚑ 键名与分层：如果要改，只能在 0.2.0 改
- 级别 P1⚑（需决策）｜类别 config 契约｜成本 M
- 证据：
  - 【事实】settings.json 里混着三类东西：程序状态（aliases、default_config、config_dirs）、全局策略（log_rotate_mb、idle_timeout_secs、watchdog_*、download_*、skill_auto_update），以及 7 个 config.toml 字段的全局默认（max_body_mb、disk_cache、forward_only、bounded_retry_paths、allowed_hosts、allowed_origins、keepalive_trigger，`src/settings.rs:68-105`）。其余可调字段（keepalive_interval_secs、max_retry_backoff_secs、超时、spool_limit_mb、keepalive_heartbeat）没有全局默认层，规则无从解释。
  - 【事实】`idle_timeout_secs` 在 settings 里表示实例闲置阈值（`src/settings.rs:64`），在 TransformConfig 里表示 worker 回收（`src/config.rs:148`），同名不同义。
  - 【事实】命名风格不统一：`max_body_mb` 对 `spool_limit_mb`。名字写 MB，实际按 MiB 计算（`src/config.rs:390-396`）。
- 建议（给用户拍板）：settings.json v1 的形状改为 `{schema, language, aliases, default_config, config_dirs, defaults:{<任何 config.toml 键>}, watchdog:{enabled, heartbeat_secs, …}, logs:{rotate_mb}, instances:{idle_threshold_secs}, update:{download_chain, download_proxy, skill_auto_update}}`。`defaults` 与 config.toml 同构，每个字段的优先级统一为「CLI 覆盖 > toml > settings.defaults > 内置」，UI 也能标出每个字段的来源。如果不改形状，至少给 settings 的 `idle_timeout_secs` 改名，并新增 `language`。代价：0.1.0 的 settings.json 里少数进阶键要按迁移表手改（别名不变），CI-09 的告警会提示。

#### CI-12 i18n 基础设施（详见 §5）
- 级别 P1｜类别 i18n｜成本 L
- 要点：
  - 消息与错误类型化：每条带稳定 code 和参数，在边界渲染。控制台按语言本地化；日志、startup.log、IPC 与 install.state 用英文加 code。
  - 库不再直接打印：`settings::load_from`、`check_and_report`、`doctor::Report::print`、`find::print_list` 改为只返回数据。
  - clap 帮助：clap 条目上的 `///` 改为 `//`，改用运行时表达式提供文案，再加一个遍历命令树的完整性测试；内置的标题与错误另行处理。
  - 测试里 113 处按中文子串断言，要固定语言或改断言 code。
  - CLI 进程的 tracing 改为输出到 stderr（见 CI-13）。

#### CI-13 机器可读面：stdout 干净、`--json`、退出码表、安装进度与后台日志
- 级别 P1｜类别 CLI 契约 / UI 前置｜成本 M
- 证据：
  - 【事实】CLI 进程在 INFO 级把 tracing 输出到 **stdout**（`src/server.rs:411-417`），于是 `tracing::info!(pid, "看护者已随实例启动拉起")`（`src/commands/start.rs:347`）这类日志行混进用户输出。将来加 `--json` 时，这些行会破坏 JSON。
  - 【事实】全仓没有 `--json`（grep）。退出码只有 0 和 1（clap 用法错误是 2）。doctor 有错误也退出 0；restore 恢复失败也退出 0（`src/commands/restore.rs:75-83`）。
  - 【事实】后台续作进程没有日志去处。Windows 上 spawn_detached 不继承任何句柄（`src/daemon.rs:609-690`），unix 上接手者的 stdout/stderr 接 null（`src/install/flow.rs:377-384`）。接手者的回滚告警等记录全部丢失，只有失败时的 `last_error` 留在 install.state 里，成功后连状态文件一起删掉。
  - 【事实】UI 要展示安装进度，但「状态文件只有 install 自己读」是用户定调（`src/install/state.rs:3-5`）。
- 建议：
  - CLI 日志改到 stderr，交互命令默认 WARN 级。
  - status、find、doctor、`config show`、`alias list` 加带 `schema` 字段的 `--json`。
  - 0.2.0 定下退出码表（例：0 成功、1 一般失败、2 用法错误、3 配置无效、4 目标不存在、5 部分失败）。
  - `install --progress json|human`：等结局的安装者本来就在轮询 install.state（`src/commands/install.rs:447-481`），由它把阶段变化转成事件输出。这样不违背「状态文件只有 install 自己读」。另加 `install --check [--json]`。
  - 后台续作的日志写到 `logs/install.log`（英文）。

#### CI-14 安全：权限、doctor 输出凭据、打码出口分裂、命令行里的密钥
- 级别 P1｜类别 security｜成本 S–M
- 证据：
  - 【事实】全仓只有可执行位 0o755（`src/install/staging.rs:105-106`、`src/install/download/crates.rs:164-165`）。config.toml、settings.json、`.restore`、日志与 run 目录都按默认 umask 落盘（config-09、security-07、daemon-ipc-08 未修）。
  - 【事实，新】validate 的错误消息里嵌着原样的 proxy 与 base_url（`src/config.rs:522-533,606-616`）。start 先做字符串替换打码再输出（`src/commands/start.rs:61-72`），doctor 却把 `{e}` 原样打印（`src/doctor.rs:206-210,293-297`）。例如 `socks://user:pass@h` 这样协议写错的代理，或带 `?key=` 的 base_url，经 doctor 输出时是明文。
  - 【事实】打码出口分在两处：URL 用库里的 `config::mask_base_url`（`src/config.rs:829-914`），不透明密钥用二进制侧的 `util::mask_secret`（`src/util.rs:38-42`）。UI 是独立的 crate，复用不了后者。
  - 【事实】`config --api-key`、`--proxy-password` 让密钥出现在命令行（`src/cli.rs:273-293`）。安装脚本的收尾提示也教用户这么写（`scripts/install.sh:334`）。
- 建议：
  - unix 上目录 0700、文件 0600：tmp 文件创建时就用 `OpenOptionsExt::mode`，已有文件显式 `set_permissions`；doctor 对组或其他用户可读的文件告警。
  - validate 改为产出类型化错误，消息里的值在源头打码，删掉 start 里的字符串替换。
  - 新建 `aproxy::redact` 模块（mask_url、mask_secret），作为 UI 也能用的唯一出口。
  - `config set api_key -` 从 stdin 读密钥；脚本的收尾提示随之更新。

#### CI-15 skill 支线仍由旧安装者执行，是「旧驱动新」的残留
- 级别 P1｜类别 install｜成本 S
- 证据：【事实】`drive_with_skill` 在调用它的进程里起 skill 下载（`src/install/flow.rs:199-245`）。在线安装和 `--from` 的安装者把 hand_over 包在它里面（`src/install/flow.rs:621-660`），所以是旧版本的代码去下载、解包**目标版本**的 skill 包。被指名的接手者不跑 skill（`src/install/flow.rs:760-765`）。交棒后，安装者的 skill 结果因为 `installer_pid != self` 不再落盘（`src/install/flow.rs:230-233`）。
- 为什么重要：skill 包的资产名、zip 布局和 npm 主包里的路径因此都成了旧代码必须看得懂的跨版本契约，违背早交接「目标版本驱动」的原则。
- 建议：skill 支线交给接手者执行。`--no-skills` 已经以 `skill: Skipped` 记在状态里（`src/install/flow.rs:292-300`）；下载代理在拉起接手者时经环境变量传过去，不要写进状态文件。配合 §8.4「skill 严格取二进制同版本」，包格式就只会被同版本的代码读取。

#### CI-16 binstall 与 cargo 渠道零校验，文档却声称有校验
- 级别 P1｜类别 install / security｜成本 S
- 证据：【事实】模块文档写「cargo-binstall=模板 checksum（有则验）；cargo(build)=crates.io checksum」（`src/install/download/mod.rs:9-13`）。实际 binstall 只算 sha256、不比对；没有模板时直接取 GitHub 资产，也不验它的 `.sha256`（`src/install/download/crates.rs:49-86`）。cargo 渠道「以 CDN 内容为准」（`src/install/download/crates.rs:24-29`）。上一轮的 security-10 与 install-13 未修。
- 建议：binstall 取 GitHub 资产时复用 github 渠道的 `.sha256` 强校验，或者干脆删掉这个渠道（它的兜底 URL 就是 GitHub 资产）。cargo 渠道用 crates.io API 的 `checksum` 比对 `.crate`。至少把文档改成如实描述。

#### CI-17 CLI 形状：启动专用的覆盖参数放在顶层，`--config`/`--log-file` 是全局参数，`upgrade` 是重复的子命令变体
- 级别 P1｜类别 CLI｜成本 S
- 证据：
  - 【事实】`--baseurl/--listen/--proxy/--api-key/--foreground` 都在顶层（`src/cli.rs:19-46`）。`--config` 与 `--log-file` 是 global（`src/cli.rs:16,40`），`aproxy status --log-file x`、`aproxy install --config x` 都被接受后再静默忽略。
  - 【事实】`Install(InstallArgs)` 与 `Upgrade(InstallArgs)` 是两个变体（`src/cli.rs:165-168`；`src/main.rs:89-92,135-137`），帮助里出现两遍。
  - 【事实】`--pre` 与 VERSION 并用时不冲突，被静默忽略（`src/cli.rs:209-214`）。`stop <target> [SECS]` 的位置参数只对 idle 有意义（`src/cli.rs:90-100`）。
- 建议：覆盖参数移到 `start` 下面（裸 `aproxy` 的语义见异议 D4）；`--config` 只放在需要它的子命令上；`#[command(visible_alias = "upgrade")]`；`--pre` 与 VERSION 互斥；`stop idle --older-than <秒>`。

### P2

#### CI-18 结构：config.rs 该拆，路径工具有重复，文件职责要理顺
- 级别 P2｜类别 结构｜成本 M
- 【事实】`src/config.rs` 共 2044 行：约 950 行是代码（大部分是文档注释），约 1100 行是测试。schema、转换器类型、保活类型、校验、暴露分析、路径、读写、URL 脱敏全在这一个文件里。
- 【事实】重复与不一致：
  - `config::expand_tilde`（`src/config.rs:779-797`）与 `settings::expand_path`（`src/settings.rs:329-341`）是两个 `~` 展开器，语义不同。
  - `settings::path_match_key` 只在 Windows 上转小写（`src/settings.rs:345-364`），`flow::is_under` 却在所有平台都转小写、把 `/` 换成 `\`（`src/install/flow.rs:76-92`）。在区分大小写的 unix 上，这个判断原则上会错。
  - 默认配置路径有三种来源：`config::config_path()`（`src/config.rs:800-803`；`alias add` 用的是它，忽略了 default_config，config-11 未修，`src/commands/alias.rs:17-24`）、`settings::default_config_path()`、doctor 与 find 里写死的 `home.join("config.toml")`（`src/doctor.rs:251`、`src/find.rs:39`）。
  - doctor 的目录扫描与 `find::discover_in` 重复（`src/doctor.rs:233-330` 对照 `src/find.rs:25-73`）。
  - 下载代理的归一逻辑写了三遍（`src/commands/install.rs:106-110,147-150,209-212`）。
- 建议：拆成 `config/{mod,transform,keepalive,validate,io}.rs`，另建 `redact.rs` 与 `paths.rs`；与 CI-08 的重构一起做，减少返工。

#### CI-19 main.rs 的分派
- 级别 P2｜成本 S
- 【事实】分派本身清楚，问题有两个。一是只读命令也会拉起续作（`src/main.rs:89-101`）：状态 halted 或续作不了时，每条命令都多出一个注定被拒的进程（`src/install/flow.rs:715-731`）；UI 若靠 CLI 轮询，就会大量产生这种进程。二是 settings 检查由库层直接打印（`src/main.rs:80-82` → `src/settings.rs:510-515`）。
- 建议：拉起续作前先判断状态可接管、未 halted、可以续作；检查结果由 bin 渲染输出。

#### CI-20 去掉兼容之后 install 还能怎样简化
- 级别 P2｜类别 install 设计｜成本 M
- 【事实】现状与建议：
  - (a) Relaying 只由 0.1.0 写入（`src/install/state.rs:39-43`），可以从词表里删掉，`(Swapped, Restarting)` 的特判也随之简化（`src/install/state.rs:90-95`）。
  - (b) Windows 上交换之后还有第二次交棒，从 staging 副本交给 bin（`src/install/flow.rs:930-945`），只为了让 cleaning 能删掉 staging。可以改成接手者不删自己镜像所在的 staging 目录，由下一次安装或 CLI 入口清扫过期的 staging。这样少一次交棒，也少一个 30 秒超时分支。
  - (c) `fleet_already_on_target`（`src/install/flow.rs:666-695`）是通用的自愈逻辑，值得保留，但注释里不要再引用 0.1.0。
  - (d) 宣告的介质在 unix 上是 `/dev/shm`，只有 Linux 有。用户已定调 macOS 必须支持（记忆 macos-support-required）。可以考虑改用 `run/install.lock` 文件锁（flock/LockFileEx）：进程一死锁就自动释放，挂死仍靠心跳判定。
  - (e) 看护者的 R1（守护启动时退役更旧的看护者，claim 没有 version 视为更旧，`src/watchdog.rs:46-97`）不是应删的兼容代码。迁移窗口里残留的 0.1.0 看护者就靠它退役，务必保留。

#### CI-21 错误信息不对症
- 级别 P2｜成本 S
- 【事实】
  - 不管什么配置错误，都附一句 `aproxy config --baseurl`（`src/commands/start.rs:80-84`）。
  - 写死路径 `~/.aproxy/config.toml`（`src/config.rs:518`）。
  - 下载产物自证失败（版本不符，与网络无关）时却建议换下载代理（`src/commands/install.rs:357-359`）。
  - 类型错误报成「JSON 语法错误」（`src/settings.rs:467`）。
  - `--skills-only` 完成后在所有平台都给 `mklink /D` 示例，而且只打印 aproxy-cli 一个目录（`src/commands/install.rs:191-194`；`src/install/skills.rs:1-22` 的文档还是单 skill 时代的）。替换方案见 §8.8。
  - install 的帮助写「全程对客户端 ≈ 无感」（`src/cli.rs:160-161`），可超过 10 秒的在途请求会被宽限强退切断（`src/server.rs:321-322`，install-09 未修）。
- 建议：随 i18n 重写文案时一并修正。

#### CI-22 `mask_secret` 对短密钥整段明示
- 级别 P2｜成本 S
- 【事实】`mask_secret` 保留前 6 个字符（`src/util.rs:38-42`），所以不超过 6 个字符的值会原样显示（`"abc"` → `"abc***"`）。`proxy_username` 原样打印（`src/commands/config.rs:440`）。
- 建议：短值整体打码，或者只显示长度。

#### CI-23 未使用的依赖
- 级别 P2｜成本 S
- 【事实】`tower`、`tower-http`（带 trace、cors 特性）在 src 里没有任何使用（`Cargo.toml:46-47`；grep 无命中）。security-11 未修。

#### CI-24 代码异味
- 级别 P2｜成本 S
- 【事实】`if let Some(v) = Some(state.target_version.clone())`（`src/commands/install.rs:540`）；`dest.parent().unwrap()`（`src/install/download/crates.rs:56,134,179`）；panic 文案是中文（`src/config.rs:943`、`src/settings.rs:27`）。

---

## 2. 对已定决策的异议（单列，需用户裁决）

| # | 已定决策（来源） | 问题 | 建议 |
|---|---|---|---|
| D1 | CLI 用 `--baseurl`（记忆 baseurl-rename，用户要求） | 其余多词旗标都用连字符分词：`--api-key`、`--log-file`、`--download-proxy`、`--proxy-username`；键名是 `base_url`。帮助、文档与 UI 映射都要为它破例 | 若采纳 CI-05，这个旗标只剩 start 的覆盖参数一处。0.2.0 可以改成 `--base-url`，不留别名，也可以维持现状。优先级低，但只有现在能改 |
| D2 | 接上顶层 `--api-key`（记忆 baseurl-rename） | 密钥会进 `.restore`、守护进程与看护者的命令行，帮助却写「仅本次运行生效」（CI-02） | 建议删掉，密钥只放 config。如果保留，只经规格文件传递（0600），不进 argv |
| D3 | 铁律 3「ACK 齐了才交换」：PrepareSwap 广播（install-v1 计划） | 运行期没有任何行为依赖 swap_phase：守护只在状态里报告它（`src/daemon.rs:1220-1228,1315-1316`），抑制补种靠的是宣告（`src/server.rs:213-214`）。所以 ACK 实际只是「每个实例都应答 IPC」再加一个标志。收敛重启用的是安装器的 `current_exe()`（`src/install/broadcast.rs:51,70-71`）；有了早交接，这个 exe 是 staging 里的目标版本。【推断】于是交换前会先用 staging 路径把不健康的实例拉成新版本，滚动时再重启一次 | 门槛保留为「所有存活实例都应答」，去掉标志和收敛重启（失败就带指引退出）；终验改看 pid 或创建时间是否变化。也可以维持现状。优先级低 |
| D4 | 裸 `aproxy` 等于启动代理（cli.rs 的帮助） | agent 或 UI 为了看帮助误敲 `aproxy`，就会在后台起一个守护进程 | 裸命令改为输出帮助或 status，启动只走 `aproxy start`。优先级低 |
| D5 | skill references「latest 直改，大版本把 latest 复制为 `references/<旧版本>/` 留存」（记忆 aproxy-cli-skill-versioning，用户 2026-09-07 定调） | 留存是为了让持有旧二进制的用户查到旧行为。`skill install` 严格取二进制同版本之后（§8.4），每个二进制拿到的就是它自己版本的 skill，留存目录只会让包变大，让 agent 多做一层判断。`latest` 这个目录名也会误导，它描述的其实是本版本 | 0.2.0 不做 `references/0.1.0/` 留存，并考虑把 `references/latest/` 改为 `references/`（SKILL.md 的链接随之修改）；留存策略改为「随版本发布的 skill 只描述本版本」 |

---

## 3. 0.1.0 → 0.2.0 断点分析

### 3.1 谁在驱动
【事实】早交接是 v0.1.0 之后才合入的（`9e91a95` 晚于 `d376275`）。0.1.0 的安装器负责选版、下载、自证、广播与**交换**，在 Windows 上交换后尝试接力，接力不成就自己滚动重启和终验（`v0.1.0:src/install/flow.rs:637-698`）。0.2.0 能影响的只有它发布出去的东西，以及它的二进制被 0.1.0 执行时的表现。

### 3.2 逐步推演（假设：资产名不变、删掉 compat、没有 CI-01 的门）

| # | 0.1.0 的动作 | 依赖 0.2.0 的什么 | 结果 |
|---|---|---|---|
| 1 | 选版：GitHub `/releases?per_page=100` 交给 `pick_latest`。只认 `^v\d` 且符合版本文法的 tag，要求有本平台 baseline 资产 `aproxy-<target>[.exe]`，stable 通道排除预发布（`v0.1.0:src/install/download/github.rs:111-147`）。GitHub 不可达时读 npm 的 `dist-tags.latest`（`v0.1.0:src/install/download/npmpkg.rs:185-229`） | tag、资产名、prerelease 标志、npm dist-tag | 0.2.0 正式版发出后会被选中【推断】 |
| 2 | 下载：github 渠道取 `releases/tags/v0.2.0` 的资产加 `.sha256`（`github.rs:45-58,149-190`）。失败依次换 npm（`@meowo/aproxy-<平台>` 包内的 `package/bin/aproxy[.exe]`，`npmpkg.rs:13-44,253-261`）、binstall（没有模板时取 GitHub 资产，`crates.rs:69-79`）、cargo（`.crate` 本地编译，`crates.rs:117-151`） | 资产名、npm 包名与包内路径、crate | 成功 |
| 3 | 自证：`--version` 输出里第一个以数字开头的 token 要等于目标版本（`v0.1.0:src/install/staging.rs:42-58`），在建 install.state 之前做（`v0.1.0:src/commands/install.rs:307-371`） | `--version` 的格式 | `aproxy 0.2.0` 通过 |
| 4 | 建 install.state 与宣告（全局名）；Windows 上停掉旧看护者；向 **0.1.0 实例**广播 PrepareSwap（`v0.1.0:src/install/flow.rs:464-505,637-680`） | 无 | 正常 |
| 5 | **交换**：bin 里换成 0.2.0（`v0.1.0:src/install/flow.rs:681-684`），从此不可逆 | 无 | 发生 |
| 6 | Windows：写入 relaying，拉起 `bin/aproxy.exe install --continue`（不带 `--handover-from`），等 30 秒看阶段是否越过 relaying（`v0.1.0:src/install/swap.rs:229-263`；`flow.rs:686-695`） | 0.2.0 会不会接 relaying | relaying 豁免已删，又没有 `--handover-from`，0.2.0 拒绝接手（`src/install/flow.rs:742-748` 删除之后）；30 秒后 0.1.0 自己接着做（`v0.1.0:flow.rs:694-697`） |
| 7 | 滚动：经 IPC 停掉一个 0.1.0 实例，用 0.2.0 按 `.restore` 里的 0.1.0 argv 拉起，**就绪判据是注册表 `.pid` 里有这个 pid**（`v0.1.0:src/install/restart.rs:186-235`、`v0.1.0:src/daemon.rs:670-690`） | 0.2.0 是否接受 0.1.0 的参数与配置；`.pid` 能否被 0.1.0 解析 | 接受：判为就绪（`.pid` 字段是 0.1.0 所需的超集）。不接受：8 秒后判失败，用 `.old`（unix 是 `aproxy.old`）拉回，置 halted |
| 8 | 终验：IPC ping 每个实例，要求版本等于目标且不在更换阶段（`v0.1.0:src/install/flow.rs:386-410`） | Windows 要旧管道名（S1），unix 要无 `v` 请求的旧形状应答（S3） | S1、S3 已删，两个平台都 ping 不通，全部判为不合格 |
| 9 | 补 restart 一轮：再拉一个 0.2.0，它因端点和端口被第一个 0.2.0 占着立即退出；ping 仍不通，置 halted；用旧二进制拉回，端口被占同样起不来，得到 `RestartError::Down`（`v0.1.0:src/install/restart.rs:275-370`） | 无 | install 以 1 退出，报「新版本启动失败…旧二进制拉回也失败；实例当前下线…执行 aproxy restore」；state 为 failed+halted，`.old` 与 staging 残留 |

### 3.3 结局（都是【推断】，依据上表的代码路径）
- **A. 没有运行中的实例**：步骤 4、6–9 都是空操作，报「安装完成」，bin 换成 0.2.0，沿用 0.1.0 的 config 与 settings。安装本身成功，之后能不能用取决于配置兼容（CI-04 的固化默认值、CI-09 的未知键）。
- **B. 有实例，0.2.0 接受它们的参数与配置**：实例实际上已经由 0.2.0 在服务，安装却报失败，文案还说实例下线，这是错的。state 为 failed+halted。如果 0.2.0 保留 `fleet_already_on_target`，又能解析 0.1.0 写的 state，那么下一条 0.2.0 命令拉起的续作会清场。`.old` 与 staging 残留。
- **C. 有实例，0.2.0 拒绝它们的参数或配置**（改了旗标名、删了字段、校验更严、`.restore` 格式变了）：第一个实例被拉回 0.1.0，从 `.old` 运行，滚动中止；bin 已是 0.2.0，其余实例仍是 0.1.0。0.2.0 的 CLI 联系不上这些实例（Windows 管道命名空间不同，unix 应答形状不认），status 把它们列为「无响应」，只能 `stop --force`。restart 与看护者重拉都用 0.2.0 加 0.1.0 的参数，同样起不来。【事实】0.2.0 的看护者不会把它们当挂死处决：读不到心跳节（节名按 home 命名空间）时退化为纯死亡检测（`src/watchdog.rs:546-549`）。
- 补充：CI-02、CI-03 落地之后，0.2.0 不再接受 0.1.0 写下的 `--daemon-child` 加原样 argv，第 7 步的新实例起不来，没有门的话几乎必然落到结局 C。若只改了 `.restore` 的格式，0.1.0 在第 9 步读不出新实例写的记录，会以「终验未通过」的 failed（不带 halted）收场，同样发生在交换之后。
- **结论：半途坏掉，断点在交换之后。** 只有结局 A 算干净；B 与 C 都发生在不可逆的交换之后。

### 3.4 交换前的检查点（不写兼容代码，0.2.0 能动的杠杆）

| 杠杆 | 0.1.0 在哪一步看它 | 能挡住的渠道 | 代价 |
|---|---|---|---|
| **`--version` 改为 `aproxy v0.2.0`**（推荐） | 第 3 步；所有渠道与 `--from` 都经过 | 全部：github、npm、binstall、cargo、url 模板、`--from` | 永久的格式变更：0.2.0 的 probe 要接受 `v` 前缀，测试替身与文档同步。0.1.0 的报错不对症（「下载产物校验失败（不可运行）: … --version 输出无版本号: "aproxy v0.2.0\n"」，后面还附一句下载代理的提示），要靠发布说明解释 |
| 改 GitHub 资产名（例如带版本号或打成压缩包） | 第 1、2 步（github、binstall 兜底、url 模板） | 只有 GitHub 一系；npm 要另改包内路径；cargo 渠道挡不住 | 0.1.0 的 `upgrade` 会静默报「已是最新」，有误导性。资产命名是用户定调项，安装脚本、binstall、文档都要跟着改 |
| 改 npm 平台包的包内路径或包名 | 第 2 步 npm | 只有 npm | 改包名需要新包首发，而新包无法用 OIDC 首发（记忆 trusted-publishing-new-crate-limit） |
| 不设门，只在发布说明里写「先 stop all 再 upgrade」 | — | 只能走到结局 A | 没读发布说明的用户和 agent 会直接进入 B 或 C |

### 3.5 推荐
采用 CI-01 的 `--version` 门，在第一个 0.2.0 预发布之前合入，并配 CI 拒绝用例。资产名维持用户定调的命名，不动。
这道门只拦得住二进制。0.1.0 的安装者从一开始就并行下载目标版本的 skill，结局与二进制无关；要不要连 skill 包一起挡住、怎样挡，见 §8.4。

### 3.6 发布说明里的迁移步骤（草稿）
> 0.2.0 不能从 0.1.x 原地升级：0.1.x 的 `aproxy install`/`upgrade` 会在下载之后报错退出，不会改动任何东西。请按以下步骤重新安装。
1. 用 0.1.0 记下现有实例：`aproxy status`（端口、配置文件）、`aproxy alias list`，并备份 `~/.aproxy/` 下的 `*.toml` 与 `settings.json`。
2. 用 0.1.0 停掉全部实例：`aproxy stop all`，再执行 `aproxy status` 确认已经没有实例。0.1.0 的看护者在实例清零后最多 5 分钟自行退出；0.2.0 的实例启动时也会退役更旧的看护者。
3. 换二进制。
   - 用一键脚本或 `aproxy install` 安装的：删除 `~/.aproxy/bin/` 下的 `aproxy(.exe)`、`aproxy.old(.exe)`、`aproxy.bat`、`aproxy`，然后重新运行一键安装脚本。
   - npm 安装的：`npm i -g @meowo/aproxy@latest`。
   - cargo 安装的：`cargo install aproxy --force`。
   - 不论哪种渠道都必须在第 2 步之后做：0.2.0 的 CLI 控制不了 0.1.0 的实例。
4. 清理 0.1.0 的残留：`~/.aproxy/run/install.state`（如果有）、`~/.aproxy/staging/`，以及 `~/.aproxy/run/*.restore`（第 2 步之前崩溃过的实例会留下）。
5. 配置：按 CHANGELOG 的字段变更表修改，然后运行 `aproxy doctor`（0.2.0 会报出未知键）。用 0.1.0 的 `aproxy config` 或 `alias add` 写过的文件里，会带着当时的默认值（例如 `keepalive_interval_secs = 15`）。想跟随新的默认值，就删掉这些行。
6. 用 0.2.0 逐个启动：`aproxy start <别名|路径>`。端口不变，agent 客户端的 base URL 不用改。
7. 安装 skill：`aproxy skill install`，再按输出的路径重新链接到 agent 的 skills 目录（§8）。

- 升级失败、暂时留在 0.1.0 的：如果 §8.4 的改名没有全部落地，0.1.0 的安装者可能已经把 0.2.0 的 skill 装进了 `~/.aproxy/skills/`。执行 `aproxy install --skills-only 0.1.0` 可以恢复。
- 已经在门合入之前用 0.1.0 升级过、落到结局 B 或 C 的：先 `aproxy status`，对列为「无响应」的 0.1.0 实例执行 `aproxy stop --force <端口>`，删掉 `run/install.state`，再按第 6 步启动。
- 配套（P1，安装脚本）：0.2.0 的一键脚本检测到已安装的二进制输出裸版本号（0.1.x 的格式）时，打印上面的步骤，而不是只说一句「已安装，本脚本不做覆盖」（`scripts/install.sh:248-250`、`install.ps1:120-125`、`install.cmd:68-69`）。

### 3.7 以后的版本：让断点永远落在交换之前
- 0.2.0 起由目标版本驱动。交接发生在广播与交换之前，代码注释写明「交不出去就以失败收场，现场是干净的」（`src/install/flow.rs:873-883`）。所以以后要放弃对某个旧版本的支持，只需要目标版本在接手时检查两件事并拒绝：安装者的版本（CI-03 建议的 `installer_version`），以及在跑实例的版本（IPC 应答里有 version）。拒绝发生在交换之前。
- 旧安装者在交接之前仍要做选版、下载和自证。它依赖的东西从 0.2.0 起都是永久接口：tag 文法、资产名与 `.sha256`、npm 包名与包内路径、crate 名、`--version` 格式。CI-15 落地之后，skill 包的格式不再属于这一类。
- 版本下限（CI-06）防止反方向的同类事故。

---

## 4. config / settings / CLI 契约建议改动清单

| 对象 | 现状 | 建议 | 级别 | 来源 |
|---|---|---|---|---|
| config.toml 写回 | 整份重写、非原子，丢注释与未知键，固化默认值 | 用 toml_edit 只改目标键，tmp+rename，新文件只写显式键 | P0 | CI-04 |
| settings.json 写回 | 整份重写，丢未知字段，固化默认值 | Option 加 skip 只写显式值，flatten 保留未知字段，加 CAS | P0（CAS 为 P1） | CI-04/10 |
| `.restore` | 原样 argv，可能带 `--api-key` | 版本化的结构化记录，不含 argv 和机密 | P0 | CI-02 |
| 内部入口 | 隐藏全局旗标，挂在 `install` 下的隐藏旗标 | 隐藏内部子命令加规格文件；install.state 记 installer_version | P0 | CI-03 |
| `config` 写入面 | 15 个专用旗标，只覆盖约三分之一字段 | `config get/set/unset/show`，同构的 `settings get/set/unset`；删掉专用旗标 | P0（决策） | CI-05 |
| `--version` | `aproxy 0.2.0` | `aproxy v0.2.0`（永久格式） | P0 | CI-01 |
| 安装版本下限 | 无 | `INSTALL_FLOOR = 0.2.0`，在四处拒绝 | P0 | CI-06 |
| install.state | failed 覆盖阶段；Relaying；last_error 是中文自由文本 | 加 `failed_at`、`installer_version`、`error_code`（可选）；删 Relaying；未知阶段用 `#[serde(other)]` 容忍 | P1 | CI-07/03/20、§5 |
| 未知键 | 静默忽略 | 告警并给近似建议，不报错 | P1 | CI-09 |
| settings schema 版本 | 无 | `"schema": 1`，写入者遇到更高 schema 拒写 | P1 | CI-10 |
| settings 分层与形状 | 三类东西混装，只有 7 个字段有全局默认 | `defaults` 与 config.toml 同构，其余分节 | P1⚑ 决策 | CI-11 |
| 键名 | settings 与 transform 的 `idle_timeout_secs` 同名；`max_body_mb` 对 `spool_limit_mb`；MB 实为 MiB | 给前者改名；文档写明 MiB | P1⚑ | CI-11 |
| config.toml 版本字段 | 无 | **不加**：人手写的文件，靠「只增不改、未知键告警、写回保留」演进就够了，加了版本号反而逼人维护 | — | CI-04/09 |
| `language` | 无 | settings 加 `language`（`auto`/`en`/`zh-CN`），环境变量 `APROXY_LANG` 优先 | P1 | §5 |
| 启动覆盖参数 | 顶层；`--config`/`--log-file` 为全局参数 | 移到 `start`；`--config` 只放在需要它的子命令上 | P1 | CI-17 |
| `upgrade` | 重复变体 | `visible_alias` | P1 | CI-17 |
| `--json` 与 stdout | 没有；tracing 输出到 stdout | 加 `--json`（带 schema）；日志改到 stderr | P1 | CI-13 |
| 退出码 | 只有 0 和 1，doctor、restore 失败也是 0 | 定一张退出码表 | P1 | CI-13 |
| 安装进度 | 用户看不到，接手者无日志 | `--progress json`、`--check`、`logs/install.log` | P1 | CI-13 |
| 机密参数 | `config --api-key`、`--proxy-password` 走命令行 | `set <键> -` 从 stdin 读 | P1 | CI-14 |
| skill 命令 | `install --skills-only`，默认取渠道最新版 | `skill install`（`upgrade` 为别名）/`list`/`show`，严格取二进制同版本；删除 `--skills-only` | P1（命令形状定型后冻结） | §8.3、§8.4 |
| `--help` 末尾 | 无 | 固定英文的一行，指向 `aproxy skill list`/`show`，所有子命令都带 | P1 | §8.2 |
| skill 本地记录 | 无，不知道装了哪个版本、哪些文件 | `<home>/skills/manifest.json`：版本、文件、大小、哈希，兼作提交标记；未知 schema 视为未安装 | P1 | §8.5 |
| skill 包 | 三个打包者；`aproxy-skills.zip` 加每个 skill 的单包 | 一个带 `manifest.json` 的 zip 供三个渠道共用；换资产名与 npm 包内路径；crate 带 zip 而不带目录；删除单包 | P1⚑ | §8.6 |
| `--baseurl`、顶层 `--api-key`、裸 `aproxy` | 用户定调 | 见 D1、D2、D4 | 决策 | §2 |

---

## 5. i18n：分类计数、混用点与 clap 方案

### 5.1 本范围内的中文字符串（按字符串字面量计，测试模块除外）
方法：脚本扫描 `.rs` 源码里含中日韩字符的字符串字面量，按所在的宏或调用归类。`println!/eprintln!` 归「打印」，`tracing::*!` 归「日志」，`expect`/`panic` 归「panic」，其余作为返回值或传参往外传的归「消息串」，消费者逐个人工核对。另统计 `src/cli.rs` 的 `///` 行。误差约 ±5%。

| 类别 | 数量 | 去向 | 处理 |
|---|---|---|---|
| 直接打印 | 121（commands/config 57、install 30、start 24、doctor 3；find.rs 4、doctor.rs 1、settings.rs 1、download/mod.rs 1） | 控制台 | 本地化 |
| clap 帮助 | `src/cli.rs` 122 行 `///`，约 66 个帮助项（参数加子命令） | `-h`/`--help` | 本地化（§5.3） |
| 消息串：只给用户 | 41（settings 的检查 16、doctor 的发现 16、find 2、commands/install 6、commands/mod 1） | 控制台（doctor 将来也会有 `--json`） | 本地化；`--json` 输出带 code |
| 消息串：**混用** | 169（config.rs 21、commands/start 6、install/** 142） | 控制台，**以及** startup.log、install.state 的 `last_error`、日志 | 类型化加 code；控制台本地化，落盘与日志用英文 |
| 日志 tracing | 21 | 日志文件；CLI 进程里会跑到 stdout（CI-13） | 固定英文 |
| panic/expect | 9 | 开发者 | 固定英文 |
| 范围外参考 | 打印 71、日志 105、消息串 141、panic 16（status/stop/logs/restore/restart/alias/server/daemon/watchdog/proxy/transform/decode/util） | 其中 daemon.rs 的 IPC 错误文案上了线（`src/daemon.rs:1217,1256-1290`）；proxy.rs 有 HTTP 错误体 | IPC 与 HTTP 固定英文，CLI 打印前按 code 本地化 |
| 测试断言 | 113 处按中文子串断言（src 单测 36、tests/ 77） | — | 测试固定 `APROXY_LANG=zh-CN`，或改为断言 code |
| 安装脚本 | `install.sh`、`install.ps1` 输出中文，`install.cmd` 输出英文 | 首装用户 | 范围边缘：脚本按 `LANG` 或 `Get-UICulture` 选择，或统一用英文 |

### 5.2 同一个字符串既给用户又进日志、或被传出的地方
| # | 源头 | 去向 |
|---|---|---|
| 1 | `Config::validate()`（`src/config.rs:516-599`）、`heartbeat_problem`（`:30-49`） | start 的 stderr，加守护子进程的 startup.log（`src/commands/start.rs:151-157` 调用 `src/server.rs:469-489`）；doctor（`src/doctor.rs:206-210,293-297`，未打码，见 CI-14） |
| 2 | `listen_exposure_warning`（`src/config.rs:745-767`） | start 的 stderr、守护的 `tracing::warn!`、startup.log（`src/commands/start.rs:167-174`）；doctor（`src/doctor.rs:127-145,212-217,299-304`）。代码注释明说「三处口径一致」，这与「日志英文、控制台本地化」直接冲突 |
| 3 | install 主流程的 `Err(String)`（`src/install/flow.rs`、`restart.rs`、`swap.rs`、`staging.rs`、`download/*`） | 控制台 `[ERROR] 安装失败: {e}`（`src/commands/install.rs:424,431`）；`fail_in` 写进 install.state 的 `last_error`（`src/install/state.rs:374-381`），**由另一个进程读出来再打印**（`src/commands/install.rs:458-461`）；续作写日志（`src/commands/install.rs:26`） |
| 4 | `RestartError` 的 Display（`src/install/restart.rs:56-80`） | 包进第 3 项 |
| 5 | `settings::load_from` 的 eprintln（`src/settings.rs:244-248`） | 库层直接打印；守护与看护者里同样会执行，只是 stderr 被丢掉 |
| 6 | `config::load_from` 的 `tracing::warn!`（`src/config.rs:916-931`），加 `warn_if_config_broken` 的 eprintln（`src/commands/config.rs:51-57`） | 同一件事报两遍：一遍作为日志，一遍作为控制台文案。i18n 之后两者语言不同，应只留一处 |
| 7 | CLI 进程的 tracing 输出到 stdout（`src/server.rs:411-417`） | 日志行出现在用户控制台，比如 install 里的「实例已滚动到新版本」（`src/install/flow.rs:528`） |
| 8 | 下载错误（github/npmpkg/crates） | 控制台；`fetch_artifact` 把各级错误汇总；skill 的 outcome 写日志 |

### 5.3 clap 方案评估（clap 4.6.6，`Cargo.lock:349-360`）
- **语言要在解析之前确定**：`APROXY_LANG` 优先，其次直接读 settings.json 的 `language`（静默容错，不打印），再次系统界面语言，最后用 en。系统语言：Windows 用 `GetUserDefaultUILanguage`（windows-sys 已经是依赖，需要开 `Win32_Globalization` 特性）；POSIX 依次看 `LC_ALL`、`LC_MESSAGES`、`LANG`；macOS 用 `sys-locale` crate（很小）。`zh*` 映射为 zh-CN，其余映射为 en（繁体中文用户是回落到 zh-CN 还是 en，需要定）。守护、看护者与续作进程不需要语言。
- **`#[arg(help = <运行时表达式>)]`：可行。** clap derive 在构建 `Command`（`augment_args`）时对属性表达式求值，而 `Cli::parse()` 就是在运行期构建 Command，所以按运行期语言给文案没有问题。风险是 clap 条目上的 `///` 中文文档注释仍被 derive 读取：显式 `help` 能盖住短帮助，但多段的文档注释仍可能生成 `long_help`，`--help` 时会把中文漏出来。稳妥做法是把 clap 条目上的 `///` 改成 `//`（中文开发注释照留），文案全部来自目录，再用测试兜底（见下）。
- **备选：构建后改写。** 先 `Cli::command()`，再按 id 用 `mut_arg`/`mut_subcommand` 填文案。优点是 derive 保持干净、文案集中；缺点是文案与参数定义分开，改名时容易漏，只能靠测试兜。两种方案任选一种都可以，**测试是必需的**：分别以 en 与 zh-CN 构建命令树，遍历所有未隐藏的参数与子命令，断言帮助非空；en 下渲染所有子命令的 `render_long_help()`，结果里不得出现 CJK 字符；zh-CN 下不得回落到英文。
- **clap 内置文字**（写死为英文）：
  - 标题 Usage/Commands/Options/Arguments：给每个命令设 `help_template`，用 `{usage}`、`{positionals}`、`{options}`、`{subcommands}` 这几个标签，配本地化的字面标题；没有内容的段落不写，模板在构建后按命令的实际参数生成。
  - `-h`/`-V` 的说明文字：`disable_help_flag(true)`、`disable_version_flag(true)`，自己定义 `ArgAction::Help`/`Version` 参数并给本地化文案；`help` 子命令用 `disable_help_subcommand(true)` 关掉。
  - `[possible values: …]`：`hide_possible_values(true)`，取值写进帮助文案。
  - 解析错误：改用 `Cli::try_parse()`。`DisplayHelp`/`DisplayVersion` 照常打印后退出；`UnknownArgument`、`InvalidSubcommand`、`InvalidValue`、`MissingRequiredArgument`、`ArgumentConflict`、`TooManyValues`、`NoEquals`、`ValueValidation` 这几类用 `e.get(ContextKind::…)` 取上下文，渲染一行本地化消息加「用 `aproxy <命令> --help` 查看用法」，以 2 退出；其余类型保持 clap 的英文。
  - 结论：能做到「帮助与常见错误随系统语言」，代价中等；不常见的错误仍是英文，可以接受。`value_name` 这类元变量（PATH、URL、PORT|all|ALIAS|idle）保持英文记号。`--version` 不做本地化。
- **消息架构**：按领域写类型化消息与错误枚举（例如 `ConfigError`、`InstallError`、`CliMsg`），每个提供 `fn code(&self) -> &'static str` 和 `fn render(&self, Lang) -> String`，用穷尽 match 写两种语言，编译期就能保证完整。这比引入 `rust-i18n`/Fluent 更契合体积纪律和「漏翻译编不过」的要求。日志写 `tracing::warn!(code = e.code(), "{}", e.render(Lang::En))`；install.state 存英文 `last_error` 加可选的 `error_code` 与参数，等结局的一方按 code 本地化，不认识的 code 就显示英文原文。
- **规模**：本范围约 400 处，范围外约 210 处是打印或给用户看的（打印 71 加消息串 141，其中一部分只上线、不显示）。范围内属于 L，建议分三批：clap 与 CLI 打印、配置诊断、install 错误。

---

## 6. 上一轮相关条目现状（2026-10-04 13 维审查）

| 条目 | 摘要 | 现状 | 证据 |
|---|---|---|---|
| config-01 / security-03 | 日志、status、start 明文输出凭据 | 已修 | 日志经 `mask_base_url`（`src/proxy.rs:1281-1284`）；`redact_reqwest_error`（`src/proxy.rs:3205-3207`）；start（`src/commands/start.rs:280-288`） |
| config-02 / security-04 | 只遮密码；transform 的 args/extra 不打码 | 已修 | userinfo 整体遮（`src/config.rs:829-914`）；`src/commands/config.rs:27-46,418-426` |
| config-03 | 未知键静默忽略 | 未修 | CI-09 |
| config-04 / ui-02 | config 写回丢注释与未知键、非原子 | 未修 | `src/config.rs:939-946`，CI-04 |
| config-05 | 非法头或 api_key 被静默跳过 | 未修 | `src/proxy.rs:1203-1240,4286`，CI-08 |
| config-06 | doctor 不查默认配置、不注入 settings、退出码恒 0 | 部分修：默认配置只查非回环暴露 | `src/doctor.rs:120-145`、`src/commands/doctor.rs:8-22` |
| config-07 / ui-01 | settings 损坏后写命令覆盖原文件 | 大部分已修（写命令拒绝）；未知字段丢失、默认值固化、类型错误报成语法错误，仍未修 | `src/settings.rs:260-276,301-320,467` |
| config-08 / security-05 | 非回环监听无告警 | 已修 | `src/config.rs:720-767`、`src/commands/start.rs:167-174`、`src/doctor.rs:127-145` |
| config-09 / security-07 / daemon-ipc-08 | unix 权限 | 未修 | CI-14 |
| config-10 | `start <别名>` 指向缺失文件时报错误导 | 未修（启动时的 `[ERROR]` 别名检查算旁路提示） | `src/commands/start.rs:22`、`src/commands/mod.rs:69-81` |
| config-11 | alias add 的默认路径；接受目录 | 未修 | `src/commands/alias.rs:17-24` |
| config-12 | `config --listen/--proxy` 存盘前不校验 | 未修 | `src/commands/config.rs:131,180,225` |
| install-01 | 滚动失败实例永久丢失 | 已修（在途记录、回写 `.restore`、旧二进制拉回、halted） | `src/install/restart.rs` 的模块文档；`f09c3ce` |
| install-02 | 没有稳定通道 | 已修 | `Channel`/`decide_latest`（`src/install/download/mod.rs`）；`src/cli.rs:209-214` |
| install-03 | format-v* 抢占 latest | 已修 | 主线 tag 过滤；`.github/workflows/release-format.yml:283` |
| install-04 | GLIBC 2.39 | 已修 | `.github/workflows/release.yml:31-38,225-238` |
| install-05 | 测试 tag 抢先 | 已修（选版只认项目版本号文法） | github.rs 的 `is_release_grammar` |
| install-06 / security 管辖 | 在线路径漏了管辖检查 | 已修 | `src/commands/install.rs:286-292` |
| install-07 | 在线来源 failed 后无法续作 | **未修**，另发现 `--abort` 对一切 failed 都拒绝 | CI-07 |
| install-08 | 同版本也重装 | 已修：latest 不高于当前即短路；显式版本按设计照装 | `src/commands/install.rs:245-267` |
| install-09 | 滚动重启不排空 | 未修（10 秒宽限） | `src/server.rs:321-322` |
| install-10/11/14、docs-01、security-12 | 一键脚本的几个问题 | 已修 | `scripts/install.cmd:82`、`scripts/install.ps1:40-42`、`scripts/install.sh:59-75,212,341` |
| install-12 | `--no-skills` 不进状态 | 已修 | `src/install/flow.rs:292-300` |
| install-13 / security-10 | binstall/cargo 零校验，文档不符 | 未修 | CI-16 |
| install-15 | 宣告节用全局名 | 已修（按 home 命名空间；0.1.0 的全局名 S5 随 compat 删除） | `src/install/announce.rs:24-35` |
| install-16 | npm 转发器细节 | 范围外，未核 | — |
| docs-02 | 新装机收尾提示 | 已修（但提示把 `--api-key` 写在命令行上，见 CI-14） | `scripts/install.sh:332-335` |
| docs-09 | CLI 只有中文 | 0.2.0 计划中 | §5 |
| docs-10 | 文档里出现不存在的 `aproxy watchdog` | 代码侧已修；docs 未核 | `src/settings.rs:106-107` |
| docs-12 | settings 没有读写命令 | 未修 | CI-05 |
| docs-14 | clap 帮助的换行 | 未修（没有 `verbatim_doc_comment`），随 i18n 一并处理 | `src/cli.rs` |
| security-01 | Host/Origin 校验 | 已修 | `src/config.rs:300-344` |
| security-09 | `--api-key` 进 `.restore` | 未修，另发现看护者的命令行也带着它 | CI-02 |
| security-11 | tower-http 未使用 | 未修（tower 也未使用） | CI-23 |
| daemon-ipc-06 | 退出码 | 部分修：stop 未确认时退出 1，restart all 汇总后退出 1；restore 失败与 doctor 仍退出 0 | `src/commands/restart.rs:91`、`src/commands/restore.rs:75-83` |
| ui-03 | IPC 没有错误码与能力声明 | 已修（IPC v1 的 `error_code` 与 ops 列表） | `src/daemon.rs:284`、compat 中的 `ops` |
| 发布前应修 #7 一键脚本 | | 已修 | 同上 |
| 发布前应修 #8 凭据脱敏 | | 大部分已修；残留：doctor 原样输出、`.restore` 与看护者命令行里的 key | CI-14、CI-02 |
| 路线图 0.1.x #1 写回安全 | | 部分修（只做了 settings 损坏时拒写） | CI-04 |
| 路线图 0.1.x #8 校验与 doctor 统一管线 | | 部分修（只做了非回环告警） | CI-08 |
| 路线图 0.1.x #9 install 健壮性 | | 部分修：管辖检查、同版本短路、`--no-skills` 已修；在线续作、`--check`、binstall 校验未修 | CI-07/13/16 |
| 路线图 0.1.x #10 unix 权限 | | 未修 | CI-14 |
| 路线图 0.2.0 #4 配置编辑后端 | | 未开始 | CI-05/08/11 |
| 路线图 0.2.0 #7 错误码与 i18n | | 部分：IPC 有错误码；CLI、配置与安装没有 | CI-12 |
| 路线图 0.2.0 #8 安装过程机器可读接口 | | 未开始 | CI-13 |

---

## 7. 推断与未核实项（建议优先验证的）
1. §3 的结局 B、C 是按 v0.1.0 与当前代码推演出来的，按约束没有实际运行。最有价值的验证是 CI-01 第 4 点的 CI 拒绝用例；也可以在合入门之前，在 CI 里跑一次「不设门」的对照，确认推演无误。
2. toml 0.8 序列化时跳过值为 None 的字段，这一点依据的是 crate 的已知行为，没有在本机编译验证；CI-04 所说的「固化默认值」只涉及非 Option 字段，不受这一点影响。
3. clap derive 在显式 `help` 与多段文档注释并存时如何生成 `long_help`，没有在本机验证。§5.3 的建议（把 `///` 改成 `//` 再加测试）不依赖这个细节。
4. 中文字符串计数是启发式扫描的结果，误差约 ±5%；169 条混用消息串按模块归类，没有逐条追到最终消费者。
5. D3 中「收敛重启会先用 staging 路径把实例拉成新版本」是推断，需要看早交接之后 `current_exe()` 在接手者里的实际取值，并做一次有不健康实例时的安装演练。
6. §8.6 第 4 步：CI 里生成、未被 git 跟踪的文件能否经 `include` 进 crate，以及它与 `.gitignore` 怎样交互，没有验证。落地时用 `cargo package --list` 确认。
7. §8.7 第 3 步引用的 Rust std 行为（Windows 上 junction 也报告为 `is_symlink()`）取自 rust-lang/rust 主干源码。本机没有 rust-src，没有对照所用工具链的版本逐一核对；第 5 步的哈希核对不依赖这一点。
8. §8.6 第 7 步说 PowerShell 按控制台代码页解码原生命令的输出，这是依据其已知行为的推断，没有在本机复现。`File::try_lock` 在 Rust 1.89 稳定，依据的是发布说明，也没有在本机核对。

---

## 8. skill 命令的落地建议

> 依据：用户 2026-10-11 提出的新需求（lead 转述），以及 `.agents/plan/release-0.2.0.md` 中 W4 的草案。本节只补 W4 还没定的部分：版本对应、白名单从哪来、show 的读取规则、打包方式怎么改。与 W4 一致的地方不再重复，不一致的地方汇总在 8.10。

### 8.1 现状（只列与新命令有关的）
- 【事实】skill 落在 `<home>/skills/<name>/`。zip 顶层的每个非隐藏目录都被当作一个 skill，逐个「旧目录挪走、新目录挪进」（`src/install/skills.rs:86-138`）。不记录装的是哪个版本、装了哪些文件；新版本删掉的 skill 会一直留着；没有名字白名单。`skill_dir_in` 仍写死 `aproxy-cli`（`src/install/skills.rs:17-22`）。
- 【事实】没有任何互斥。`--skills-only`（`src/commands/install.rs:144` 起）跳过安装状态机，与安装里的 skill 支线共用 `<home>/skills/.dl` 和 `.staging`。两边开头都先 `remove_dir_all`（`src/install/skills.rs:40,68,80-82`），并发时会删掉对方的半成品。
- 【事实】解包用的是黑名单：反斜杠先转成 `/`，然后只拒绝以 `/` 开头、含 `:`、含 `..` 段的条目（`src/install/skills.rs:153-156`）。条目数和大小都没有上限（`io::copy`，:166）；Windows 保留名（`CON`、`aux.md`）、结尾的点、大小写冲突都没有拦。zip 里的符号链接条目会被写成普通文件，不会真的建成链接。
- 【事实】版本：`--skills-only` 默认取渠道的最新版，查不到才用自身版本（`src/commands/install.rs:162-182`）。安装时的 skill 支线取目标版本，由调用者进程执行（CI-15）。
- 【事实】同一份 skill 有三个打包者，彼此不一致：
  - release.yml 先 `cp -r` 再 zip，另外给每个 skill 出一个单包（`.github/workflows/release.yml:253-282`）。
  - npm 脚本自己再打一遍，并排除 `*.zip`（`npm/build-and-publish.sh:194-211`）。其实 publish-npm 任务已经把 skills 任务的 zip 下载到了 `artifacts/`（`.github/workflows/release.yml:361-364`）。
  - cargo 渠道在用户机器上从 .crate 现场组 zip（`src/install/download/crates.rs:178-250`）。在 Windows 上条目名带反斜杠（:244），要靠解包时的转换才对得上。`Cargo.toml:37` 逐个列出 skill 目录，新增 skill 时忘了改这一行，cargo 渠道就会悄悄缺掉它。
- 【事实】单包 `aproxy-skill-<名>.zip` 没有消费者：代码只认 `aproxy-skills.zip`（`src/install/download/mod.rs:44`），README 和 docs 也没有提到它。
- 【事实】现有 14 个文件，最大 35 KB，合计约 266 KB。其中 11 个含非 ASCII 字符，主要是引用的中文日志与 CLI 输出（例如 `端口 <addr> 被其他程序占用，无法启动`）、`…`，以及测试脚本里的中文样例对话。`.gitattributes` 只管 `*.cmd` 与 `*.sh` 的行尾。

### 8.2 `--help` 末尾的提示行
- 机制【事实】：在 clap 4.6.6 中，短帮助用 `after_help`；长帮助优先用 `after_long_help`，没有设置时回落到 `after_help`（`clap_builder-4.6.6/src/output/help_template.rs:351-366`）。所以只设 `after_help`，就能覆盖 `-h`、`--help`，以及 `aproxy help <子命令>`（如果保留 help 子命令，§5.3 建议关掉它）。本仓库没开 `wrap_help`（`Cargo.toml:54`），此时 `wrap` 是空操作（`clap_builder-4.6.6/src/builder/styled_str.rs:77-78`），一行文字输出出来也是一行。
- 子命令不继承 `after_help`。用 derive 就得在每个变体上加属性，新增子命令时很容易漏。建议在构建好的 `Command` 上递归装饰：`mut_subcommands`（`clap_builder-4.6.6/src/builder/command.rs:426`）只作用于直接子命令，所以要在闭包里递归调用自身，最后再 `from_arg_matches`。§5.3 要给每个命令设 `help_template`，同样得在构建后遍历命令树，两件事可以放在同一次遍历里。注意：自定义模板必须保留 `{after-help}` 标签，否则这一行不会输出（模板逐个标签渲染，`after-help` 标签才会调用 `write_after_help`，`clap_builder-4.6.6/src/output/help_template.rs:240-241`）。
- 文案：固定英文、纯 ASCII、不超过 80 列（以后如果为中文帮助打开 `wrap_help`，也不会被折行）；不随 skill 是否已装而变化，让帮助输出保持确定；直接给出能执行的命令。例如（77 列）：
  `Agents: run 'aproxy skill list' and 'aproxy skill show <name>' for full docs.`
- 测试：递归遍历装饰后的命令树，对每个子命令断言 `render_help()` 与 `render_long_help()` 去掉尾部空白后以这一行结尾；断言这一行是 ASCII 且不超过 80 列；断言 `--version` 的输出里没有它。clap 报用法错误（缺参数之类）时输出的不是帮助，不带这一行，这是对的。如果 D4 把裸 `aproxy` 改成输出帮助，这一行也会自然带上。

### 8.3 命令形状
```
aproxy skill install [--download-proxy URL]  装与本二进制同版本的 skill；已装且校验通过时什么也不做
aproxy skill upgrade                         install 的 visible_alias
aproxy skill list [<name>] [--json]          不带名字：每个 skill 的版本、状态、位置、description；带名字：列出它的文件
aproxy skill show <name> [<file>]            原样输出一个文件，默认 SKILL.md
```
- 删除 `install --skills-only`（`src/cli.rs:226-230`）与 `run_skills_only`（`src/commands/install.rs:144` 起）。保留 `install --no-skills` 与 `skill_auto_update`，它们管的是装二进制时要不要顺带装 skill。要同步改的文档：`commands.md:453-477`、`behaviors.md:459-460`、`compatibility.md:50,64`、`settings-json.md:225`（都在 `.claude/skills/aproxy-cli/references/latest/`），以及 `docs/INSTALL_AGENT.md:221-233`。
- `upgrade` 做成 `install` 的 `visible_alias`，与 CI-17 对 `aproxy upgrade` 的建议一致。在 8.4 的语义下，两者没有可以区分的行为。
- `show` 的 stdout 只有文件内容，一个字节都不多；版本不一致之类的提示走 stderr。未安装、名字或文件不在记录里、校验失败，都以非零码退出（并入 CI-13 的退出码表），并给出下一步该执行的命令。文件找不到时，顺便列出这个 skill 可用的文件。
- `list` 在未安装时打印 `No skills installed. Run: aproxy skill install`，退出码为 0，因为它是状态查询。W4 写的「非零退出」只用在 `show` 上。`description` 取自 SKILL.md 的 frontmatter，帮 agent 判断该看哪个 skill：安装时提取出来写进记录，由 CI 保证它只有一行。`list <name>` 列出该 skill 的文件，读不了目录的 agent 靠它发现 references。
- 语言：`list`、`show` 及其报错是给 agent 看的，建议与 `--help` 提示行一样固定英文；`skill install` 的进度信息按普通 CLI 输出做 i18n。这与「命令行输出做 i18n」的总规则有出入，需要确认。
- 本地没有 skill 时，提示里除了 `aproxy skill install`，再给出该版本在 GitHub 上的目录 `https://github.com/MoYeRanqianzhi/aProxy/tree/v<版本>/.claude/skills`，覆盖下载不通的情况。不把 skill 嵌进二进制。

### 8.4 版本对应：`skill install` 装哪个版本
建议**等于正在运行的二进制的版本**（`env!("CARGO_PKG_VERSION")`；发布构建前已对齐到 tag，`.github/workflows/release.yml:171-186`）。不取 latest，0.2.0 也先不提供 `--version`。理由：
1. skill 描述的是这个二进制的命令、旗标和字段，SKILL.md 自己也要求先核对 `aproxy --version`（`.claude/skills/aproxy-cli/SKILL.md:130-132`）。装上更新版本的 skill，agent 就会敲出旧二进制没有的旗标。
2. 每个二进制只读自己版本的包，包的格式（资产名、zip 布局、清单 schema）就不再是跨版本契约，以后每个版本都能自由改，这正是 CI-15 想要的效果。`--version X` 会让 0.2.0 去读 0.3.0 的包，把跨版本读取又带回来。真有需要以后再加：加参数不破坏兼容，删参数才破坏。
3. 想要新的 skill，就升级二进制。`aproxy install`/`upgrade` 成功后，由目标版本（接手者）安装目标版本的 skill（CI-15）。这样每条路径上都满足「skill 版本 = 二进制版本」，`skill install` 只用于补装、修复，以及用过 `--no-skills` 之后。

边界情况：
- 二进制的版本没有发布过 skill（本地开发构建、发布任务失败）：报错并给出 8.3 的 GitHub 链接，不回落到 latest，回落就是错配。
- headless 变体用同一个包。npm 渠道的 skill 在主包 `@meowo/aproxy` 里（`src/install/download/npmpkg.rs:80,265-270`），headless 用户走 npm 时也从主包取。
- 已装的版本与二进制不一致（换渠道装了二进制、用了 `--no-skills`、skill 下载失败）：`list` 标出来；`show` 照常输出，同时在 stderr 提示一行。

0.1.0 一侧：
- 【事实】0.1.0 执行 `aproxy upgrade` 时，一开始就并行下载**目标版本**的 skill，skill 的结局与二进制的结局无关（`v0.1.0:src/install/flow.rs:165-200`）。`skill_auto_update` 默认开启（`v0.1.0:src/settings.rs:141,224`），下载链默认是 github → npm → binstall → cargo（`v0.1.0:src/install/download/mod.rs:215-221`）。
- 【推断】所以即使 CI-01 的门让二进制在交换之前干净地失败，用户的 `<home>/skills/` 也已经换成了 0.2.0 的内容，0.1.0 的 agent 读到的是 0.2.0 的文档。
- 不写兼容代码就能挡住：
  - GitHub：skill 包换一个资产名。0.1.0 下载 `v0.2.0/aproxy-skills.zip` 会得到 404；它的「latest skill」查询按资产名筛选 release（`v0.1.0:src/install/download/github.rs:111-147`），会回落到最后一个带旧名字的 0.1.x，那正是它该装的版本。
  - npm：主包里的路径不再用 `skills/aproxy-skills.zip`（0.1.0 写死在 `v0.1.0:src/install/download/npmpkg.rs:265-270`）。
  - cargo：0.1.0 从 .crate 的 `.claude/skills/` 现场组包（`v0.1.0:src/install/download/crates.rs:178-215`）。只有 crate 不再带这个目录才挡得住，8.6 的「三个渠道用同一个 zip」正好做到这一点。
- 三处都挡住以后，0.1.0 的 skill 支线会失败。这条支线不是强制的，不影响安装结局，原有的 skill 保持不动。

### 8.5 白名单从哪来：要不要随 zip 发布清单
结论：show 的白名单应当是**安装时写下的本地记录**。发布清单值得做，但它解决的是一致性和包的自证，不是读取安全。
- 读取安全靠「在同一块内存上先核对哈希、再输出」（8.7 第 5 步）。show 只输出 sha256 等于记录中登记值的字节。要在记录里登记一段内容的哈希，就得先拿到这段内容，所以不论路径被怎样替换（符号链接、junction、硬链接、ADS、8.3 短名、大小写，或检查与读取之间的竞争），能输出的都只有记录作者早就拿到的内容。即使有人改了 `APROXY_HOME`，自己造了目录和记录，show 也最多告诉他「某个文件是否恰好等于他已知的某段内容」。路径规则的作用是给出清楚的错误、尽早拒绝，它不是唯一的防线。
- 发布清单不增加真实性：它装在包里，与包共用同一个信任根。GitHub 渠道有 `.sha256`，npm 渠道校验 integrity（`src/install/download/npmpkg.rs:68-117`），cargo 渠道目前没有校验（CI-16）。包没有校验，里面的清单也就没有校验。
- 发布清单带来的好处：
  - (a) 包能自证版本：安装时核对「清单里的版本 = 请求的版本」，能发现发错的包。
  - (b) 解包从黑名单变成白名单：只解清单里列出的条目，边解边核对大小和哈希，有多余或缺失的条目就整体失败。zip 炸弹、保留名、奇怪的条目都随之被拦住。
  - (c) 三个渠道的内容能互相核对：cargo 渠道缺了某个 skill 会报错，不会悄悄少一个。
  - (d) 本地记录就是「通过校验的清单 + 来源」，只有一种格式。
- 格式（发布清单与本地记录共用，本地记录多一个 `source`）：
```json
{
  "schema": 1,
  "version": "0.2.0",
  "skills": {
    "aproxy-cli": {
      "description": "Operate and configure aProxy ...",
      "files": {
        "SKILL.md": { "size": 9688, "sha256": "…" },
        "references/latest/commands.md": { "size": 35267, "sha256": "…" }
      }
    }
  }
}
```
  - 发布清单放在 zip 根部，叫 `manifest.json`。本地记录放在 `<home>/skills/manifest.json`，在各个 skill 目录之外，用户把单个 skill 目录链接到 agent 的 skills 目录时不会把它带过去。
  - 本地记录兼作「安装完成」的提交标记：开始安装时先删旧记录，所有目录换好后用 tmp + rename 写入新记录。中途崩溃就等于没有记录，show 会拒绝并提示重装。
  - 读到不认识的 `schema` 时，按未安装处理并提示重装，不写兼容代码。
  - 键在打包、安装、读取三处都按 8.7 的规则校验。记录本身也在磁盘上，读取时要再校验一遍。
- 如果决定不做发布清单：由安装器在解包后自己算哈希、写记录，版本取请求的值。安全性不变，少的只是 (a)–(c)。

### 8.6 release.yml 与各渠道怎么打包
原则：一个打包脚本，打出一个 zip，三个渠道都用它。
1. 新增打包脚本，例如 `scripts/pack-skills.py`（各 job 都有 python3）：
   - 输入只取 `git ls-files -s .claude/skills`，未被跟踪的文件不会混进来；mode 必须是 100644，不能是符号链接（120000）。
   - 生成 `manifest.json`：版本取 tag，description 取自 frontmatter。
   - 按路径排序、固定时间戳、只写普通文件，打出可复现的 zip 和 `.sha256`。
   - 名字、路径、大小与 frontmatter 的规则不在脚本里重复实现，由第 5 步的 Rust 测试负责。skills 任务本来就 `needs: test`（`.github/workflows/release.yml:257`），打包时这些检查已经在同一个提交上通过。
2. skills 任务（`.github/workflows/release.yml:253-282`）改为调用这个脚本，并换资产名（8.4）。删掉单包 `aproxy-skill-<名>.zip`：它没有消费者，有了 `skill show` 也不需要手动下载。同步修改文件头部注释（:7）和发布说明（:308）。
3. publish-npm：`build-and-publish.sh` 不再自己打包，直接把 `artifacts/` 里的这个 zip 放进主包的新路径（替换 `npm/build-and-publish.sh:194-211`）。
4. publish-crates：在 `cargo publish` 之前把同一个 zip 放进 crate（这个任务已经在用 `--allow-dirty`，`.github/workflows/release.yml:449-460`）。`Cargo.toml:37` 的 include 去掉两个 skill 目录，换成这个 zip。cargo 渠道改为从 .crate 里取出这个 zip，做法与 npm 渠道相同，`fetch_build_skills` 和 `add_skill_tree`（`src/install/download/crates.rs:178-250`）就可以删掉。这样 crate 里也不再有 `.claude/skills/`，顺带挡住了 0.1.0 的 cargo 一级（8.4）。CI 生成的文件与 include、`.gitignore` 怎样交互，要用 `cargo package --list` 验证（§7 第 6 条）。
5. 在 PR 阶段就拦住：校验规则只写在 Rust 里（8.7），再加一个测试，用安装器自己的校验函数遍历仓库里的 `.claude/skills`，并核对 frontmatter 的 `name` 等于目录名、description 只有一行。这样，安装器会拒绝的 skill 改动在 PR 的 `cargo test` 就会失败，不必等到发布或用户安装时才暴露。规则只有一份实现，打包脚本与安装器不会各说各的。
6. `.gitattributes` 加一行 `.claude/skills/** text eol=lf`，让任何平台检出的字节都相同。这样即使有人在 Windows 上本地打包（新包首发就要在本地手工发布），哈希也对得上。
7. 可选：0.2.0 把日志改成英文后，skill 里引用的中文日志也会跟着改掉。之后可以加一条 CI 规则，要求 skill 文件只含 ASCII（测试脚本里的中文样例改用 `\u` 转义）。【推断】Windows 上的 agent 如果经 PowerShell 截获 `skill show` 的输出，会按控制台代码页解码，非 ASCII 字符可能变成乱码（§7 第 8 条）。

### 8.7 show 的读取规则（不能借它读 skill 以外的文件）
名字与路径规则（打包、安装、读取三处共用同一个函数）：
- skill 名：`[a-z0-9]+(-[a-z0-9]+)*`，不超过 64 个字符（Agent Skills 规范的 name 规则），而且必须等于 frontmatter 的 `name`。
- 文件路径：
  - 只用 `/` 分段。
  - 每段非空，只含 `[A-Za-z0-9._-]`，不以 `.` 开头（排除了 `.`、`..` 和隐藏文件），也不以 `.` 结尾（Windows 会吞掉结尾的点）。
  - 去掉扩展名后不能是 Windows 保留设备名（`CON`、`PRN`、`AUX`、`NUL`、`COM0-9`、`LPT0-9`）。
  - 同一个 skill 内，按大小写折叠后不能重名。
  - 字符集本身已经排除了 `\`、`:`（盘符与 ADS）、空格、控制字符和 `$`；以 `//` 开头的路径（UNC）和空段也都通不过。

读取步骤：
1. 读 `<home>/skills/manifest.json`。路径固定，读取有大小上限，也按第 3、4 步拒绝链接。文件不存在、解析失败或 schema 不认识，都按「未安装」提示，并以非零码退出。
2. `name` 与 `file` 要和记录里的键逐字节精确匹配：不折叠大小写、不做规范化、不做前缀匹配。用户的输入从不拼进路径，拼路径用的是记录里校验过的键。
3. 从 `<home>/skills` 往下逐级调用 `symlink_metadata`：每一级都不能是链接，最后一级必须是普通文件。【事实】Rust std 在 Windows 上把带 name-surrogate 位（0x20000000）的重解析点都报告为 `is_symlink()`，符号链接和 junction 都在其中（rust-lang/rust `library/std/src/sys/fs/windows.rs` 的 `FileType::new`）。在 Windows 上再检查 `MetadataExt::file_attributes()` 的 `FILE_ATTRIBUTE_REPARSE_POINT` 位，任何重解析点都拒绝。`<home>/skills` 本身及其上级目录不检查，那是用户自己的布局。
4. 打开文件：unix 用 `O_NOFOLLOW | O_NONBLOCK`，遇到 FIFO 不会卡住；Windows 用 `FILE_FLAG_OPEN_REPARSE_POINT`，打开的是链接本身，不跟随。打开后用句柄的 metadata 再确认一遍：是普通文件，不是重解析点，大小等于记录里的值，而且不超过上限。上限建议 1 MiB，现在最大的文件是 35 KB。
5. 把文件读进内存（`take(size + 1)`），核对长度和 sha256，再检查是否为合法的 UTF-8，然后把**同一块内存**写到 stdout。任何一项不一致，就报「文件已被改动或损坏，执行 `aproxy skill install` 修复」并以非零码退出，不输出任何内容。前几步可能被竞争绕过，这一步不会，所以它是决定性的一步。
6. 规范化前缀检查（`canonicalize` 后用 `starts_with` 比较）可以保留作纵深防御，但不能替代第 5 步。

报错只回显用户给出的名字和键，不回显文件内容。

### 8.8 安装侧要一起改的
- 解包按清单白名单进行（8.5 (b)）。如果不做清单，就按 8.7 的规则校验每个条目名，并给条目数和总字节数设上限。符号链接条目直接拒绝。
- 落位保留现有的逐目录替换：用户可能把 `~/.claude/skills/aproxy-cli` 链接到了 `<home>/skills/aproxy-cli`，目录路径必须保持不变。旧记录里有、新清单里没有的 skill 目录要删掉；记录之外的目录不动，show 也不认它们。
- 互斥：`skill install` 与二进制安装里的 skill 步骤拿同一把锁 `<home>/skills/.lock`，沿用 install.state 的 `create_new` + pid + 过期判定写法（`src/install/state.rs:8-11,280-296`）。`File::try_lock` 需要 Rust 1.89，而本仓库是 `rust-version = 1.88`（`Cargo.toml:26`）。
- skill 由接手者在交换成功后安装（CI-15）。install.state 里 `skill` 字段的语义随之定稿，0.2.0 发布后冻结。
- 完成提示：列出每个 skill 的绝对路径，并按平台给出链接示例。unix 用 `ln -s`；Windows 用 `mklink /J`（junction 不需要管理员权限或开发者模式，`/D` 需要），路径写展开后的绝对路径。用它替换 `src/commands/install.rs:191-194` 那句提示：那句只适用于 Windows，而且 `~` 在 cmd 里不会展开（CI-21）。写死 `aproxy-cli` 的 `skill_dir_in` 一并删掉。

### 8.9 分级与成本
| 项 | 级别 | 成本 |
|---|---|---|
| `aproxy skill install/upgrade/list/show`，删除 `--skills-only`，同步文档与 skill | P1（用户需求）；命令形状在 0.2.0 定型后冻结 | M |
| show 的读取规则（8.7）与本地记录（兼作提交标记） | P1，随 show 一起交付，两者缺一不可 | M |
| `--help` 提示行，以及覆盖全部子命令的测试 | P1 | S |
| 「skill 版本 = 二进制版本」，skill 由目标版本安装 | P1 | S（含 CI-15） |
| 单一打包脚本、三个渠道同一个 zip、发布清单、资产名与包内路径改名 | P1⚑（挡住 0.1.0 只能在 0.2.0 做） | M |
| 解包白名单与上限、清理残留的 skill、加锁 | P1 | S |
| `.gitattributes` 行尾；skill 文件只含 ASCII | P2 | S |

### 8.10 与 W4 草案不同、需要确认的地方
1. 白名单是安装时写下的本地记录，发布清单只是它的来源之一（8.5）。草案写的是「发布的清单作白名单」，两者的区别在于：安全不依赖发布侧，而依赖 show 在同一块内存上核对哈希。
2. `skill upgrade` 与 `skill install` 同义，都对齐到二进制的版本。如果用户的本意是「upgrade 取最新」，就会与 8.4 第 1 条冲突，不建议这样做。
3. 0.2.0 不提供 `skill install --version`（8.4 第 2 条）。
4. `list` 在未安装时以 0 退出，只有 `show` 以非零退出（8.3）。
5. `list`/`show` 的输出和报错固定用英文（8.3）。
6. skill 包换资产名和 npm 包内路径，crate 改为带 zip 而不带目录，用来挡住 0.1.0（8.4、8.6）。
7. skill 跟随二进制版本以后，`references/latest/` 加「大版本留存旧版目录」的策略就不再必要，见 D5。
