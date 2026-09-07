---
name: future-optimizations
description: 未来优化展望（2026-09-07 用户定调）：panic=abort 的重启条件与实测数据；target-cpu 指令集多版本发布
metadata:
  type: project
---

# panic=abort（未来展望，暂不采用）

用户定调：**等本项目代码足够安全、足够成熟，且需要追求更多优势时，可以考虑 abort**。

实测数据（2026-09-07，rustc 1.96.0，fat LTO + opt-level=z + strip）：
- unwind（现行）：4.06 MB
- abort：2.77 MB（再省 32%，磁盘占用而非运行性能）

不采用的核心理由：axum/hyper 每连接是独立 tokio 任务，unwind 保证单连接 panic 只断该连接、实例继续服务；abort 会把任何一处 bug 的爆炸半径放大为整个实例死亡。若未来采用，**必须配套看门狗自动重启**（重启窗口期全连接断流仍劣于连接级隔离），且前提是测试覆盖与代码成熟度足以压低 panic 源。

# target-cpu 指令集多版本发布

正式发布时 CI 矩阵出双版本：baseline（无 target-cpu，可分发）+ `x86-64-v3`（AVX2，2013 年后主流 CPU，RUSTFLAGS="-C target-cpu=x86-64-v3"，产物命名区分）。本代理 IO 密集，收益预期有限，作为免费附赠而非性能故事；正式发布前先建能压满 aproxy 的基准（wrk/bombardier + 高性能 mock）。

相关：[[daemon-model]] [[warning-zero-policy]]
