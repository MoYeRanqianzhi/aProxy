# 0.2.0 审查：发布工程 / 依赖卫生 / 测试基建 / 文档 / 项目记录

> 审查日期 2026-10-11，范围见任务单第 1-6 项。只读：没有改动仓库里任何已有文件，没有运行 cargo build/test/clippy，
> 没有启动 aproxy 进程，没有读写 `~/.aproxy/`，没有结束任何进程。gh 只用了只读命令（`release list/view`、`pr list/view`、
> `run list/view`、`api` 的 GET）；另用 curl 读 crates.io 公共 API、`npm view` 读 registry，用 WebSearch 查 Tauri 相关外部事实。
> 基线提交 `c1d4b96`（master 与 origin/main 同点），距 v0.1.0 共 63 个提交。
>
> **证据口径**：每条发现分「事实」（附文件:行、命令输出或外部链接）与「推断」（我的推理，没有直接实测，标明需要怎样验证）。
> 没写「推断」的内容都是我读到或跑到的。外部事实的来源链接放在正文里。
>
> **前提说明**：任务单把 UI 描述为「很可能是 workspace 新成员，可能再抽 aproxy-ipc」；而记忆 `0.2.0-direction` 与 `plan/release-0.2.0.md`（用户 2026-10-11 定调）已经是
> 「Tauri 2 编进本体、cargo feature `ui` 默认启用、`--no-default-features` 出无 UI 版」。本报告按后者评估，并在 D.1 / D.3 / RT-05 里写明独立二进制、独立 crate 两种备选与它们的差别。

## 0 总体判断

1. **0.1.0 发布前那批发布工程阻断项基本都已修好，而且修得有质量**：发版线隔离、Latest 标记、npm dist-tag、发布 job 幂等重跑、glibc 下限
   （zigbuild + objdump 断言）、Release 门禁覆盖 windows+ubuntu、`--workspace`、权限最小化、环境 tag 策略，全部能在代码和 GitHub 设置里
   对上证据（附表 A）。format 线的 Trusted Publishing 也已实际跑通。这部分不需要重做。
2. **0.2.0 真正的发布风险不在旧账，而在两件新事**：
   - **去掉 0.1.0 兼容之后，0.1.0 自带的升级入口没有护栏**（RT-01）。0.1.0 的选版逻辑、npm 兜底、安装器流程都已发出去收不回；「在改动任何东西
     之前失败」这个目标（plan W1）对这条路径无法由 0.2.0 的代码保证。
   - **UI 编进本体 + 默认启用，把「发布资产命名 / 变体选择」「Linux 动态库依赖」「测试 harness 链接」三个原本稳固的基础同时拉进了设计空间**
     （RT-03 / RT-04 / RT-05）。资产命名是跨版本契约，0.2.0 之后冻结；Linux 的 UI 版会让「缺 webkit2gtk 的服务器上连 `aproxy --version` 都跑不起来」；
     Tauri 代码一旦进入 lib 或任何测试二进制，Windows 上 `cargo test` 会整体崩溃（上游 tauri#13419）。这三件事现在做决定几乎零成本，发布后再改是破坏性的。
3. **测试基建的隔离债务已基本还清**（`a8ac56a` 之后所有子进程调用都带 `APROXY_HOME` 注入，我逐个核对过），但 `tests/common` 一直没建：
   9 个 helper 在多个测试文件里各写一份、3 套守卫、2 套端口派生。i18n 要给每个 CLI 子进程统一加 `APROXY_LANG=en`，没有公共入口就要改 65 处
   `Command::new`（RT-15）。直接匹配中文输出的断言约 116 处（tests 79 + src 单测 37，附表 C），量不大，真正的风险是语言选择的实现方式（RT-16）。
4. **文档与项目记录有若干会误导人的陈旧点**：`.agents/TODO.md` 477 行里 57 个完成项没删、22 个未完成项里 8 个已做完或可删、7 个应并入 0.2.0 计划，
   且第 21 行与用户 10-11 的定调直接矛盾；`docs/architecture.md` 的「版本路线」仍写「0.2.0 引入 UI（Web 面板形态待定）…属远期」；Release 页没有变更内容；
   skill 里有 237 行引用程序的中文输出原文，校验脚本只在 gitignore 的本地目录里。
5. **CI 红灯已成常态**：最近 30 次 CI 运行无一整体成功（macOS 28/28 失败，2 次取消），信息量为零（RT-13）。UI 落地后这会掩盖真正的新失败。

## 1 发现总览

级别：P0 = 0.2.0 前必须改（含必须先做的决定）；P1 = 0.2.0 应改；P2 = 之后。成本：S 小、M 中、L 大。

| ID | 级别 | 类别 | 一句话 | 成本 |
|---|---|---|---|---|
| RT-01 | P0 | 发布策略/决策 | 去兼容后 0.1.0 的升级入口（install/npm/脚本）没有护栏，W1「改动前失败」做不到；需要预发布窗口 + 一次实测 + 迁移说明 | M |
| RT-02 | P0 | 版本 | 工作区仍是 0.1.0，而 IPC/磁盘/CLI 即将不兼容：dev 构建与真 0.1.0 同号不同协议，所有按版本比较的判定失灵 | S |
| RT-03 | P0 | 永久接口/决策 | 发布资产命名与变体选择在 0.2.0 定稿；UI 引入第二维度；无后缀应保持 headless | M-L |
| RT-04 | P0 | 可行性/决策 | Linux UI 版：动态依赖 webkit2gtk/GTK 与「缺库代理起不来」、glibc 下限 2.28 与 zigbuild 方案冲突 | L |
| RT-05 | P0 | 结构/测试 | Tauri 代码不能进入 lib 和任何测试 harness（Windows 清单问题）；根包要有 build.rs 与 `custom-protocol` | M |
| RT-06 | P1 | 版本 | 兄弟 crate（envelope 0.2.0 未发布）的冻结时机；新 crate 首发的前置 | S |
| RT-07 | P1 | CI/门禁 | 双构建矩阵（headless / ui）、依赖隔离守卫、系统依赖、MSRV、工具链钉版、cargo-deny 与 Tauri | M |
| RT-08 | P1 | 发布产物 | 必需/可选资产清单、`needs` 连坐、构建冒烟、-v3 选择只看 AVX2 | M |
| RT-09 | P1 | 维护 | release.yml 与 release-format.yml 近乎复制（矩阵、对齐脚本、glibc 断言、crates plan） | M |
| RT-10 | P1 | 发布说明 | Release 页没有变更内容，0.2.0 尤其需要迁移说明 | S |
| RT-11 | P1 | 安装入口 | 引导脚本与 agent 指南取自 main 分支，且脚本零 CI 覆盖；文案语言不一致 | M |
| RT-12 | P1 | 依赖 | tower/tower-http 未使用；5 个 dependabot PR 悬着；0.2.0 是升大版本的最后廉价窗口 | M-L |
| RT-13 | P1 | CI/决策 | CI 整体恒红（macOS）；注释与数字过期 | S |
| RT-14 | P1 | 流程 | 没有发版清单文档；0.2.0 应走 rc 演练；npm `next` 落后 | M |
| RT-15 | P1 | 测试基建 | `tests/common` 未建；i18n 迁移以它为前提；proxy_integration.rs 7091 行 | M |
| RT-16 | P1 | i18n/测试 | 断言面与语言选择机制（不要进程级缓存）；中文目录需要自己的覆盖 | M-L |
| RT-17 | P1 | skill | 237 行中文输出原文；校验脚本不在仓库；references 留存规则字面执行会归档错内容 | M |
| RT-18 | P1 | CHANGELOG | `[Unreleased]` 叙述的是 0.1.x 补丁线读者，而 0.2.0 直接删兼容，发布时自相矛盾 | S |
| RT-19 | P1 | 项目记录 | TODO.md 清理（57 个完成项、22 个未完成项的处置） | S |
| RT-20 | P1 | 渠道 | cargo install / crates.io include 白名单 / npm / binstall 对 `ui` feature 的语义 | M |
| RT-21 | P1 | skill 发布 | skills 包两条生成路径，无清单与版本映射；`aproxy skill show` 白名单需要单源清单 | S-M |
| RT-22 | P2 | 测试覆盖 | read/connect 超时、HTTP/SOCKS 上游、下载渠道离线 mock 仍无测试 | M-L |
| RT-23 | P2 | 供应链/签名 | action 钉 SHA、provenance、tag 创建限制、Windows 签名、第三方许可证清单 | M |
| RT-24 | P2 | 文档 | CONTRIBUTING、人类版排障页、systemd/launchd、两个子 crate 无 README、format 缺 binstall 元数据 | S-M |
| RT-25 | P2 | 记录卫生 | plan/ 状态、DOCS.md 过时、memory 元数据缺失 | S |

## 2 发现明细

### RT-01 ［P0｜发布策略/决策］去掉兼容层后，0.1.0 自带的升级入口没有护栏

**事实**
- 0.1.0 已发布的二进制里，`aproxy install`/`upgrade` 取「版本号最大的稳定版」（CHANGELOG 0.1.0「`aproxy install --pre` 与更新通道」）；npm 渠道兜底读
  `latest` dist-tag（现状 `npm view @meowo/aproxy dist-tags`：`latest=0.1.0`、`next=0.1.0-rc.1`）；一键脚本「已安装则不覆盖，升级走 `aproxy install`」
  （`scripts/install.sh:248-252`）。所以 0.1.0 用户拿到 0.2.0 的路径，就是他手里的 0.1.0 二进制自己驱动 `install`。
- `src/compat_0_1_0.rs:1-23` 自己写明了为什么需要兼容层：升级窗口里「0.1.0 的安装器、留在 PATH 上的 0.1.0 CLI 要控制新实例」，三处差异是 Windows 管道名、
  线上格式（没有 `v` 字段）、安装宣告名。删除后，0.1.0 安装器在滚动重启阶段按旧管道名找新实例会找不到。
- 失败发生在 0.1.0 的代码里（下载 → `<staged> --version` 自证 → 广播 PrepareSwap → 交换 → 滚动重启 → 按旧协议探测就绪）。0.2.0 的代码只有两个介入点：被自证调用时
  （无法区分调用者）、被拉起为守护时（bin 已经换掉）。因此 `plan/release-0.2.0.md` W1 里「必须在改动任何东西之前失败」对这条路径**无法由 0.2.0 的代码保证**。
- 现有记录互相矛盾：`TODO.md:21` 和记忆 `ipc-v1-upgrade-contract` 写「CHANGELOG 写明升级下限（先装到某个 0.1.x 再升 0.2.0）」，但不存在可装的 0.1.x；
  用户 10-11 的定调（记忆 `0.2.0-direction`）是「0.1.0 用户重新安装」。
- 现有的 `upgrade-from-0-1-0` CI 任务（`ci.yml:70-96`）正是这条路径的真实演练，计划要整条删掉。

**推断（需要实测）**：0.1.0 有「滚动重启失败回滚」（CHANGELOG 0.1.0）。最好的结局是实例被拉回、bin 里却已是 0.2.0、`install.state` 留在 failed，之后每条 0.2.0 命令
面对这份旧现场；最坏的结局是实例丢失。config-cli-install 审查会读 `git show v0.1.0:src/install/...` 给代码层结论；发布层需要一次真机实测来定。

**为什么重要**：用户极少，但这些用户恰好是「已经把 agent 会话挂在 aproxy 上」的人，实例丢失等于会话断流——正是项目要避免的事。而且这是发出去就收不回的东西。

**建议（组合）**
1. 先发 `0.2.0-rc.N`（GitHub prerelease，npm `next`）。0.1.0 的 Stable 通道不会选中预发布，这给出一段对旧用户无风险的演练窗口。
2. **别删 `upgrade-from-0-1-0` 任务，改成一次性的结局实测**：真 v0.1.0 二进制对 rc 构建执行 `install`，记录实例是否存活、`bin/` 与 `install.state` 的终态、
   之后 0.2.0 命令能否自愈。结果决定下一步。
3. 若结局是「安全失败/可恢复」：把该任务改成回归断言（升级不留烂摊子），随 0.2.x 保留；若结局是「丢实例」：二选一——(a) 稳定版 0.2.0 推迟、npm `latest` 暂不指向它，
   给旧用户迁移时间；(b) 0.2.x 保留 S1–S3（管道名/线上格式）一个 minor。需要用户拍板。
4. 无论结果如何：Release 正文、CHANGELOG、README、`INSTALL_AGENT.md` 写清手工迁移（`aproxy stop all` → 删除 `~/.aproxy/bin` → 重新安装 → 配置去留）；三个引导脚本现在
   「已安装则不覆盖」，需要 `--reinstall`/`-Force`（现在没有），否则「重新安装」要靠用户手删目录。
5. 更正 TODO.md:21 与记忆 `ipc-v1-upgrade-contract` 里的「升级下限」措辞。

**成本** M（一次实测 + 文档 + 脚本参数）

### RT-02 ［P0｜版本］工作区版本号仍是 0.1.0，而协议即将与 0.1.0 不兼容

**事实**
- `Cargo.toml:21` `version = "0.1.0"`，`Cargo.lock` 同；`aproxy-envelope` 已是 0.2.0（`Cargo.toml:78` 依赖它）。skill `compatibility.md` 写明约定：「开发构建报告上一个发布版本号」。
- 按 `CARGO_PKG_VERSION` 做 semver 比较的地方至少有：看护者 R1 退役旧看护者（`src/watchdog.rs:62,796,983`）、status 的「实例版本与 CLI 版本不同」提示（`src/commands/status.rs:44`）、
  install 的降级防呆与「已是最新」（`src/commands/install.rs:164,230`）、`flow.rs:327` 的接手判定。
- `TODO.md`、`plan/release-0.2.0.md` 当前没有任何一处提到要提前把工作区版本号改掉（W7 只提到发布时的 0.2.0）。

**推断**：compat 删除后，从 main 构建的二进制与真实 0.1.0 **同号但互不兼容**。同号意味着：不退役对方的看护者、不提示版本差异、`install latest` 判「已是最新」，
而作者本机恰好是 0.1.0 生产实例与开发构建共存的环境，最先踩到。

**建议**：在第一个破坏性提交之前，把根包与 `Cargo.lock` 里的 aproxy 版本改成 `0.2.0-alpha.0`。选它而不是 `-dev`：semver 预发布标识按 ASCII 比较，`dev` > `alpha`，
dev 构建会排在真实的 `0.2.0-alpha.N` 之后；`alpha.0` 小于任何 `alpha.N`。发布时仍由 tag 对齐步骤覆盖（`release.yml:176`）。同步修改 `compatibility.md` 的约定一句。

**成本** S

### RT-03 ［P0｜永久接口/决策］发布资产命名与变体选择必须在 0.2.0 定稿，UI 引入第二个维度

**事实**
- 资产名 `aproxy-<target>[-v3][.exe]` 现在至少在 7 处各写一份：`release.yml` 构建矩阵、`release-format.yml` 同款矩阵、`src/install/download/mod.rs:29-42,258-266`、
  `scripts/install.sh` / `install.ps1` / `install.cmd` 的平台解析、`npm/build-and-publish.sh:127-137` 的 MAPPINGS 与 `npm/aproxy/bin/aproxy.js` 的映射、零配置 cargo-binstall
  （靠命名命中内置模板，记忆 `trusted-publishing-p0` 要点 6）。
- `latest_version` 用「本平台 baseline 资产是否存在」判断 release 已齐（`src/install/download/github.rs:103,190-196`）。
- 安装器要下载**未来版本**的资产，所以命名是跨版本契约，0.2.0 之后冻结（和记忆 `ipc-v1-upgrade-contract` 里的永久接口同类，但那份清单里没有它）。
- 用户定调（`plan/release-0.2.0.md` W5 待定第 1 条）：UI 编进本体、`ui` feature 默认启用、Linux 同时发 UI 版与无 UI 版，musl 与 aarch64 Linux 只出无 UI 版。

**建议（给用户拍板）**
- **无后缀资产保持为无 UI（headless）基线。** 理由：零配置通道（binstall、npm 平台包、`install.sh` 的旧逻辑）只能按名字取资产，不能探测 GTK；Linux 上取到需要 webkit2gtk 的二进制，
  结果是代理起不来（RT-04）。这也保住「aproxy 本体体积不受影响」那条原则在这些通道上的含义。
- UI 版加后缀 `-ui`，只出 baseline 指令集，**不与 `-v3` 组合**（WebView 主导，v3 收益无意义，且会让矩阵翻倍）。变体空间保持一维 `{"", "-v3", "-ui"}`，
  `asset_name(target, variant)` 签名不用改。
- 完整性判据不变：每个 target 的 headless baseline 资产必须存在；UI 资产在清单里标 required/optional（RT-08）。
- 选择策略：升级默认「与当前二进制同款」（需要一个稳定的自报入口。**不要动 `--version` 第一行**，安装器的自证依赖它——放进 `aproxy version --json` 或类似子命令）；首装按平台能力选
  （Windows x64/arm64 → ui；Linux gnu 且 glibc 满足 UI 下限且探到 libwebkit2gtk-4.1 → ui；其余 headless）；`--variant ui|headless|v3|baseline` 手动覆盖。
- **单源**：新增一份机器可读的资产清单（target × flavor × runner × 是否 required × glibc 下限），由 workflow 矩阵（生成或校验）、`install`（`include_str!`）、脚本（发布时内嵌）、
  npm 组包和一个断言测试共同读取。这同时解掉上一轮 release-08/13 与路线图第 15 项的「单源常量 + 断言测试」建议。

**成本** M-L

### RT-04 ［P0｜可行性/决策］Linux UI 版：动态库依赖、glibc 下限与「缺库代理起不来」

**事实**
- Linux gnu 产物当前用 cargo-zigbuild 按 `GNU_GLIBC_FLOOR=2.28` 链接，构建后用 `objdump -p` 断言（`release.yml:38,213-250`）；`install.sh:29-31` 与 `aproxy.js:24` 的 `MIN_GLIBC` 必须与它一致。
- Tauri 2 在 Linux 需要 `libwebkit2gtk-4.1`；官方建议在「同时具备 4.1 包的最老基线」上构建，文档点名 Ubuntu 22.04 / Debian 12
  （[Prerequisites](https://v2.tauri.app/start/prerequisites/)、[AppImage](https://v2.tauri.app/distribute/appimage/)、[Debian](https://v2.tauri.app/distribute/debian/)：「在更新的系统上构建会抬高最低 glibc」）。
  Ubuntu 20.04 没有 4.1 包（[gitbutler#7671](https://github.com/gitbutlerapp/gitbutler/issues/7671) 里 20.04 上 `tauri info` 报 webkit2gtk-4.1 未安装）；
  [tauri#9662](https://github.com/tauri-apps/tauri/issues/9662) 的讨论里有评论者称在 Ubuntu 24 上构建的 AppImage 因 glibc 不满足而无法在 20.04 上运行（社区说法，非官方结论）。
- 守护、看护者、`install --continue` 续作与 GUI 是同一个可执行文件（控制面审查 CP-19 已指出「缺库连代理都起不来」）。

**推断**
1. Linux UI 版不能走 zigbuild 2.28 方案：它要链接宿主机的 GTK/WebKit 共享库，glibc 下限随构建机走（≥ 2.35）；需要 `ubuntu-22.04`（x64）与 `ubuntu-22.04-arm`（Tauri 文档提到的 runner）
   这类基线，而不是 `ubuntu-latest` / `ubuntu-24.04-arm`。
2. 动态链接的 UI 版在缺 libwebkit2gtk 的服务器上，加载器直接报缺 `.so`，`aproxy --version`、`start`、看护者全部起不来。这是对「不中断」使命级别最高的回归，必须靠**发布策略**
   避免（RT-03 的命名 + 安装器探测），不能靠文档。
3. Windows 缺 WebView2 运行时是否会拖累守护启动：wry 在 Windows 上静态链接 loader、窗口创建时才失败（推断，需 spike 验证）。

**建议**
- Linux UI 版独立常量 `GNU_UI_GLIBC_FLOOR`（预计 2.35）与对应的 `MIN_GLIBC_UI`；构建后断言「最高 `GLIBC_` ≤ floor」且 `ldd` 引用的库在白名单内（webkit2gtk-4.1、gtk-3、ayatana-appindicator3、libsoup-3.0…）。
- `aproxy install` 选 `-ui` 之前探测库存在（`ldconfig -p` 或 dlopen 探针），探不到回 headless 并提示一句；musl/aarch64-musl 永远 headless。
- npm、binstall、`cargo install` 默认不给 UI 版（RT-20）。
- 先做 spike：`ubuntu:22.04` 容器里构建 UI 版，在 `debian:12-slim`（无 GTK）与装了 GTK 的镜像里各跑 `--version` / `start` / `ui`，核对失败形态与体积、RSS。

**成本** L

### RT-05 ［P0｜结构/测试］Tauri 代码不能进入 lib 和任何测试 harness

**事实**
- 根包同时是 lib + bin：`src/lib.rs` 声明 11 个 `pub mod`（外加私有的 `compat_0_1_0`）；bin 专有模块 `server.rs`、`commands/config.rs`、`commands/mod.rs` 带单测（grep `cfg(test)`）；`tests/` 9 个集成测试文件链接 lib。
- 外部事实：Tauri（经 muda/rfd）静态导入 comctl32 的 `TaskDialogIndirect`，只有 exe 带 Common-Controls v6 清单时才可用；`tauri-build` 通过 `cargo:rustc-link-arg-bins` 只给 **bin** 嵌清单，
  lib 的单测 harness 与集成测试 exe 一启动就以 `0xc0000139 STATUS_ENTRYPOINT_NOT_FOUND` 退出，上游 issue 是
  [tauri#13419](https://github.com/tauri-apps/tauri/issues/13419)，下游常见绕法是 `/MANIFEST:EMBED` 对所有产物嵌清单或 `/DELAYLOAD:comctl32.dll`
  （例：[atlas#332](https://github.com/pacifio/atlas/issues/332)）。
- Tauri 脚手架里生产构建依赖 `custom-protocol` feature，否则产物会去连 devUrl（我对模板注释的记忆，**需 spike 确认**）。

**后果（推断）**：如果 UI 模块声明在 `lib.rs`，或 lib 引用了 `ui` feature 的 Tauri 依赖，Windows 上 `cargo test --workspace` 整体崩溃；bin 若保留单测，bin 的 harness 也有同样风险
（`link-arg-bins` 是否覆盖 bin 的 test harness 需实测）。

**建议**
1. `src/ui/**` 只由 `main.rs` 声明，`lib.rs` 不声明；lib 里放无 Tauri 的 view-model/数据适配（可 headless 测试），Tauri command 只做 1:1 转发。
2. 把 bin 里的单测迁进 lib（控制面审查 CP-17 的「bin 变薄壳、serve_forever 迁入 lib」本来就要求），`[[bin]]` 设 `test = false`。
3. 根包 `build.rs`：`CARGO_FEATURE_UI` 存在时才调 `tauri-build`（optional build-dependency），无 UI 构建时空实现；`tauri.conf.json`、`capabilities/`、`icons/` 放根包。
4. `ui = ["dep:tauri", "dep:tauri-build", "tauri/custom-protocol", …]`，feature 定义和理由写进 `Cargo.toml` 顶部 workspace 注释那一段。
5. **不要为 UI 另建 `aproxy-ui` crate**：`aproxy` 对它的 path 依赖在 `cargo publish` 时必须有 version 且该 crate 已在 crates.io（即使 optional），多一个要首发、要绑 trusted publisher 的包（RT-06）；
   而 `generate_context!` 与清单/图标资源仍要落在最终二进制所在的包里，换来的隔离并不比 feature + 依赖守卫（RT-07）多。
6. 先做 15 分钟 spike：Windows 上 `cargo test --workspace`（默认 features）能起来；`cargo build --release` 产物能弹窗。

**成本** M

### RT-06 ［P1｜版本］兄弟 crate 的冻结时机与新 crate 首发

**事实**
- crates.io 上 `aproxy-envelope` 只有 0.1.0，`aproxy` 0.1.0、`aproxy-format` 0.1.0/0.1.1（curl crates.io API 实测）；工作树里 envelope 已是 0.2.0 且被 aproxy 依赖。
- `release.yml:418-426` 的 plan 步骤会在**第一个**包含它的 tag 上发布 envelope 0.2.0；「Align version」只对齐 aproxy 包（`release.yml:176-200`），envelope 的版本是静态的。
- 记忆 `release-version-alignment`：版本号一旦发布永久占用（alpha.10/11 被测试 tag 抢注）。数据面审查 DP-13 也要求「信封 0.2.0 发布前定稿」。

**后果**：如果 `0.2.0-rc.1` 就把 envelope 0.2.0 发上去，之后信封再改，只能 0.2.1，与 aproxy 0.2.0 的依赖声明、aproxy-format 的下一版依赖全部错位。

**建议**：信封定稿（DP-13）排在第一个 0.2.0 预发布 tag 之前；或预发布期间给 envelope 用 `0.2.0-rc.N` 并由对齐步骤同步。写进发版清单（RT-14）：任何兄弟 crate 变更必须在 tag 之前 bump。
若结构拆分真的新增 crate（本报告建议不拆），新 crate **首发不能走 OIDC**，必须所有者本地 token 首发（记忆 `trusted-publishing-p0`；`~/.cargo/credentials.toml` 里的 token 在 10-06 记录为已失效，要重发）
再用 API 绑定 trusted publisher（`release.yml`、环境 `release`），列为 tag 前置。

**成本** S

### RT-07 ［P1｜CI/门禁］双构建矩阵、隔离守卫、系统依赖、MSRV、工具链、cargo-deny

**事实**
- 三个 workflow 的 test 步骤都是 `cargo clippy|test --workspace --locked`（`ci.yml:47,50,67,68`、`release.yml` test 矩阵、`release-format.yml` test-format）。
  `ui` 进 default features 之后，每个 job 都要编译 Tauri：ubuntu 需要 apt 装 libwebkit2gtk-4.1-dev、libgtk-3-dev、libayatana-appindicator3-dev、librsvg2-dev、libxdo-dev 等
  （[Prerequisites](https://v2.tauri.app/start/prerequisites/)），还要在 cargo 之前跑 Node 前端构建。
- MSRV：`Cargo.toml:26` 声明 1.88，但 CI 全部是 `dtolnay/rust-toolchain@stable`（3 个 workflow 共 10 处），没有任何 job 用 1.88 验证（上一轮 release-16 未修）。
  aproxy-format 声明 1.96.1。本机 stable 是 1.98.1。Tauri 依赖树会抬高 UI 版的 MSRV（未测）。
- `deny.toml` 只有 sources/advisories(v2, yanked=deny)/licenses，没有 ignore。Tauri 的 Linux 依赖树含 gtk-rs GTK3 绑定的一组 unmaintained 通告
  RUSTSEC-2024-0411…0420（无补丁版本，见 [RUSTSEC-2024-0418](https://rustsec.org/advisories/RUSTSEC-2024-0418.html) 与 Tauri 自己的跟踪 issue
  [tauri#11928](https://github.com/tauri-apps/tauri/issues/11928)）。Review workflow 会在 UI 依赖进来的那一次提交变红。

**建议**
1. `headless` 任务（windows + ubuntu）：`cargo clippy --workspace --all-targets --locked --no-default-features -- -D warnings` 和 `cargo test --workspace --locked --no-fail-fast --no-default-features`。
   不装 GTK、不装 Node，快，是核心使命的主门禁。
2. `ui` 任务：默认 features，装系统依赖 + Node，跑 clippy / test / 前端 lint + 单测。
3. **依赖隔离守卫**（脚本，会失败的那种）：`cargo tree -p aproxy --no-default-features -e normal,build --prefix none` 的输出不得含 tauri、wry、tao、webkit2gtk、gtk、muda、tray-icon、window-vibrancy…
   再加 headless 发布产物体积预算（0.1.0 基线：windows x64 6.51 MiB、linux gnu 6.11 MiB，`gh release view v0.1.0` 实测）。这把「aproxy 本体体积不受影响」从注释变成检查。
4. MSRV 任务：`cargo +1.88 check --locked --no-default-features -p aproxy -p aproxy-envelope`；UI 版 MSRV 单独测出来写进 README。
5. 工具链：发布门禁钉具体版本（`rust-toolchain.toml` 或 workflow 级 env），另设「最新 stable」金丝雀；升级工具链是一次显式提交。否则发布日遇到新 clippy lint 就被 `-D warnings` 挡住。
6. deny.toml：UI 依赖入库前，用 `ignore = [{ id = …, reason = … }]` 逐条登记并写复查条件；许可证白名单和 `[graph]` 目标重跑一次。
7. rust-cache 按 flavor 分 key；Windows ARM 与 Tauri 的编译时间要在 rc 阶段实测（`lto="fat"` + `codegen-units=1` 对 ~500 个 crate 的树会很慢，推断）。
8. dependabot 加 `npm`（`/ui`）生态，并给 tauri 族分组（避免每周一堆单独 PR）。

**成本** M

### RT-08 ［P1｜发布产物］必需/可选资产清单、构建冒烟、变体可用性

**事实**
- `release` job `needs: [build, skills]`（`release.yml:284`）。矩阵任意一格失败（`fail-fast: false` 只保证其余格跑完）则 release 与两个渠道 job 全部跳过。
  UI 格是新增且最容易失败的，它会连坐 headless 产物；反过来某格缺失也没有「期望资产清单」断言去发现（只有 install 端的 baseline 判据）。
- 构建后冒烟只有 zigbuild 目标跑 `--version`（`release.yml:213-250`）；musl、i686、aarch64、-v3、macOS 都不运行（上一轮 release-13 未修）。
- `-v3` 选择只看 `is_x86_feature_detected!("avx2")`（`src/install/download/mod.rs:258`），但 v3 资产只有 windows x64 与 linux gnu x64 两个 target；其余 AVX2 机器上 github 渠道 404 后掉到下一级渠道（上一轮 release-08 未修）。
  AVX2 不等于 x86-64-v3（还要 BMI1/2、FMA、LZCNT、MOVBE）。v3 与 baseline 体积几乎相同（windows 6.49 vs 6.51 MiB；linux 6.11 vs 6.11 MiB），仓库里没有 v3 的收益数据（记忆 `release-engineering` 承诺的「如实呈现」没兑现）。

**建议**
- 清单（RT-03）标 required/optional；`release` 之前加断言步骤：rc 允许 optional 缺失并在 Release 正文列出，stable 要求 required 全在。UI 格放进独立 job `build-ui`，
  `release` 对 headless 必需格硬依赖、对 UI 格在 rc 上容忍。
- 凡能在 runner 上原生运行的格，都跑 `--version` + 起一个隔离 home 的守护 ping/stop 冒烟。
- `-v3` 选择由清单判断「该 target 是否发布 v3」，CPU 检测补全 BMI/FMA/LZCNT/MOVBE；基准数据要么用 `examples/loadgen.rs` 补出来，要么发布说明里不再暗示收益。

**成本** M

### RT-09 ［P1｜维护］release.yml 与 release-format.yml 近乎复制

**事实**：构建矩阵整份复制；「Align version」（`release.yml:176,395` / `release-format.yml:160,329`）、glibc 断言（`:213` / `:194`）、crates plan（`:418` / `:355`）各有两份内联脚本，
注释自认「与 release-format.yml 的同名步骤同一套逻辑，改动须同步」。`npm/build-and-publish.sh:86-111` 的 `GROUP`/`GROUP_ARG` 解析冗余。UI flavor 会带来第三份矩阵消费者。

**建议**：抽 `_build.yml`（`workflow_call`：包名/二进制名/flavor/资产前缀/是否 zigbuild）与 `scripts/ci/{align-version.py,assert-glibc-floor.sh,plan-crates.sh}`；两个 workflow 只留触发器与参数。
**注意**：npm/crates.io 的 Trusted Publisher 按 workflow 文件名匹配（记忆 `trusted-publishing-p0` 的 10-04/10-06 教训），publish job 必须仍留在顶层文件里，只抽构建部分。
`build-and-publish.sh` 改子命令式。

**成本** M

### RT-10 ［P1｜发布说明］Release 页没有变更内容

**事实**：`gh release view v0.1.0` 正文 = 下载与更新链 + 基础信息 + SHA256 表 + `Full Changelog: compare/v0.1.0-rc.1...v0.1.0`，没有任何变更列表——`generate_release_notes`（`release.yml:333`）只列 PR，
而本仓库直推 main，没有 PR。正文自己写「变更记录：见下方 Automatic release notes」，那里是空的。内容全在 `CHANGELOG.md`。format-v0.1.1 同。

**建议**：release job 从 CHANGELOG.md 抽对应版本节（`scripts/ci/changelog-section.py <tag>`），拼进正文；找不到该节则 job 失败，兼作「打 tag 前必须更新 CHANGELOG」的门禁。0.2.0 的 Release 页还要有迁移说明（RT-01）和平台/flavor 矩阵。
GitHub 的自动笔记对多发版线分组正确（format-v0.1.1 的比较基线就是 format-v0.1.0），不需要为 UI 另开 tag 线。

**成本** S

### RT-11 ［P1｜安装入口］引导脚本与 agent 指南取自 main，且零 CI 覆盖

**事实**
- README 的全部入口（`README.md:23-39`：agent 一句话、`install.ps1`、`install.sh`、`install.cmd`）都是 `raw.githubusercontent.com/.../main/...`；`INSTALL_AGENT.md` 同样取自 main。
- CI 没有 shellcheck、PSScriptAnalyzer 或任何脚本冒烟（`ci.yml` 与 `review.yml` 里都没有；上一轮第 7 项里的静态检查建议未做）。
- `install.sh` 的 Chinese 文案 111 行、`install.ps1` 74 行；`install.cmd` 已是纯英文，三者不一致。`per_page=100` 单页列表（`install.sh:139`、`install.ps1:70`、`github.rs:204`）：目前仓库共 17 个 release，余量大，三条发版线并行后会收窄。

**后果**：0.2.0 开发期间，main 上任何一次对脚本或 `INSTALL_AGENT.md` 的改动（例如资产命名、`aproxy skill`）都在 0.2.0 发布**之前**就影响新用户：脚本期望的资产不存在于仍是最新稳定版的 0.1.0。

**建议**
- 引导 URL 改为随 Release 发布的资产（`releases/latest/download/install.sh`；Latest 因 `make_latest: false` 始终是 v* 稳定版），或至少 tag 固定。`INSTALL_AGENT.md` 同理，或在每条指令后标「自 X 版起」。
- CI：shellcheck + PSScriptAnalyzer + 三平台冒烟（对「最新已发布 release」与「本地 file:// 镜像的待发 release 目录」各跑一遍，`APROXY_HOME` 隔离）。
- 脚本文案纳入 i18n 方案（单文件 `curl | sh` 不能依赖外部目录：内联两套文案 + 与 `APROXY_LANG` 同一条判定规则）。
- 列表分页（Link 头）或在单页取不到稳定版时回退 `/releases/latest`（Latest 由 make_latest 保护）。

**成本** M

### RT-12 ［P1｜依赖］无用依赖、悬着的升级 PR、大版本窗口

**事实**
- `tower`、`tower-http`（`Cargo.toml:46-47`）在 `src/`、`tests/`、`examples/`、两个子 crate 里**零引用**（逐个 grep 核实）。`Cargo.lock` 里它们仍由 axum/reqwest 传递引入，所以删除不改依赖图，
  只去掉 `trace`/`cors` 特性（数据面审查 DP-28 同样指出）。`[dev-dependencies]` 里 `tracing-subscriber`、`tokio` 与正式依赖完全相同（`Cargo.toml:104-105`），冗余。其余直接依赖都有引用。
- `gh pr list`：#6 toml→1.1.6、#7 sha2→0.11.0、#8 zip→8.6.0、#9 clap→4.6.7、#10 base64→0.23.1 **全部仍开着**，最老的 #6-#8 自 09-17；它们的检查结果来自旧 CI（作业名 `unix (ubuntu-latest)` 已不存在），
  #7 的 windows 与 cargo-deny 红。`dependabot.yml` 无 groups、无 base64 ignore（#10 会破坏已发布 envelope 的公共 API）。已关闭的 axum 0.8（#1）、dirs 7（#2）、tower-http 0.7（#4）、windows-sys 0.61（#5）是 09-10 关的。
- crates.io 最新版（2026-10-11 查询）对 `Cargo.lock`：axum 0.7.9 → 0.8.9；reqwest 0.12.28 → 0.13.5；toml 0.8.23 → 1.1.8；zip 2.4.2 → 9.0.1（PR 的 8.6.0 已落后）；sha2 0.10.9 → 0.11.0；
  dirs 6.0.0 → 7.0.0；base64 0.22.1 → 0.23.1；clap 4.6.6 → 4.6.7；tokio 1.53.1 → 1.53.2。Tauri 2.12.3 / wry 0.57.0 / tao 0.37.1 / window-vibrancy 0.8.1。

**建议**：0.2.0 是「配置往返、解压路径防护、HTTP 栈」可以放心动的最后窗口——这几项正是 axum 0.8（路由语法、Host 校验中间件）、reqwest 0.13、toml 1.x、zip 9、sha2 0.11 会触碰的地方，放在 IPC/契约重构之前或同期做，
别等 UI 落地后再动。步骤：删 tower/tower-http 与重复 dev-deps → 逐个评估大版本升级（每个单独提交、`--workspace` 全量 + headless 守卫）→ 关闭 #10 并加 ignore → dependabot 加 groups。

**成本** M-L（axum 0.8 需要实测中间件与路由）

### RT-13 ［P1｜CI/决策］CI 整体恒红

**事实**（`gh run list/view`，最近 30 次 `ci.yml` 运行，2026-10-04 至 10-06）：**无一整体成功**。macOS job：28 次完成的全失败，2 次整体取消；windows 仅 1 次失败（`d993d1f`，`cargo fmt` 步骤）；
ubuntu 5 次失败，全集中在 10-05/06 的 IPC 重构提交上，我抽查了其中 4 次：unix 测试的编译错误（`InstanceStatus` 字段）、clippy `zombie_processes`、
`daemon::tests::binding_over_a_stale_socket_file_succeeds` 与三项 `install_flow` 用例的真实失败——都是重构引入的真回归，不是偶发（也说明 ubuntu 门禁有效）；`Review`（cargo-deny）全绿。CI 日志里 macOS 现为 **6 个** lib 测试失败
（`daemon::tests::survey_keeps_records_of_live_but_unresponsive_instances`、`install::announce::tests` 三项、`watchdog::tests` 两项），而 `ci.yml:99-103` 的注释仍写「3 个」。
`TODO.md:466` 的「CI 现状处理（待用户定）」自 09-12 悬而未决。

**后果**：红灯是常态，新增的 macOS 失败（包括 headless 构建被 UI 依赖弄坏）没有人看得见。

**建议（决策）**：0.2.0 的 UI 不含 macOS，所以二选一——(a) macOS 拆两步：`cargo build/clippy --no-default-features`（必须绿，守编译面）+ `cargo test` 标 `continue-on-error: true` 并把失败清单（固定 6 项）写进 job summary，数量变化即告警；
(b) 暂时移出 CI，README 平台矩阵已如实标注。无论哪种都更新 ci.yml 注释与记忆里的数字。

**成本** S

### RT-14 ［P1｜流程］发版清单缺失，0.2.0 应走 rc 演练

**事实**：发版知识散在 5 条记忆（`release-engineering`、`release-version-alignment`、`github-release-settings`、`trusted-publishing-p0`、`ci-unix-blindspot`）和已删除的 `plan/release-0.1.0.md`（`git show 58e62cd`）里，
仓库没有 RELEASING 文档。0.1.0 靠 `rc.1` 演练，0.2.0 改动面更大（新产物矩阵、前端构建、新系统依赖、可能的新 tag 前置）。npm `next` 现指向 `0.1.0-rc.1`，落后于 `latest=0.1.0`（`npm view` 实测）。
`release` 环境只放行 `v*`/`format-v*`，没有 required reviewers（记忆 `github-release-settings`）。

**建议**：新增 `docs/RELEASING.md`（公开，开源团队的 agent 与贡献者都读）：版本位置清单（根 Cargo.toml + Cargo.lock、envelope、format）、CHANGELOG 节、skill 留存决策、前置（新 crate 首发、环境 tag policy、secrets）、
tag 规则（预发布用 `rc.N`，禁止 `tN` 这类会污染 npm `next` 与 crates.io 的测试 tag）、发布后核验清单（GitHub 资产数 = 清单、npm dist-tags 有传播延迟、crates.io、三平台 `install` 冒烟）、失败 job 的重跑与补发流程。
0.2.0 走 `rc.1 → rc.N → 0.2.0`，rc 期间同时完成 RT-01 的实测与 RT-04 的 spike。

**成本** M

### RT-15 ［P1｜测试基建］`tests/common` 仍未建

**事实**（脚本统计 + 阅读）
- 同名 helper 在多个测试文件重复定义：`bin_works`×3（install_flow、install_flow_lib、install_swap）、`free_port`×3、`ipc_ping_in_dir`×2、`format_echo_path`×2、`isolate_env_proxy`×2、
  `local_client`×2、`make_executable`×2、`proxy_config_for`×2、`text`×2。
- 三套守卫：`DaemonGuard`（proxy_integration）、`RestartGuard` 与 `ChildGuard`（restart_integration）；两套端口派生：`daemon_test_port`（步长 16 × 96）与 `restart_test_port`（步长 32 × 64），
  都以 pid 取模为基址、bind 试探后释放，存在探测与 spawn 之间的竞争窗口。
- `transform_integration.rs:27` 自注「测试 crate 之间不共享私有项」；`format-echo` 靠 `current_exe` 的祖先目录找 `examples/`（`transform_integration.rs:77-92`），`cargo test --test transform_integration` 单跑会找不到；
  `install_flow_lib.rs:61,225,581` 用进程级 `set_var("APROXY_HOME")` + 静态锁串行 14 项。
- `tests/proxy_integration.rs` 7091 行 / 103 项。全仓测试约 500 项（src 单测 271、tests/ 186、envelope 8、format 37；按 `#[test]`/`#[tokio::test]` 计）。
- **已知不稳定用例**：(1) `alias_start_and_stop_roundtrip`（`TODO.md:408`，近 30 次 CI 未复现，本机无新证据）；(2) `install_flow_lib` 组内并行偶发（`TODO.md` 外部转换器节的「残余观察」，60 秒卡死监视器窗口，单跑必过）；
  (3) `forward_only_enforces_max_body_mb` 偶发失败已于 `5148d80` 查明为真缺陷并修复；(4) `continue_from_swapping…` 的 HandedOver 竞态已于 alpha.15 前修复；(5) macOS 6 个 lib 测试恒红（RT-13）。
  近 30 次 CI 里 windows 与 ubuntu 的失败没有一次是偶发测试（见 RT-13）。
- 隔离债务本身**已还清**：`a8ac56a`（2026-10-06）起，我逐个核对了 `tests/` 里全部 `Command::new(...)`，需要 home 的都带 `APROXY_HOME` 或 envs 循环注入；src 单测只有 3 处只读 `dirs::home_dir()`。

**为什么现在做**：i18n 要给每个 CLI 子进程统一加 `APROXY_LANG=en`。没有公共入口，就是改 `proxy_integration.rs` 里 65 处 `Command::new`，漏一处就是 CI（英文环境）与开发机（中文环境）结果不一致。

**建议**：`tests/common/mod.rs`（若要给 aproxy-format 共用则做 dev-only 的 `aproxy-testkit` 成员，注意 `publish = false`）：
- `TestHome`：tempdir + `{"watchdog": false}`；`cmd()` 返回已注入 `APROXY_HOME`、`APROXY_LANG=en`、清掉代理环境变量的 `Command`；Drop 时逐端口 `stop <端口>`（只允许这一种，禁止 `stop all`——记忆 `never-kill-aproxy-by-name`）。
- 端口：产品已支持端口 0 并经 IPC 读回实际地址，测试优先用 `:0`；必须固定时 bind 后持有句柄到 spawn 前。
- 守卫合一；`format-echo` 改成稳定的定位方式（例如 dev 专用 workspace 成员，由 `CARGO_BIN_EXE_*` 给路径）。
- 在这一步之后再把 `proxy_integration.rs` 按域拆成 5-6 个文件（IPC/daemon、retry/spool、keepalive、transform、config/CLI），i18n 的断言迁移 diff 才能分批。

**成本** M

### RT-16 ［P1｜i18n/测试］断言面与语言选择机制

数字见附表 C。**直接匹配中文输出**（`contains/starts_with/ends_with/find` 的参数含中文）：tests 79 处、src 单测 37 处，合计约 116 处，必改。含中文字面量的断言行（含失败说明文字，上界）：tests 287、src 159。
`assert_eq!`/`assert_ne!` 的期望值几乎不含中文（tests 0、src 3），中文基本只出现在失败说明里，不受影响。非测试 src 里含中文字符串字面量的行有 686（34 个文件，proxy.rs 105、commands/config.rs 57、watchdog.rs 39、commands/install.rs 39…），是 i18n 迁移的上界。

**建议**
1. 能用稳定标识断言的一律不断言散文：错误码、`--json` 字段、退出码（控制面 CP-04、数据面 DP-01 的机读码就是为此）。
2. 必须断言文案的少数用例：子进程用 `APROXY_LANG=en`（由 RT-15 的 `TestHome::cmd()` 注入）；库内测试用**显式的 `Lang` 参数或线程局部覆盖**。**不要用进程级 `OnceLock` 缓存语言**：集成测试同进程并发，一经初始化无法更改，
   测试之间互相污染——`install_flow_lib` 的 `set_var` + 静态锁已经是同类债务。
3. 目录完整性测试：每个 key 两种语言都有、占位符集合一致。
4. 中文目录要有自己的覆盖：至少一组集成测试以 `APROXY_LANG=zh-CN` 跑 `status`/`stop` 等关键命令，断言非空且含 CJK，否则中文输出零覆盖。
5. 测试里给 HTTP 客户端的错误体/SSE 终态事件改断言英文（用户定调这两类固定英文）。

**成本** M-L（分批，与各模块重构同步迁移，避免翻译要删的代码）

### RT-17 ［P1｜skill］中文输出原文、校验脚本、references 留存

**事实**
- 带中文的 skill 行共 250：aproxy-cli references 184（commands.md 57、troubleshooting 43、config-toml 34、behaviors 31、clients 8、settings-json 8、compatibility 3）、aproxy-format 66（troubleshooting 39、protocol 11、
  guide 3、`scripts/test_format.py` 13）。其中 10 个 md 文件的 237 行，每一行都落在反引号之间（Grep 模式 `` `[^`\n]*\p{Han}[^`\n]*` `` 的命中数恰等于带汉字的行数；skill 正文是英文，中文只出现在引用的程序输出里）。两个 SKILL.md 无中文。
- 校验「skill 里引用的中文都能在 src/ 逐字找到」的脚本 `check_cjk_quotes.py`、`check_skill_docs.py` 在 `.agents/skill-eval.local/`（gitignored，不进仓库也不进 CI）。
- v0.1.0 tag 里的 skill 是**中文旧版**（`git show v0.1.0:.claude/skills/aproxy-cli/SKILL.md` 开头即中文）；现在的 `references/latest` 已改成英文并描述开发线（`compatibility.md` 自述「描述 0.1.0 以及其后的开发线」）。
  所以记忆 `aproxy-cli-skill-versioning` 的留存动作「复制 latest → references/0.1.0/」若字面执行，归档下来的是一份**既不是 0.1.0 发布内容、也不是 0.1.0 行为**的东西。

**后果（推断）**：i18n 之后程序输出随语言环境，agent 看到的可能是中文也可能是英文；skill 现在「中文原文 + 英文释义」的写法对英文环境失配。

**建议**
1. 0.2.0 的 skill 改引**英文目录的文本**，去掉中文原文与释义（约 237 行重写，不是翻译）；skill 明确教 agent「运行 aproxy 命令前设 `APROXY_LANG=en`」得到可预期输出（这也是 `--help` 末尾提示行的天然搭档）。
2. 校验脚本转正：`scripts/check-skill-quotes.py` 进 CI——skill 里反引号包裹的消息必须在英文目录中逐字存在；同时检查 `compatibility.md` 的版本行与 `Cargo.toml` 一致（上一轮 docs-08/S02 的「文档版本 CI 守卫」）。
3. references 留存：若要留，必须 `git show v0.1.0:` 取原始内容；或按 SECURITY.md「只有最新版收安全修复」主张不留存，`compatibility.md` 只保留 0.1.0 → 0.2.0 差异表。**这是对记忆规则的例外，需用户确认**（规则要求 0.1 → 0.2 重新判断是否有行为断层——有）。
4. `skill-evals/` 评测用例（`.agents/skill-evals/*/evals.json`）里若有对中文输出的断言，随之迁移。

**成本** M

### RT-18 ［P1｜CHANGELOG］`[Unreleased]` 在 0.2.0 发布时会自相矛盾

**事实**：`CHANGELOG.md:6-51` 的 `[Unreleased]` 含「0.1.0 的 CLI、安装器与守护仍能与新版本互通（…兼容层处理，0.2.0 删除）」（约 `:24`）、「这两条兼容在 0.2.0 删除」（约 `:40`）、
「0.1.x 期间安装器同时发布 0.1.0 的全局宣告名」（约 `:43`）等。这些是写给 0.1.x 补丁线读者的话；0.2.0 直接删除兼容层，发布时读者看到的是自相矛盾的叙述。
另有若干「修复」条目修的是 0.1.0 里不存在的、仅存在于未发布代码里的东西，需要逐条判断是否还属于「对 0.1.0 用户可见的缺陷」。

**建议**：0.2.0 节整体重写为「破坏性变更 / 迁移 / 新增 / 变更 / 修复」五段；「修复」只保留 0.1.0 用户可见的缺陷；兼容层相关叙述全部删去。

**成本** S

### RT-19 ［P1｜项目记录］TODO.md 清理

**事实**：`.agents/TODO.md` 477 行，开头自称「完成项删除并留 git 历史」，但 57 个 `[x]` 项全部还在；22 个 `[ ]` 项里 8 个已做完或可直接删、7 个应并入 0.2.0 计划、2 个转数据面审查、3 个并入测试基建、2 个保留（附表 B）。
`TODO.md:21` 与用户 10-11 定调矛盾（RT-01）。

**建议**：照附表 B 处置，TODO.md 压到 ~40 行：0.2.0 指针（指向 `plan/release-0.2.0.md`）、macOS、看门狗二期、少数观察项。durable 内容大多已有记忆（random-log-names、bounded-retry-paths、forward-only、compressed-body-inspection、
install-*、unix-*、trusted-publishing-p0…），删之前逐条核对即可。

**成本** S

### RT-20 ［P1｜渠道］`cargo install` / crates.io / npm / binstall 对 `ui` feature 的语义

**事实**
- `Cargo.toml:37` `include` 是 crates.io 白名单（`/src/**`、Cargo.toml、Cargo.lock、README、LICENSE、两个 skill）；没有 `build.rs`、`tauri.conf.json`、`capabilities/`、`icons/`、前端产物的条目。`ui/dist` 是构建产物，不入库。
- `README.md:57`「`cargo install aproxy` 源码编译，需 rustc 1.88+」；`aproxy install` 的 cargo 渠道会 `cargo install aproxy --root ~/.aproxy`（`src/install/download/crates.rs`）。
- npm 平台包靠 `os/cpu/libc` 由 npm 自动装配（`build-and-publish.sh:127-137`），无法表达「机器上有 GTK」。

**推断**：`default = ["ui"]` 的 crate 被 `cargo install` 时，Linux 缺 GTK 开发库直接编译失败，且前端产物不在包里；MSRV 与编译时间也都变了。

**建议**（需要用户拍板）
- 保持用户「UI 默认启用」的产品意图，但把它落在**发布产物**层：二进制资产的默认选择（RT-03）。Cargo 层的 `default` 对 crates.io 渠道会制造无法安装的默认；若坚持 `default = ["ui"]`，则 crates.io 渠道文档与 `aproxy install` 的 cargo 渠道必须显式传 `--no-default-features`，并在 README 写清源码构建前置（Node、GTK）。
- npm 与 binstall 一律 headless；README 渠道表里加一列「flavor」。
- `include` 同步更新，并在发布 job 里 `cargo package --list` 对照期望清单；若发布 UI 版 crate，前端构建要放进 `publish-crates`（Node + `--allow-dirty`）。

**成本** M

### RT-21 ［P1｜skill 发布］skills 包生成路径与清单

**事实**：skills 包有两条独立的生成路径——GitHub 资产在 `release.yml` 的 `skills` job（`cd stage && zip -qr ../aproxy-skills.zip */`，约 `:257-282`），npm 主包里的 `aproxy-skills.zip` 在 `build-and-publish.sh` 里另行 zip
（`zip -qr … $names -x '*.zip'`）；两者没有版本文件、没有清单，时间戳不同所以哈希不同。W4 计划要求 `aproxy skill show` 用「随包清单（版本、文件列表、哈希）」做白名单。

**建议**：一个脚本 `scripts/ci/package-skills.sh` 同时被两处调用，生成同一份 `manifest.json`（版本 = tag、每个文件的 sha256、总文件数上限）放进 zip 根；`skill install` 下载时同时校验 zip 的 `.sha256` 与清单自洽；
`aproxy skill show` 只认清单内路径。清单里的版本用于 `skill list` 判断与二进制是否同版本。

**成本** S-M

### RT-22 ［P2｜测试覆盖］仍无行为测试的使命路径

**事实**（上一轮 tests-06/07/08）：`connect_timeout_secs` / `read_timeout_secs`（`config.rs:260-266`，`proxy.rs:338-342`）在 `tests/` 里零引用；上游 HTTPS、HTTP/SOCKS 代理无集成测试（`config.rs:1724` 只有校验单测）；
`retry_on_binary_error_body_and_clean_log_preview`（`proxy_integration.rs:574`）仍不断言日志预览，只断言重试了 2 次——压缩体覆盖由 `src/proxy.rs:4333,4355` 的单测与 `proxy_integration.rs:3130` 补上；
下载渠道：`pick_latest` 有 11 项纯函数单测（`github.rs:225-378`），但 `fetch` / npm / crates 仍硬编码主机（`github.rs:48,204`、`crates.rs:17`），没有离线 mock。W4 的 `aproxy skill install` 会再加一条下载路径。

**建议**：给下载上下文一个可注入的基址（用现有的「url 模板通道」做测试入口，不加生产环境变量，避免多出一个可被劫持的开关），补 axum mock 覆盖 github（含 format-v* 干扰、缺资产、sha 不匹配）、npm integrity、crates；
read/connect 超时与 SOCKS 上游各补一个集成用例。

**成本** M-L

### RT-23 ［P2｜供应链/签名］

**事实**：所有 action 仍按版本 tag 引用（`actions/checkout@v7.0.1`、`softprops/action-gh-release@v3.0.3`、`rust-lang/crates-io-auth-action@v1.0.5`…），`dtolnay/rust-toolchain@stable` 是分支引用；
没有 `actions/attest-build-provenance`（GitHub 渠道的二进制只有同 Release 里的 `.sha256`，防不了 Release 本身被篡改；npm 已有 provenance）；`release` 环境无 required reviewers，仓库没有限制 `v*` tag 创建的 ruleset
（误推一个 `v0.2.0` 到错误提交会不可撤销地发布到 npm/crates.io）；CARGO_REGISTRY_TOKEN 仍在仓库级（记忆 `github-release-settings`）。

**UI 带来的新点**：Windows 上未签名 GUI exe 经浏览器下载会触发 SmartScreen；经 `aproxy install`/脚本/npm 下载不带 Mark-of-the-Web（推断）。Tauri 自带的 updater 签名不需要——更新走 `aproxy install`。
前端 npm 依赖会编进用户机器上运行的二进制：`package-lock.json` 入库、CI 用 `npm ci --ignore-scripts`，并给 `SECURITY.md` 增加 UI/CSP/IPC 访问控制一节。Rust 与前端依赖都应生成第三方许可证清单（目前二进制没有）。

**建议**：action 钉 SHA（至少 release/publish 两个持写权限或 OIDC 的 job）、加 attestation、给 `v*`/`format-v*` 加 tag ruleset；Windows 签名（OSS 免费方案的可用性我未核实）作为决策项。

**成本** M

### RT-24 ［P2｜文档］

CONTRIBUTING.md 缺失（仓库只有 README「开发」三行命令）；人类版排障页缺失（skill 有 `troubleshooting.md`，面向 agent）；开机自启只有 Windows 任务计划程序（`README.md:198-200`），
无 systemd `--user`（需写明 enable-linger）/launchd；`aproxy-envelope`、`aproxy-format` 没有 README，crates.io 页是空的（`readme` 字段也未设）；`aproxy-format/Cargo.toml` 没有 `[package.metadata.binstall]`（tag 是 `format-v{version}`，默认模板取 `v{version}` 会 404）；
`aproxy-format` 的 `rust-version = 1.96.1` 意味着装它需要近 3 个月内的工具链（README 应写明）；`aproxy.js` 的 30 行中文错误消息属于「给用户看的」，该走 i18n。

**成本** S-M

### RT-25 ［P2｜记录卫生］plan/、DOCS.md、memory

见附表 F。

## 3 附表 A：上一轮相关条目现状

状态：**已修** / **部分** / **未修** / **失效（前提已变）**。证据列给出可复核位置。

### A.1 发布阻断与应修大项（只列本报告范围内的）

| 上一轮条目 | 现状 | 证据 |
|---|---|---|
| 阻断 1 发版线隔离、format-v* 劫持 latest、稳定通道 | **已修** | `release-format.yml:283` `make_latest: false`；`gh release list`：Latest = v0.1.0，format-v0.1.1 无 Latest；`github.rs` `pick_latest`（`:111`）+ 11 项单测（`:225-378`）；三脚本同文法（`install.sh:96-158`、`install.ps1:65-100`、`install.cmd:82`）；`npm_dist_tag`（`build-and-publish.sh:37`）。遗留：单页 `per_page=100`（RT-11） |
| 阻断 2 质量门禁（F1、--workspace、ubuntu、no-fail-fast、拆分 macOS） | **已修**（主体） | `tests/proxy_integration.rs:5141` `wait_daemon_ready_in`；`ci.yml` windows/ubuntu 两个 job 都带 `--workspace`、`--no-fail-fast`（`:47-50,:67-68`），ubuntu 带 clippy `-D warnings`；`release.yml` test 矩阵 windows+ubuntu；macOS 独立 job（`ci.yml:98`，仍是不带这两个参数的 `cargo test --locked`）。遗留：整体恒红（RT-13） |
| 阻断 3 发布链可重跑 | **已修** | `release.yml` 拆 `publish-npm`（`:350`）/`publish-crates`（`:381`）；plan 步骤只在 200 跳过、404 发布、其余硬失败（`:418`）；`npm_version_exists`（`build-and-publish.sh:50`）；format 线补发实测成功（记忆 `trusted-publishing-p0`，crates 10-04、npm 10-06） |
| 阻断 4 gnu 产物依赖 GLIBC_2.39 | **已修** | `release.yml:38` floor 2.28 + zigbuild + objdump 断言（`:213-250`）；`install.sh` MIN_GLIBC 与 libc 探测（`:161-195`）+ 自证；`aproxy.js:24`。遗留：冒烟只覆盖 zigbuild 目标（RT-08） |
| 阻断 7 一键脚本批量修复 | **已修**（主体） | `install.cmd:82` 已用 `-Headers @{'User-Agent'='%UA%'}`；`install.sh` sha256 三选一（`:59-80`）、`main()`（`:212`）；`install.ps1` 函数包裹（`:43`）。遗留：无 shellcheck/PSScriptAnalyzer/冒烟（RT-11） |
| 阻断 10 对外文档与事实对齐 | **部分** | README 平台矩阵（`README.md:73`）、README_EN 对齐（438 行）、转换器限制句（`README.md:15`）、SECURITY.md、CHANGELOG.md 均已有。遗留：Release 正文无变更内容（RT-10）；skill references 留存按 0.1.0 例外未做 |
| 阻断 11 workflow 权限最小化 | **已修**（零风险子集） | `release.yml:22` 顶层 `contents: read`、release job 单独 write（`:288`）；全部 checkout `persist-credentials: false`；`ci.yml`/`review.yml` 显式只读；`gh api` 回读 main 保护：`enforce_admins=true`、禁强推/删除、无必需 PR/检查；环境 tag 策略见记忆。遗留：action 未钉 SHA、无 attestation（RT-23） |

### A.2 release-*

| 条目 | 现状 | 证据 |
|---|---|---|
| release-01 Release 门禁只跑 windows | **已修** | `release.yml` test 矩阵 `[windows-latest, ubuntu-latest]` |
| release-02 缺 `--workspace` | **已修** | `ci.yml`、`release.yml`、`release-format.yml` 均带；`Cargo.toml` 顶部注释写明不设 default-members 的理由 |
| release-03 unix 遇首个失败即停 | **已修** | ubuntu job 带 `--no-fail-fast`（`ci.yml:68`），`release.yml`/`release-format.yml` 同 |
| release-04 format-v* 共用 Release 列表 | **已修** | 见阻断 1 |
| release-05 publish 非幂等 | **已修** | 见阻断 3 |
| release-06 `\|\| echo` 吞错 | **已修** | plan 步骤替代（`release.yml:418`） |
| release-07 npm 恒 latest | **已修** | `npm_dist_tag`；`npm view`：latest=0.1.0、next=0.1.0-rc.1 |
| release-08 -v3 选择只看 AVX2 | **未修** | `src/install/download/mod.rs:258` |
| release-09 依赖升级 PR | **未修** | #6-#10 全开（RT-12） |
| release-10 供应链姿态 | **部分** | 权限已收紧；SHA 钉、attestation 未做（RT-23） |
| release-11 format 线发布链未跑通 | **已修** | format-v0.1.1 经 OIDC 发布成功（记忆 `trusted-publishing-p0`） |
| release-12 install.sh 恒取 gnu | **已修** | `install.sh:161-195` libc 探测 + musl 回退 + `--version` 自证 |
| release-13 build 矩阵无冒烟 | **部分** | 仅 zigbuild 目标跑 `--version`（RT-08） |
| release-14 aproxy-format 缺 binstall 元数据 | **未修** | `aproxy-format/Cargo.toml` 无 `[package.metadata.binstall]`（RT-24） |
| release-15 文档/发布说明漂移 | **部分** | CHANGELOG/平台矩阵已有；自动 notes 仍空（RT-10）；`architecture.md` 版本路线仍旧（附表 E） |
| release-16 MSRV 无 CI | **未修** | 无 1.88 job（RT-07） |

### A.3 tests-*

| 条目 | 现状 | 证据 |
|---|---|---|
| tests-01 F1 | **已修** | `proxy_integration.rs:5141` |
| tests-02 子 crate 测试未进门禁 | **已修** | `--workspace` |
| tests-03 CI 信号失真 | **部分** | 门禁已对；整体恒红（RT-13） |
| tests-04 Release 门禁只跑 windows | **已修** | 同 release-01 |
| tests-05 全局 run 目录守护测试 | **已修** | `a8ac56a`（2026-10-06）；本次核对 `tests/` 全部 `Command::new` 带 home 注入（RT-15） |
| tests-06 `retry_on_binary_error_body…` 名不副实 | **部分** | 该测试仍不断言预览；压缩体覆盖由 `src/proxy.rs:4333,4355` 与 `proxy_integration.rs:3130` 补上 |
| tests-07 核心路径行为测试缺口 | **未修** | `tests/` 无 `read_timeout_secs`/`connect_timeout_secs`/SOCKS（RT-22） |
| tests-08 下载渠道零测试 | **部分** | `pick_latest` 纯函数单测；fetch/npm/crates 无离线 mock |
| tests-09 restart 端口由 pid 算术派生 | **已修** | `restart_integration.rs:28-40` 已 bind 试探（但与 proxy_integration 的版本仍是两份，RT-15） |
| tests-10 | 有意设计（上一轮结论） | — |
| tests-11 子进程无 RAII | **部分** | 有三套守卫，未统一；看门狗测试仍有固定 sleep（`proxy_integration.rs:6285,6315,6384,6405,6532,6541` 的 `thread::sleep` 2-5 秒） |
| tests-12 unix 上 `APROXY_RUN_DIR` 单独重定向时 logs_dir 不同源 | **未修** | `daemon.rs:27-37` 两个函数仍各自派生（控制面审查 CP-05 接手） |
| tests-13 .gitignore 不覆盖 target-* | **已修** | `.gitignore` `/target*/` |

### A.4 docs-*

| 条目 | 现状 | 证据 |
|---|---|---|
| docs-01 install.sh 只用 sha256sum | **已修** | `install.sh:59-80` |
| docs-02 缺接入指南 | **已修** | `README.md:96-158`；skill `clients.md` |
| docs-03 平台声明与事实不符 | **已修** | `README.md:73`、`architecture.md` 平台节；但 `ci.yml:99-103` 注释仍写「3 个」测试失败（实为 6，RT-13） |
| docs-04 README_EN 过时 | **已修** | README_EN 438 行、章节对齐 |
| docs-05 跨协议 SSE 限制未披露 | **已修** | `README.md:15` |
| docs-06 排障表称 status 显示看护者缺席 | **部分** | 文档已改为如实（skill `behaviors.md:415`）；功能（status 显示看护者）未做，控制面 CP-10 接手 |
| docs-07 CHANGELOG/CONTRIBUTING/SECURITY/TROUBLESHOOTING | **部分** | CHANGELOG、SECURITY 已有；CONTRIBUTING、人类版排障页没有（RT-24） |
| docs-08 skill 版本留存 / 文档版本 CI 守卫 | **未修** | 无守卫；留存规则字面执行会归档错内容（RT-17） |
| docs-09 约 200 处仅中文输出 | **失效→W3** | i18n 计划接手；规模见附表 C |
| docs-10 幽灵命令 `aproxy watchdog` | **已修** | 全仓只剩 `src/watchdog.rs:15` 的「没有公开的 `aproxy watchdog` 子命令」 |
| docs-11 零散漂移与失效指针 | **部分** | `.agents/DOCS.md` 仍过时（附表 F） |
| docs-12 settings 读写命令 | **未修** | `cli.rs` 无 settings 子命令 |
| docs-13 systemd/launchd | **未修** | `README.md:198-200` 只有 Windows |
| docs-14 clap 帮助可读性、子 crate README | **未核实** / 子 crate README **未修** | 两个子 crate 无 README |

### A.5 路线图 0.1.x 第 12、14、15、17、18 项

| 项 | 现状 |
|---|---|
| 12 测试补强与基建统一 | **部分**：隔离已清；`tests/common`、TestHome/ChildGuard 统一、read_timeout 等回归、下载离线 mock 未做；压缩体 e2e 由单测 + 一个集成用例覆盖 |
| 14 依赖升级 PR 处置 | **未修**：#6-#10 全开，无 groups/ignore |
| 15 指令集变体选择与基准 | **未修**：见 release-08/13，无 v3 数据 |
| 17 小清理 | **部分**：tower/tower-http 仍在（`Cargo.toml:46-47`）；tcp_nodelay 仍未开（数据面 DP-28）；其余条目归数据面/控制面审查 |
| 18 文档补全 | **部分**：CHANGELOG、SECURITY、接入指南已有，幽灵命令已清；人类版 TROUBLESHOOTING、CONTRIBUTING、systemd/launchd、docs-lint/版本 CI 守卫未做 |

## 4 附表 B：TODO.md 逐条处置

`.agents/TODO.md` 共 477 行；`[x]` 57 项；`[ ]` 22 项。

### B.1 未完成项（22）

| 行 | 条目 | 处置 | 理由 |
|---|---|---|---|
| 21 | 0.2.0 时删掉 0.1.0 升级兼容 | **并入 0.2.0 W1 并改写** | 「先装某个 0.1.x 再升 0.2.0」已被用户 10-11 推翻（RT-01）；留一行指向计划 |
| 30 | format 扩展的开放问题 | **并入 W2 契约定稿** | `state` 大小上限、心跳信封缺上游失败摘要都是信封协议问题，冻结前必须决定（数据面 DP-13） |
| 68 | 其余全局 run 目录测试的隔离化 | **关闭** | `a8ac56a` 已隔离；本次核对 `tests/` 所有 `Command::new` 均注入 home |
| 131 | restore_recovers 写真实 run 目录 | **关闭** | `TODO.md:76` 的 `[x]` 条已记录隔离化重写完成，此条是遗留的未勾选重复 |
| 139 | resolve_config_target 抽公共函数（观察项） | **删除** | 控制面 CP-08/CP-17 的 ops 层重构会整体重排；不是缺陷 |
| 174 | 随机命名日志 e2e 集成测试 | **并入 W2/W5** | 日志传输方式（IPC 流式）将改；在 IPC 契约测试里覆盖 |
| 233 | forward_only 在 status/find/doctor 无模式标识 | **并入 W2** | 前提「需 bump IPC_PROTO_VERSION」已过时，新增可选字段不需要 bump（控制面 CP-10） |
| 236 | 判定侧与回放侧 is_streaming 可能分叉 | **转数据面审查，TODO 删除** | low，纯数据面 |
| 240 | forward_only 落地确认 | **关闭** | 已落地且独立审查闭环（上方 `[x]`）；内容已在记忆 `forward-only` |
| 261 | mock 上游要覆盖压缩 | **关闭** | `src/proxy.rs:4333,4355` + `proxy_integration.rs:3130` 已覆盖；e2e 层 br/zstd 如要补记在 RT-22 |
| 264 | 磁盘模式压缩体判定（已知边界） | **删除** | 记忆 `compressed-body-inspection` 已记录该边界（该文件约 63 行） |
| 308 | unix 上 APROXY_RUN_DIR 影响 IPC 寻址 | **并入 W2 磁盘布局/`home` 定义** | 前提「Windows 管道全局」已被 `7def3bf`（管道名加 run 目录标识）改变；控制面 CP-05 |
| 313 | upstream client 开启 TCP_NODELAY | **转数据面，TODO 删除** | 数据面 DP-28 已收 |
| 315 | 看门狗测试子进程清理 RAII 化 + UDS roundtrip 单测 | **并入 RT-15（tests/common）** | 守卫统一 |
| 317 | 测试守护泄漏治理 | **并入 RT-15** | `TestHome` 的 Drop 走 `stop <端口>`；清理铁则已在记忆 `never-kill-aproxy-by-name` |
| 360 | F2 is_aproxy_process 二进制名耦合 | **关闭** | CHANGELOG 0.1.0「进程身份判定去名称化」；`is_aproxy_process` 在 src 中已不存在 |
| 369 | 正式发布：GitHub 构建指令集多版本 | **关闭** | v0.1.0 已发 -v3；遗留的选择逻辑与基准在 RT-08 |
| 370 | H. 配置热重载 | **并入 W2 或明确延后** | 与 UI 配置编辑、CP-18「看门狗开关只在启动时读取」同一话题 |
| 408 | alias_start_and_stop_roundtrip 偶发并行失败 | **观察项，转 RT-15 验收** | 近 30 次 CI 该用例从未失败；`tests/common` 落地后本机循环复测，不复现即删 |
| 438 | 看门狗二期（可选） | **保留（远期）** | 与 0.2.0 无关 |
| 450 | macOS 平台支持修复 | **保留，更新描述** | 用户定调「必须完成」但不在 0.2.0 范围；描述里「3 个 lib 测试失败」应为 6 个 |
| 466 | CI 现状处理（macOS 红，待用户定） | **升为 0.2.0 决策（RT-13），决策后删除** | 悬置 1 个月 |

小计（22）：关闭或删除 8（68、131、139、240、261、264、360、369）；并入 0.2.0 计划 7（21、30、174、233、308、370、466）；转数据面审查 2（236、313）；并入测试基建 3（315、317、408）；保留 2（438、450）。

### B.2 已完成项（57）

全部删除（git 历史保留）。各节标题与处置：

| 节（行） | 处置 |
|---|---|
| 0.1.0 已发布（7-12） | 保留 2 行事实指针或并入记忆 |
| 下一版发布前（14-30）里的 `[x]`（skill 重写、去 alpha 兼容、IPC 整理、看门狗即时重拉、install 测试外网依赖、413 修复、自定义心跳） | 删除；各自已有记忆/CHANGELOG |
| 外部转换器 + aproxy-format（32-70） | 删除；记忆 `transform-format` |
| 发版 alpha.15/16（72-86、144-156） | 删除（历史发布流水） |
| 两轮大审查（87-142） | 删除；结论在审查文件与记忆 |
| 日志随机命名、受限重试路径、仅转发模式、压缩响应体检查（158-266） | 删除；各有记忆 |
| 近期收口 / alpha.4 之后 / alpha.6 之后 / unix 实测 / 压力实测 / 修复审查（268-366） | 删除；记忆 `unix-*`、`daemon-model` 等 |
| 中期功能 / install 三平台 / 渠道 P0（367-446） | 删除；记忆 `install-*`、`trusted-publishing-p0` |
| 已结案（有意跳过）（471-477） | 保留一条「已结案」索引或并入记忆，因为它们解释了「为什么不做」 |

## 5 附表 C：测试中文断言统计（i18n 影响面）

方法：脚本按行扫描（近似，量级可信、个位数不保证）。三档含义：
- **A 直接匹配输出**：中文字面量是 `.contains/.starts_with/.ends_with/.find` 的参数——程序输出语言一变就必须改；
- **B 断言行含中文字面量**：`assert*`/`contains` 等所在行出现中文字面量，含失败说明文字（上界）；
- **C 含中文字面量的全部行**（含夹具、注释外的字符串）。

### C.1 tests/

| 文件 | A | B | C |
|---|---:|---:|---:|
| tests/proxy_integration.rs | 46 | 158 | 409 |
| tests/install_flow.rs | 11 | 29 | 62 |
| tests/restart_integration.rs | 11 | 35 | 101 |
| tests/transform_integration.rs | 6 | 28 | 64 |
| tests/install_flow_lib.rs | 4 | 25 | 76 |
| tests/install_swap.rs | 0 | 9 | 11 |
| tests/install_staging.rs | 1 | 2 | 3 |
| tests/upgrade_from_0_1_0.rs（随 W1 删） | 0 | 1 | 8 |
| tests/compat_v0_1_0.rs（随 W1 删） | 0 | 0 | 12 |
| **合计** | **79** | **287** | **746** |

### C.2 src/ 单测（`#[cfg(test)]` 之后）

| 文件 | A | B | C |
|---|---:|---:|---:|
| src/proxy.rs | 9 | 33 | 67 |
| src/config.rs | 4 | 26 | 39 |
| src/watchdog.rs | 0 | 20 | 51 |
| src/daemon.rs | 4 | 18 | 34 |
| src/doctor.rs | 8 | 13 | 19 |
| src/install/state.rs | 1 | 10 | 18 |
| src/transform.rs | 3 | 12 | 29 |
| src/settings.rs | 4 | 9 | 15 |
| src/decode.rs | 0 | 7 | 26 |
| src/install/skills.rs | 2 | 4 | 9 |
| src/install/flow.rs | 0 | 3 | 4 |
| src/install/announce.rs | 0 | 1 | 7 |
| src/install/download/mod.rs | 1 | 1 | 1 |
| src/install/staging.rs | 1 | 1 | 1 |
| src/util.rs | 0 | 1 | 1 |
| 其余（commands/mod.rs、install/broadcast.rs、retry.rs） | 0 | 0 | 3 |
| **合计** | **37** | **159** | **324** |

`assert_eq!/assert_ne!` 的期望值含中文字面量：tests 0 处、src 3 处（`install/state.rs` 2、`util.rs` 1）；其余中文都在失败说明里。

### C.3 其他 i18n 影响面

| 对象 | 规模 |
|---|---|
| 非测试 src 里含中文字符串字面量的行 | 686 行 / 34 个文件（proxy.rs 105、commands/config.rs 57、watchdog.rs 39、commands/install.rs 39、daemon.rs 34、commands/start.rs 33、install/flow.rs 26、transform.rs 25、config.rs 24、server.rs 23、download/crates.rs 23…） |
| skill 引用的中文输出原文 | 250 行：aproxy-cli references 184、aproxy-format 66（含 `test_format.py` 13）；10 个 md 共 237 行，汉字都落在反引号引用里 |
| 引导脚本 | `install.sh` 111 行、`install.ps1` 74 行含中文；`install.cmd` 已是纯英文 |
| npm | `npm/aproxy/bin/aproxy.js` 30 行含中文错误消息；`build-and-publish.sh` 64 行（维护者日志，保持中文） |
| 不受影响（保持中文） | workflow 日志、代码注释、CHANGELOG、docs、`.agents` |

## 6 附表 D：workspace 布局、发版线、资产命名、CI 矩阵

### D.1 发版线

| 线 | tag | 内容 | 现状能否承受 UI |
|---|---|---|---|
| 主线 | `v*` | aproxy（headless + 将来的 ui flavor）、skills 包、npm `@meowo/aproxy`、crates `aproxy` + `aproxy-envelope` | UI 编进本体，不新增发版线；只增加资产变体（RT-03） |
| format 线 | `format-v*` | aproxy-format 二进制、npm `@meowo/aproxy-format`、crates `aproxy-format` | 不变 |
| （假设）独立 UI 线 | `ui-v*` | — | 若用户改回独立二进制：选版文法 `^v\d`（`github.rs:66`、三脚本）本来就排除它；需要 `make_latest: false`、`release-ui.yml`、环境 tag policy 加一条、独立的 trusted publisher 绑定（若发 npm/crates） |

现有选版/latest 过滤的结论：**文法过滤（只认 `vX.Y.Z[-(alpha|beta|rc).N]`）加 `make_latest: false` 足以承受第三条线，不需要改**；需要改的只有单页 `per_page=100`（RT-11）和资产必需判据（RT-08）。

### D.2 Tauri 应用发布：需要什么与现状的差距

| 项 | 需要 | 现状 | 差距 |
|---|---|---|---|
| 前端构建 | Node 版本钉、`package-lock.json`、`npm ci --ignore-scripts`、产物 `ui/dist` 在 cargo 之前就绪 | 仓库无 Node 工具链、无前端目录 | 全新；build 与 publish-crates job 都要加；dependabot 加 npm 生态 |
| 原生构建依赖 | Linux：webkit2gtk-4.1、gtk-3、appindicator、librsvg、xdo（apt）；Windows：MSVC + WebView2；macOS 不在范围 | release 矩阵无 | Linux UI 版需 ubuntu-22.04 基线（RT-04）；CI 每个 job 都要装（RT-07） |
| 构建入口 | 根 `build.rs` 调 `tauri-build`（Windows 资源/清单）、`custom-protocol` feature、`tauri.conf.json`、`capabilities/`、icons | 根包无 build.rs | RT-05 |
| 打包形态 | 用户定调为单文件二进制（`aproxy ui`）；无 MSI/NSIS/AppImage | 现有就是单文件分发 | 无安装器意味着没有开始菜单/桌面条目/卸载项；若要非技术用户双击入口，要由 `install` 代建快捷方式（决策） |
| 代码签名 | Windows Authenticode 否则浏览器下载触发 SmartScreen；Tauri updater 签名不需要 | 无任何签名 | 决策项（RT-23）；主渠道（脚本/npm/install）无 MOTW，推断影响有限 |
| 更新 | 走 `aproxy install` 自更新，不用 Tauri updater | install 已有完整状态机 | GUI 进程在 install 期间的行为（镜像被占用、换版后提示重启）见控制面 CP-19 |
| 产物校验 | `.sha256` 已有；provenance 无 | — | RT-23 |
| 体积/内存 | headless 各平台 4.5-6.5 MiB（x64 的 windows/linux 为 6.1-6.5，v0.1.0 资产实测） | UI 版未测 | 守护/看护者与 GUI 同一镜像（CP-19）：需在 rc 阶段实测 RSS 与冷启动，看门狗「约 2-3 MB」的设计前提可能不再成立 |

### D.3 workspace 布局建议

与用户已定的「UI 编进本体、`ui` feature 默认启用」一致，并保住「三包依赖隔离」「headless 体积不受影响」两条原则：

```text
aProxy/
├─ Cargo.toml          [workspace] members = [".", "aproxy-envelope", "aproxy-format"]   ← 不变；不设 default-members 的既有理由仍成立
│                      [features] default = ["ui"]；ui = ["dep:tauri", "dep:tauri-build", "tauri/custom-protocol", …]
│                      [[bin]] aproxy  test = false                                       ← bin 不含单测（RT-05）
├─ build.rs            CARGO_FEATURE_UI 存在时才调 tauri-build；否则空
├─ tauri.conf.json  capabilities/  icons/         ← 仅 ui 构建使用，放根包
├─ ui/                 前端源码、package.json、package-lock.json；dist/ 为构建产物，.gitignore
├─ src/                lib（无 Tauri、无 UI 依赖）
│   ├─ ipc/            协议/客户端/端点：模块化 + 契约测试冻结，不拆 crate（控制面审查 §6 同结论）
│   ├─ i18n/           消息目录 + 语言判定；构建期可导出 JSON 给前端共用
│   ├─ ui_model/       无 Tauri 的 view-model：IPC → 界面数据，headless 可测
│   └─ …               （控制面审查 §6 的 home/fsx/registry/spawn/platform/watchdog/ops 拆分）
│   └─ main.rs         薄壳；`#[cfg(feature = "ui")] mod ui;` 只在 bin 目标里声明（src/ui/**）
├─ aproxy-envelope/    信封契约（独立版本；冻结时机见 RT-06）
├─ aproxy-format/      官方示例二进制（独立发版线）
├─ tests/              + common/mod.rs（RT-15）
└─ release/assets.toml 资产清单（target × flavor × runner × required × glibc floor；RT-03）
```

不建议新增的东西与理由：
- `aproxy-ui` crate：见 RT-05 第 5 点（发布顺序、首发、清单与资源仍要落在二进制所在包）。
- `aproxy-ipc` crate：UI 已在本体内，同一 crate 内共享即可；真有第三方消费者再按模块边界抽出（依赖只有 serde、serde_json、tokio net、sha2、windows-sys）。代价是先做好契约测试（golden JSON），那才是稳定性的来源。
  参见控制面审查 §6。

### D.4 资产命名建议（待用户确认，RT-03）

```text
aproxy-<target>[-v3][.exe]        headless，指令集变体；每个 target 必有 baseline（完整性判据，不变）
aproxy-<target>-ui[.exe]          UI flavor，仅 baseline 指令集；按 target 在清单里标 required/optional
aproxy-skills.zip / aproxy-skill-<名>.zip    各带 manifest.json（RT-21）
<每个资产>.sha256                  不变
```

### D.5 CI 矩阵建议

| 任务 | 平台 | 命令/内容 | 备注 |
|---|---|---|---|
| headless | windows, ubuntu | clippy + test，`--workspace --locked --no-default-features` | 核心使命主门禁，无 GTK/Node |
| ui | windows, ubuntu | 默认 features；装 apt 依赖 + Node；clippy + test + 前端 lint/单测 | ubuntu 用 22.04 基线 |
| 依赖守卫 | ubuntu | `cargo tree --no-default-features` 正则黑名单 + headless 体积预算 | RT-07 |
| msrv | ubuntu | `cargo +1.88 check --no-default-features -p aproxy -p aproxy-envelope` | |
| deny / review | ubuntu | 现有；UI 依赖入库前先登记 RUSTSEC ignore | |
| scripts | ubuntu, windows, macos | shellcheck、PSScriptAnalyzer、对已发布 release 与本地镜像各冒烟一次 | RT-11 |
| macos | macos | build + clippy `--no-default-features` 必须绿；test 标 `continue-on-error` 并输出失败清单 | RT-13 决策 |
| 0.1.0 升级回归 | windows, ubuntu | 现有 `upgrade-from-0-1-0`，断言改为「升级不留烂摊子」 | RT-01 |
| 发布 | 同 CI + build 矩阵（flavor 维度）+ 资产清单断言 | `build-ui` 独立 job；headless 必需格硬依赖 | RT-08 |

## 7 附表 E：0.2.0 要更新的文档

**文档与代码的一致性（已核对的部分）**：`Config` 的 25 个用户可配字段（`spool_dir_override` 是测试注入、serde skip，不算）全部出现在 README 双语的配置节与 skill 的 `config-toml.md`
（`proxy_username`/`proxy_password` 只在 skill 里，README 以注释带过）；`Settings` 的 20 个字段全部在 skill 的 `settings-json.md`；`cli.rs` 的 12 个子命令全部在 README 命令表。
README 配置示例缺 `every_attempt`（`README.md:226-231`）。其余漂移见下表。

| 文档 | 位置 | 要改什么 |
|---|---|---|
| README.md / README_EN.md | `README.md:29`（「仓库还没有稳定版时（0.1.0 发布前）回退到预发布」，现已不成立）、`:57`（`cargo install` 需 rustc 1.88+，UI 后失真） | 新增「从 0.1.0 迁移」节；渠道表加 flavor 列；平台表加 UI 列；命令表加 `skill`、`ui`、`--lang`/`APROXY_LANG`；配置节补 `language`、`ui` 段，以及 `every_attempt`（`README.md:226-231` 的 request_transform 示例没有它；`architecture.md` 有）；安全须知加 UI/IPC；开发节加 features、Node、GTK 前置；两份保持章节对齐（现 285 vs 438 行） |
| docs/architecture.md | `:516-` 版本路线（写「0.2.0 引入 UI（Web 面板形态待定）…属远期」）；`:467-486` 测试/CI 节；平台节；`:22-23,:35` compat 引用 | 重写版本路线；CI 节改为 headless/ui 双矩阵；删 compat 叙述；新增 i18n、ui 进程模型、flavor 与资产命名、skill 命令、IPC v2 契约 |
| docs/INSTALL_AGENT.md | `:123`「`install` 命令从 0.1.0-alpha.10 起存在」（alpha 已不支持）；`:35-42,:100-111` 通道说明 | 加迁移步骤与 `skill` 命令；入口 URL 策略（RT-11） |
| SECURITY.md | `:9,:47`「只有最新发布的 0.1.x 稳定版会收到安全修复」 | 改为 0.2.x；加 UI 威胁面（WebView、CSP、capabilities 白名单、无远程内容） |
| CHANGELOG.md | RT-18 | 0.2.0 节重写 |
| .claude/skills/aproxy-cli | 7 个 reference 文件、`SKILL.md` | 英文目录引用、新命令（`skill`、`ui`、`--lang`）、`aproxy skill show` 提示、`compatibility.md` 重写（RT-17） |
| .claude/skills/aproxy-format | 4 个 reference + `test_format.py` | 同上；信封字段按定稿更新 |
| .agents/DOCS.md | 附表 F | |
| docs/benchmark-*.md | — | 历史报告，无需改 |

## 8 附表 F：.agents 记录

### F.1 plan/

| 文件 | 状态 | 处置 |
|---|---|---|
| `release-0.2.0.md` | 活跃 | 保留；审查收齐后更新工作流与顺序 |
| `install-v1.md`（620 行） | **工作已完成**：TODO 约 378 行记「十步全部落地」，早交接 `9e91a95` 之后流程又变；但源码 5 处注释引用其路径（`src/watchdog.rs:3`、`install/swap.rs:1`、`install/restart.rs:28`、`install/mod.rs:3`、`commands/install.rs:4`） | 按 AGENTS.md「工作结束，计划删除、持久内容入记忆」：把仍有效的取舍（四条铁律、forward-fix、ACK 协议）补进 `architecture.md` 的 install 节或一条记忆，源码注释改指记忆名，再删。W1/W2 会再改 install 流程，建议在 W1 之后做 |
| `watchdog-v1.md`（137 行） | **工作已完成**（G2 W1-W7）；与现状漂移：写「`aproxy watchdog` 隐藏子命令」「v1 不含 unix 适配」，现为 `--daemon-watchdog` 且已有 unix；一处源码引用（`watchdog.rs:3`） | 同上；控制面 CP-11/CP-14 会再改看护者，改完后一并处理 |
| `skill-standardization.md`（42 行） | **半活跃**：评测已出（新 100% / 旧 87.5%）；「接下来」三条里只剩「按用户反馈改」「每次行为变化同步 skill」，后者被 W3/W4 吸收 | 并入 `release-0.2.0.md` 的 W3/W4 后删除；`.agents/skill-evals/` 用例集保留（回归资产，i18n 后要同步断言） |
| `skill-rewrite-brief.local.md` | 本地（gitignored） | 不处理 |

### F.2 DOCS.md（`.agents/DOCS.md`，44 行）过时点

1. `:6`「**待同步**：main.rs 拆分后的模块分工、磁盘缓存双模缓冲」——`architecture.md` 已包含（main.rs 114 行，双模缓冲在测试节与正文）。
2. `:11`「references/latest/ 五分文件」——现有 7 个（behaviors、clients、commands、compatibility、config-toml、settings-json、troubleshooting）。
3. 未索引：`docs/INSTALL_AGENT.md`、`docs/benchmark-watchdog.md`、`SECURITY.md`、`CHANGELOG.md`、`README_EN.md`、`.agents/plan/*`、`.agents/skill-evals/`、本次 `.agents/review/2026-10-11-0.2.0-audit/`。
4. `.agents/docs/` 下 `unix-testing.md`（13.6 KB）、`unix-stress-review.md`（18.8 KB）、`review-history.md` 是历史报告，建议标注「历史，不随代码更新」或归并进对应记忆。

### F.3 memory/

- 38 条记忆中只有 8 条带完整的 `scope/status/last_verified`（AGENTS.md 规定的 frontmatter）；**10 条完全没有 frontmatter**（`ci-unix-blindspot`、`install-pitfalls`、`install-three-platform-e2e`、`never-kill-aproxy-by-name`、
  `trusted-publishing-p0`、`install-online-deep-test`、`macos-support-required`、`release-version-alignment`、`compressed-body-inspection`、`transform-format`），它们在 MEMORY.md 里被 `[[名字]]` 引用却没有 `name:` 可解析；另 20 条缺 `scope/status/last_verified`。
- 将过时的：`ipc-v1-upgrade-contract`（W2 重写永久接口清单时一并处理，并改掉「升级下限」）；`release-engineering` 的版本路线段；`macos-support-required` 的「3 个失败」（现 6 个）；`unix-first-test` 里「APROXY_RUN_DIR 平台语义待决策」（已被 `7def3bf` 部分改变）。
- 建议新增（0.2.0 发布时）：release flavor/资产命名契约（RT-03 定稿后）；发版清单指向 `docs/RELEASING.md`。

## 9 与其他审查的交叉点

- **控制面 CP-19**（install 与 GUI 进程共存、缺库、RSS）与本报告 RT-04 互补；CP-17 的「bin 变薄壳、serve_forever 迁入 lib」是 RT-05 第 2 点的前提；CP-04 的 `--json`/退出码是 RT-16 第 1 点的前提。
- **数据面 DP-13**（信封定稿）决定 RT-06 的时机；DP-01/02/03 的错误码化决定 RT-16 第 5 点的测试断言对象；DP-28 已收 tower 与 tcp_nodelay。
- **config-cli-install**（未见其报告）读 `git show v0.1.0:src/install/...` 给 RT-01 的代码层结论；发布层的实测由 rc 阶段完成。

## 10 未核实与局限

- 没有运行任何 cargo 命令，所以以下均为推断，需 spike：Tauri `custom-protocol` 要求、`link-arg-bins` 对 bin 测试 harness 的覆盖、WebView2 缺失对守护启动的影响、UI 版体积/RSS/编译时间/MSRV、`cargo package --list` 对 `include` 的实际结果。
- RT-01 的 0.1.0 回滚行为没有实测（也不应在有生产实例的机器上做）；结论依赖 rc 阶段在隔离 home / CI 里的演练。
- 测试中文断言统计用正则近似，多行宏与间接常量（`contains(&msg)` 而 `msg` 是别处定义的常量）可能漏计；src 单测范围按「首个 `#[cfg(test)]` 之后」估算。
- CI 历史只抽查了 windows 的 1 次失败（`d993d1f`，fmt）和 ubuntu 5 次失败中的 4 次（见 RT-13），其余按作业结论统计；`ci.yml` 在 2026-10-05 之前的运行没有看。
- GitHub 的 `releases` 列表接口是否对内嵌 `assets` 数组设上限我没有核实（目前最大 28 个资产）；资产数增长到几十个后值得验证。
- 没有审 `aproxy-format`、`aproxy-envelope` 自身的测试内容与 Windows 签名的免费方案可用性。
- 外部来源：[Tauri Prerequisites](https://v2.tauri.app/start/prerequisites/)、[Tauri AppImage 分发](https://v2.tauri.app/distribute/appimage/)、[Tauri Debian 分发](https://v2.tauri.app/distribute/debian/)、
  [tauri#9662](https://github.com/tauri-apps/tauri/issues/9662)、[gitbutler#7671](https://github.com/gitbutlerapp/gitbutler/issues/7671)、[tauri#13419](https://github.com/tauri-apps/tauri/issues/13419)、[RUSTSEC-2024-0418](https://rustsec.org/advisories/RUSTSEC-2024-0418.html)、
  [tauri#11928](https://github.com/tauri-apps/tauri/issues/11928)、[atlas#332](https://github.com/pacifio/atlas/issues/332)。
