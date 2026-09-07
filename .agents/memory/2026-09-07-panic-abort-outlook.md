---
name: panic-abort-outlook
description: abort 未来展望（遥遥无期）：实测省 32% 体积，待代码足够安全成熟且需更多优势时才考虑，须配看门狗
metadata:
  type: project
---

# panic=abort（遥遥无期的展望，暂不采用）

用户定调（2026-09-07）：**等本项目代码足够安全、足够成熟，且需要追求更多优势时，可以考虑 abort**——明确区别于指令集多版本发布（那是正式发布必然项，见 [[release-engineering]]）。

实测数据（2026-09-07，rustc 1.96.0，fat LTO + opt-level=z + strip）：
- unwind（现行）：4.06 MB
- abort：2.77 MB（再省 32%，磁盘占用而非运行性能）

不采用的核心理由：axum/hyper 每连接是独立 tokio 任务，unwind 保证单连接 panic 只断该连接、实例继续服务；abort 把任何一处 bug 的爆炸半径放大为整个实例死亡。若未来采用，**必须配套看门狗自动重启**（重启窗口期全连接断流仍劣于连接级隔离，见 TODO G2），且前提是测试覆盖与代码成熟度足以压低 panic 源。

相关：[[daemon-model]] [[warning-zero-policy]]
