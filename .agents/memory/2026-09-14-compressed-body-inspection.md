# 2026-09-14 压缩响应体让「错误判定 + 日志预览」双双静默失效（用户实测暴露）

## 症状

用户报告：日志里的报错内容一直显示二进制，无法定位问题。

```
WARN aproxy::proxy: 错误响应预览 preview=（二进制/压缩内容，共 1658 字节，
     hex 前 48: 1b dd 15 00 e4 cf 9f 5a 5f bf 44 13 b0 3d 67 81 ...）
```

同一个 1658 字节内容在生产日志里反复出现（`12233.log` 235 次、另 4 种不同
前缀各若干次），状态行是 `status=404 Not Found is_streaming=false`。

## 取证（决定性，一行 curl 复现）

```
$ curl -H 'accept-encoding: br' https://opencode.ai/nonexistent-probe-xyz
HTTP/1.1 404 Not Found
Content-Type: text/html
Content-Encoding: br        ← 关键
bytes: 1658
0000000: 1b dd 15 00 e4 cf 9f 5a 5f bf 44 13 b0 3d 67 81   ← 与日志 hex 逐字节一致
```

**根因**：那是 Cloudflare 返回的 **brotli 压缩 HTML 404 页**（opencode.ai 对
不存在的路径如此应答）。客户端（Claude Code）发
`accept-encoding: gzip, deflate, br, zstd` → 原样透传上游 → 上游回 brotli →
**reqwest 刻意关闭自动解压**（保真透传契约）→ 代理拿到的就是压缩字节。

而 brotli **没有 magic number**（gzip `1f 8b`、zstd `28 b5 2f fd` 都能一眼认出），
所以连「这是压缩体」都判断不出，`preview_body` 只能打 hex。

## 影响面远大于日志：错误判定也跑在压缩字节上

同一根因，三项能力同时**静默**失效（不报错、不退化为可感知的异常）：

1. `retry::is_error_body` 对压缩体做 `serde_json::from_slice` 必然失败
   → **HTTP 200 携带错误 JSON 不再触发重试**（只剩状态码判定在工作）
2. `retry::is_stream_error_body` 的 SSE 行扫描对压缩流全失效
   → **流尾 error 事件检测不到**（「流式中断可重试」的使命级能力缺口）
3. `proxy::preview_body` 只剩 hex 摘要 → 排障读不出内容（用户报告的这一项）

教训与 `ci-unix-blindspot` 同构：**「编译/测试通过」不等于「能力在真实上游下成立」**。
上二者是平台面盲区，这次是**编码面盲区**——测试里的 mock 上游从不压缩，
所以 220 项测试全绿也照不出这个问题。

## 修复（9ed566f + f37e813）

新增 `src/decode.rs`：按 `content-encoding` 解出一份**副本**喂给检查路径，
**转发给客户端的字节始终是上游原样**（保真透传不变）。

- 支持 gzip / x-gzip / deflate（zlib 与裸 deflate 都试）/ br / zstd + 多层编码逆序解
- 截断流取「已解出的前缀」（磁盘模式只喂 1 KiB 头部快照）
- 输出超 8 MiB 即拒绝（防解压炸弹）
- 新依赖 `brotli` + `ruzstd`，**均为纯 Rust**（无 C 依赖，musl/aarch64 交叉编译不受影响；
  gzip/deflate 复用已有的 flate2）

接线点：`should_retry_response`（内存路径判定 + 预览）与 `should_retry_response_disk`
（仅头部快照预览）。日志同时补 `content-type` / `content-encoding` 两个字段——
解码失败退化为 hex 时，它们正是判断「上游到底回了什么」的关键线索。

**已知边界**：磁盘模式（响应 > 1 MiB）的**判定**不解码——增量扫描器吃原始字节流，
为它改流式解码收益与风险不成比例（> 1 MiB 的压缩错误体现实中不存在），
判定仍以状态码为准。已写在 `decode` 模块文档。

## 验证

- 单测 3 条回归：gzip 压缩的 200+error JSON 必须重试（含未压缩反向锚点）、
  brotli 压缩 404 页必须重试且预览可读、无 content-encoding 时不解码；
  `decode` 模块 13 条（含解压炸弹、截断前缀、多层编码）
- **端到端复现**（隔离实例打真实上游）：同一份 Cloudflare brotli 404 页，
  日志从 hex 变为可读正文
  `content_type=text/html content_encoding=br preview=<!DOCTYPE html>...（共 5598 字节）`
- `cargo test --locked` 220 项全绿 + clippy `-D warnings` 零警告

## 纪律（写给后续协作者）

1. **检查路径的输入必须是「语义等价的可读字节」**。任何以「原样透传」为由绕开
   解码的检查，都要先问一句：上游压缩了怎么办？本项目的答案是——检查解码、
   转发不解码。
2. **mock 上游要覆盖压缩**。现有集成测试的 mock 从不发 `content-encoding`，
   这是 220 项全绿却漏掉此 bug 的直接原因（后续补测方向）。
3. **诊断信息要自证**。「hex 摘要」这种降级输出必须同时给出足以定位的元数据
   （此处是 content-encoding），否则降级 = 失明。

## 附带发现：测试守护进程泄漏

排查中实测：机器上累积了 **46 个 aproxy 进程**，绝大多数是历史测试遗留的
守护（最老 70 小时），其中 `start alias-test-3032 --daemon-watchdog` 这个
**看护进程**锁住了 `target/debug/aproxy.exe`，导致后续 `cargo test` 无法重新链接
（`failed to remove file ... 拒绝访问`），并让 `restart_integration` 在并行跑时
失败 215 秒（**单跑 4.5 秒通过**——与既有记录 `alias_start_and_stop_roundtrip`
同族的端口/时序竞争）。

注意：**不得按名批量杀**（见 [[2026-09-11-never-kill-aproxy-by-name]]）。
本次处置是「等它空闲自灭（看护者有 `watchdog_idle_exit_secs`）」，不自灭再用
测试守护的精确 `APROXY_HOME` + `stop <端口>` 收尾。该清理的 RAII 化已在 TODO。

相关：[[2026-09-11-ci-unix-blindspot]]、[[2026-09-07-log-mojibake]]、
[[2026-09-11-never-kill-aproxy-by-name]]、[[2026-09-12-install-online-deep-test]]
