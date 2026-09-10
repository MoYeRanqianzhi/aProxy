---
name: unix-stress-review
description: unix 分支压力实测审查全记录（2026-09-10）：S1-S6 修复+复验+R1/R2 关闭+纯净性核查（越界功能稿完整封装于 stash，HEAD 零残留）；分支可合并，合并窗口待解 TODO 冲突
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

**修复审查轮独立复验（同日，审查窗口执行）**：4 个修复提交与归档方向逐项对应；
核心探针独立复验 P1 9/9（S4 关卡不挡处决）、P6 5/5（S1）、P7 5/5 前任被真杀+
claim 单一 pid（S2）。

**R1/R2 已关闭（75afe6b）**：registry_contains_pid_in 四分支单测 +
is_aproxy_process 真实 zombie/ENOENT 判死单测 + identity_check_tolerates_swapped_binary
集成测试（unix）+ p7 探针改三条硬断言（接管即杀前任 SIGCONT 不可恢复/接管者在任/
claim 单一 pid）。**实测纠正假设**：rename 原子覆盖原路径才产生 exe「 (deleted)」
后缀；mv 走开则 exe 跟随新路径名（basename 变化判异己，属 F2 边界）。

**纯净性审查轮（同日末轮，用户报告修复窗口曾越界开始功能修改后回滚）**：

- **stash@{0} `name-free-identity-refactor-draft(未获确认的方向稿)`**（基线
  f5e3a4f，约 60% 完成）：「时间戳身份」功能重构稿——InstanceInfo.process_start
  新字段（注册表/IPC 载荷变更）、force_terminate API 签名变更、删除
  is_aproxy_process 整函数、adopt_scan 收养改 IPC 应答制、Watched.start_time。
  **背景（见 identity-no-name）：用户已定调镜像名比对是严重谬误，本稿是定调后
  正确方向的实现尝试**；方向稿本身因「批评 ≠ 授权」被回滚（见
  criticism-not-authorization），方案 A（本分支全换）/ B（独立成支）待用户
  拍板。保留不 drop。
- **HEAD 零残留验证法**：精确 grep 特征符号（字段定义/新 API 名/新结构体字段）
  全 0 命中；force_terminate 原签名、is_aproxy_process 健在；amend（980602b→
  75afe6b）树完全相同仅消息调整；3 个 dangling commits 是 2026-08-30 master
  旧 stash 残骸，与本分支无关。
- **最终复验（HEAD 含 75afe6b）**：Windows fmt/clippy/lib 115/集成 36+2 全绿；
  remote fmt/clippy/lib 114/**集成 37**（含 swap 新测试）/restart 2 全绿；p7
  新断言 **7/7**。
- **分支状态定论**：S1-S5 修复与测试本身纯净可保留（S3/S4 的名称实装是待替换
  的过渡态，方案 A/B 决定替换节奏）；分支可合并，合并窗口唯一待办：
  .agents/TODO.md 与 master 的内容冲突（机械冲突保留双方）。

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
   claim 心跳过期不是 3×scan_secs（与 scan 无关，固定 90s）。
6. **共享服务器多会话教训**：并行 cargo 会话共用 target/ 会产生假编译错误
   （proxy_integration 曾报编译失败但同代码干净重跑即过，37/37）——**遇编译
   错误先查并行会话与构建锁，再怀疑代码**；grep 过滤输出会吞掉错误详情，
   复算时须抓全量输出。

相关：[[unix-first-test]] [[daemon-model]] [[review-findings]] [[protect-production-instances]]
