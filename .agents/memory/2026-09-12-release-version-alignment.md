# 2026-09-12 release 版本对齐机制与版本号污染事故

## 机制（release.yml，tag 触发的 build 与 publish 两个 job 都要有）

发布构建前必须把 tag 版本写入 Cargo.toml/Cargo.lock（纯 python，显式
utf-8，\r?\n 兼容 CRLF，不用 sed——BSD/GNU -i 分裂；Windows runner 强制
shell: bash；cargo publish 加 --allow-dirty）。

原因链：产物自报版本（--version）= Cargo.toml 版本；install 特定版本的
自证校验要求自报 == 目标（tag）；crates.io 按 .crate 内 manifest 注册版本。

## 事故：alpha.10/alpha.11 正式号被测试 tag 抢注

测试 tag（v0.1.0-alpha.10t1 等）发布时未对齐版本 → crates.io 按 .crate 内
alpha.10 注册 → **正式号永久被占**（crates.io 不可删版本，yank 不释放号）。
正式版顺延：当前 Cargo.toml = **0.1.0-alpha.12（尚未正式发布）**。

被污染的版本号清单（crates.io）：
- 0.1.0-alpha.10：内容 = t1 时代源码（非正式 alpha.10 内容）
- 0.1.0-alpha.11：内容 = alpha.11t1 时代源码
- npm 的 0.1.0-alpha.10t1/11t1/12t1：部分渠道不齐（npm 有、crates.io 无）

## 纪律

- **测试 tag 一次性**：npm/crates.io 版本不可覆盖，发布失败后必须 bump tN，
  绝不重打同号
- 测试 tag 格式 `v<semver>t<N>`（用户 2026-09-11 授权）；semver 中 10t2 >
  10t1（ASCII 比较）合法可比较
- 正式发布前确认：build + publish 双 job 的对齐都在、npm dist-tags 与
  crates.io 版本列表无无后缀号冲突

## 2026-10-04 补充：tN 测试 tag 与选版、npm next

- **选版排除 tN**（0dc9440）：`aproxy install` 与三个安装脚本只认
  `vX.Y.Z` / `vX.Y.Z-(alpha|beta|rc).N`。原因：semver 规定字母数字标识优先于
  纯数字标识，`alpha.12t3 > alpha.17`，仓库里带全套资产的 alpha.12t3/11t1 会把
  pre 通道用户「升级」到旧测试构建。推论：测试 tag 必须保持非文法形式，用文法
  合法的号做测试会被用户渠道选中。
- **npm 仍会被 tN 污染**：`npm/build-and-publish.sh` 对含 `-alpha` 的版本发
  `next` dist-tag，tN 版本同样匹配，于是一次 tN 发布会把 `@next` 移到测试构建，
  直到下一个真实预发布才移回；crates.io 照旧永久占号。
- **建议（未经用户定调）**：0.1.0 起的发布演练用 `rc.N`。rc 本身就是演练用的
  预发布号，文法合法，走 next 通道也正确，而 tN 会造成上面两种污染。
  Recheck when：用户对演练方式另有决定，或 build-and-publish.sh 的 dist-tag 规则变化。

相关：[[2026-09-12-install-online-deep-test]]、[[release-engineering]]
