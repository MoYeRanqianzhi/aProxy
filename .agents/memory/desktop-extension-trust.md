---
name: desktop-extension-trust
description: How aProxy treats the risk of theme and extension packages - recall before designing anything about installing, loading or permissions of third-party packages
metadata:
  type: feedback
  scope: Desktop theme packages and extension packages, from 0.2.0 on
  status: active
  last_verified: 2026-11-23
---

不给扩展包和主题包做沙箱、隔离或逐项写授权。安全上要做的是：安装前用弹窗把风险和可信度事实讲清楚，每条风险一个弹窗，
要点「确认」才继续。

**Why:** 用户 2026-11-23 裁定：安装第三方包的风险属于社会工程学，技术手段挡不住；之前那套 iframe 沙箱、租约、逐项授权与
审计（t10 的第一轮和 Revision 3）既重又费，挡不住真正的风险，还让「主题」退化成粘贴一段字符串。扩展包要有，但只需要
安装入口和接口暴露。

**How to apply:**
- 包里的 `manifest.json` 带 `repository`。不填即闭源，要额外提示闭源的风险。
- 开源的经 GitHub API 取 star 等可信度数据，并用 release asset 的 sha256 摘要校验安装文件。哈希没过、star 低、
  API 连不上，各自额外弹窗。
- 扩展直接在主窗口运行，拿到的 `api` 与界面能做的一样，写操作不再另外授权。
- 只保留把文件装对所需的检查（不写出目标目录、大小上限），不要把它扩成隔离方案。
- 有人提议给扩展加隔离或授权时，先拿这条去问用户，不要自己加回来。

**Evidence:** 用户 2026-11-23 的原话（本会话）。设计写在 [w5e-extensions](../plan/w5e-extensions.md) 顶部那一节。

**Recheck when:** 用户改变对扩展风险的看法，或扩展能拿到的能力超出窗口本身能做的事（例如直接执行本机程序）。
