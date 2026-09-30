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

use std::io::{BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("echo");
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
            // echo（默认）：原样回显
            _ => respond(&line, |env| env),
        }
        if cmd == "exit1" {
            // 不会到达（exit1 在读取后立即退出），防御性兜底
            std::process::exit(1);
        }
    }
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
