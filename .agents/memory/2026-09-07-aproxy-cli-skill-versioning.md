---
name: aproxy-cli-skill-versioning
description: 改 aproxy-cli skill 或发大版本之前读——latest 直改、大版本才留存的版本化策略，两次例外，以及持续更新义务
metadata:
  type: project
  scope: .claude/skills/aproxy-cli/ 的全部文档；每次发版
  status: active
  last_verified: 2026-10-13
---

# aproxy-cli skill 版本化文档策略

skill 位置：`.claude/skills/aproxy-cli/`（项目内、git 管理）。

## 目录结构与版本策略（2026-09-07 用户定调）

- 最新版文档在 `references/latest/`（commands / config-toml / settings-json / behaviors / clients /
  troubleshooting / compatibility 七个分文件 + SKILL.md 导航）。
- **小版本（0.1.x 内）直接修改 latest/ 下的文档**，不新建目录、不留存。
- **只有大版本更替**（如 0.1 → 0.2、alpha → stable）才把整个 `latest/` 复制为
  `references/<旧版本号>/` 留存，再重建 latest。目的：用户可能还在用旧版本，
  旧版行为文档必须可查。
- **每个留存版本一个 compatibility.md** 专门描述该版本大概兼容什么、与新版差异。
- SKILL.md 的导航路径 `references/latest/` 不随版本变化（留存目录从 compatibility
  链可达）。

**Why**：不是每个小版本都值得一份完整文档拷贝（膨胀且维护成本高）；但大版本
行为断层会让持有旧二进制的用户查到错误行为。折中即「latest 直改 + 大版本留存」。

**0.1.0 例外（2026-10-04）**：alpha.17 → 0.1.0 **不**整份留存 references。依据是
审查记录 `.agents/review/2026-10-04-0.1.0发布前-13维审查.md` 的决策 14 推荐 (b)：
0.1.0 相对 alpha.17 没有使旧文档误导用户的行为断层。用户 2026-10-04
答复「同意你的建议」，这一推荐随之生效。2026-10-06 起用户把 0.1.0 定为真正的第一版、
alpha 不再支持：compatibility.md 只讲版本判定与「0.1.0 vs 开发线」的差异表，alpha 逐版
差异已删（git 历史可查）；skill 全部改为英文并按 skill-creator 标准重写（AGENTS.md
project_conventions）。

**0.2.0 例外（计划 D-9，2026-10-11 主代理按推荐执行、已告知用户；2026-10-13 文档终审复核）**：0.1 → 0.2 也**不**留存
`references/0.1.0/`。理由：
- 0.1.0 用户装到的是随 0.1.0 发的那份 skill（脚本与 `install` 都取同 tag 的包）；
- 字面复制现在的 latest 会归档错内容：`v0.1.0` tag 里的 skill 是中文旧版，现在的 latest 是英文，并含 0.1.0 之后的行为；
- 0.1.x 不能原地升级，旧版文档只在迁移期有用。迁移期的错配由 compatibility.md 的「On 0.1.0」对照表加迁移与退回步骤覆盖。

留存规则本身不变：0.2 → 0.3 时照常判断有没有行为断层；真要留存，内容必须取自对应 tag（`git show v<版本>:`），不能复制当时的 latest。

## 持续更新义务

**How to apply**：后续任何改变 CLI 用法、配置字段、默认值、行为语义、错误文案
的代码改动，同一提交（或紧随其后的提交）必须同步更新 `references/latest/` 对应
文档；行为断层的大版本发布前执行「复制 latest → references/<版本>/」留存动作（例外见上）。
skill 与代码不一致 = 文档缺陷，与测试红灯同级对待。

**Evidence:** 用户 2026-09-07 定调与 2026-10-04 答复（见上）；计划 `.agents/plan/release-0.2.0.md` 的 D-9；
2026-10-13 文档终审对 compatibility.md「On 0.1.0」对照表的核对（对 HEAD 代码）。

**Recheck when:** 用户改变版本化策略；0.2 → 0.3 发版前。

相关记忆：[[release-engineering]]（正式发布流程）。
