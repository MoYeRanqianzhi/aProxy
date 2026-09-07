---
name: release-engineering
description: 发布工程铁则：正式发布起 GitHub Actions 必须构建 target-cpu 指令集多版本（baseline + x86-64-v3）
metadata:
  type: project
---

# target-cpu 指令集多版本发布（正式发布必然项，非展望）

用户定调（2026-09-07）：**等正式发布开始，就需要 GitHub 构建多版本**——这是发布工程的必做项，不是可选项。区别于 abort（[[panic-abort-outlook]]，遥遥无期的或然展望）。

执行要点：
- CI 矩阵：baseline（不设 target-cpu，产物分发到任何 x86_64 机器）+ `x86-64-v3`（AVX2，约 2013 年后主流 CPU）
- v3 版构建：`RUSTFLAGS="-C target-cpu=x86-64-v3"`，产物命名区分（如 `aproxy-x86_64-v3.exe`），发布页两者都放
- 不用 `native`：本机构建产物分发会让他机非法指令崩溃
- 预期管理：本代理 IO 密集，指令集收益可能有限——发布时顺带出基准数据如实呈现

相关：[[daemon-model]]
