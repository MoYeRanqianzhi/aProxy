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

## 响应侧报「url 反查不到渠道」

请求侧把 url 改写到了渠道表之外的地址（自定义 format 改了 url 但响应侧
不认识）。渠道表的 `url` 必须与请求侧实际发出的地址一致（preserve_path
场景按前缀匹配）。

## 日志在哪

`aproxy logs <端口>`（日志按启动随机命名，地址经 IPC 查询，不要拼路径）。
转换相关日志：请求侧「请求已由外部转换器改写」/「请求转换失败」、响应侧
「响应转换失败」、启动时「外部转换器已启用」。
