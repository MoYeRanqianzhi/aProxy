# 排障

## 请求 502 且错误含「format 进程启动失败」

`request_transform.command` 找不到。检查：
- 命令存在（写绝对路径最稳：`~/.aproxy/bin/aproxy-format`）；
- 相对路径按 PATH 与工作目录解析——守护进程的工作目录不可靠，别依赖；
- Windows 上带空格的路径在 toml 里正常写（不走 shell，按 argv 数组执行）。

## 请求 502 且错误含「format 报告转换失败」

format 输出了 error 行——这是 format 的**业务判定**（如 model 未命中渠道）。
错误文案来自 format 的 error 字段，去 format 的逻辑里找原因。

## 请求 502 且错误含「转换超时」

`timeout_secs`（默认 30s）内没等到回行。慢转换调大它；persistent worker
可能卡在上一请求（超时的 worker 会被 kill 剔除，下次请求起新的）。

## 请求 502 且错误含「format 进程意外退出且无输出」

format 崩了（非零退出且没输出信封行）。用错误输入手动喂数据复现：

```bash
printf '%s\n' '<信封 JSON>' | <command> <args>
```

注意：persistent 池复用到**空闲期间已死**的 worker（还没产出任何输出就失败）
时，池内已自动换新 worker 重试了一次——能看到这个 502，说明新开的 worker 也
死了，即 format 本身起不来（command 路径对但程序启动即崩、缺运行时依赖、参数
错等），不是偶发的池状态问题。aProxy 丢弃 format 的 stderr，崩溃原因要手动
复现才能看到。

## 请求 502 且错误含「format 输出违反信封协议」

format 的 stdout 内容不是「每个请求恰好一行合法信封」。信封协议没有请求序号，
worker 的第 N 行输出只能靠「一请求一行、按序」对应第 N 个请求，所以任何对应
关系存疑的输出都会让该 worker 被剔除、当次请求 502（否则下一个请求会读到上一个
请求的输出）。常见原因：

- **往 stdout 打了日志/横幅/调试输出**：stdout 只能写信封行；日志写 stderr
  （aProxy 会丢弃 stderr，需要留痕请写你自己的文件）。
- **`jq` 忘了 `-c`**：默认 pretty-print 输出多行，每个请求回了不止一行。
- 一个请求回了多行（循环里重复 print）、空闲时 stdout 冒出未被请求的输出
  （后台线程打印）——这类也会被检出并剔除 worker。
- 输出不是合法 JSON、含非 UTF-8 字节，或 `body` 与 `body_b64` 同时出现。

排查：用上节命令手动喂一行信封，看 stdout 是否**恰好一行**合法 JSON：

```bash
printf '%s\n' '<信封 JSON>' | <command> <args> | wc -l   # 期望 1
```

## persistent worker 不退出 / 挂成孤儿进程

format 的循环没处理 stdin EOF。铁律：**读到 EOF 就 exit**（aProxy 实例停止
时靠关闭管道回收 worker）。while 循环模板见 guide.md。

## 响应看起来没转换

- `response_transform` 配置了吗（请求/响应是**两条独立配置**）；
- 转换失败时会**透传原始响应**（响应侧失败语义）——查
  `aproxy logs <端口>` 里的「响应转换失败」warn 行看原因；
- bounded_retry_paths 命中的透传路径**不进响应转换**（错误响应不经 format）。

## 多 key 轮换不生效（永远同一个 key）

轮换计数在进程内存——**spawn 模式每请求新进程，计数恒 0**。改为
`mode = "persistent"`。池内各 worker 各自独立计数（各从首 key 起）——
轮换不均时用信封 `worker_id` 做起始偏移（官方 aproxy-format 已处理）。

## 聚合路由报「model 未命中任何渠道」

渠道表的 `models` glob 不匹配请求的 model 名。检查：
- 模型别名表 `[models]` 是否需要映射；
- glob 语法（`claude-*` 匹配前缀；无 `models` 字段 = 匹配全部）；
- `client_format = "auto"` 检测失败也会报错（错误文案带「检测失败」）——
  请求字段名不像已知协议时改为显式声明格式。

## auto 模式报错「client_format = "auto" 只支持同协议」

`client_format = "auto"` 按 body 形态启发式检测客户端协议，**只放行同协议**：
检测出的客户端协议与路由到的渠道协议不同时，请求侧直接 502 报错（发往上游
之前拒绝，不产生计费），文案点名检测结果、渠道协议，并提示显式声明。原因是
信封没有请求→响应的上下文，响应侧拿不到客户端协议，跨协议的响应无法转回。

解法：在聚合配置里**显式声明 `client_format`**（如 `"anthropic_messages"` /
`"openai_chat"`），改后 `aproxy restart <端口或别名>`。

另一个相关坑：auto 的检测本身是启发式的——**Anthropic 的 `system` 是可选
字段**，不带 `system` 的标准 Anthropic 请求（`{model, max_tokens, messages}`）
会被判成 OpenAI Chat。同协议路由下这不会出错（原样直通），但检测结果与真实
客户端不符时，若路由到另一协议的渠道就会被上面的规则拒绝——同样用显式声明
解决。生产实例一律显式声明。

## 跨协议的流式（SSE）响应没有被转换

官方 aproxy-format 的跨协议转换只支持**非流式**。跨协议 + 流式请求
（如 Claude Code 默认 `stream: true`，渠道是 OpenAI 协议）时，请求侧转换
成功、上游回 SSE，响应侧报「SSE 流式响应的跨协议转换尚未支持」，aProxy 按
响应侧失败语义透传上游原始流——客户端收到的是渠道协议格式的流，无法解析。
同协议 SSE 原样直通，不受影响。需要跨协议流式时：让客户端协议与渠道协议一致，
或自己实现流式转换的 format。

## 响应侧报「url 反查不到渠道」

请求侧把 url 改写到了渠道表之外的地址（自定义 format 改了 url 但响应侧
不认识）。渠道表的 `url` 必须与请求侧实际发出的地址一致（preserve_path
场景按前缀匹配）。

## 日志在哪

`aproxy logs <端口>`（日志按启动随机命名，地址经 IPC 查询，不要拼路径）。
转换相关日志：请求侧「请求已由外部转换器改写」/「请求转换失败」、响应侧
「响应转换失败」、启动时「外部转换器已启用」。
