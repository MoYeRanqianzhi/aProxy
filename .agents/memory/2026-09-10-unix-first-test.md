---
name: unix-first-test
description: unix 分支首次实机测试（2026-09-09 Ubuntu）：看门狗曾整体失效、extern 符号名写错两处 P0；mock 脚本 Content-Length 错标误导排查一小时；APROXY_RUN_DIR 平台语义差异
metadata:
  type: project
---

# unix 分支首次实机测试（2026-09-09~10）

ssh remote（Ubuntu 24.04 VPS）实测 v0.1.0-alpha.6。完整报告：
`.agents/docs/unix-testing.md`。修复分支 `fix/unix-first-test`
（worktree `.worktree/unix-fixes`），待其他窗口合并。

关键事实：

1. **unix 看门狗曾整体失效**：`open_sync_handle` unix 占位返 None →
   adopt_scan 收养不进任何实例 → 死亡检测/respawn 全链路死。已按设计
   意图实装为轮询退化（句柄=pid，/proc stat 排 Z，terminate=SIGKILL）。
2. **裸 extern 声明符号名**：`libc_kill` 不存在，libc 符号是 `kill`——
   libc crate 命名习惯带进 extern "C" 块的典型错误，Windows cfg 屏蔽下
   不可见。
3. **测试平台假设三处**：大小写去重断言（Windows-only 语义）、
   `ps -p` 对 zombie 返回成功（改 `ps -o stat=`）、taskkill 无 cfg。
4. **APROXY_RUN_DIR 平台差异（未修，待决策）**：unix UDS 路径在 run_dir
   内 → 隔离实例只能被同环境变量的调用方管理；Windows 管道名全局唯一。
   新增 pub `ipc_request_to(endpoint)` 供显式寻址。
5. **mock 教训（重罚级）**：手写 `Content-Length: 21` 但 body 20 字节 →
   hyper 解码污染 → aproxy 502 路径「卡死」假象，排查约一小时。mock 的
   Content-Length 必须程序化生成。同类：python http.server 无 NODELAY
   造成 42ms Nagle 假延迟，压测数据须先核 mock 质量。
6. 性能基线（NODELAY mock）：c8 经代理 ~2300 RPS（开销 ~8-19%），
   c1 转发开销 ~0.8ms；浸泡 5 分钟 RSS 恒定 79.5MB；valgrind 0 leak；
   Linux release 3.97MiB（Windows 4.06MiB 同量级）。

相关：[[daemon-model]] [[review-findings]] [[warning-zero-policy]]
