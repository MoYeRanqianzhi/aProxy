---
name: random-log-names
description: 守护日志随机命名 + IPC 上报真实路径 + log_file 自定义（用户三点定调）：端口命名是易变标识做永久命名的反模式；清理判据重构为引用集
metadata:
  type: project
---

# 守护日志随机命名 + IPC 上报（2026-09-22 落地）

## 设计动因

用户实测发现：日志按 `<端口>.log` 命名没有考虑配置改换端口——端口是易变标识，拿它当永久文件名与别名系统「实例不依赖端口」的哲学直接冲突（实测 T4 换端口场景已暴露排障断档）。用户三点定调：

1. **随机命名**：格式 `<纳秒hex>-<pid低16位hex>.log`（如 `19ac3f2e8b5d-1a2b.log`），无新依赖，同 pid 同纳秒不可能碰撞。
2. **IPC 是日志地址的权威**：InstanceInfo.log_path 经 IPC/注册表上报，`aproxy logs`/`status`/start 成功提示一律问实例拿真实路径，**不提供按端口拼路径的回退**（alpha 阶段无兼容承诺）。
3. **语义 B**：自定义日志位置 = config.toml `log_file` 字段 + CLI `--log-file`（仅本次），**有意不进 settings.json 全局默认层**（用户明确：日志去向是每实例语义，不许 settings 默认）。

**Why**：客户端拼接路径 = 重复实现命名规则且端口一变就错；实例自己是自身元数据的权威来源。
**How to apply**：任何「实例元数据由客户端推算」的设计都要改为经 IPC 上报（见 [[bounded-retry-paths]] 的配置化教训同源：不内置任何 URL/路径假设）。

## 关键实现决策

- **OnceLock 传递**：随机名在 main.rs 日志初始化时一次性生成写入 `RESOLVED_DAEMON_LOG`，serve_forever 的注册表/恢复记录/轮转共用同一份——再解析一次会生成不同的随机名。
- **前台实例 log_path 空串**：显式信号，`aproxy logs` 立即报「日志输出在它的控制台」（取代原 5 秒文件等待的间接探测）。
- **孤儿清理判据重构**（最大隐性代价）：文件名随机化后不再携带归属，判据改为「活实例 log_path ∪ .restore 记录的 log_path」之外的 .log 删除（startup.log 除外、非 .log 保留、空串不参与引用集、比较经 path_match_key 归一防 Windows 分隔符差异误删）。推论：旧端口命名日志升级后按孤儿清理；优雅停止的日志随清；崩溃日志由 .restore 引用保留到恢复成功。
- **.restore 文件格式改为结构体**（`{args, log_path}`）：**读侧宽容旧格式（纯 args 数组，log_path 空串）**——否则升级混版本窗口会把用户有效恢复记录当损坏删掉；写侧只写新格式。
- serde default 仅作混版本解析容错，`IPC_PROTO_VERSION` 不 bump（字段级兼容，与 last_activity_secs 先例一致）。

## 测试基建教训（同日）

install_flow_lib 的 `continue_from_*` 两个测试串行跑一次双双超时（151s）、单跑与复跑全过（46s）——**先做 stash 对照实验（HEAD 串行过 + 改动版复跑更快）再下结论**，不要把环境抖动误判为改动回归。daemon_test_port 的「探测-绑定」窗口使同候选集并行互踩（同日又实证一次），新测试取偏移必须查全部已用偏移。
