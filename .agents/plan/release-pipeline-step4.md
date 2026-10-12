# 第 4 步：发布流水线（交接记录）

范围与用户决定以 [release-0.2.0](release-0.2.0.md) 的「alpha.1 的范围与顺序」第 4 条和 W5「所有平台都发两个版本」为准；
主代理 2026-10-12 派给 step3-desktop 的九项：20 产物双矩阵、发布清单、npm headless 组与启动器提示、crates.io 发
aproxy-desktop、Linux 自动选择变体、release-format 门禁、binstall 选变体、workflow_dispatch 演练、文档与 skill。

约束：共享工作区（`git commit -- <路径>`）；同一时刻只跑一个 cargo；不在本机跑真实的 `aproxy install`；主代理第一次
推送之后才可以推 origin main 跑演练，且每个推送的提交都要让 ci.yml 保持绿、绝不打 tag、绝不发布到 npm 或 crates.io。

## 已定的做法

- **gen/schemas（U2）**：`aproxy-desktop` 的 include 带 `/gen/schemas/**`；打包前先在同一平台上构建一次本包。
  2026-10-12 在 Windows 上 `cargo package -p aproxy-desktop` 校验通过（不带时报「Source directory was modified」）。
  三个平台的 schema 内容逐字节相同，tauri-utils 内容不变不重写，cargo 比内容哈希。
- **workflow_dispatch 恒为演练**：不设「真发布」开关，发布只由 tag 触发；演练构建全部 20 个产物、生成并核对清单、
  跑 `npm publish --dry-run` 与 `cargo publish --dry-run`（不上传、不拿凭据、不进 release 环境），不建 Release。
- **版本对齐**抽成 `.github/scripts/align-version.py`：tag 推送取 tag，演练取输入或 Cargo.toml；同时改 aproxy、
  aproxy-desktop 的版本、根包对 aproxy-desktop 的版本要求（`=` 精确钉住，两者同步发版）与 Cargo.lock。
- **发布清单**由 `examples/release-manifest.rs` 生成：从每个二进制里嵌着的 C5 版本行（`VERSION_LINE` 是 concat! 字面量）
  用 `VersionLine::parse` 取变体、目标、指令集档，文件名必须与命名规则一致，`.sha256` 必须相符；写完用
  `Manifest::parse` 读回逐条核对。在只读权限的独立 job 里跑，release job 只上传。
- **Linux gnu 带界面版**在 `container: ubuntu:22.04` 里构建（ubuntu-24.04 / 24.04-arm），断言 GLIBC_ 需求不高于
  `DESKTOP_GLIBC_FLOOR`（演练实测三个带界面 gnu 产物最高都是 GLIBC_2.34，05efe7b 定为 2.34；install.sh 与 npm 启动器
  用同一个数）。headless gnu 照旧 zigbuild 到 2.28（实测最高 GLIBC_2.28）。
- **npm**：加 headless 组（`@meowo/aproxy-headless` + 9 个平台包）；带界面组的两个 musl 平台包装 headless 的 musl
  二进制（musl 只有它）。启动器：带界面组在 glibc 低于带界面版下限时直接提示改装 headless；退出码 127 时重跑一次
  `--version` 抓 stderr，含 `error while loading shared libraries` 等加载失败字样就提示。
- **crates.io 首发**：aproxy-desktop 是新 crate，可信发布做不了首发。拟请所有者本地发一个 0.0.0 占位版，再给它加
  release.yml / release 环境的可信发布配置，之后 CI 经 OIDC 发真版本（待主代理与所有者确认）。
- **安装器选变体**：DownloadCtx 增加变体，资产名、npm 包名、binstall 地址、cargo `--no-default-features` 都随它；
  普通安装也接受 `--variant`。安装器（普通与 `--force`）不预先探测本机，只看下载下来的二进制 `--version` 能不能跑：
  没写 `--variant` 的带界面版跑不起来就换同目标 headless，再换 musl headless（`DownloadCtx::attempts` /
  `force::fallbacks`）；写明了的不换。主代理 2026-10-12 按第 4 步审查定的：预检会把这一版的系统要求冻进永久的
  强制安装路径，`src/install/variant.rs` 已删。只有 install.sh（每次从 main 现取）在首装、没指定变体时先看本机。

## 进度

- [x] release-format 门禁按 headless 跑（f1f463d），glibc 断言与 crate 计划改用共用脚本
- [x] U2 实测；align-version.py、release-manifest 示例（假产物正反例与一个真 debug 二进制核对通过）、
  release.yml 重写、version-smoke.sh 扩展（本机对 desktop / headless / 交叉目标三种期望核对过）
- [x] npm 组包脚本（假 npm 下三组组包核对过）与启动器（headless 组在本机解析到 aproxy.exe）
- [x] 安装器（Rust）变体感知与 Linux 自动选择；install.sh（假 ldd / getconf / ldconfig 六种系统核对过）。
  install.ps1 / install.cmd 没改：Windows 上带界面版随处能跑，装好后 `aproxy install --variant headless` 可换
- [x] 文档与 skill；binstall 给两个 musl 目标加了 overrides（逐个目标名，旧版 binstall 不认 cfg）
- [x] 第 3 步审查转来的「普通安装不区分变体」已由 7a8882a 覆盖：变体取正在运行的构建（就是它 `--version` 的
  变体段），github 资产名、npm 包名、binstall 地址、cargo 的 `--no-default-features` 都随它；Linux 首装按探测选
- [x] 主代理首推 5928fcb 后 ci.yml 的 ubuntu (desktop) 红在 tests/install_force.rs 的 4 个 online_* 测试（调试版超过
  256 MiB 的资产上限）：be3893e 改调试构建只带行号表，fd622cd 的 CI（run 38273956900）里转绿。macOS 红是既有的
  （fad48c7 上就是同样 4 个 lib 测试，daemon 那个读不到自身进程的创建时间），不归第 4 步
- [ ] fd622cd 的 CI 里 ubuntu (headless) 偶发一次 ETXTBSY（install_swap 落位后核对「Text file busy」）：127dafa 让
  run_version 退避重试（03ec68a 收紧到合计约 1.6 秒、算在 VERSION_TIMEOUT 之内）。之后 65dd651、8b02a31 两次
  ubuntu CI 都没再出现，再看一两次再下结论
- [x] 第 4 步审查（`.agents/review/2026-10-12-release-pipeline-review.md`，主代理的处理决定在文末）：8296a02（暂存件
  补 x 位）、d9d1fc1（install.sh 写明 desktop 照装、--variant 缺值报错）、96d014f（删掉安装前的探测与 variant.rs，
  回退链 DownloadCtx::attempts / force::fallbacks 带测试，通道为空也能换变体，命名只留一处，v3 只在发布它的目标上选）、
  a86e2c0（文档与 skill）。两种变体全量测试各 559 通过
- [x] 演练 run 38279211569（65dd651，2026-10-12 16:41–17:04 UTC，约 23 分钟）全绿：20 个构建（含 i686 / arm64
  Windows、macOS x86_64 的带界面版与 3 个容器 job）、清单（20 个资产读回核对通过）、Release 预览、npm 两组 20 个包与
  crates 三个包的 dry-run 都过。冒烟 18 个真的执行了 `--version`（容器里的 3 个带界面版与 Linux 暂存件都在内），
  x86_64-apple-darwin 两个是交叉编译，只核对嵌着的版本行。带界面版 glibc 下限按实测改成 2.34（05efe7b）
- [x] 第 4 步修复核对（审查文件「修复核对（2026-10-13）」）的 N1–N6：4468243（N1 回退守显式 `--cpu`；N4 只在动态
  加载器报错时提示装库，普通安装同样收紧，提示不再写死库与版本；N5 没写 `--variant` 而清单里没有本变体时也回退）、
  ac6ce9d（N6 install.sh 在 glibc 太旧时说明是 glibc）；N2 随 05efe7b 提交；N3 在 03ec68a 里已经成立。Windows 上两种
  变体的 fmt、clippy（含 Linux 交叉 clippy）与 lib / install_flow / install_force 测试通过；Linux 上的测试等推送后的 CI
- [ ] 推送 05efe7b..（含本条）之后看 ci.yml 门槛与 ubuntu (headless) 有没有再撞 ETXTBSY
- [ ] 所有者：aproxy-desktop 在 crates.io 的首发与可信发布配置（步骤在给主代理的报告里）
