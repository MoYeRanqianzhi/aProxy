# 2026-09-11 安全铁则：绝不按进程名批量杀 aproxy.exe（实际事故记录）

## 事故

维护 agent 执行 `taskkill //F //IM aproxy.exe //FI "PID ne <旧pid>" ...`（按记忆中
的生产 pid 排除）批量强杀 aproxy.exe：记忆 pid 过期（生产实例随重启换 pid，
事故时实查已是 7508/12345，而非记忆中的 24064/45640），当时机器上所有
aproxy.exe（含用户生产实例）全部被杀。**用户的 Claude Code 会话本身经
aproxy 代理连接 API——会话当场断连**，由用户手动恢复。

## 铁则（对所有在这台机器工作的 agent 生效）

1. **绝不** `taskkill //IM aproxy.exe` / `Stop-Process -Name aproxy` /
   `pkill aproxy` 等按名批量杀——过滤器写得再周全也不行（pid 基线必过期）。
2. 清理测试守护只用：`APROXY_HOME=<测试目录> aproxy stop <端口>`（优雅、
   精确、端口已知）。
3. 识别进程现场实查（`netstat -ano | grep :<端口>`），绝不引用记忆 pid。
4. 测试一律 APROXY_HOME=tempdir 隔离（本仓库测试基建已全量支持）。

## 关联

- 用户生产实例：127.0.0.1:12345（pid 随重启变化）等用户自有端口，任何
  操作不可触碰。
- install 测试的实例端口从测试进程 pid 派生（27000+/25000+ 段），与生产
  端口天然隔离。
