---
name: github-release-settings
description: 改发版 workflow、新增 tag 格式、改分支策略或排查「发布 job 被环境拦下 / push 被拒」时回想——仓库的发布安全设置现状与取舍
metadata:
  type: reference
  scope: GitHub 仓库 MoYeRanqianzhi/aProxy 的仓库级设置（不在代码里，git 历史看不到）
  status: active
  last_verified: 2026-10-04
---

# GitHub 仓库发布安全设置（2026-10-04）

用户 2026-10-04 授权「全都由你来决定和操作」后由主代理配置，`gh api` 回读核实：

- **私密漏洞报告：已开启**（`private-vulnerability-reporting` → enabled:true）。
  SECURITY.md 的报告入口依赖它。
- **release 环境的部署限制：只允许两类 tag**，即 `v*` 与 `format-v*`（type=tag 的
  deployment branch policy），没有 required reviewers。效果：只有发版 tag 触发的
  run 才能进入 `environment: release` 的 job（publish-npm、publish-crates 等持有
  OIDC 发布权限的 job），分支上的 workflow 进不来。
  **新增发版 tag 格式时必须同步加一条 policy**，否则发布 job 会被环境直接拒绝。
- **main 分支保护：禁止强推、禁止删除，管理员同样受约束**（enforce_admins）。
  不要求 PR、不要求状态检查，发版提交仍可直接 push 到 main。改为 PR 流程时，要先确认
  发版步骤（版本号提交）不会被挡住。
- **`CARGO_REGISTRY_TOKEN` 仍在仓库级**，没有迁入 release 环境：迁移需要密钥原值，
  而 GitHub 不允许读回；删除则会去掉 OIDC 失效时的兜底（workflow 写的是
  `steps.crates-auth.outputs.token || secrets.CARGO_REGISTRY_TOKEN`）。现状下 fork
  PR 拿不到仓库 secret，风险可接受。若所有者重新签发 token，应直接建为 release
  环境的 secret 并删除仓库级那份。

**Evidence**：2026-10-04 `gh api repos/MoYeRanqianzhi/aProxy/{private-vulnerability-reporting,
environments/release,branches/main/protection}` 回读结果与上文一致。

**Recheck when**：新增或改名发版 workflow / tag 格式；改为 PR 合并流程；所有者轮换
crates.io token；GitHub 调整环境部署策略的语义。

相关：[[trusted-publishing-p0]] [[release-engineering]] [[ci-unix-blindspot]]
