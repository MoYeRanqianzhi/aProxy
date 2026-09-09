# install / upgrade（二进制安装与升级）v1 实现计划

> 状态：已批准方向（用户 2026-09-09 确认：渠道全部配置零舍弃、项目将开源、
> 命名定为 install 且 upgrade 为其变体）。执行时按步提交，每步全绿。
> 设计讨论结论全部沉淀于此，实现以本文件为准。

## 目标与边界

使命对齐：「绝对不间断」在**升级场景**的缺失一环——替换二进制期间服务不中断、
中断点（崩溃/断电/关机/强杀）不留烂摊子。

- `aproxy install [版本|latest|--from <路径>]`：把目标二进制安全落位到规范
  位置 `~/.aproxy/bin/aproxy.exe`，全程对客户端表现 ≈ 无感（逐实例滚动，
  任一时刻至多一个实例在重启）。
- `aproxy upgrade` = `install latest` 的别名（可发现性；二者共存）。
- **非目标**：不管理包管理器管辖目录（scoop/npm/cargo 自身目录）的升级；
  不做自动定时升级（用户显式触发）；不下载依赖之外的任何东西。

## 四条铁律（顺序不可错，用户定调）

1. **先标记**：install 第一件事写状态文件（含目标版本与阶段）——此后任何
   时刻崩溃/断电，下一次启动都能识别「正在 install」并恢复。
2. **先下载后 rename**：任何文件交换之前，新二进制必须已完整落盘 staging
   并通过校验。绝不先 rename 再下载。
3. **ACK 齐了才交换**：所有运行实例经 IPC 广播并正确表达「进入二进制更换
   阶段」后，才允许动二进制。
4. **逐个重启、最后删除**：滚动重启（任一时刻至多一个实例下线）；旧二进制
   在终验通过后才删除。

## 规范位置与管辖边界

- 安装二进制唯一管辖位置：`~/.aproxy/bin/aproxy.exe`。
- 旧二进制：`~/.aproxy/bin/aproxy.exe.old-<旧版本>`（只保留最近 1 份——
  连续升级互相覆盖，更早版本走 GitHub Releases 随时可取）。
- 备料区：`~/.aproxy/staging/<目标版本>/`（与 bin 同在 APROXY_HOME 下——
  rename 原子性的同卷前提；staging 文件保留至 done/abort 才清理，它是
  swapping 中断的恢复源，见下文专节）。
- **状态文件放 run/ 下**（用户定调 2026-09-09：根目录只放长期稳定件，
  状态类文件进子目录防混乱）：`~/.aproxy/run/install.state.json`——自动
  享受 APROXY_RUN_DIR 重定向（测试隔离白送），run/ 本身在主目录下跨重启
  稳定，断电恢复语义不受影响。
- **管辖检查**（安装前）：枚举运行实例的进程镜像路径（QueryFullProcessImageNameW），
  任何实例的 exe 不在 `~/.aproxy/bin/` 下 → 拒绝安装并提示两条出路：
  对应包管理器渠道升级，或 **`aproxy install --adopt`**。
- **`--adopt`（收编，用户定调：显式执行，绝不自动）**：包管理器安装的
  aProxy 迁移到标准位置——把当前运行 exe 自身作为 `--from` 源复制进
  staging，走完整标准流水线（swap → 滚动重启实例）落到 `~/.aproxy/bin/`。
  之后升级由 install 自管；包管理器那份闲置（提示可自行 uninstall）。
  复用 `--from` 流水线，实现成本 ≈ 一个标志。

## 状态文件（安装的真相源 + 安装锁）

`~/.aproxy/run/install.state.json`（见上节——run/ 子目录 + 重定向友好）。

```json
{
  "phase": "downloaded",          // 见状态机
  "target_version": "0.1.0-alpha.7",
  "source": "github|from|cargo|npm",
  "staged_path": "C:/Users/x/.aproxy/staging/0.1.0-alpha.7/aproxy.exe",
  "sha256": "…",
  "old_path": "~/.aproxy/bin/aproxy.exe.old-0.1.0-alpha.6", // swap 后填充
  "instance_snapshot": ["12345", "12349"],  // ACK 阶段的实例清单
  "started_at": 0, "updated_at": 0,
  "installer_pid": 0
}
```

- **create_new = 安装锁**：已存在 → 并发安装拒绝（带 stale 判定：updated_at
  超 10 分钟且 installer_pid 不存活 → 视为残留，提示 `install --resume`）。
- 每次阶段推进原子重写（tmp + rename，同 `write_instance_file` 既有模式）。
- `--resume` 继续；`--abort` 回滚（仅 swapping 前可完全回滚，之后只进不退）。

## 阶段状态机

```
marking → downloading → downloaded → broadcasting → acked
        → swapping → swapped → relaying → restarting → verifying → cleaning → done
失败/中止：failed（保留现场可 --resume）| aborted（干净回滚）
```

| 阶段 | 动作 | 状态文件更新时机 |
|---|---|---|
| marking | 写状态文件（create_new 抢锁） | 写入即 marking |
| downloading | 渠道备料到 staging + sha256/可执行校验 | 进入前置 downloading，校验过后置 downloaded |
| broadcasting | IPC 广播 PrepareSwap + ACK 确认（含重试/restart 收敛） | 进入前置 broadcasting，全 ACK 后置 acked |
| swapping | rename 旧 exe → .old；staging → bin 落位；`--version` 验证新 exe | rename 前置 swapping，落位+验证后置 swapped |
| relaying | 旧安装进程 spawn 新二进制 `install --relay`，确认接管后自行退出 | 新进程接管后置 relaying（见接力协议） |
| restarting | 逐实例 stop(优雅)→新 exe spawn→IPC 就绪→下一个 | 每重启一个实例更新一次（记录进度） |
| verifying | 全实例 ping：version==target && swap_phase==false | 前置 verifying，通过后置 cleaning |
| cleaning | 删 .old（被锁则保留待下次启动 cleanup 例行重试，绝不强杀） | 删除成功置 done |

### swapping 极端情况专节（用户定调：最要命的失败态）

正常路径 rename 旧→落位新是**毫秒级**两步（同卷 rename 原子）；但两步之间
崩溃 = bin 目录只剩 `.old`、`aproxy.exe` 不存在——**所有 aproxy 指令无处可
落**（二进制没了，status/stop/install 全都调不出来），重启后也无法自愈。

三层防线：

1. **预防（主防线）**：交换顺序设计为「bin 永不空窗」——**先落新、后改名旧**：
   staging 新 exe rename 到 bin 时路径已被旧 exe 占用，不可行；改为
   **copy 新 exe 到 bin 下的临时名（aproxy.exe.new，不占锁）→ rename 旧
   exe → rename .new → aproxy.exe**，两步 rename 之间窗口从「整个文件
   落位」缩到一次系统调用；`.new` 的存在本身就是「swapping 进行中」的标记。
2. **检测**：入口检测（见恢复节）发现 swapping 中断 → 无论手动修复还是
   --resume，路径都成立（staging 完整在盘）。
3. **兜底（手动修复指南，skill 专节）**：一切 CLI 失效时的 agent 手动步骤
   （全部是文件操作，不需要 aproxy 二进制可用）：
   - `~/.aproxy/staging/<版本>/aproxy.exe` 仍在 → **复制**到
     `~/.aproxy/bin/aproxy.exe`（Windows copy 对 .new 无锁——它没在运行）；
   - staging 也没了（极端中的极端）→ `bin/aproxy.exe.old-<版本>` 直接改名
     回 `aproxy.exe`（旧版本可用 > 没有版本），再跑 `aproxy install` 重来；
   - 两者皆失 → 从 GitHub Releases 重新下载或 `--from` 任意可用二进制。
   恢复后 `install --resume`/`--abort` 清理状态文件。

注：防线 1 的 copy+双 rename 使「bin 完全不存在二进制」只能发生在
copy+rename(旧) 之间约一次系统调用的窗口，且即使发生，手动修复的三个
退路全部成立——该状态**永远可恢复**，只是可能需要人工。

无实例快路径：broadcasting/relaying 直接跳过（restarting 为空）。

## 断电恢复矩阵（核心交付物——每行都必须有集成测试）

| 崩溃点 | 现场特征 | 下次启动恢复动作 | 服务是否受影响 |
|---|---|---|---|
| marking 后 | 状态文件在，staging 空 | 残留判定 → 提示 --resume/--abort；无实际影响 | 无 |
| downloading 中 | staging 有半截文件 | 删半截重下（--resume）或 --abort | 无 |
| downloaded/broadcasting | staging 完整，二进制未动 | 直接重入该阶段（幂等） | 无 |
| **swapping 中**（bin 空窗：旧已改名、新未落位） | bin 只有 .old（可能还有 .new），staging 完整 | **关键恢复**：从 staging 重新落位 → 验证 → 续 relay；CLI 全失效时走手动修复三退路（见 swapping 专节） | 无（实例全在跑） |
| swapped | bin=新 exe，.old 存在 | 验证新 exe → 续 relay | 无 |
| relaying 中 | relay 未起/已死 | 重新 spawn relay（幂等，读同一状态文件） | 无 |
| restarting 中 | 部分实例新版本 | 续滚动（跳过已就绪的） | 单实例滚动窗口内 |
| verifying/cleaning | 全新，.old 残留 | 补验证/删 .old（锁住则下次再试） | 无 |
| 任何阶段 | 状态文件损坏 | 按 aborted 处理 + 审计日志；swapping 前无影响，swapping 后以 .old 存在性推断 | 视阶段 |

恢复的检测点：**每个 aproxy 进程入口**（main.rs 子命令分派前）读状态文件，
活动中的安装 → 显著提示 + 给出 `install --resume` / `--abort`；不自动执行
破坏性动作（人/agent 确认后执行）。

## IPC 扩展（PrepareSwap 广播 + ACK）

- `IpcRequest` 增变体 `PrepareSwap`：实例收到后置 `IpcStats.swap_phase = true`
  （内存态，重启自然清除——「退出更换阶段」由逐实例重启天然完成）。
- `IpcStats` 增 `swap_phase: bool` → ping 响应携带（serde default，双向兼容）；
  status 输出显示「二进制更换中」，对外可观察。
- **ACK 判定** = 安装器 ping 该实例读到 `swap_phase == true`。
  表达的本质：实例把阶段写进自己的可观测状态，而不是口头 ok。
- **重试收敛策略**（用户定调）：3 轮 × 每轮 3 次 ping（间隔 500ms）；轮间对
  未 ACK 实例执行 **restart**（用安装器自身 exe——顺带把落后实例拉到安装器
  版本，旧实例没有 PrepareSwap op 的兼容问题由此收敛）；终失败 → **abort**
  （不是强杀——绝对避免服务中断原则贯穿始终），明确指出问题实例。
- 兼容：旧实例（无此 op）serde 反序列化失败 = 未表达 → 走 restart 路径 →
  新进程具备 op → ACK。混版本舰队自动收敛到安装器版本。
- 竞态窗口：acked 与 swapping 之间用户新启实例 → swap 前重比对实例清单
  （快照 diff），有变化 → 重新广播；restarting 中新启的旧版本实例 →
  verifying 的版本检查拦下 → 补 restart。

## 看护者换血（顺序关键，Windows 语义特有）

- **广播之前停掉旧看护者**（`force_terminate` + 镜像验证，删 claim）：旧看护者
  的 `current_exe()` 指向旧路径，rename 后其 respawn 会用 `.old` 拉起旧二进制，
  升级永不完成（Linux 无此问题——exec 按路径解析自动拿到新文件）。
- 看护者不承载流量，停掉零服务影响；换血窗口内实例崩溃 = 失去自动重拉
  （秒级~分钟级，可接受的微窗）。
- relaying 之后由新二进制 `ensure_watchdog_if_enabled` 重新出簇（既有逻辑复用）。

## 接力协议（Windows 无 exec 的替代）

- swapping 完成后，旧安装进程（跑在 .old 镜像上）spawn 新二进制
  `aproxy install --relay`（隐藏标志，读同一状态文件）。
- 旧进程轮询状态文件 `phase >= relaying && updated_at 刷新`（新进程接管即
  推进状态文件 = 自证存活），确认后**自行退出**（=「旧二进制自动停止」）。
- 回滚窗口说明：swapped 之前均可完全回滚（rename 回去——运行中的旧安装进程
  自身 image 允许再次 rename）；实例开始重启后只进不退（forward-fix：继续
  用新 exe 重启）。

## 版本与变体

- `install`（无参）= latest；**<1.0 时代整线是 prerelease，latest 含
  prerelease**（GitHub API releases/latest 在仅有 prerelease 时会落空——用
  列表接口取第一个，标记排除逻辑留 `--stable` 标志给未来）。
- `install <版本>` 任意指定；**默认拒绝降级**（target < 当前 → 拒绝，
  `--allow-downgrade` 防呆放行）。
- 指令集变体：`std::arch::is_x86_feature_detected!("avx2")` 选 `-v3` 资产，
  失败回退 baseline；`--variant` 手动覆盖。
- 校验：GitHub 渠道 sha256（发布资产附 `.sha256`）；`--from` 校验 = 临时执行
  `--version` 验证可运行且版本号 == 目标。

## 渠道矩阵（全部配置，按期上线；零经济成本，开源无顾虑）

| 渠道 | 形态 | 期 |
|---|---|---|
| `--from <路径>` | 安装器内置（流水线收敛点，今天可用） | **P0** |
| GitHub Releases | API 拉取 + sha256 + AVX2 变体选择 | **P0**（硬依赖：CI 产物规范） |
| `install.ps1` / `install.sh` | 引导脚本（irm \| iex）：首次安装到 ~/.aproxy/bin；检测到已安装则指路 `aproxy install` | P1 |
| cargo（crates.io） | `cargo install aproxy --root ~/.aproxy`（产物恰落规范位置）——**源码本地编译**，编译经 staging 交换，绝不直写锁定 exe | P1 |
| cargo-binstall | Cargo.toml `[package.metadata.binstall]` 模板指向 GH 资产，零成本顺带预编译能力 | P1 |
| npm | 主包 + optionalDependencies 平台包（esbuild 模式）；postinstall 从已装平台包**复制**（非网络下载）到 ~/.aproxy/bin | P2 |
| Scoop bucket / winget-pkgs PR / Chocolatey | manifest 指向 GH Releases；管辖外目录 → install 拒绝换血并指路 | P2 |

bootstrap 脚本与包管理器 = **首装渠道**；装机后的升级一律 `aproxy install` 自管。

### Cargo 问题的结论（讨论沉淀）

crates.io 只分发 `.crate` 源码包，官方 `cargo install` 一律本地编译（需
Rust 工具链）。绕开编译的两条路：cargo-binstall 约定（P1 顺带支持）；
`--root ~/.aproxy` 只影响落位不改变编译事实。

## 依赖与前置

- **CI 发布产物规范**（P0 GitHub 渠道的硬依赖，`--from` 不依赖）：资产命名
  `aproxy-<版本>-<target>[-v3].exe` + `.sha256`，与
  `.agents/memory/2026-09-07-release-engineering.md` 的指令集矩阵合并落地。
- 仓库公开（开源已定，GH API 匿名限流 60/h 够用）。

## skill 增补（commands.md / behaviors.md / compatibility.md）

- install/upgrade 命令参考（含 --from/--variant/--allow-downgrade/--resume/--abort）。
- behaviors.md「二进制更换阶段」节：语义、status 可见性、**ACK 失败处置**——
  某实例多轮未表达 = 该实例有隐患，skill 指引：将其关闭后重试安装
  （为什么不能强杀：避免服务中断原则）。
- **手动更新兜底节**：install 反复失败的特殊情况由 agent 手动执行——
  `aproxy stop all` → 替换 `~/.aproxy/bin/aproxy.exe` → `aproxy restore`；
  明确标注此路径**有服务中断**，仅作兜底。
- compatibility.md：install 自引入版本起可用；混版本舰队收敛规则。

## 测试矩阵

- 单测：状态机迁移合法性 / 恢复矩阵逐行注入 / staging 校验 / 降级防呆 /
  变体选择逻辑。
- 集成（需目录重定向，见开放问题 1）：`--from` 全流程（有实例/无实例）/
  各崩溃点 kill 安装进程 → --resume 恢复 / 多实例逐个重启顺序断言（任一时刻
  至多一个下线）/ ACK 失败 → restart 收敛 / 管辖外实例拒绝。
- Windows 专属：rename 锁定 exe 语义（已有实证）/ .old 被锁时 cleaning 重试。

## 分步提交（每步：实现 + 测试 + clippy 零警告 + fmt + commit）

0. **APROXY_HOME 全量重定向**（前置基建步，不激活 install）：settings.rs
   集中读取 + 全部路径函数改造 + 既有测试基建迁移 + roundtrip 测试。
1. **状态文件与状态机骨架**（lib）：schema/create_new 锁/原子重写/迁移合法性
   + 恢复矩阵表驱动单测。
2. **staging 备料与校验**：--from 渠道（复制 + `--version` 校验）+ sha256 通用件
   + `--adopt` 标志（复用 --from 流水线）。
3. **交换与接力**：copy+双 rename 舞（.new 中间态）/ `--version` 验证 /
   --relay 隐藏标志 / 回滚窗口。
4. **IPC PrepareSwap + swap_phase**（协议 + 实例侧 + status 展示）+ ACK 收敛
   （重试/restart/abort）+ 看护者换血顺序。
5. **滚动重启与终验清理** + 无实例快路径 + 竞态窗口防护。
6. **入口检测与 --resume/--abort**：main.rs 活动安装提示 + 恢复矩阵全链路
   集成测试（崩溃点注入，含 swapping 空窗的手动修复三退路验证）。
7. **GitHub Releases 渠道**：API 列表/下载/变体选择/sha256（P0 收尾）。
8. **收尾**：skill 三处增补（含手动更新兜底 + swapping 手动修复指南）+
   README + architecture.md 节 + TODO 收口。
   （P1/P2 渠道各自独立任务，不阻塞本计划验收。）

## 已定夺（原开放问题，2026-09-09 用户拍板）

1. **APROXY_HOME**：采纳全量重定向方案——bin/staging/logs/spool/run 全部
   相对 APROXY_HOME（未设 = `~/.aproxy`），APROXY_RUN_DIR 仍独立可覆盖
   （粒度优先）。路径推导集中在 settings/daemon 的既有函数改造一次；
   集成测试从此全量隔离，logs/spool 不再有真实目录污染。
2. **relay 存活判据**：状态文件 phase+updated_at 轮询（简单方案）。
3. **failed 现场保留**：staging 永久保留，用户显式 --abort 才清理。

## APROXY_HOME 的影响面（实现注意）

- 覆盖点收敛到少数函数：`settings::home_dir()`（或等价）、`daemon::run_dir/
  logs_dir/spool 根`、`config::config_path` 默认值、install 的 bin/staging
  推导——全部改为 `home()` 前缀拼接，APROXY_HOME 读取集中在 `settings.rs`。
- 既有行为不变：未设环境变量 = `~/.aproxy`，用户无感知。
- 测试基建：tests 里的 tempdir + env 注入模式不变，只是注入的是
  APROXY_HOME（APROXY_RUN_DIR 注入继续有效，优先级：RUN_DIR > HOME 派生）。
- install.state.json 在 run/ 下：测试注入 HOME 后状态文件自动隔离，无需
  额外处理。
