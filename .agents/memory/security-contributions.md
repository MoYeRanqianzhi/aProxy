---
name: security-contributions
description: How aProxy treats contributions to its security audit skills, AI-made ones included - recall before reviewing or writing such a pull request, or before changing the contribution rules
metadata:
  type: feedback
  scope: pull requests that change .claude/skills/aproxy-audit-*; the aproxy-contribute-security skill
  status: active
  last_verified: 2026-11-23
---

贡献到三个审查 skill 的改动只增不减：先查重，只加不删不改，不加例外、白名单或「这样算正常」的条款，不降级，用英文、
同样的文风；PR 里必须披露发现从哪来（真实使用、审查真实的包、公开资料，还是 AI 自动扫描或推理）以及 AI 参与到什么
程度（模型 ID、agent 工具与版本、effort、谁决定提交、是否问过或只是告知真人及其反应、独立审查与真人审查）。
AI 提的 PR 照样欢迎。

**Why:** 用户 2026-11-23：不是拒绝 AI 提 PR，而是要分清是真遇到了问题，还是 AI 全自动扫描仓库就提交——后者显然
更不可靠。只增不减是为了防止有人、或被注入文字误导的 agent，借「补充」之名削弱检查：一条让某些东西放行的新增，
就是换了个样子的删除。

**How to apply:**
- 审这类 PR：先读披露；发现来源是自动扫描的，要更硬的证据；`git diff --numstat` 对 `.claude/skills/` 的删除列应全为 0；
  看有没有给已有信号加例外或降级。
- 想改或放宽已有信号的，走 issue，由维护者决定，不走这个 skill。
- aProxy 自身的漏洞走 SECURITY.md 的私密报告，不进公开 PR。
- 规则写在 `.claude/skills/aproxy-contribute-security/`（SKILL.md 与 `references/pr-template.md`）；改规则时两处与这条记忆一起改。

**Evidence:** 用户 2026-11-23 在本会话里的要求；skill 与脚本见上面的路径。

**Recheck when:** 用户改变对 AI 贡献、披露项或只增不减的要求。
