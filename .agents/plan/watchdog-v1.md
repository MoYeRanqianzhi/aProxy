# G2 看门狗 v1 实现计划

> 状态：已批准方向（用户 2026-09-08 确认出计划）。执行时按步提交，每步全绿。
> 设计讨论结论全部沉淀于此，实现以本文件为准。

## 目标与边界

使命对齐：「绝对不间断」缺失一环——unwind 管连接级隔离（现有），看门狗兜底
进程级死亡与 runtime 挂死。**v1 不含**：scoped runtime 注入（挂死原地救，
spike 后再议）、unix 适配（架构兼容但只在 Windows 实测）。

## 架构总览

- **全局单看护进程**（非每实例监督者）：`aproxy watchdog` 隐藏子命令，同二进制
  分离进程，看护全部实例。内存 ~2-3MB 与实例数无关。
- **等待全部下沉内核**：
  - 进程死亡：`OpenProcess(PROCESS_SYNCHRONIZE)` 句柄 → win32 线程池
    `RegisterWaitForSingleObject`，零等待线程，signal 即时（秒级）。
  - 挂死：每实例一个命名共享内存节（`CreateFileMappingW`，8 字节心跳时间戳），
    守护热路径响应回放完成后一条 `Relaxed store`；看护单一 tokio 任务 30s
    `wait_timeout` 醒来扫全表。超时不直接杀——先 IPC ping 二意见，两路独立
    信号都失败才判挂死。
- **选举规范**（防多看护者并存）：排序定发起者 + 原子 claim 定在任者
  - 真相源 `run/watchdog.claim`：`{看护者 PID, 进程创建时间, 心跳时间戳}`
  - 在任判定四条件：claim 存在 && PID 探活 && GetProcessTimes 创建时间匹配
    （防 PID 复用）&& 心跳新鲜（<90s，兼测假死）
  - 拉起权：发现缺席 → 存活实例集合中 **PID 最小**的守护才有权 spawn
  - 上任裁决：看护启动时 `CREATE_NEW` 原子接管 claim，失败者自查退出
- **互保环**：守护每 5 分钟自检（claim 判定 + ping `aproxy-watchdog` 管道），
  看护缺席/假死 → 走选举补种。看护者崩溃实例不死（解耦），仅丢失后续保护。
- **崩溃重拉**：`.restore` 残留 = 异常死亡信号（现有）；`spawn_detached` +
  restore 参数 + IPC ping 就绪判定（现有）+ 退避状态机。
- **crashloop 防护**：1s→2s→4s…（封顶 300s）指数退避；连续 5 次失败放弃，
  大声写 startup.log，保留 `.restore` 等人工 restore。
- **生命周期**：`aproxy start` 首次发现无在任看护者 → 走选举 spawn；全部实例
  清零后看护者闲置 5 分钟自灭；`stop all` 后系统干净。
- **诚实边界（写进文档）**：进程级死亡的自愈空窗 = 在途请求全灭（连接断）
  + ~1s 重拉；看门狗救的是「之后永久断流」不是在途请求。

## settings.json 配置项（watchdog 分层 = settings 全局层，无 toml 覆盖）

看门狗是**系统级单例**（一个看护进程看护全部实例），不像 disk_cache/max_body_mb
是 per-实例行为——因此只在 settings.json 配置，config.toml 不参与（避免多份
toml 对同一看护者语义打架）。

| 字段 | 类型 | 默认 | 语义 |
|---|---|---|---|
| `watchdog` | bool | **true** | 看门狗总开关：false 时 `aproxy start` 不 spawn 看护者，现有守护在任的看护者正常退出（清 claim），整体回到现状行为。已运行实例不受影响（解耦） |
| `watchdog_heartbeat_secs` | u64 | 30 | 心跳扫描周期/新鲜度判定基准（新鲜 = 3×周期）。仅调优用，一般不动 |
| `watchdog_stale_after_cycles` | u64 | 1 | 连续几轮心跳未更新 + ping 失败才判挂死（1 = 一轮超时即二意见）。挂死误杀的调节阀 |
| `watchdog_max_restarts` | u32 | 5 | crashloop 上限：连续失败达此值放弃该实例（退避封顶 300s），保留 .restore 等人工 |
| `watchdog_idle_exit_secs` | u64 | 300 | 全部实例清零后看护者闲置自灭的等待秒数；0 = 永不自灭（常驻） |

默认值原则：开箱即用全保护；调优项都给保守默认，不需要用户理解也能正确工作。
`aproxy doctor` 收录四项的越界检查（如 heartbeat=0）。

## 模块划分

- `src/watchdog.rs`（lib）：选举/claim/共享内存心跳/退避状态机——纯逻辑 +
  可注入测试（时间与进程探活抽象为 trait）
- `src/commands/watchdog.rs`：`aproxy watchdog` 隐藏子命令入口（serve 循环）
- `src/daemon.rs`：新增 claim/pid 文件与 win32 线程池等待封装
  （windows-sys 已有依赖，加 `Win32_System_Threading` 相关 feature）
- `src/server.rs`：守护侧挂载——心跳节创建、热路径 store、5 分钟自检任务
- `src/settings.rs`：五字段 + Default + roundtrip 测试
- `src/main.rs`/`src/cli.rs`：子命令注册
- skill：`references/latest/` 增补 behaviors.md 看门狗节 + settings-json.md 五字段
  （遵循 skill-versioning 记忆：小版本直改 latest）

## 分步提交（每步：实现 + 测试 + clippy 零警告 + fmt + commit）

1. **settings 五字段** + doctor 检查（纯配置层，先行合入不激活）
2. **claim 选举原语**：`watchdog.claim` 读写、在任判定、CREATE_NEW 原子接管、
   PID 复用防冒名——单测覆盖边界表全部场景（模拟 PID 复用/心跳过期/竞态接管）
3. **共享内存心跳**：守护侧节创建 + store；看护侧读表扫描。Windows API 封装
   + 集成测试（测试端口派生规则）
4. **`aproxy watchdog` 主循环**：收养现有实例（OpenProcess + 线程池等待）、
   死亡→按 `.restore` 重拉 + IPC 就绪判定、退避状态机、闲置自灭
5. **守护侧互保**：5 分钟自检 + 选举补种挂载进 serve_forever；
   `start` 首启触发出簇；`watchdog=false` 全链路关闭语义
6. **收尾**：skill 文档增补、README、architecture.md 看门狗节、bump alpha.6 + tag

## 测试矩阵（边界表全量化）

- 选举：无 claim / claim 残留 / PID 复用 / 心跳过期 / 并发接管（多线程竞态测试）
- 死亡检测：kill -9 等价（taskkill /F）→ 秒级检出 → 重拉 → ping 就绪
- 优雅 stop 不复活：stop → .restore 删 → 看护不重拉
- crashloop：毒配置实例连续失败 → 退避 → 5 次后放弃 + startup.log 记录
- 互保：taskkill 看护者 → 守护 5 分钟内（测试可缩周期）补种，且仅一个新看护者
- 配置：watchdog=false 全链路无看护者；默认值 roundtrip
- 全程遵守：测试端口派生、绝不触碰生产实例（12345/12349）、DaemonGuard 清理
