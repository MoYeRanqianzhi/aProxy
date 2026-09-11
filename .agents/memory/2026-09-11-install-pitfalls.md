# 2026-09-11 install 实测深坑集（滚动重启轮）

四个测试驱动逐层挖出的坑，前两个已入 commit message，这里补全通用教训：

1. **tokio runtime drop 等待无限任务（交棒进程全体挂死，泄漏 27 个）**：
   `#[tokio::main]` 正常 return 后 runtime drop 会等所有 spawned 任务完成——
   持有无限 loop ticker（宣告心跳）的进程永远退不出。**凡是「本进程到此
   结束」的路径必须 `std::process::exit()` 硬退**，不能依赖 main 返回；
   或者保证所有 spawn 的任务都被 abort。识别信号：进程无监听、无日志、
   活着不干活。

2. **IPC 消失 ≠ 进程终止**：守护 shutdown 先关 IPC 再清理最后退出，
   「ping 失败」只能证明服务停止。等进程终止要用退出码
   （`GetExitCodeProcess != STILL_ACTIVE`，新原语 `watchdog::process_exited`）；
   `process_start_time(pid).is_none()` 在进程对象被任何句柄（Rust Child、
   看护 sync handle）维持时**永远误报存活**。另外终止等待预算必须覆盖
   守护的 10s 宽限强退（server.rs select 双臂）。

3. **spawn_detached 只继承父进程环境**：库层测试在测试进程内直接调
   flow 函数时，spawn 出的隔代守护继承的是**测试进程** env——仅给直接
   子进程 `cmd.env` 注入 APROXY_HOME 管不到它，新守护会把注册表写进
   真实 `~/.aproxy/run`。库层测试必须 `unsafe { env::set_var }` 进程级
   注入（cargo test 每 target 独立进程，单测试文件内安全）。

4. **测试基建三坑**：固定端口会撞 WinNAT 排除区间（bind 报「无权限或
   被系统保留」）——一律 bind 试探选端口；「等状态文件消失」有启动竞态
   （还没创建 ≠ 已完成）——两段式先等出现再等消失；守护优雅退出会删
   `.restore`——restart 必须**先取恢复参数再 stop**。

关联：[[never-kill-aproxy-by-name]]（本轮测试泄漏 33 个进程曾致系统
内存不足，全部按 pid 精确清理，生产实例分毫不碰）
