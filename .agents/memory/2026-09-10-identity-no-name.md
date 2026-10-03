---
name: identity-no-name
description: 用户定调：进程身份判定严禁依赖二进制名称（严重谬误）——正确锚点是 spawn 链/IPC 端点归属/进程创建时间戳；2026-10-04 已按方案 A 实施（pid + 创建时间），改身份/收养/处决逻辑前回想
metadata:
  type: project
---

# 进程身份判定与名称解耦（2026-09-10 用户定调；2026-10-04 方案 A 已实施）

**现状（2026-10-04，合并提交 5c71f35）**：`is_aproxy_process` 已删除。守护注册时把自身进程创建时间写进
`InstanceInfo.process_start`；`watchdog::record_identity` 按「pid + 创建时间」判定
Alive / Unverifiable / Reused / Gone；收养、处决、`--force`、install 换血停旧看护者、选举存活判定
全部改用它，Windows 终止在同一进程句柄上先核对创建时间再 TerminateProcess（无复用窗口）。
旧记录（process_start = 0）一律保守：宁可不看护也不误杀（收养需 IPC 回报 pid 一致、选举不计入、
`--force` 先 ping 确认）。官方 Release 资产名 aproxy-<target>(.exe) 直接运行也受看护。
主仓库 stash@{0} 的方向稿已被这次实现取代，是否丢弃由用户决定。下文为当初定调的推理，保留供回溯。

**用户原话定性**：「怎么能通过名称来判断呢？？这是严重谬误！！」（针对
`is_aproxy_process` 的镜像名/exe basename 比对，以及我在 F2 决策里给出的
两个选项——它们都建立在名称判断前提上，同样被否）。

## 为什么名称判断是谬误

进程身份问的是「这个 pid 是不是我放在这个端口的守护实例」，由归属关系
决定，与二进制叫什么无关。名称比对两头皆错：改名合法守护被排除（生产
真实存在 `aproxy-using.exe` 形态），冒名进程改名即绕过——对安全零贡献、
对正确性纯伤害。Windows 侧 merge-base 前已有此语义不能作为合理化依据
（我曾说「方向正确」，是没理解项目的表现）。

## 正确锚点（项目里已有，无需名称）

1. **spawn 链**：respawn 的守护是看护者亲手拉起，pid 直接可信
2. **IPC 端点归属**：应答 `<port>` 端点 ping 协议且回报 pid 一致的进程
   就是该端口的实例——协议应答是最强归属证明
3. **进程创建时间戳**：claim 文件已在用（`created_at_process` 比对）；
   收养时记录一次即可绑定「同一个进程」，pid 复用必不匹配。
   注意选举场景不能用 IPC ping 判死（挂死实例恰恰需要看护，registry_
   pids_in 注释已声明），时间戳比对是选举的正确存活判定

## 当前状态（2026-09-10）

- 工作区已回滚到 `f5e3a4f`（R1/R2 修复完整保留），HEAD 未动
- 未获确认的重构稿在 stash：`stash@{0}`（4627432，message
  「name-free-identity-refactor-draft(未获确认的方向稿)」，约 60% 完成：
  InstanceInfo.process_start 字段 + --force/选举/收养/处决/claim 五处
  调用点替换）——方向未批准前不得恢复使用
- **待用户拍板的范围决策**：
  A. 本分支做完整替换（Windows 行为随之变化，偏离「零变化」承诺，合并
     说明需明示）
  B. 本分支只撤 unix 侧名称实装（回退到已有原语），机制替换独立成支评审
- F2 的旧选项（「接受改名不受看护」/「放宽比对」）全部作废——两者都以
  名称判断为前提

相关：[[unix-first-test]] [[unix-stress-review]] [[criticism-not-authorization]]
