//! 集成测试用假 format 程序：子命令切换行为，跨平台可用（不依赖系统
//! python——Windows runner 的 python3 是 Store stub，不可靠）。
//!
//! cargo test 会先编译全部 example 再跑测试，二进制必已存在；测试按
//! current_exe 祖先目录定位 `target/<profile>/examples/`。
//!
//! 子命令：
//! - `echo`    读一行信封 → 原样回一行（循环直到 EOF——协议义务）
//! - `upper`   body 文本大写后回显（验证转换确实发生）
//! - `error`   输出 error 行（单请求失败，进程不崩）
//! - `exit1`   读一行后以非零退出（进程级失败模拟）
//! - `sleep <ms>` 睡指定毫秒后回显（超时测试）
//! - `rotate`  从 extra 解析 {"keys":[...]}，按请求计数轮换 authorization 头
//!   （多 key 轮换场景验证；计数在进程内存——persistent 模式下复现）
//! - `stateful` 跨阶段状态验证：把收到的 `stage|request_id|state` 写进
//!   `x-stage-seen` 头（请求侧进发往上游的请求头，响应侧进回给客户端的响应
//!   头）；请求阶段回信带 `state = "from-request-<request_id>"`
//!
//! 违反协议 / 进程生命周期类（进程池加固的回归面）：
//! - `banner`  启动时先往 stdout 打一行非信封横幅，之后照常回显——模拟
//!   「format 把启动日志写进 stdout」，stdout 从此比请求多一行
//! - `multiline` 每个请求回**两行**相同信封（单次写入，两行同批到达）——模拟
//!   jq 未加 -c / console.log 与 write 混用等「一请求多行」，且多出的那行恰好
//!   是合法信封（最危险的形态：错位后解析照样成功，串包静默）
//! - `late-stray <ms>` 回显后再等 <ms> 毫秒，把同一信封**再写一行**——模拟
//!   多余输出晚于回复到达（Node 的 console.log 交错、后台线程打日志）：
//!   aproxy 读回复时它还没来，worker 归还空闲表后才冒出来
//! - `die-idle <ms>` 回显；每处理完一个请求后空闲超过 <ms> 毫秒即自行退出
//!   （模拟空闲期崩溃 / 被杀 / format 自带空闲退出——池里留下的是死 worker）
//! - `oneshot [<marker>]` 首个请求正常回显；之后再读到输入行即**无输出退出**
//!   （复用 worker「写入成功、读到 EOF」的确定性复现）。给了 marker 文件路径
//!   时：每次启动往 marker 追加一行（行数 = 启动次数）；启动前 marker 已存在
//!   → 首个请求即无输出退出（模拟「换新 worker 也起不来」）

use std::io::{BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("echo");
    if cmd == "die-idle" {
        // 需要「带超时的读行」，与下方同步逐行循环的结构不同，单独实现
        let idle_ms: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(300);
        die_idle(std::time::Duration::from_millis(idle_ms));
        return;
    }
    if cmd == "banner" {
        // 横幅先于任何请求写出：aproxy 读到的第一行必然是它
        print_line("format-echo banner: ready");
    }
    // oneshot 的「首个请求也直接退出」开关：marker 已存在 = 本进程不是第一个。
    // 每次启动都往 marker 追加一行：测试按行数数出 aproxy 一共拉起了几个
    // worker（重试上限的可观测断言面）
    let oneshot_dead_on_arrival = cmd == "oneshot"
        && args.get(1).is_some_and(|marker| {
            let path = std::path::Path::new(marker);
            let existed = path.exists();
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = f.write_all(b"spawned\n");
            }
            existed
        });
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut counter: u64 = 0;

    // 循环逐行处理直到 EOF（persistent 协议义务；spawn 模式处理一行后由
    // aproxy 侧 kill/自然退出，行为兼容）
    while let Some(Ok(line)) = lines.next() {
        counter += 1;
        match cmd {
            "exit1" => std::process::exit(1),
            "sleep" => {
                let ms: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(100);
                std::thread::sleep(std::time::Duration::from_millis(ms));
                respond(&line, |env| env);
            }
            "error" => {
                let out = format!(r#"{{"headers":{{}},"error":"{cmd}-rejected-{counter}"}}"#);
                print_line(&out);
            }
            "upper" => {
                respond(&line, |mut env| {
                    env.body = env.body.take().map(|b| b.to_uppercase());
                    env
                });
            }
            "rotate" => {
                respond(&line, |mut env| {
                    // extra: {"keys":["k1","k2",...]}；轮换 authorization 头。
                    // worker_id 计入偏移：池内各 worker 起点错开（协议字段语义）
                    let keys: Vec<String> = serde_json::from_str::<serde_json::Value>(&env.extra)
                        .ok()
                        .and_then(|v| {
                            v.get("keys").and_then(|k| {
                                k.as_array().map(|a| {
                                    a.iter()
                                        .filter_map(|x| x.as_str().map(String::from))
                                        .collect()
                                })
                            })
                        })
                        .unwrap_or_default();
                    if !keys.is_empty() {
                        let idx = ((counter - 1) as u32 + env.worker_id) as usize % keys.len();
                        env.headers
                            .insert("authorization".to_string(), format!("Bearer {}", keys[idx]));
                    }
                    env
                });
            }
            "stateful" => {
                respond(&line, |mut env| {
                    let id = env.request_id.clone().unwrap_or_default();
                    let seen = format!(
                        "{}|{id}|{}",
                        env.stage.as_deref().unwrap_or("-"),
                        env.state.as_deref().unwrap_or("-")
                    );
                    env.headers.insert("x-stage-seen".to_string(), seen);
                    env.state = (env.stage.as_deref() == Some("request"))
                        .then(|| format!("from-request-{id}"));
                    env
                });
            }
            "rewrite" => {
                // url 改写验证：extra 即新的完整 url（协议转换的核心语义）
                respond(&line, |mut env| {
                    if !env.extra.is_empty() {
                        env.url = Some(env.extra.clone());
                    }
                    env
                });
            }
            "scrub" => {
                // 头表替换「删头语义」+ method 改写的可观测验证：
                // 删 authorization 头、method 改写为 PUT（headers/method
                // 可变性的两条路径都有确定性断言面）
                respond(&line, |mut env| {
                    env.headers.remove("authorization");
                    env.method = Some("PUT".to_string());
                    env
                });
            }
            "multiline" => {
                // 两行拼成**一次** write_all 写出（不走 print_line：它分两次写
                // 正文与换行，stdout 的 LineWriter 会拆成两次系统调用）——两行
                // 同批进管道，aproxy 一次读就能把两行都收进缓冲，让「读完首行
                // 后缓冲里还有残行」可确定性复现
                let out = match aproxy_envelope::TransformEnvelope::from_line(line.trim_end())
                    .ok()
                    .and_then(|env| env.to_line().ok())
                {
                    Some(l) => l,
                    None => r#"{"headers":{},"error":"bad envelope"}"#.to_string(),
                };
                let stdout = std::io::stdout();
                let mut o = stdout.lock();
                let _ = o.write_all(format!("{out}\n{out}\n").as_bytes());
                let _ = o.flush();
            }
            "late-stray" => {
                let ms: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(100);
                respond(&line, |env| env);
                std::thread::sleep(std::time::Duration::from_millis(ms));
                respond(&line, |env| env);
            }
            "oneshot" => {
                if oneshot_dead_on_arrival || counter > 1 {
                    // 无输出退出：aproxy 侧写入已成功，读侧拿到 EOF
                    std::process::exit(0);
                }
                respond(&line, |env| env);
            }
            // echo（默认）：原样回显
            _ => respond(&line, |env| env),
        }
        if cmd == "exit1" {
            // 不会到达（exit1 在读取后立即退出），防御性兜底
            std::process::exit(1);
        }
    }
}

/// `die-idle`：读行放到独立线程，主线程按「空闲超时」等下一行——超时即
/// exit(0)，留下一个对 aproxy 而言「空闲表里的死 worker」。
fn die_idle(idle: std::time::Duration) {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    // 首个请求之前不计空闲（worker 刚启动、尚未被用过）
    let mut next = rx.recv().ok();
    while let Some(line) = next {
        respond(&line, |env| env);
        // 空闲超时与 stdin EOF（发送端随读线程退出而断开）都收尾退出
        next = rx.recv_timeout(idle).ok();
    }
    std::process::exit(0);
}

fn respond(
    line: &str,
    f: impl FnOnce(aproxy_envelope::TransformEnvelope) -> aproxy_envelope::TransformEnvelope,
) {
    match aproxy_envelope::TransformEnvelope::from_line(line.trim_end()) {
        Ok(env) => {
            let out = f(env);
            match out.to_line() {
                Ok(l) => print_line(&l),
                Err(e) => print_line(&format!(r#"{{"headers":{{}},"error":"serialize: {e}"}}"#)),
            }
        }
        Err(e) => print_line(&format!(
            r#"{{"headers":{{}},"error":"bad envelope: {e}"}}"#
        )),
    }
}

fn print_line(s: &str) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = out.write_all(s.as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}
