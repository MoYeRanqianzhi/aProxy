//! 集成测试的本地 format 程序。握手独立于阶段行为，测试可分别模拟不兼容、未就绪、
//! 管道错位和合法拒绝。记录文件由测试指定，只含测试构造的帧。
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    io::{BufRead, Write},
    time::Duration,
};

/// 输出没有换行的固定块。测试预算限制写出总量，旧读者也只会等到超时而不会耗尽内存。
fn unfinished_frame(bytes: usize) {
    let mut out = std::io::stdout().lock();
    let block = [b'x'; 8192];
    for _ in 0..bytes.div_ceil(block.len()) {
        if out.write_all(&block).is_err() {
            return;
        }
    }
    let _ = out.flush();
    std::thread::sleep(Duration::from_secs(12));
}

/// 将观察结果写到测试指定的日志；先落下已收帧的确认，再由测试释放闸门。
fn observe(event: &str, seq: Option<u64>) {
    if let Some(path) = std::env::var_os("ECHO_OBSERVE") {
        let mut bytes =
            serde_json::to_vec(&json!({"event":event,"pid":std::process::id(),"seq":seq})).unwrap();
        bytes.push(b'\n');
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
    }
}
/// 只让测试在它观察到指定事实后继续；父测试异常离开时也不会无限占据子进程。
fn wait_gate(path: &std::path::Path) {
    let until = std::time::Instant::now() + Duration::from_secs(8);
    while !path.is_file() {
        assert!(
            std::time::Instant::now() < until,
            "fixture gate not released"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// 每个进程按行处理到 EOF；同批多余帧和延迟多余帧要区分，才能测出两种错位。
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let mode = args.first().map_or("echo", String::as_str);
    let arg = args.get(1).map_or("", String::as_str);
    let delay = Duration::from_millis(arg.parse().unwrap_or(100));
    if mode == "banner" {
        println!("format-echo banner: ready");
    }
    if mode == "stderr" {
        eprintln!("worker diagnostic");
        eprintln!("value={}", std::env::var("ECHO_SECRET").unwrap_or_default());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut worker = 0;
    let mut count = 0usize;
    let mut seen_hello = false;
    let dead = mode == "oneshot" && !arg.is_empty() && {
        let exists = std::path::Path::new(arg).exists();
        writeln!(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(arg)
                .unwrap(),
            "spawned"
        )
        .unwrap();
        exists
    };
    loop {
        let line = if seen_hello && mode == "die-idle" {
            rx.recv_timeout(delay).ok()
        } else {
            rx.recv().ok()
        };
        let Some(line) = line else {
            observe("eof", None);
            if mode == "stray-on-eof" || (mode == "stage-stray-on-eof" && count != 0) {
                println!("unexpected diagnostic on stdout");
            }
            if mode == "slow-eof" {
                std::thread::sleep(Duration::from_secs(6));
            }
            break;
        };
        if let Some(path) = std::env::var_os("ECHO_RECORD") {
            let mut bytes = line.as_bytes().to_vec();
            bytes.push(b'\n');
            OpenOptions::new()
                .append(true)
                .create(true)
                .open(path)
                .unwrap()
                .write_all(&bytes)
                .unwrap();
        }
        let m: Value = serde_json::from_str(&line).unwrap();
        let stage = m["stage"].as_str().unwrap();
        let seq = m["seq"].as_u64().unwrap();
        observe(stage, Some(seq));
        let mut r = json!({"v":1,"seq":seq});
        if !seen_hello {
            assert_eq!((stage, seq), ("hello", 0));
            seen_hello = true;
            worker = m["worker_id"].as_u64().unwrap() as usize;
            if mode == "stderr-bytes" {
                let mut err = std::io::stderr().lock();
                err.write_all(b"invalid:").unwrap();
                err.write_all(&vec![0xff; 4000]).unwrap();
                err.write_all(b"\n").unwrap();
                writeln!(err, "text:{}", "界".repeat(3000)).unwrap();
            }
            if mode == "partial-hello" {
                print!("{{");
                std::io::stdout().flush().unwrap();
                break;
            }
            if mode == "unfinished-hello" {
                unfinished_frame(64 * 1024);
                break;
            }
            if mode == "gate-hello" {
                wait_gate(std::path::Path::new(arg));
            }
            if mode == "malformed-hello" {
                println!("not JSON");
                continue;
            }
            if mode == "no-hello" {
                std::thread::sleep(Duration::from_secs(60));
                break;
            }
            if mode == "exit-hello" {
                break;
            }
            if mode == "slow-hello" {
                std::thread::sleep(delay);
            }
            r = json!({"v":1,"seq":0,"name":"format-echo","version":"1.0","protocol":1,"features":[],"stages":["request","retry","decide","response","heartbeat"]});
            if std::env::var_os("ECHO_DECIDE_SUCCESS").is_some() {
                r["features"] = json!(["decide.success"]);
            }
            if std::env::var_os("ECHO_DECIDE_FULL").is_some() {
                r["request_body"] = json!({"decide":"full"});
            }
            if mode == "metadata-only" {
                r["request_body"] = json!({"request":"none","retry":"digest"});
            }
            if mode == "missing-seq" {
                r.as_object_mut().unwrap().remove("seq");
            }
            if mode == "wrong-hello-seq" {
                r["seq"] = json!(1);
            }
            if mode == "bad-protocol" {
                r["protocol"] = json!(2);
            }
            if mode == "missing-stage" || std::env::var_os("ECHO_REFUSE_STAGE").is_some() {
                r["stages"] = json!([]);
            }
            if mode == "bad-body-need" {
                r["request_body"] = json!({"request":"all"});
            } else if mode == "bad-notify-need" {
                r["request_body"] = json!({"notify":"full"});
            }
        } else {
            count += 1;
            let obj = if stage == "response" {
                "response"
            } else {
                "request"
            };
            let reason = "error-rejected";
            if matches!(stage, "request" | "retry" | "response")
                && matches!(
                    mode,
                    "echo"
                        | "inspect"
                        | "gate-hello"
                        | "gate-request"
                        | "slow-eof"
                        | "sleep"
                        | "multiline"
                        | "late-stray"
                        | "oneshot"
                        | "die-idle"
                )
            {
                r[obj] = m[obj].clone();
                r["state"] = json!(format!("{worker}/{}", std::process::id()));
            }
            match mode {
                "held-beat" if stage == "heartbeat" => {
                    wait_gate(std::path::Path::new(arg));
                    r["data"] = json!(": late\n\n");
                }
                "recover-response" if stage == "response" => {
                    if OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(arg)
                        .is_ok()
                    {
                        break;
                    }
                    r["response"] = json!({"body":"fresh reply"});
                }
                "probe-response" if stage == "response" => {
                    if !std::path::Path::new(arg).is_file() {
                        r["seq"] = json!(seq + 1);
                    } else {
                        wait_gate(&std::path::Path::new(arg).with_extension("release"));
                        r["response"] = json!({"body":"probe reply"});
                    }
                }
                "alternate-beat" if stage == "heartbeat" && m["heartbeat"]["beat"] == 2 => {
                    r["data"] = json!(": second\n\n");
                }
                "bad-beat" if stage == "heartbeat" => r["seq"] = json!(seq + 1),
                "crash-then-unfinished" if stage == "response" => {
                    if OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(arg)
                        .is_ok()
                    {
                        break;
                    }
                    unfinished_frame(3 * 1024 * 1024);
                    break;
                }
                "stderr-burst" => {
                    // 让已知值横跨 8 KiB 截点；后续小行验证丢弃汇总与新额度。
                    eprintln!("{}private-fixture", "x".repeat(8192 - 9));
                    for _ in 0..57 {
                        eprintln!("small-stderr-line");
                    }
                }
                "gate-response"
                    if stage == "response"
                        && m["request"]["url"]
                            .as_str()
                            .is_some_and(|url| url.ends_with("/hold")) =>
                {
                    wait_gate(std::path::Path::new(arg));
                }
                "gate-decide" if stage == "decide" => wait_gate(std::path::Path::new(arg)),
                "gate-request" if m["request"]["body"] == "blocked" => {
                    wait_gate(std::path::Path::new(arg))
                }
                "sleep" => std::thread::sleep(delay),
                "oneshot" if count > 1 || dead => break,
                "exit1" => std::process::exit(1),
                "exit-after" if count > arg.parse().unwrap_or(1) => break,
                "invalid" => {
                    println!("not-json");
                    continue;
                }
                "bad-until-file" if !std::path::Path::new(arg).exists() => {
                    r["seq"] = json!(seq + 1)
                }
                "bad-seq" => r["seq"] = json!(seq + 1),
                "bad-seq-once" | "crash-once"
                    if OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(arg)
                        .is_ok() =>
                {
                    if mode == "crash-once" {
                        break;
                    }
                    r["seq"] = json!(seq + 1);
                }
                "error" => r["fail"] = json!({"retry":false,"reason":reason}),
                // 大状态与二进制样例从临时文件读，不受系统命令行长度上限影响。
                "reply-by-stage" => {
                    let script: Value =
                        serde_json::from_slice(&std::fs::read(arg).unwrap()).unwrap();
                    let key = if stage == "retry" {
                        format!("retry:{}", m["retry"]["attempt"])
                    } else if stage == "decide" {
                        format!("decide:{}", m["attempt"])
                    } else {
                        stage.to_string()
                    };
                    if let Some(fields) = script
                        .get(&key)
                        .or_else(|| script.get(stage))
                        .and_then(Value::as_object)
                    {
                        r.as_object_mut().unwrap().extend(fields.clone());
                    }
                }
                "reply-on-path" if m["client"]["path"] != "/limited" => {}
                "reply" | "reply-on-path" => {
                    let fields: serde_json::Map<String, Value> = serde_json::from_str(arg).unwrap();
                    r.as_object_mut().unwrap().extend(fields);
                }
                "append" if stage == "heartbeat" => {
                    r["data"] = json!(format!("{}: {arg}\n\n", m["data"].as_str().unwrap()));
                    r["state"] = json!("heartbeat cannot store this");
                }
                "append" => {
                    r[obj] = json!({"body":format!("{}{arg}",m[obj]["body"].as_str().unwrap())})
                }
                "upper" => {
                    if let Some(body) = m[obj]["body"].as_str() {
                        r[obj] = json!({"body":body.to_uppercase()});
                    }
                }
                "rotate" => {
                    let keys: Vec<String> = serde_json::from_str(arg).unwrap();
                    let mut h = m[obj]["headers"].clone();
                    h["authorization"] = json!(format!(
                        "Bearer {}",
                        keys[(worker + count - 1) % keys.len()]
                    ));
                    r[obj] = json!({"headers":h});
                }
                "stateful" => {
                    let id = m["request_id"].as_str().unwrap();
                    let mut h = m[obj]["headers"].clone();
                    h["x-stage-seen"] = json!(format!(
                        "{stage}|{id}|{}",
                        m["state"].as_str().unwrap_or("-")
                    ));
                    r[obj] = json!({"headers":h});
                    if stage == "request" {
                        r["state"] = json!(format!("from-request-{id}"));
                    }
                }
                "heartbeat" if stage == "heartbeat" => {
                    if !arg.is_empty() {
                        std::thread::sleep(delay);
                    }
                    let b = &m["heartbeat"];
                    r["data"] = json!(format!(
                        ": hb seq={} attempt={} body=false\n\n",
                        b["beat"], b["attempt"]
                    ));
                    r["state"] = json!(format!("hb-{}", b["beat"]));
                }
                "heartbeat" if stage == "response" => {
                    r[obj] = json!({"body":format!("{}: state={}\n\n",m[obj]["body"].as_str().unwrap_or(""),m["state"].as_str().unwrap_or("-"))})
                }
                "failover" | "failover-reject" => {
                    let mut h = m[obj]["headers"].clone();
                    if stage == "retry" {
                        if mode == "failover-reject" {
                            r["fail"] = json!({"retry":false,"reason":"no backup"});
                        } else {
                            h["x-retry-seen"] = json!(format!(
                                "attempt={} status={} original={}",
                                m["retry"]["attempt"],
                                m["retry"]["last_failure"]["details"]["status"],
                                h.get("x-transformed").is_none()
                            ));
                            r[obj] = json!({"url":arg,"headers":h});
                        }
                    } else {
                        h["x-transformed"] = json!("1");
                        r[obj] = json!({"headers":h});
                    }
                }
                "rewrite" => r[obj] = json!({"url":arg}),
                "scrub" => {
                    let mut h = m[obj]["headers"].clone();
                    h.as_object_mut().unwrap().remove("authorization");
                    r[obj] = json!({"method":"PUT","headers":h});
                }
                _ => {}
            }
        }
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        if count != 0 && mode == "multiline" {
            out.write_all(format!("{r}\n{r}\n").as_bytes()).unwrap();
        } else {
            writeln!(out, "{r}").unwrap();
        }
        out.flush().unwrap();
        if count != 0 && mode == "late-stray" {
            let dir = std::path::Path::new(arg);
            wait_gate(&dir.join(format!("release-{}", std::process::id())));
            writeln!(out, "{r}").unwrap();
            out.flush().unwrap();
            observe("stray-written", Some(seq));
        }
    }
    if mode == "stderr-on-eof" {
        eprintln!("last diagnostic after stdin EOF");
    }
}
