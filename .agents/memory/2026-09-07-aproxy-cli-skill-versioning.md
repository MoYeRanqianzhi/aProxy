---
name: aproxy-cli-skill-versioning
description: aproxy-cli skill 的版本化文档策略与持续更新义务——latest 直改、大版本才留存
metadata:
  type: project
---

# aproxy-cli skill 版本化文档策略

skill 位置：`.claude/skills/aproxy-cli/`（项目内、git 管理）。

## 目录结构与版本策略（2026-09-07 用户定调）

- 最新版文档在 `references/latest/`（commands / config-toml / settings-json /
  behaviors / compatibility 五个分文件 + SKILL.md 导航）。
- **小版本（0.1.x 内）直接修改 latest/ 下的文档**，不新建目录、不留存。
- **只有大版本更替**（如 0.1 → 0.2、alpha → stable）才把整个 `latest/` 复制为
  `references/<旧版本号>/` 留存，再重建 latest。目的：用户可能还在用旧版本，
  旧版行为文档必须可查。
- **每个留存版本一个 compatibility.md** 专门描述该版本大概兼容什么、与新版差异。
- SKILL.md 的导航路径 `references/latest/` 不随版本变化（留存目录从 compatibility
  链可达）。

**Why**：不是每个小版本都值得一份完整文档拷贝（膨胀且维护成本高）；但大版本
行为断层会让持有旧二进制的用户查到错误行为。折中即「latest 直改 + 大版本留存」。

## 持续更新义务

**How to apply**：后续任何改变 CLI 用法、配置字段、默认值、行为语义、错误文案
的代码改动，同一提交（或紧随其后的提交）必须同步更新 `references/latest/` 对应
文档；行为断层的大版本发布前执行「复制 latest → references/<版本>/」留存动作。
skill 与代码不一致 = 文档缺陷，与测试红灯同级对待。

相关记忆：[[release-engineering]]（正式发布流程）。
