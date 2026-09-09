---
name: unix-stress-review
description: unix 分支压力实测审查（2026-09-10 第二轮）：S1-S6 六项缺陷已全部修复并复跑实证（p6_storm 3/5→5 全过、p7 接管即杀前任）；探针资产与探针编写教训
metadata:
  type: project
---

# unix 分支压力实测审查（2026-09-10 第二轮）

审查+实测 `fix/unix-first-test`（0b53a10）：逐行 diff 审查、Windows/unix 语义对照、
Ubuntu 实机全量测试（全绿）+ 10 项压力场景。问题归档：`.agents/docs/unix-stress-review.md`
（S1-S6 完整根因/证据/修复方向 + 修复记录节）。探针资产 `.agents/bench/probe/`。

**S1-S6 已全部修复（2026-09-10 修复轮）**：a389d40（S1 respawn 就绪判定只读化 +
handle_death 以 .restore 为唯一判据）、01978b4（S2 refresh_claim 归属校验 +
unix terminate_verified 实装 SIGKILL）、f13bc0a（S3 is_aproxy_process /proc exe
实装 + S4 处决防误杀关卡）、9cc53c8（S5 心跳/socket 退出清理 + S6 macOS 回退
缺陷声明）。复跑实证：p6_storm 修复前 3/5 → PASS=5 FAIL=0；p7_takeover claim
单一 pid、前任被真杀。双平台全量测试全绿。

关键事实：

1. **S1 根因（已修）**：respawn_instance 就绪判定复用 list_instances_in
   （ping 失败即删注册文件）→ 多实例同时崩溃时先 respawn 者的就绪等待顺带删掉
   其余死实例的 .pid → 死亡事件被误判优雅退出 → 永不 respawn。P6 死亡风暴
   实机复现（3 杀只活 1）。两平台同病。
2. **S2 根因（已修）**：refresh_claim 无条件覆写无夺权检测——P7 实测双看护者
   共存、claim pid 10s 内翻转。claim 心跳过期阈值=3×watchdog_heartbeat_secs=90s
   （与 scan_secs 无关）。
3. **挂死处决全链路实机通过**：SIGSTOP → 判定 → SIGKILL → respawn 11s → 重新收养
   （本分支实装的 unix watchdog 核心价值验证成立）。
4. **重试语义实测全过**：50 并发 502 窗口 50/50 恢复；conn refused 10/10；90s 长挂
   不断流；重试风暴期健康实例 p50=17ms 零失败（隔离性成立）。
5. **探针教训**：注册表 .pid 是 pretty JSON、claim 是单行 JSON——pid 提取须
   `grep -oE '"pid": ?[0-9]+'`；bash 编排 wait 会等常驻 job，spawn 后须 disown；
   claim 心跳过期不是 3×scan_secs（与 scan 无关，固定 90s）；p7 探针里「unix
   no-op 未杀」文案按旧行为写，修复后行为已变，探针文案未随行为更新。

相关：[[unix-first-test]] [[daemon-model]] [[review-findings]]
