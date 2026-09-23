# 版本兼容性

判定本文档是否适用于用户手上的 aProxy 版本，以及跨版本操作时的行为差异。

## 版本判定方法

```
aproxy --version            # CLI 侧版本
aproxy status               # 每行 v<semver> = 各实例实际运行的守护进程版本
```

两者可能不同：status 显示的是**已在运行**的守护进程版本（升级二进制后旧实例
仍跑旧版本，直到重启）。操作某实例前以 status 显示的版本为准。

## 当前文档版本

| 项 | 值 |
|---|---|
| 文档适用版本 | **0.1.0-alpha.16**（含 alpha.4→alpha.16 引入的全部行为） |
| 代码版本坐标 | Cargo.toml `version` 字段；alpha 线于 2026-09 发布 |
| 大版本线 | 0.1.x（0.1 系列内小版本不另开目录，直接更新 latest/ 文档） |

## alpha.16 关键行为（相对 alpha.15）

- **config 纯读对缺失的显式 `--config` 报错**：`aproxy --config <路径>
  config --show`（或无修改参数）在指定文件不存在时不再静默展示内置默认值，
  而是报「指定的配置文件不存在」退出 1——此前打错路径看到的是默认配置
  （抬头却是给的路径），排障被误导。**写操作不受影响**：`--config <新路径>
  config --baseurl …` 仍是「创建新实例配置」的合法入口（多开工作流）；
  默认路径（不带 --config）首配场景照旧。
- **依赖收口**：rustls 0.23.44 → 0.23.45（RUSTSEC-2026-0285，TLS 1.3
  握手消息跨加密级别边界）。

## alpha.15 关键行为（相对 alpha.14）

- **守护日志随机命名（不再按端口命名）**：日志文件改为启动时刻随机命名
  （十六进制时间戳-pid 格式，如 `19ac3f2e8b5d-1a2b.log`），每次启动（含
  restart、restore 恢复）都是新文件——换端口后日志不断档。文件名不含端口，
  日志地址一律经 IPC 向实例询问（`aproxy status`/`aproxy logs`/start 成功
  提示来自实例上报的 log_path，客户端不拼路径）；`--foreground` 实例
  log_path 为空，`aproxy logs` 立即报「日志输出在它的控制台」。
- **孤儿清理判据重构**：由「按文件名端口归属」改为「活实例上报的 log_path ∪
  `.restore` 记录的 log_path」之外的 `*.log`（startup.log 除外）。推论：
  **旧版按端口命名的日志在升级后会被视为孤儿清理**；实例优雅停止后其日志在
  下次清理时删除；崩溃实例的日志由 `.restore` 引用保留到恢复成功。
- **新增 `log_file` 配置（config.toml 字段 + CLI `--log-file`）**：自定义
  守护日志文件路径，优先级 CLI > toml > 内置随机名；相对路径相对 APROXY_HOME
  解析；运行期轮转同样适用。**有意不设 settings.json 全局默认层**（与
  `max_body_mb`/`disk_cache`/`forward_only`/`bounded_retry_paths` 四字段
  形成对照）。
- **无跨版本兼容层**：alpha 阶段不做「按端口拼日志路径」的回退（serde
  default 仅作混版本解析容错）。混版本舰队中旧版本实例的日志仍按端口命名
  存在，但清理器只有一份（最新 CLI）——旧命名的日志会被新判据当孤儿清理。

## alpha.14 关键行为（相对 alpha.13）

- **受限重试路径 `bounded_retry_paths`（新配置字段，默认空 = 功能关闭）**：
  config.toml 每实例 + settings.json 全局默认（toml 显式值 > settings > 空）。
  命中任一正则模式（对「`路径?查询串`」整体匹配、自动锚定，普通路径即精准
  匹配；查询串须显式 `\?` 写进模式，通配 `.*`）的请求，上游「有响应的失败」
  达 3 次尝试后不再重试，把最后一次上游响应**原样透传**——确定性报错的上游
  端点不再让客户端无限等。网络错误与未命中请求照旧无限重试。典型场景：
  Claude Code 走非官方 API 时 compact 因上游不支持 `count_tokens` 被无限
  重试卡死，把该路径加入本配置即可解决。写法见 config-toml.md，语义见
  behaviors.md。**旧版二进制读到该字段静默忽略**（行为不变，升级安全）。
- **新增依赖**：`regex`（纯 Rust，musl/aarch64 交叉编译与 CI 不受影响）。
- settings.json 层的非法正则纳入 `aproxy doctor` 预检；start 因该字段失败时
  错误消息点名来源（toml 或 settings.json）。

## alpha.13 关键行为（相对 alpha.12）

**无行为变化**——alpha.13 与 alpha.12 是同一份代码，仅为版本号重发：alpha.12
发布后发现 crates.io 上它并非可解析的最高版本（同为 `alpha.12` 前缀的早期测试
版本 `0.1.0-alpha.12t2/t3` 在 semver 里序位更高，数字标识符优先级低于字母数字），
导致 `cargo install aproxy` 与该下载链的 cargo-binstall 兜底档会解析到旧测试
构建。两个测试版本已在 crates.io 上 yank（可逆，仅影响新解析，已锁定的 lockfile
不受影响），并用本版本号给出一个序位明确更高的正式版。

## alpha.12 关键行为（相对 alpha.10 及更早）

- **压缩响应体检查（修复静默失效）**：上游按 `accept-encoding` 压缩时，
  「HTTP 200 + error JSON」的重试判定与流尾 SSE error 事件检测此前在压缩体上
  **全部静默失效**（JSON 解析与行扫描对压缩字节必然失败），日志预览也只能打
  hex 摘要（brotli 无 magic number）。本版起检查路径先解一份副本再判定
  （gzip/deflate/br/zstd，多层按逆序解），**转发给客户端的字节不变**——仍是
  上游原样。已知边界：磁盘模式（响应 >1 MiB）仍走原始字节的增量扫描，不做
  解码（>1 MiB 的压缩错误体现实中不存在）。语义见 behaviors.md
- **仅转发模式 `forward_only`（新字段，默认 false）**：config.toml 与
  settings.json 均支持（toml 显式值 > settings 全局默认，与 `max_body_mb`/
  `disk_cache` 同款分层）。开启后该实例**放弃重试/缓冲/心跳**，请求体与响应
  流式直通——**这是显式取舍**，不是普通加速开关。`max_body_mb` 仍强制。
  旧二进制读到该字段会忽略（行为不变），新二进制读旧配置文件取默认 false
  （升级安全）。语义详见 config-toml.md 的 `forward_only` 节与 behaviors.md

## alpha.10 关键行为（相对 alpha.9 及更早）

- **install/upgrade 命令引入**（`--from/--adopt/--skills-only/--abort` 及在线
  渠道）：本版本起可用。更早版本的二进制没有 install——升级到 alpha.10 用
  引导脚本重装或手动替换二进制
- **APROXY_HOME 环境变量**：config/settings/run/logs/spool/bin/staging 全部
  相对该主目录派生（未设 = `~/.aproxy`，行为不变）；APROXY_RUN_DIR 仍独立
  可覆盖（粒度优先）
- **install.state 残留语义**：`~/.aproxy/run/install.state` 存在 = 有未完成
  的安装——看护者/CLI 入口会自动拉起续作（全自动）。**不要手删**；确认要
  放弃用 `aproxy install --abort`
- **混版本舰队收敛**：install 广播 PrepareSwap 后，旧版本实例（无此 op）
  未表达 = 未 ACK，install 按轮次 restart 它们（用安装器自身 exe）——舰队
  自动收敛到安装器版本
- settings.json 新字段：`download_chain`（下载链条严格数组）、
  `skill_auto_update`（默认 true）、`download_proxy`（下载代理）——全部
  serde default，旧文件缺字段读默认，升级无感

## alpha.6 关键行为（相对 alpha.5 及更早）

对旧实例执行操作时注意这些差异：

1. **restart 与 `--force`（新命令）**：CLI 支持 `aproxy restart`（只重启不
   启动）与 stop/restart 的 `--force`（零等待强杀）。对旧版本实例 restart
   仍可工作（其 IPC shutdown 协议自 alpha 线稳定）；旧实例被 alpha.6 看护者
   重拉时也走同一路径。`--force` 对任何实例有效（纯进程操作）。
2. **看门狗（新，默认开启）**：全局看护进程自动重拉崩溃/挂死的实例。
   旧版本守护无心跳节——看护者对它退化为纯进程死亡检测（挂死不检测）。
3. **IPC v2**：响应携带 `proto` 字段与观测计数（请求/重试/最近错误）。
   status 对 v1 旧实例（alpha.5 及更早）不显示计数（显示为无观测数据）；
   CLI 的 Stats 查询对旧实例自动降级为 Ping。
4. **4xx 重试口径**（文档修正）：4xx/5xx 一律重试（`is_retryable_status`
   自原型起即为 400..=599）——此前部分文档误写「4xx 不重试」。任何版本的
   实际行为一致。

## alpha.4/alpha.5 关键行为（相对 alpha.3 及更早）

对旧实例执行操作时注意这些差异：

1. **磁盘缓存（alpha.4 新）**：alpha.4 默认开启 `disk_cache`，spool 溢写
   `~/.aproxy/spool/<端口>/`。旧实例无此目录无此行为。
2. **请求体上限放宽（alpha.4 变更）**：旧版硬编码 10 MiB（超限 413）；
   alpha.4 默认 128 MB 且可配（`max_body_mb`，0=不限）。
3. **新配置字段（alpha.4/5）**：`max_body_mb`、`disk_cache`（settings.json
   全局默认 + toml 覆盖）、`idle_timeout_secs`/`log_rotate_mb`（alpha.5）。
   旧二进制读到新字段会忽略（行为不变），新二进制读旧文件取默认（升级安全）。
4. **base_url 旧字段名**：`upstream_url` 为兼容别名，任何版本可读、保存写新名。

## 兼容性总原则

- **配置向前兼容**：新版二进制读旧配置文件/旧 settings.json → 缺字段取默认，
  不报错；旧二进制读新配置文件 → 未知字段忽略（serde 默认行为）。
- **混跑安全**：多实例可各自跑不同版本（实例独立进程独立配置）；但同一份
  settings.json 的别名表被两个版本共用，删除字段前确认没有旧实例还在读。
- **操作兼容**：status/stop/logs/restore 对旧版本实例全部有效（IPC 协议自
  alpha 线稳定）；新字段相关的 config --show 展示对旧实例无意义。

## 版本留存策略（维护者用）

- 小版本（0.1.x 内）：直接修改 `references/latest/` 下文档，不留存。
- 大版本更替（如 0.1 → 0.2、alpha → stable）：把整个 `latest/` 复制为
  `references/<旧版本号>/` 留存，再重建 latest；旧版本目录内含自己的
  compatibility.md 描述其适用范围。
- `SKILL.md` 的导航相对路径 `references/latest/` 不随版本变化。
