# 开发环境事实

> 本机（Windows 11）开发环境的非显然事实。新 agent 接手前先读。

## 环境代理污染

本机常设 `HTTP_PROXY/HTTPS_PROXY/ALL_PROXY`（如指向 127.0.0.1:10808/7890 的
外部代理）。后果与隔离：

- 直接 curl/reqwest 访问 127.0.0.1 的测试流量也会被送进外部代理 → 随机假失败
- 集成测试两层隔离：测试客户端一律 `no_proxy()`（`local_client()`）；
  aproxy 内部 upstream 客户端按生产语义读环境变量构建，靠 `NO_PROXY=127.0.0.1,localhost`
  排除环回目标（`isolate_env_proxy()`，须在首次 `AppState::new` 前设置）
- 手动跑 aproxy 的 IPC/status/stop 命令时也要 `export NO_PROXY=...`
  （reqwest 不用于 IPC，但 status 等路径的一致性依赖测试环境稳定）

## 生产实例保护（铁律）

- 用户的真实 aProxy 实例可能随时在运行（曾见 12345/12349 端口，exe 名
  aproxy-using.exe）——**任何操作绝不触碰它**：不 stop、不 kill、不占用端口
- 测试/冒烟只用派生高位端口或显式临时端口（如 59041），用完即清
- taskkill 只允许作用于自己拉起并持有 pid 的进程

## 真实目录

- 配置：`~/.aproxy/config.toml`（用户真实配置含生产 base_url，不手动改）
- 运行数据：`~/.aproxy/run/`（.pid/.restore）、`~/.aproxy/logs/`
- 集成测试不写真实 run/logs（守护测试例外：会经生产路径写，但 guard 清理）；
  单测全部 tempdir 注入

## 工具链

- Windows 11 + Git Bash（Bash 工具）+ PowerShell；路径含空格时引号
- Rust edition 2024；`cargo clippy --all-targets` 零警告是提交门槛（见
  [[warning-zero-policy]]）
- `clippy --fix` 的 let-chains 合并会打乱缩进——跑完必须 `cargo fmt`

## 三平台实测环境（2026-09-14 实测）

三个平台各有各的坑，动手前先看这里能省一轮：

- **WSL（Debian，WSL2）**：有 rustup 工具链（cargo 1.98.1），但**没有 git / curl /
  python3**，且**网络不可用**（TLS 全失败，疑似 Windows 侧 TUN 劫持）——因此
  **拉不下新依赖**。仓库经 `/mnt/g/ClaudeProjects/aProxy` 直接访问（无独立克隆，
  同一棵工作树，故不需要 git）。2026-09-14 就是卡在这：`cargo test` 报
  `failed to get brotli-decompressor as a dependency`。要在 WSL 跑全量，
  得先把 crates 的 registry 缓存同步进去（或改用远端机器）。
  **教训**：新增依赖后，WSL 不再能靠 `git pull` 直接验证。
- **远端 `ssh remote`**（Ubuntu，kernel 6.8，x86_64，10 核）：工具链在
  `~/.cargo/bin`（非交互 shell 的 PATH 里没有，须 `export PATH=$HOME/.cargo/bin:$PATH`），
  有 git 与 curl，网络正常。历史测试克隆散落在 `/root/aproxy-{e2e,fix,test,unixfix}`，
  **新验证请用全新目录**（如 `/root/aproxy-wfverify`）克隆，别覆盖它们。
  glibc 2.39（与 CI ubuntu-24.04 同级）。
- **CI（GitHub Actions）**：`gh run list --limit 1` 会误抓到并发的 Review workflow，
  **必须 `--workflow=CI` 过滤**（见 [[2026-09-11-ci-unix-blindspot]]）。
  Release 的 test 门禁跑在 **windows-latest**，与 CI 的 windows job 同平台。

## 已知坑

- `cargo test` 全量在 Windows 上约 20-40s（集成 17s 主导），守护测试串行更稳
  （`-- --test-threads=1`；并行下不同测试的 status 全局断言可能互相串扰——
  断言只应涉及自己派生的端口）
- tasklist 输出是 GBK；经 `String::from_utf8_lossy` 处理足够（只判 pid 数字）
