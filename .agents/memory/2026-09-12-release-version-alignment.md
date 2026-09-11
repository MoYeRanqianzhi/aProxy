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

相关：[[2026-09-12-install-online-deep-test]]、[[release-engineering]]
