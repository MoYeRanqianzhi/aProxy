# 第 4 步（发布流水线）代码审查

审查对象：`git log a7cf72f..5928fcb` 中第 4 步的提交——f1f463d、7a8882a、f155ff9、bef2471、2701713、bbc0bc7，
以及交接记录 62223c6（`.agents/plan/release-pipeline-step4.md`）与 5928fcb（TODO）。夹在中间的 W2 文档提交不在范围内。
审的是已提交的版本（`git show` / `git diff a7cf72f 5928fcb`），行号以 5928fcb 为准。

日期：2026-10-12

方式：只读——git、grep、读文件、`gh run list` / `gh run view`；没有编译、没有跑测试、没有启动 aproxy，没有读写
`C:/Users/MoYeR/.aproxy/`。写报告时工作区里有别人未提交的改动（CHANGELOG.md、commands.md、release-body.sh、
Cargo.toml、src/cli.rs 等），没有看。

证据类别：**读代码**（看了实现得出）、**命令验证**（命令的输出直接证明）、**推断**（由代码与已知机制推出，没有运行）。

## 必须修

### 1. 暂存脚本丢了可执行位，Linux 与 macOS 上能在本机跑的 10 个产物会在 `--version` 冒烟这一步失败

- 位置：`.github/scripts/stage-asset.py:18`（`shutil.copyfile(src, dest)`）；调用处 `.github/workflows/release.yml:222-227`
  与 `:288-293`，冒烟跑的是暂存后的 `dist/$ASSET`；`.github/scripts/version-smoke.sh:42-55`（目标等于 host、x64 上的
  i686、同架构 gnu 上的 musl 都判为可跑，真的执行 `"$bin" --version`）。
- 为什么：`shutil.copyfile` 只拷数据，目标文件由 `open(dst, 'wb')` 新建，权限是 0666 & ~umask（runner 与容器里是
  0644）。本机 CPython 3.12.9 的 `Lib/shutil.py:260-273` 就是这样实现的：Linux 走 `_fastcopy_sendfile`，macOS 走
  `_fastcopy_fcopyfile(..., posix._COPYFILE_DATA)`，都不碰权限位。一个 x 位都没有的文件，root 也 execve 不了
  （EACCES），`out=$("$bin" --version)` 以 126 失败，`set -euo pipefail` 让这一步失败。会挂的是：Linux headless gnu
  3 个（release.yml:165-167）、musl 2 个（:169-170）、macOS aarch64 的两个变体（:160、:162，macos-latest 是
  arm64）、ubuntu:22.04 容器里的 Linux 带界面版 3 个（:247-249）。Windows 不看 x 位，x86_64 macOS 是交叉产物只查
  嵌入的版本行，其余 10 个能过。`manifest` 要求 `build` 与 `build-linux-desktop` 全部成功（:335），所以演练与正式
  发布都到不了清单这一步。旧流水线先对 `target/...` 跑 `--version`、再用 pwsh `Copy-Item` 暂存（a7cf72f 的
  release.yml:238-247），改成冒烟暂存件之后才暴露；交接记录说 version-smoke 是在本机（Windows）核对的
  （release-pipeline-step4.md:37），看不出这个问题。
- 怎么修：`shutil.copy2(src, dest)`，或 copyfile 之后 `os.chmod(dest, 0o755)`。upload-artifact 本来就不保留权限，
  下游都自己补（npm 组包 `npm/build-and-publish.sh:173` 的 `chmod +x`、安装器 `src/install/download/mod.rs:486` 的
  `ensure_executable`、`scripts/install.sh:305` 的 `chmod 755`），所以只改这一处。
- 证据：读代码（stage-asset.py、release.yml、version-smoke.sh、CPython shutil 源码）；没有在 Linux / macOS 上实际运行。

### 2. main 上的 CI 已经红，发布门槛会以同样的方式红：Linux 带界面的 debug 二进制超过了强制安装的 256 MiB 资产上限

- 位置：`tests/install_force.rs:961`、`:1056`、`:1106`、`:1261` 四个测试，它们把 `CARGO_BIN_EXE_aproxy`（debug
  构建）当作发布资产下载（`:928`、`:1002`、`:1072`、`:1187`）；上限 `src/install/force.rs:151`（`ASSET_LIMIT`）；
  门槛 `.github/workflows/release.yml:132`（与 ci.yml:151 同一条 `cargo test --workspace --locked --no-fail-fast`），
  `build` 与 `build-linux-desktop` 都 `needs: test`（:146、:241）。
- 为什么：CI run 38268203272（push，5928fcb）的 `ubuntu (desktop)` job 114875672331 结论 failure：
  `online_latest_never_downgrades_silently ... panicked at tests/install_force.rs:961:10: "aproxy-bin 下载失败: 超过 256 MiB 的上限"`，
  另外三个（`online_latest_takes_the_newest_release_that_carries_a_manifest`、
  `online_falls_back_to_musl_when_the_gnu_asset_does_not_run`、`online_rejects_what_does_not_match`）同样，
  `9 passed; 4 failed`；`ubuntu (headless)`、`windows (desktop)`、`windows (headless)` 都绿。根因不在第 4 步的代码
  （这段范围里 tests/install_force.rs 没改，7a8882a 改的是 install_flow.rs），而是有了带界面的版本之后，Linux 的 debug
  二进制带着 GTK / WebKit / Tauri 的完整 DWARF 变得很大：Cargo.toml 没有 `[profile.dev]`，默认带完整调试信息；Windows
  的调试信息在单独的 PDB 里，所以不受影响（后半句是推断）。但它直接挡住第 4 步：release.yml 的
  `test (ubuntu-24.04, desktop)` 跑的是同一批测试，演练会停在门槛上，20 个构建一个都不会开始。交接记录的约束「每个推送
  的提交都要让 ci.yml 保持绿」（release-pipeline-step4.md:8）眼下也不成立。
- 怎么修：别动 `ASSET_LIMIT`——已发布的二进制读清单时永远按 256 MiB 判，真发布的资产（release profile、strip、LTO）
  离它很远，它应当留着当健全性检查，清单示例也按它核对每个资产（examples/release-manifest.rs:232-234）。缩小测试用的
  debug 二进制即可，例如根 Cargo.toml 加 `[profile.dev.package."*"] debug = "line-tables-only"`（或 `false`），或在
  Linux 上用 `split-debuginfo = "unpacked"`，或只在 ci.yml / release.yml 的测试 job 里设
  `CARGO_PROFILE_DEV_DEBUG=line-tables-only`。改完要在 CI 上确认四个测试转绿。
- 证据：命令验证（`gh run view 38268203272 --job 114875672331 --log-failed`）；体积的成因是推断，二进制实际多大没有量。

### 3. `install --force` 新加的预检把 0.2.0 这一版带界面构建的系统要求冻进了永久的强制安装路径；写了 `--variant desktop` 时应当交给下载后的 `--version` 判断

- 位置：`src/install/force.rs:1154-1172`；判断依据在 `src/install/variant.rs:17`（glibc 2.35）、`:21`（WebKitGTK
  2.40）、`:30-32`（musl 一律没有带界面版）、`:87-92`（soname `libwebkit2gtk-4.1.so.0`）。
- 契约本身没有被破坏：force.rs 在这段范围里只有两处改动——`MANIFEST_LIMIT` / `ASSET_LIMIT` 改成 `pub`（:149、
  :151），以及这段预检；`tests/force_contract.rs`、`src/ledger.rs`、`src/lib.rs` 都没改；预检以 10
  （`EXIT_UNCHANGED`）结束，发生在动任何东西之前，C8 的步骤顺序、退出码 0/10/11/12/2 与 C6 的读者都没变。
- 问题在用途：强制安装存在的理由是「每一版都能强制安装任何另一版」，它为此只依赖两样与版本无关的东西：目标 release
  自带的清单（C6，回答「有没有这个构建」）和下载后在本机执行 `--version`（C8 第 1 步，回答「这台机器跑不跑得起
  来」）。预检用的三条事实只描述 0.2.0 自己的带界面构建：musl 没有带界面版、glibc ≥ 2.35、`libwebkit2gtk-4.1.so.0`
  ≥ 2.40。它们会随以后的发布变化（比如给 musl 出带界面版、构建基线换到更新的系统、Tauri 换到另一个 WebKitGTK API 与
  soname），而 0.2.x 二进制里的这段代码永远改不了。到那时用 0.2.x 执行
  `aproxy install --force --variant desktop <新版本>`，会在一台其实跑得起来的机器上以 10 拒绝，而清单与探测本来都会
  放行。反过来，预检说能跑而其实跑不了的情形（缺别的库，或者 dlopen 用的 `RTLD_LAZY` 不像真实启动那样立即绑定），
  仍然要靠探测挡住。
- 它也没有带来新的结果：没有预检时，显式 `--variant desktop` 在缺 WebKitGTK 的机器上会下载 → `--version` 失败 →
  `acquire_online` 改找同变体的 musl 产物 → 清单里没有，错误列出 musl 目标只有 headless（force.rs:661-684，
  `missing` 在 :325）→ 同样以 10 结束。预检省掉的只是一次下载，换来一句更直白的提示。
- 没写 `--variant` 的分支几乎不起作用：这时变体取正在运行的二进制自己的（force.rs:1149-1153），只有带界面的二进制会
  进预检；它既然在运行，`libwebkit2gtk-4.1.so.0` 已经由动态加载器装进进程（DT_NEEDED；Rust 在 Linux 上默认完整
  RELRO，v2_40 的符号已经绑定），dlopen 拿到的就是这一份，WebKit 那条必然通过。唯一可能触发的是 glibc 那条：
  `DESKTOP_GLIBC_FLOOR` 是按构建系统定的保守值，产物的实际需求可能更低（交接记录 :42 正打算演练后按 objdump 改
  小），落在两者之间的机器上会把一个跑得好好的带界面安装改成 headless。
- 怎么修（建议）：删掉 force.rs 里这段预检。显式 `--variant desktop`：照常下载、探测，失败以 10 结束，错误里可以附上
  `desktop_blocker` 给出的原因，作为解释而不是门槛。没写 `--variant`：若要兑现文档里「缺库时改装 headless」，就在
  `acquire_online` 里把回退做成探测驱动——带界面的产物跑不起来、变体又不是用户写的，就换同目标的 headless，再不行
  换 musl headless。这是第 1 步「取得目标」内部的追加，不改步骤顺序与退出码，也和清单一样与版本无关。改了要同步
  README.md:73、docs/INSTALL_AGENT.md:166-170、docs/architecture.md:622-627、skill 的 commands.md:689-691 与
  CHANGELOG.md:37。
- 为什么放在「必须修」：影响面窄，`--from` 也能绕开；但这段代码一旦随 0.2.0 发出去就收不回来（alpha 里的代码不改会
  原样进正式版），修改窗口只在 0.2.0 正式版之前。
- 证据：读代码（force.rs、variant.rs、`git diff --stat` 确认契约文件未改）；「以后的构建要求会变」是推断；「没写
  `--variant` 时 WebKit 检查必然通过」依据二进制以 DT_NEEDED 链接 WebKitGTK 与 Rust 的默认 RELRO，是推断，没有在
  Linux 上运行验证。

## 应该修

### 4. 普通安装的预检有同样的结构问题，还会把「已是最新」变成重装另一个变体，并把整套 GTK / WebKit dlopen 进安装器进程

- 位置：`src/commands/install.rs:322-335`（预检；显式时 :325-330 以 1 退出，隐式时 :334 改 `ctx.variant`）、:336
  （`switching`）、:361（`UpToDate(_) if switching => current`）；`src/install/variant.rs:73-108`。
- (a) 显式 `--variant desktop` 被本机门槛拒绝：普通安装同样是旧二进制去装新 release，门槛同样会过时。它比强制安装好
  一点：有自然的出路（先按原变体升级，再用新版本换变体），所以放在这一档。下载后的探测（:459-464）已经能挡住跑不
  起来的产物，且显式时不回退（:428-436），结果同样是失败退出。
- (b) 隐式分支：同第 3 条，只有带界面的二进制会走到，WebKit 检查必然通过，只剩 glibc 常数偏保守时的误判。误判之后
  `switching` 为真，`aproxy install` 在已是最新时走 :361，把当前版本重装成 headless——用户只想看看有没有更新，带界面
  的版本却没了（会打印说明）。演练后把常数改成实测值能消掉这个误判，更稳的是不让预检决定变体：隐式时直接走
  :427-436 的探测链（带界面 → 同目标 headless → musl）。
- (c) dlopen 本身（关注点 2）：unsafe 的用法是对的——参数是 C 字面量，NULL 都检查了，dlerror 是线程局部的、文本立即
  拷走，两个函数在公开头文件里是 `guint f(void)`，transmute 成 `extern "C" fn() -> u32` 没问题；
  `cfg(all(target_os = "linux", target_env = "gnu"))` 把 `gnu_get_libc_version` 与 dlopen 限在 glibc 上，musl 与非
  Linux 只编进返回 None 的桩，`major_minor` 也只在测试与 glibc 上编译。要注意的是：它在 headless 的安装器进程里装进
  整套 GTK / WebKit / GLib / Mesa 依赖并跑它们的 ELF 构造函数，然后同一个进程继续做广播、交换、滚动重启；dlopen
  进来的库若用 initial-exec TLS（glvnd 的 libGLdispatch 在 aarch64 上是出名的例子），可能报
  `cannot allocate memory in static TLS block`，被当成「没有可用的 WebKitGTK」——显式时直接拒绝，用户无法纠正。按第
  3 条与 (a)(b) 改，dlopen 只在探测失败后用来写原因（或者干脆用动态加载器打出的 stderr 当原因），这些风险就不在
  正常路径上了。
- 证据：读代码；(b) 的触发条件与 (c) 的 TLS 假阴性是推断，没有在真机上复现。

### 5. CHANGELOG 说 `--force` 下载后带界面版跑不起来也会改装 headless，代码不是这样

- 位置：`CHANGELOG.md:37`（「所以在线安装（含 `--force`）……；下载后带界面的版本跑不起来时同样改装 headless」）。
  实际 `src/install/force.rs:661-684` 只把 gnu 换成同变体的 musl，带界面的 musl 不存在，于是以 10 结束。skill
  （commands.md:566-569 讲的是普通安装，:689-696 讲强制安装列出可用组合）与 README.md:73 是对的。
- 怎么修：按第 3 条改了代码，这句就成立；不改代码就把这半句限定到普通安装。
- 证据：读代码。

### 6. skill 里引用的「通道为空」提示已经过时

- 位置：`.claude/skills/aproxy-cli/references/latest/commands.md:555` 写的是 `<channel> 通道暂无可用版本，保持当前 <v> 不变。`；
  7a8882a 把 `src/commands/install.rs:373-377` 改成了 `{} 通道暂无可用的 {} 版本，保持当前 {current} 不变。`（中间插了
  变体）。skill 给代为操作的 agent 看，它们会按引用的原文匹配输出。
- 怎么修：改成 `<channel> 通道暂无可用的 <variant> 版本，保持当前 <v> 不变。` 一类的写法。
- 证据：读代码（`git diff a7cf72f 5928fcb -- src/commands/install.rs`）。

### 7. 变体的选择与回退链没有测试

- 位置：`src/commands/install.rs:316-336`、`:427-436`、`:477-484`（变体不符算链条失败）；`src/install/force.rs:1154-1172`。
- 现有测试覆盖了 `--from` + `--variant` 的核对（tests/install_flow.rs:1092 起）、资产命名、npm 包名、musl 回退恒为
  headless baseline、清单的 select / missing；普通在线安装的「隐式带界面失败 → headless → musl」「显式不回退」「换变体
  且已是最新时重装当前版本」以及两处预检都没有。这些逻辑写在 `run_online` 里，不拆开就测不了。
- 怎么修：把 `attempts` 的构造抽成纯函数（输入：上下文、是否显式），用单元测试钉住几种组合；强制安装的回退若按第 3
  条改进 `acquire_online`，沿用 tests/install_force.rs 现有的本地 HTTPS 夹具加一两个用例。
- 证据：读代码（grep tests）。

## 可以不修

- **a.** 换变体而通道为空时以 0 结束、什么都没换（`src/commands/install.rs:372-382`）。用户写了 `--variant`，退出码 0 会
  让脚本与 agent 以为换好了。实际很难遇到（当前版本本身就在通道里，会走 UpToDate 分支）；可以和 :361 一样重装当前
  版本，或者以 1 结束。读代码。
- **b.** install.sh 找不到库时拒绝显式 `--variant desktop`（`scripts/install.sh:366-369`），而 `find_webkit` 只要找到
  ldconfig 就只看缓存（`:254-258`），装在缓存之外的库（/usr/local 没跑 ldconfig、只靠 LD_LIBRARY_PATH）会被判成没有。
  隐式时没关系（后面的 self_test 兜底），显式时用户越不过去。脚本从 main 现取、随时能改，所以不急；改法同第 4 条：显式
  时直接下载、交给 self_test，`:418-422` 本来就会把动态加载器的报错打出来。读代码。
- **c.** install.sh 的 `--variant` 放在最后、没给值时（`:315` 先 shift，`:320` 再 shift），dash 报
  `shift: can't shift that many`，bash 以 sh 运行时静默以 1 退出，都到不了 `:322-325` 的「未知变体」。读代码 + 推断
  （没有运行）。
- **d.** 资产命名规则写了两份：`src/install/download/mod.rs:45-54`（`.exe` 按 `cfg!(windows)`）与
  `examples/release-manifest.rs:91-106`（按目标串含 `-windows-`）。清单 job 用后者核对 release.yml 的 ASSET
  （:138-143），普通安装用前者按名字去 GitHub 取；两者之间没有测试相连，改了一边普通安装会 404 而演练看不出。命名
  事实上已经冻结（旧二进制按自己的规则取），漂移不太可能；要一处真相，可让库函数按目标串判断 `.exe`，示例直接调用。
  读代码。
- **e.** `cargo binstall aproxy` 在 glibc 的 Linux 服务器上会装带界面的 gnu 版本，缺 WebKitGTK 时起不来（binstall 不懂
  变体，overrides 只覆盖两个 musl 目标，Cargo.toml:59-65）。README.md:62-65 与 README_EN.md:135-139 写了 `--pkg-url`
  的做法，接受。读代码。
- **f.** 相邻的旧问题（不是第 4 步引入的）：`prefers_x86_64_v3()`（`src/install/download/mod.rs:264-273`）在任何带 AVX2
  的 x86 / x86_64 上都选 `-v3`，包括 macOS x86_64 与 Windows i686，而这两个目标不发布 `-v3`。普通安装的 github 渠道于是
  去取 `aproxy-x86_64-apple-darwin-v3` 这类不存在的资产而失败，落到后面的渠道（npm 与 binstall 渠道固定 baseline，最终
  能装上，只是慢、多一条错误）。release.yml:142-143 的注释「失败回退 baseline」说的是强制安装的 `Manifest::select`
  （force.rs:290-295），普通安装没有这条回退。读代码，没有运行。
- **g.** 如果保留预检：dlopen 用 `RTLD_LAZY`（variant.rs:90），比真实启动（完整 RELRO 的二进制、发行版多以 -z now
  构建的库）宽松，`RTLD_NOW` 更接近「动态加载器会不会拒绝」。按第 3、4 条改了就无所谓。推断。
- **h.** npm 启动器的 127 探测（`npm/aproxy/bin/aproxy.js:144-153`）只在带界面组、Linux、退出码 127 时多跑一次
  `--version`，用默认的管道收 stdout / stderr，不碰用户终端（主进程 :136 的 `stdio: 'inherit'` 不受影响），10 秒超时，
  stderr 只做匹配不回显，退出码照原样传回。正则不认 `GLIBCXX_` 这类别的版本节点的报错，只是少一句提示（原始报错已经
  打在终端上）。可接受。读代码。
- **i.** 带界面组的启动器在 glibc 2.28–2.34 上直接拒绝（`npm/aproxy/bin/aproxy.js:85-91`），下限是按构建系统定的保守
  值；演练量出实测值后要与其他几处一起改（交接记录 :42-44 已列）。读代码。

## 关注点逐条结论

1. **永久契约**：force.rs 只增不改（两个上限改 `pub` + 预检），`tests/force_contract.rs`、`src/ledger.rs`、`src/lib.rs`
   没动，C5 版本行、C6 读者、C7 命令行、C8 顺序与退出码都没变。但显式 `--variant desktop` 的拒绝应当去掉，改由下载后的
   `--version` 判断；预检连默认选择也几乎派不上用场（第 3 条）。读代码。
2. **variant.rs**：unsafe 正确，cfg 边界干净，版本解析能处理发行版后缀；风险在 dlopen 进安装器进程与 TLS 假阴性（第 4
   条 c）。读代码 + 推断。
3. **普通安装**：github（`github.rs:151`）、npm（`npmpkg.rs:15-50`，headless 在 `@meowo/aproxy-headless-*`）、binstall
   （`crates.rs:65-75`，headless 自己拼地址）、cargo（`crates.rs:146-150`，`--no-default-features`）都按 `ctx.variant`
   取；升级保持变体（`DownloadCtx::build` 取 `cfg!(feature = "desktop")`，mod.rs:313-317）；`--from` + `--variant` 只核
   对（install.rs:116-127）；显式变体从不跨变体回退（install.rs:428-436，显式 headless 只回退到 musl headless）。读代码。
4. **release.yml**：顶层只读（:38-39）；`contents: write` 只在 `release`（:364-365）；`id-token: write` 只在
   `publish-npm`（:432-434）与 `publish-crates`（:491-493），两者都 `environment: release`、`needs: release`；`release`
   只在 push 时跑（:361），dispatch 下它与两个 publish 一并跳过，三个 rehearse job 只在 dispatch 时跑、没有 id-token、
   不进环境（:396、:460、:541）。tag 与 dispatch 两条分支不交叉，`DRY_RUN_VERSION` 经 env 传入，align-version.py 只放行
   `[0-9A-Za-z.+-]`。20 个产物（带界面 9 = Windows 4 + macOS 2 + Linux 3；headless 11 = Windows 4 + macOS 2 + gnu 3 +
   musl 2）的 ASSET 表达式（:173、:253）与 `asset_name`、清单示例的命名规则一致；清单的生成与读回用库里的
   `VersionLine::parse` / `Manifest::parse` 与同一组上限；glibc 断言用共用脚本，headless 2.28、带界面 2.35（实测待演
   练）；crates.io 顺序 envelope → desktop → aproxy（:516），`=` 钉住由 align-version.py 恰好替换一次并同步 Cargo.lock；
   gen/schemas 进包、打包前先构建一次（:520-522）。但第 1、2 条会让演练在门槛与冒烟两处失败。读代码。
5. **npm**：headless 组 10 个包的名字与启动器的前缀推导一致；带界面组的两个 musl 包装的是 headless 二进制，它的
   `--version` 报 headless，`aproxy install` 按 headless 升级、走 `@meowo/aproxy-headless-*`，不会误判；在 musl 上写
   `--variant desktop` 被预检的 musl 条款拒绝（第 3 条）。127 分支安全（可以不修 h）。读代码。
6. **install.sh**：glibc 版本读取（getconf → ldd）与 musl 判定稳妥；`APROXY_VARIANT`、`--variant`、`--variant=` 都认并
   校验取值；隐式时下载后 self_test 失败改装 headless，再不行换 musl；显式不回退。问题见可以不修 b、c。读代码。
7. **binstall**：两个 musl 目标的 overrides 指向 `-headless-`，README 的 `--pkg-url` 模板与 release 资产名一致；安装器的
   binstall 渠道不读 overrides，自己拼 headless 地址（crates.rs:68-75）。binstall 本身没有运行。读代码。
8. **文档与 skill**：README、README_EN、INSTALL_AGENT、architecture、skill 与代码一致，skill 仍是英文、写给 agent；不一致
   的只有 CHANGELOG 的一句（第 5 条）与 skill 引用的一条旧提示（第 6 条）。读代码。

## 我没能验证的

- 整条流水线没有跑过：`gh run list --workflow Release` 最近一次是 v0.1.0（2026-10-04），还没有 workflow_dispatch 演练。
  20 个构建、ubuntu:22.04 容器 job（含 arm64 runner 上的容器）、清单 job、两个 dry-run、macOS fat LTO 的内存与总时长，
  都只看了代码。
- 第 1 条是按代码与 CPython 源码得出的，没有在 Linux / macOS 上实际跑 stage-asset.py。
- 第 2 条的失败是 CI 日志里的事实，但 debug 二进制实际多大、按建议改调试信息之后能小到多少，没有量。
- `DESKTOP_GLIBC_FLOOR` 的实际值（objdump 的最高 GLIBC_ 需求），以及 ubuntu:22.04 里链接出的带界面版在其他发行版上能
  不能起来。
- dlopen 预检在真实系统上的行为（aarch64 + glvnd 的静态 TLS、各库的构造函数）、install.sh 的 ldconfig 与库目录探测在
  各发行版上的结果、npm 启动器 127 分支在缺 WebKitGTK 的 Linux 上的表现。
- cargo-binstall 对 overrides 与 `--pkg-url` 的实际处理。
- 10 个 headless npm 包的可信发布绑定（TODO.md:51 记的是所有者口头告知，主代理无权核验），以及 aproxy-desktop 在
  crates.io 的种子发布与可信发布配置（交接记录 :46 仍未完成）——第一次 tag 发布之前都要落实。
- 普通安装换到 headless 时，正在运行的 `aproxy desktop` 窗口会怎样（它从旧的带界面二进制运行，交换与滚动重启只管
  实例），没有追。
- 工作区里别人未提交的改动没有看，不知道是否已经覆盖了上面的某些条目。

## 主代理的处理决定（2026-10-12）

- 必须修 1–3、应该修 4–7 全部修，由 step3-desktop 实施。
- 第 3、4 条的定法：`install --force` 与普通安装都删掉本机探测式的预检。显式写了变体时，以下载后二进制的 `--version` 为准；
  没写时保持原变体，带界面版下载后起不来就回退到同目标 headless，再到 musl。`src/install/variant.rs` 用不上就删掉。
  这仍然兑现用户定的「Linux 自动选择变体」：首装靠 install.sh 的探测，npm 启动器认出缺库并提示，升级保持原变体，
  下载后起不来就回退。机制由主代理按授权定。
- 可以不修中，a、b、c、d、f 修；e、h 接受；g 在删掉探测后不再适用；i 等演练量出 glibc 实测值后与其他几处一起改。
