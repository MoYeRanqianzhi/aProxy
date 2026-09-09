---
name: unix-stress-review
description: unix 分支压力实测审查（2026-09-10 第二轮）：respawn 就绪判定竞态（list_instances_in 清理副作用误杀同注册表死实例）、refresh_claim 无条件覆写无夺权检测、多端差异清单与压力探针资产
metadata:
  type: project
---

# unix 分支压力实测审查（2026-09-10 第二轮）

审查+实测 `fix/unix-first-test`（0b53a10）：逐行 diff 审查、Windows/unix 语义对照、
Ubuntu 实机全量测试（全绿）+ 10 项压力场景。问题归档：`.agents/docs/unix-stress-review.md`
（S1-S6 完整根因/证据/修复方向）。探针资产 `.agents/bench/probe/`。

关键事实：

1. **S1 产品级竞态（待修，最高优先）**：respawn_instance 的就绪判定复用
   list_instances_in（ping 失败即删注册文件）→ 多实例同时崩溃时，先 respawn 者的
   就绪等待顺带删掉其余死实例的 .pid → 它们的死亡事件被误判优雅退出 → 永不
   respawn。P6 死亡风暴实机复现（3 杀只活 1）。两平台同病。修复：只读 pid 检索 +
   handle_death 以 .restore 为唯一优雅退出依据。
2. **S2 claim 覆写竞争**：refresh_claim 无条件覆写、无运行期夺权检测——P7 实测
   双看护者共存、claim pid 10s 内翻转。unix terminate_verified 是 no-op（假死前任
   不被杀）。claim 心跳过期阈值=3×watchdog_heartbeat_secs=90s（与 scan_secs 无关）。
3. **挂死处决全链路实机通过**：SIGSTOP → 判定 → SIGKILL → respawn 11s → 重新收养
   （本分支实装的 unix watchdog 核心价值验证成立）。
4. **重试语义实测全过**：50 并发 502 窗口 50/50 恢复；conn refused 10/10；90s 长挂
   不断流；重试风暴期健康实例 p50=17ms 零失败（隔离性成立）。
5. **多端差异（低危知悉）**：unix 会收养死条目自动 respawn（is_aproxy_process 占位
   恒 true，Windows 拒收）；/dev/shm 心跳文件与 UDS socket 死后残留；macOS 回退
   kill(pid,0) 对 zombie 失效。
6. **探针教训**：注册表 .pid 是 pretty JSON、claim 是单行 JSON——pid 提取须
   `grep -oE '"pid": ?[0-9]+'`；bash 编排 wait 会等常驻 job，spawn 后须 disown；
   claim 心跳过期不是 3×scan_secs（与 scan 无关，固定 90s）。

相关：[[unix-first-test]] [[daemon-model]] [[review-findings]]
