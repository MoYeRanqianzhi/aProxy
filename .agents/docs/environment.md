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

## 已知坑

- `cargo test` 全量在 Windows 上约 20-40s（集成 17s 主导），守护测试串行更稳
  （`-- --test-threads=1`；并行下不同测试的 status 全局断言可能互相串扰——
  断言只应涉及自己派生的端口）
- tasklist 输出是 GBK；经 `String::from_utf8_lossy` 处理足够（只判 pid 数字）
