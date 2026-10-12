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
  `DESKTOP_GLIBC_FLOOR`（取值待演练实测 objdump 后定；安装器探测用同一个数）。headless gnu 照旧 zigbuild 到 2.28。
- **npm**：加 headless 组（`@meowo/aproxy-headless` + 9 个平台包）；带界面组的两个 musl 平台包装 headless 的 musl
  二进制（musl 只有它）。启动器：带界面组在 glibc 低于带界面版下限时直接提示改装 headless；退出码 127 时重跑一次
  `--version` 抓 stderr，含 `error while loading shared libraries` 等加载失败字样就提示。
- **crates.io 首发**：aproxy-desktop 是新 crate，可信发布做不了首发。拟请所有者本地发一个 0.0.0 占位版，再给它加
  release.yml / release 环境的可信发布配置，之后 CI 经 OIDC 发真版本（待主代理与所有者确认）。
- **安装器选变体**：DownloadCtx 增加变体，资产名、npm 包名、binstall 地址、cargo `--no-default-features` 都随它；
  普通安装也接受 `--variant`；Linux gnu 上要装带界面版时先探测（glibc 版本、dlopen webkit2gtk-4.1 读版本 ≥ 2.40），
  不满足就装 headless 并说明；下载后 `--version` 起不来、变体又不是用户写明的，就改装 headless。

## 进度

- [x] release-format 门禁按 headless 跑（f1f463d），glibc 断言与 crate 计划改用共用脚本
- [x] U2 实测；align-version.py、release-manifest 示例（假产物正反例与一个真 debug 二进制核对通过）、
  release.yml 重写、version-smoke.sh 扩展（本机对 desktop / headless / 交叉目标三种期望核对过）
- [x] npm 组包脚本（假 npm 下三组组包核对过）与启动器（headless 组在本机解析到 aproxy.exe）
- [x] 安装器（Rust）变体感知与 Linux 自动选择；install.sh（假 ldd / getconf / ldconfig 六种系统核对过）。
  install.ps1 / install.cmd 没改：Windows 上带界面版随处能跑，装好后 `aproxy install --variant headless` 可换
- [x] 文档与 skill；binstall 给两个 musl 目标加了 overrides（逐个目标名，旧版 binstall 不认 cfg）
- [ ] 主代理推送后跑演练：量 DESKTOP_GLIBC_FLOOR（objdump 的实际最高需求，低于 2.35 就一起改小：release.yml、
  src/install/variant.rs、scripts/install.sh、npm/aproxy/bin/aproxy.js，以及 README、README_EN、CHANGELOG、docs、skill、
  npm README 里写的 2.35）、
  看 i686 / arm64 Windows 与 macOS x86_64 的带界面构建、容器 job、macOS fat LTO 内存、总时长
- [ ] 所有者：aproxy-desktop 在 crates.io 的首发与可信发布配置（步骤在给主代理的报告里）
