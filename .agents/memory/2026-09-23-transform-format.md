# 外部转换器（format）+ aproxy-format：设计决策留档

2026-09-23 定稿。功能：aProxy 支持把请求/响应交给外部 format 程序转换
（一行 JSON 信封 stdin/stdout 进出，类 OJ 判题机），另构建官方示例二进制
aproxy-format（协议转换 + key 轮换 + 多渠道聚合）。

## 用户拍板的决策（全部 2026-09-23）

1. **失败语义两侧不对称**：请求侧转换失败 → 502 不重试；
   响应侧失败 → 透传上游原始响应 + warn（响应已在手，可用性优先）。
   2026-10-04 澄清（WS-2，e60de76）：「不重试」不再称作「确定性失败」——错误按成因分类
   （Rejected/Protocol = format 输出本身的问题；Spawn/Io/WorkerDied/TimedOut = 进程层，常为瞬时），
   文案如实表述。池内唯一的重试是「复用的空闲 worker 在产出任何输出前就死了 → 换新 worker
   重做一次」，那是池状态问题，不改变本决策。
2. **二进制 body 走 body_b64**（base64 进信封），不拒绝不透传。
3. **配置仅 toml 层**：无 settings.json 全局默认层、无 CLI 旗标（转换是
   场景特定功能）。
4. **forward_only 与转换器互斥**（validate 启动拦截——forward_only 不缓冲
   请求体，转换器需要全量 body）。
5. **命名**：crate/bin = aproxy-format；信封契约 crate = aproxy-envelope；
   npm = @meowo/aproxy-format。
6. **crate 形态**：Cargo workspace 三成员（依赖树与体积隔离——aproxy 本体
   不受影响是硬要求；单 crate 双 [[bin]] 会让 switchyard 拉进 aproxy 编译
   闭包，直接排除）。
7. **aproxy-format 单独发 Release**：format-v* tag 触发独立 workflow
   （release-format.yml），11 变体同款矩阵；版本独立于 aproxy alpha 线
   （示例程序稳定后几乎不更新）；下载落 ~/.aproxy/bin/；npm 完整平权双包组。
8. **持续模式 = 进程池 + while 串行**（用户定调「OJ 多轮测试」）：每 worker
   一次处理一个请求、输入输出有序、无多路复用；并发靠池扩容；转换耗时
   极低（改 JSON 结构）所以小池撑高并发。

## 关键技术决策（实现期定）

- **worker 回收主机制 = stdin EOF**：实例任何形态退出 → OS 关管道 → format
  按协议义务自行 exit（覆盖 server.rs graceful 后 process::exit 跳过 Drop
  的路径）；kill_on_drop 兜底挂死 worker。对偶约束写进 skill（format 的
  while 循环遇 EOF 必须退出——铁律）。
- **Worker.stdout 必须用同一个 BufReader**（存进 Worker 结构）：每次 convert
  新建 BufReader 会把内部缓冲残留的下一行丢掉——致命 bug 结构上根除。
- **stdout 同步不变量**（2026-10-04，WS-2 修复跨请求串包）：信封协议没有请求序号，第 N 行
  只能靠「一请求恰一行、按序」对应第 N 个请求。worker 只有在输出被确认为一行合法信封、
  且其后无残留时才归还空闲表；归还与取出时各做一次非阻塞探测（probe_stdout），发现多余
  输出/已关闭即剔除。残余微秒级窗口需「信封带请求序号由 format 回显」根治，属 0.1.x 议题。
- **空闲回收用单 reaper 任务扫描**（sleep(min(idle/2,5s))），不用每 worker
  sleep 竞速——无任务爆炸与取消簿记。
- **reaper 惰性启动**（首次 convert 时）：AppState::new 可能不在 tokio
  上下文，直接 spawn 会 panic。
- **多值头仅保留首值 + warn**：set-cookie 逗号合并不合法、拒绝转换代价失衡；
  LLM API 请求侧无多值头。
- **响应侧转换前先解码压缩体**（decode::for_inspection，判定同款），输出
  强制剔除 content-length/content-encoding（字节已变换，旧声明必失真）。
- **响应信封携带 url（请求侧最终上游地址）**：多渠道聚合按它反查渠道表——
  request/response 转换器是不同进程，无共享状态，信封 url 是两侧对齐的
  唯一线索（设计铁律，写进 skill）。
- **聚合状态（轮换计数）在 worker 进程内存**：persistent 专属；spawn 退化
  为恒首 key（文档明示聚合必须 persistent）。worker_id（池槽位）进信封供
  format 做起始偏移防轮换不均。
- **switchyard-translation 0.3.0**（用户指定，已核实存在活跃：sans-I/O、
  依赖轻、WireFormat={OpenAiChat,AnthropicMessages,OpenAiResponses}、
  decode/encode_request + decode/encode_aggregated_response + decode/
  encode_stream）。**无 detect API**——自动检测是自研启发式（aproxy-format
  的 detect.rs，单测钉死）。
- **版本对齐脚本兼容性**：release.yml 的 lock 断言正则 `name = "aproxy"\n
  version =` 对 `name = "aproxy-format"` 不匹配（引号后是 `-` 不是换行）——
  三个成员版本线独立，无需改脚本。
- **npm skill 包形态变更**（alpha.17 起）：主包 skills/aproxy-cli.zip 单包
  → skills/aproxy-skills.zip 多 skill 总包。旧二进制的 npmpkg.rs 找不到新
  inner → skill 支线失败（非强制不影响安装），舰队收敛到新版本后自愈。
- **install_skill_dir 泛化**为遍历 staging 顶层目录逐个落位（返回落位名单）；
  crates.rs 的 add_skill_tree 前缀参数化；release.yml skills job 泛化为
  for 循环遍历 .claude/skills/*/。

## 测试

- 跨平台假 format：examples/format-echo.rs（echo/upper/error/exit1/sleep/
  rotate/rewrite，以及 2026-10-04 新增的 banner/multiline/late-stray/die-idle/oneshot
  ——用于钉死串包与死 worker 重试）——不用系统 python（Windows runner python3 是
  Store stub）。干净 target 下 `--lib`/`--test` 单跑不会编译 examples，需先
  `cargo build --example format-echo`。
- tests/transform_integration.rs 22 项（spawn/persistent × 两方向、url 改写、轮换、失败
  语义、超时、b64、大 body 磁盘、互斥、保活透传、串包与死 worker 回归）。

## 遗留

- 响应侧集成测试挂起问题（见 TODO，工具链 1.96.0→1.98.1 重装后复跑定位）。
- 响应侧大 body 转换内存峰值 2-3× spool_limit（文档明示；不做流式转换）。
- auto 模式（2026-10-04 起）只放行「客户端协议 = 渠道协议」：跨协议请求在请求侧以 error 行
  拒绝（aproxy 回 502、不发上游、不计费），提示显式声明 client_format；该修复随 aproxy-format
  新版本（format-v0.1.1 或之后）生效，需单独发 format 版本。

## 改信封 crate 必须同时升它的版本（2026-10-06）

aproxy-envelope 的版本号不随 tag 对齐：release.yml / release-format.yml 按 Cargo.toml 里的版本
查 crates.io，已存在就跳过发布。改了它的公开 API（例如 2026-10-06 给 TransformEnvelope 加
`stage`/`request_id`/`state`）却不升版本，主线 release 会跳过 envelope、照常发布 aproxy，而
crates.io 上的 aproxy 依赖的是旧 envelope，`cargo install aproxy` 编译失败。给结构体加 pub 字段对
0.x 是破坏性变更，升次版本（当时 0.1.0 → 0.2.0），根包与 aproxy-format 的依赖声明一起改。
**Evidence:** release.yml 的「Plan crates to publish」步骤（200 = 已存在即跳过）；Cargo.toml
`aproxy-envelope = { version = ..., path = ... }`。**Recheck when:** 发布流程改为按 tag 对齐 envelope 版本。

## 跨阶段扩展：心跳、stage/state、every_attempt（2026-10-06）

用户 2026-10-06 要求心跳可自定义、format 继续扩展、深入想清哪些决策点能交给 format。落地与取舍
（设计推敲过程见 git 历史里的 `.agents/plan/heartbeat-format-extension.md`，最后版本在删除它的提交之前）：

- 静态心跳 `keepalive_heartbeat` 只在 toml 层（协议相关、按实例）；校验规则 `config::heartbeat_problem`
  保证回放开始时客户端的 SSE 解析器停在事件边界，否则心跳会和第一个回放事件拼在一起。
- 信封加 `stage`（字符串，不用枚举：旧 format 遇到新阶段名照样能解析）、`request_id`、不透明 `state`
  （回信带上即替换，缺省 = 不变）。请求/心跳/响应转换器是不同进程，跨阶段状态只能经 aProxy 转交。
- `heartbeat_transform`：每拍 spawn 独立任务（drive 返回时会 drop 在途 future，persistent worker 被
  中途 drop 会被剔除）；上一拍没回来就发固定心跳（`in_flight`）；回放闸门 `replay_gate` 保证心跳不插进
  回放事件中间；回放开始后才回来的心跳丢弃。
- `[request_transform] every_attempt`：每次重试前用客户端原始请求再转换一次（`stage = "retry"` +
  `retry {attempt, status, error}`）。做成请求转换的开关而不是独立的 `retry_transform`：同一个程序处理
  两个阶段，官方 aproxy-format 按 `method` 区分请求/响应，零改动就会在每次重试换下一个 key。这次转换
  失败只沿用上一次的请求、不终止请求。
- 评估后**不做**的决策点：重试判定（format 能让请求停止重试 = 核心承诺取决于外部程序；确定性失败已有
  bounded_retry_paths）；按请求决定是否保活（唯一需求是 Gemini，用户定「gemini 不管」）；终态 error 事件
  渲染（极少发生，真需要时用静态模板，不为它拉起 format 进程）。
- 信封 crate 0.2.0（含 stage/request_id/state/heartbeat/retry）截至 2026-10-06 未发布：发布前的改动留在
  0.2.0 内，发布后再改公开 API 就要按上一节升版本。
