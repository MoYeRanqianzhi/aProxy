# format 编写指南：写出标准 format 工具并正确配置

## 一、标准 format 工具的结构（协议义务清单）

一个合格的 format 工具 = **单行处理函数 + 循环壳**。必须同时满足：

| # | 义务 | 违反的后果 |
|---|---|---|
| 1 | 按行读 stdin，一行一个信封 JSON | 多行混读 = 帧错乱，转换永久失败 |
| 2 | 处理完立即写一行信封回 stdout 并 **flush** | 不 flush → aproxy 等到超时（默认 30s）后 kill |
| 3 | **读到 stdin EOF 即 exit**（persistent 铁律） | 不退出 → aProxy 实例停止后 worker 挂成孤儿进程 |
| 4 | 单请求失败输出 error 行（exit 0） | 用非零 exit 表达业务失败 → worker 被当崩溃剔除，损失复用 |
| 5 | 输出信封是完整 JSON（至少 `headers`） | 输出残缺 JSON → 该请求按失败处理 |
| 6 | 不输出 `content-length`/hop-by-hop 头 | 输了也被忽略（aProxy 自动管理），徒增困惑 |
| 7 | 对未知输入走 error 行而不是 panic | panic/崩溃 → 请求侧 502、进程被剔除 |

最小合格实现（python，persistent 就绪）：

```python
#!/usr/bin/env python3
import sys, json

def process(env: dict) -> dict:
    # 你的转换逻辑：改 env["url"] / env["headers"] / env["body"]
    return env                                     # echo 最小例

def main():
    while True:
        line = sys.stdin.readline()
        if not line:                               # EOF → exit（义务 #3）
            return
        line = line.strip()
        if not line:
            continue
        try:
            env = json.loads(line)
        except Exception as e:
            out = {"headers": {}, "error": f"信封解析失败: {e}"}
        else:
            try:
                out = process(env)
            except Exception as e:                 # 义务 #4：error 行不是崩溃
                out = {"headers": {}, "error": f"转换失败: {e}"}
        sys.stdout.write(json.dumps(out, ensure_ascii=False) + "\n")
        sys.stdout.flush()                         # 义务 #2

main()
```

spawn 模式兼容：处理一行后不退出也没关系（aProxy 用毕即杀），上面的循环壳
两模式通用——**直接按 persistent 写，两种模式都能跑**。

## 二、正确配置（aproxy 侧最高频错误区）

config.toml 每实例字段（无 settings 全局层、无 CLI 旗标）：

```toml
# 请求侧：body/headers/url/method 改写后发上游
request_transform = { command = "python", args = ["fmt.py"], mode = "persistent", pool_max = 4, idle_timeout_secs = 300, timeout_secs = 30, extra = "任意字符串" }
# 响应侧：上游响应改写后回放（**独立配置**，忘配 = 响应不被转换）
response_transform = { command = "python", args = ["fmt.py"], mode = "persistent" }
```

配置易错点：

- **command 写绝对路径**：相对路径按 PATH 与守护进程工作目录解析，守护的
  工作目录不可靠。`~` 不被展开（toml 里写完整路径，或 `~/.aproxy/bin/xxx`
  展开交给 format 自己）。
- **请求与响应是两条配置**：只配 `request_transform` = 请求被转换、响应原样
  回放——协议转换场景两头都要配（且指向同一程序同一份逻辑）。
- **轮换/计数/聚合必须 `mode = "persistent"`**：spawn 每请求新进程，进程内
  状态恒重置（轮换永远第一个 key）。
- **`forward_only = true` 与转换器互斥**（启动即报错）：forward_only 不缓冲
  请求体，转换器需要全量 body。
- **`extra` 是唯一传参通道**（原样透传的字符串，格式由 format 自定）：
  传配置路径、传 key 表 JSON、传任何东西；两个 transform 的 extra 各自独立。
- 改配置后 `aproxy restart <端口或别名>` 生效；`aproxy config --show` 核对
  实际生效值。

## 三、常用技巧

### worker_id 做池内错位（轮换不均的解法）

persistent 池内各 worker 独立计数，各自从首 key 起会集中打前几个 key。
信封的 `worker_id`（0..pool_max）就是给这个的：

```python
start = env.get("worker_id", 0) % len(keys)
idx = (start + n_local) % len(keys)   # n_local = 本 worker 内请求计数
```

### extra 传结构化参数

extra 是原样透传的字符串——传 JSON 最通用（format 自解析），不限制格式：

```toml
extra = '{"keys":["sk-a","sk-b"],"model_map":{"gpt":"gpt-4o"}}'
```

### 请求侧/响应侧一套代码两用

信封**有没有 `method`** 是 aproxy 的方向约定：请求侧信封带 method，响应侧
不带。一个程序里按此分派，两个 transform 配同一文件。

### url 改写是协议转换的核心

请求侧改 `env["url"]` 实现端点/路径迁移；**响应侧信封 url 是你改写后的最终
地址**——多渠道场景用它反查渠道（两侧进程无共享状态，这是唯一对齐线索）。

### 二进制 body 用 body_b64

body 非 UTF-8（图片、压缩包）时 aproxy 自动走 `body_b64`；你的二进制输出
同理。处理逻辑开头先看 `body` 缺不缺、`body_b64` 有没有。

### SSE/大响应是整流转文本

信封模型下 aproxy 把整条响应缓冲后整体交给你——SSE 文本也是文本，逐行
转换后拼回即可（无需流式 API）。转换期间客户端拿不到增量（本就等全量）。

### 手动喂数据测试（不经 aproxy）

```bash
printf '%s\n' '{"method":"POST","headers":{},"body":"{}","url":"https://up.example.com/v1/x","worker_id":0,"extra":""}' \
  | python fmt.py
```

期望一行信封回显。error 场景单独喂一遍确认输出 `{"headers":{},"error":...}`。

## 四、易错点速查

| 错误 | 症状 | 解法 |
|---|---|---|
| 忘记 flush | 请求挂到 30s 超时 | 每行写后 flush（义务 #2） |
| 循环不处理 EOF | 实例停止后 worker 挂孤儿 | EOF 即 exit（义务 #3） |
| 用 exit 1 表达业务失败 | worker 被剔除、无 error 文案 | error 行 + exit 0（义务 #4） |
| 输出 `content-length` | 无效且困惑 | 删掉，aProxy 按实际字节回填 |
| 轮换用了 spawn | 永远第一个 key | `mode = "persistent"` |
| 只配 request 没配 response | 响应没转换 | 两条独立配置都要配 |
| 改了配置没重启 | 行为没变 | `aproxy restart <端口或别名>` |
| 响应侧回传了上游鉴权头 | key 泄漏给客户端 | 响应侧重建干净头表 |
| 假设 body 一定是 UTF-8 文本 | 二进制 body 时崩 | 先判 `body_b64` |
| 头表键写大写 | 能用但不可移植 | 统一小写键 |

更多排障（502 文案对照、日志位置）见 [troubleshooting.md](troubleshooting.md)；
字段级语义见 [protocol.md](protocol.md)。
