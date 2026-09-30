# 外部转换器（format）+ aproxy-format：设计决策留档

2026-09-23 定稿。功能：aProxy 支持把请求/响应交给外部 format 程序转换
（一行 JSON 信封 stdin/stdout 进出，类 OJ 判题机），另构建官方示例二进制
aproxy-format（协议转换 + key 轮换 + 多渠道聚合）。

## 用户拍板的决策（全部 2026-09-23）

1. **失败语义两侧不对称**：请求侧转换失败 → 502 不重试（确定性失败）；
   响应侧失败 → 透传上游原始响应 + warn（响应已在手，可用性优先）。
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
  rotate/rewrite 子命令）——不用系统 python（Windows runner python3 是
  Store stub）。
- tests/transform_integration.rs 14 项矩阵（spawn/persistent × 两方向、
  url 改写、轮换、失败语义、超时、b64、大 body 磁盘、互斥、保活透传）。

## 遗留

- 响应侧集成测试挂起问题（见 TODO，工具链 1.96.0→1.98.1 重装后复跑定位）。
- 响应侧大 body 转换内存峰值 2-3× spool_limit（文档明示；不做流式转换）。
- SSE 的协议检测（auto 模式）暂按渠道格式直通，等 switchyard 提供权威检测。
