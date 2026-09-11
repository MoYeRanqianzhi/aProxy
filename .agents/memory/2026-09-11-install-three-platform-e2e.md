# 2026-09-11 install 三平台完整实测记录（深审查轮）

## 覆盖面

| 平台 | 环境 | 全量测试 | e2e 实测 |
|---|---|---|---|
| Windows 本地 | win32 debug+release | fmt/clippy -D warnings/全量绿 | 24/24（pwsh 脚本） |
| Ubuntu 远端 | `ssh remote`（RackNerd VPS，clone github） | 198 项全绿 | 26/26 |
| WSL Debian | 本地 WSL2（rustup 手动组件安装） | 198 项全绿 | 26/26 |

e2e 脚本（三平台共用逻辑）：快速路径 --from 安装、真实守护滚动升级（pid
滚动/宣告无残留/心跳清理）、abort 幂等、jurisdiction 拒绝 + --adopt 收编
（源=自身镜像）、--skills-only 在线失败路径、进程残留差集终检。

## 实测挖出并修复的 bug（全部已提交）

1. **announce.rs E0596**（unix 编译错误，918ea33 CI unix job 红着没人看）：
   `_keepalive` 改 `Mutex<File>`（1b98662）
2. **is_x86_feature_detected!** aarch64/mac 编译失败：target_arch 门控
   （1b98662）
3. **两个平台硬编码单测**（linux 下必失败）：platform_package/asset_name
   测试平台无关化（90f91c4）
4. **库层 IPC 环境依赖泄漏**（jurisdiction 库层测试 unix 失败的完整根因）：
   check_jurisdiction/run_tail/stop_and_wait/list_instances_in/ack_one 收了
   run_dir 却用全局 APROXY_HOME 派生 unix UDS 端点——ping 空 → list_instances_in
   还会**误删测试实例注册表**。加 endpoint_for_in/ipc_ping_in/ping_endpoint
   （552f68a + 39eea00）。Windows 命名管道全局可见所以从未暴露。
5. **有实例的 Acked/Swapping 续作被状态机拒绝**：run_forward_from_staged 对
   Broadcasting/Acked 的 advance 是逆向迁移（无实例测试因 snapshot 空跳过
   广播段而侥幸通过）。加 run_tail 同款相位守卫 + 有实例 swapping 续作回归
   测试（60ef857）。

## 环境坑（复用价值）

- WSL 出口 TLS 被 Windows 侧 TUN 代理劫持（rustup UnknownIssuer + OpenSSL
  unable to get local issuer certificate，时间正常排除时钟漂移）——绕法：
  Windows 侧下载 rust 组件 tarball（USTC 镜像）+ install.sh 手动装，cargo
  复用 Windows 的 CARGO_HOME（/mnt/c/...）+ --offline 离线构建。
- `git pull` 在工作区有本地改动（scp 调试残留）时静默失败——远端实验场
  统一 `git fetch && git reset --hard origin/main`。
- PowerShell 5.1 读无 BOM UTF-8 中文脚本按 ANSI 解析直接语法崩坏——e2e
  脚本必须 pwsh 7 跑。
- 函数参数名 `$home` 撞 PowerShell 只读变量（无法覆盖）。
- cargo test 同 target 多线程并行时，进程级 `env::set_var` 的 live 测试
  互踩（对方守护注册表指到别的 tempdir）——tokio::sync::Mutex 串行化。
- .pid 注册表是 InstanceInfo JSON 不是裸 pid（e2e 解析要提 pid 字段）。
- Windows 交棒模式：install CLI 退出 0（交换完成）≠ 安装完成，剩余阶段由
  后台续作者收尾——e2e/agent 验证必须等 install.state 消失。

相关：[[2026-09-11-ci-unix-blindspot]]、[[2026-09-11-install-pitfalls]]、
[[unix-first-test]]
