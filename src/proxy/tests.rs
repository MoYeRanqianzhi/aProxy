use super::*;
use std::collections::HashMap;

// ---- 流式回放零拷贝：切块必须共享原 body 的底层分配（slice_ref 引用
// 计数切片），而非 copy_from_slice 深拷贝——深拷贝会让峰值内存瞬时
// 翻倍（50MB SSE 实测曾达 64MB）。指针区间断言：任何拷贝都会落到
// 原分配之外的新地址 ----

#[tokio::test]
async fn replay_chunks_are_zero_copy_views_of_source() {
    // 20 KiB → 3 块（8K + 8K + 4K），含非整除边界
    let body = Bytes::from(vec![0xA5u8; 20 * 1024]);
    let base = body.as_ptr() as usize;
    let end = base + body.len();

    let resp = build_stream_replay_response(StatusCode::OK, HeaderMap::new(), body.clone());
    // 逐帧轮询响应体：axum 的 Body::from_stream 原样透传我们产出的 Bytes
    //（不再复制），故帧数据指针可直接检验
    let mut streamed = resp.into_body();
    let mut frames = Vec::new();
    while let Some(frame) = http_body_util::BodyExt::frame(&mut streamed)
        .await
        .transpose()
        .unwrap()
    {
        if let Some(data) = frame.data_ref() {
            let p = data.as_ptr() as usize;
            assert!(
                (base..end).contains(&p),
                "回放块指针 {p:#x} 必须落在原 body 分配 [{base:#x}, {end:#x}) 内（零拷贝）"
            );
            frames.push(data.to_vec());
        }
    }
    assert_eq!(frames.len(), 3, "20 KiB 应切为 3 块");
    // 各块按序还原 == 原 body（切块只分不变）
    let reassembled: Vec<u8> = frames.into_iter().flatten().collect();
    assert_eq!(reassembled, body.as_ref());
}

// ---- 保活触发：请求体顶层 "stream": true 的流式判定（只看顶层、只认字面
// 量 true、解析失败一律为假） ----

#[test]
fn stream_flag_probe_reads_only_top_level_literal_true() {
    let probe = |s: &str| json_requests_stream(serde_json::Deserializer::from_slice(s.as_bytes()));
    for yes in [
        r#"{"stream":true}"#,
        r#"{"model":"m","messages":[{"role":"user","content":"hi"}],"stream":true}"#,
        " \n{ \"stream\" : true }\r\n ",
        // 重复键以最后一次为准
        r#"{"stream":false,"stream":true}"#,
    ] {
        assert!(probe(yes), "应判为流式: {yes}");
    }
    // 转义写法的键名与字面键名等价：键名里的 e 写成 JSON Unicode 转义
    // （反斜杠 + u0065）。反斜杠在运行时拼出：写成源码字面量时，转义序列
    // 容易被编辑/生成工具提前还原成字母 e，测试就悄悄退化成与上面第一条
    // 重复——下方断言守住这个前提
    let backslash = char::from(92);
    let escaped = format!("{{\"str{backslash}u0065am\":true}}");
    assert!(
        escaped.contains(backslash) && !escaped.contains("stream"),
        "前提：键名确实是转义写法: {escaped}"
    );
    assert!(probe(&escaped), "转义键名应判为流式: {escaped}");
    for no in [
        r#"{"stream":false}"#,
        r#"{"model":"m"}"#,
        r#"{"stream":"true"}"#,
        r#"{"stream":1}"#,
        r#"{"stream":null}"#,
        r#"{"stream":[true]}"#,
        r#"{"stream":{"enabled":true}}"#,
        r#"{"stream":true,"stream":false}"#,
        // 嵌套层的 stream 不算
        r#"{"messages":[{"stream":true}]}"#,
        r#"{"meta":{"stream":true},"stream":false}"#,
        // 顶层不是对象
        r#"[{"stream":true}]"#,
        r#""stream""#,
        "true",
        // 非 JSON / 截断 / 尾随垃圾 / 空体
        r#"{"stream":true"#,
        r#"{"stream":true} x"#,
        "stream=true",
        "",
    ] {
        assert!(!probe(no), "不应判为流式: {no:?}");
    }
}

#[tokio::test]
async fn stream_flag_probe_streams_disk_spooled_body() {
    // 溢写磁盘的大请求体：stream 键排在数 MB 字段之后，从文件流式扫描
    let dir = tempfile::tempdir().unwrap();
    let big = "y".repeat(3 * 1024 * 1024);
    for (stream, want) in [("true", true), ("false", false)] {
        let path = dir.path().join(format!("req-{stream}.spooltmp"));
        let json =
            format!(r#"{{"model":"m","messages":[{{"content":"{big}"}}],"stream":{stream}}}"#);
        std::fs::write(&path, &json).unwrap();
        let body = RequestBody::Disk {
            path,
            len: json.len() as u64,
        };
        assert_eq!(
            request_body_wants_stream(&body).await,
            want,
            "stream={stream}"
        );
    }
}

// ---- 保活通道：上游真实头的可提交条件 ----

#[test]
fn head_is_committable_requires_2xx_uncompressed_sse() {
    let headers = |pairs: &[(&'static str, &'static str)]| {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_static(v));
        }
        h
    };
    let sse = headers(&[("content-type", "text/event-stream; charset=utf-8")]);
    assert!(head_is_committable(StatusCode::OK, &sse));
    assert!(head_is_committable(
        StatusCode::OK,
        &headers(&[
            ("content-type", "text/event-stream"),
            ("content-encoding", "identity")
        ])
    ));
    // 非 2xx：要重试，不能先提交上游的错误状态行
    assert!(!head_is_committable(StatusCode::TOO_MANY_REQUESTS, &sse));
    // 非 SSE：心跳注释只在 SSE 里合法
    assert!(!head_is_committable(
        StatusCode::OK,
        &headers(&[("content-type", "application/json")])
    ));
    assert!(!head_is_committable(
        StatusCode::OK,
        &headers(&[("content-type", "application/x-ndjson")])
    ));
    // 压缩流：往里插明文心跳会让客户端解压失败
    assert!(!head_is_committable(
        StatusCode::OK,
        &headers(&[
            ("content-type", "text/event-stream"),
            ("content-encoding", "gzip")
        ])
    ));
}

// ---- 断开提示：只收到心跳的等待 ≥60s 后断开才提示检查客户端的流空闲超时 ----

#[derive(Clone, Default)]
struct CapturedLogs(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 已提交、提交于 `waited` 之前的保活出口（测试构造用；通道两端随返回值存活）
fn committed_sink(
    waited: Duration,
    heartbeats_only: bool,
) -> (
    KeepaliveSink,
    tokio::sync::mpsc::Receiver<Result<Bytes, std::io::Error>>,
    tokio::sync::watch::Sender<bool>,
) {
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let (gone_tx, gone_rx) = tokio::sync::watch::channel(false);
    let sink = KeepaliveSink::Committed {
        tx,
        gone_rx,
        at: tokio::time::Instant::now() - waited,
        heartbeats_only,
        beat: Heartbeat {
            fixed: Bytes::from_static(b": keepalive\n\n"),
            dynamic: None,
        },
        replay_gate: Arc::default(),
    };
    (sink, rx, gone_tx)
}

#[test]
fn heartbeat_only_disconnect_logs_generic_client_hint() {
    let logs = CapturedLogs::default();
    let sink_logs = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || sink_logs.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let text = || String::from_utf8_lossy(&logs.0.lock().unwrap()).to_string();
    const HINT: &str = "流空闲超时";

    // 尚未提交 / 只收心跳但等待不足 60s（多为主动取消）/ 已开始回放真实字节：
    // 只记断开，不提示
    KeepaliveSink::Pending {
        resp_tx: None,
        skeleton_due: false,
        beat: Heartbeat {
            fixed: Bytes::new(),
            dynamic: None,
        },
    }
    .log_client_gone("等待上游");
    let (short, _rx1, _g1) = committed_sink(Duration::from_secs(30), true);
    short.log_client_gone("等待上游");
    let (replaying, _rx2, _g2) = committed_sink(Duration::from_secs(900), false);
    replaying.log_client_gone("回放");
    assert!(text().contains("客户端已断开"), "{}", text());
    assert!(!text().contains(HINT), "以上三种都不应提示: {}", text());

    // 只收心跳等了 300s（Codex 的默认事件级超时）后断开：提示，且不点名客户端
    let (codex_like, _rx3, _g3) = committed_sink(Duration::from_secs(300), true);
    codex_like.log_client_gone("退避等待");
    let out = text();
    assert!(
        out.contains(HINT) && out.contains("300"),
        "只收心跳 ≥60s 后断开应提示并带上等待秒数: {out}"
    );
    assert!(
        !out.contains("Claude Code") && !out.contains("CLAUDE_STREAM_IDLE_TIMEOUT_MS"),
        "提示不点名具体客户端: {out}"
    );
}

#[tokio::test]
async fn dynamic_heartbeat_arriving_after_replay_started_is_dropped() {
    // 心跳转换器慢（400ms）：这一拍的调用还没回来，回放已写出第一块真实
    // 字节。迟到的动态心跳必须丢弃——写出去就插进了回放的事件中间
    let echo = crate::transform::format_echo_path();
    let pool = Arc::new(crate::transform::TransformPool::new(Arc::new(
        crate::config::TransformConfig {
            command: echo.display().to_string(),
            args: vec!["heartbeat".to_string(), "400".to_string()],
            timeout_secs: Some(10),
            ..Default::default()
        },
    )));
    let (mut sink, mut rx, _gone) = committed_sink(Duration::ZERO, true);
    sink.arm_dynamic_heartbeat(Arc::new(DynamicHeartbeat {
        pool,
        ctx: ExchangeCtx::new("1".to_string()),
        source: HeartbeatSource::new(
            &http::Method::POST,
            "http://upstream.invalid/v1/messages",
            &HeaderMap::new(),
            &RequestBody::Memory(Bytes::new()),
        ),
        seq: Default::default(),
        attempt: Default::default(),
        in_flight: Default::default(),
        warned: Default::default(),
    }));
    assert!(sink.heartbeat(), "发起这一拍的动态心跳（在途）");
    let real = Bytes::from_static(b"data: real\n\n");
    assert!(sink.send(Ok(real.clone())).await);
    assert_eq!(rx.recv().await.unwrap().unwrap(), real);
    // 等心跳转换回来（400ms + 进程启动）后，通道里不应再出现任何字节
    tokio::time::sleep(Duration::from_millis(2000)).await;
    assert!(rx.try_recv().is_err(), "回放开始后到达的心跳不应写出");
}

#[tokio::test]
async fn sink_send_ends_heartbeat_only_state() {
    // 写出任何真实字节（回放 / 终态 error 事件）后不再是「只收心跳」：此后的
    // 断开与客户端流空闲超时无关
    let (mut sink, mut rx, _gone) = committed_sink(Duration::ZERO, true);
    assert!(
        sink.send(Ok(Bytes::from_static(
            b"data: x

"
        )))
        .await
    );
    assert!(rx.recv().await.is_some());
    assert!(matches!(
        sink,
        KeepaliveSink::Committed {
            heartbeats_only: false,
            ..
        }
    ));
}

// ---- 入站 Host 归一：Host 头与配置条目必须归一到同一形态才能比较 ----

#[test]
fn normalize_host_forms() {
    for (raw, want) in [
        ("127.0.0.1:12345", "127.0.0.1"),
        ("LocalHost", "localhost"),
        ("localhost:1", "localhost"),
        ("[::1]:12345", "[::1]"),
        ("[::1]", "[::1]"),
        ("::1", "[::1]"),
        ("MyProxy.Local:999", "myproxy.local"),
        // 配置条目误写成 URL：取主机部分
        ("http://myhost:8080/", "myhost"),
        ("https://[::1]:9/x", "[::1]"),
        ("evil.example.com", "evil.example.com"),
    ] {
        assert_eq!(normalize_host(raw), want, "{raw}");
    }
    assert_eq!(
        normalize_origin(" http://LOCALHOST:5173/ "),
        "http://localhost:5173"
    );
}

// ---- 响应 spool 收尾 flush：最后一块的写失败不得被吞 ----

#[tokio::test]
async fn spool_finish_surfaces_error_of_last_write() {
    // 注入写失败：以**只读**句柄充当 spool 写句柄，阻塞池里的真实写入必然
    // 失败（Windows 拒绝访问 / unix EBADF），而 tokio File 的 poll_write
    // 派发后立刻报 Ok——这正是缺陷成立的前提：最后一块之后再无写操作，
    // 只有收尾 flush 能观察到这个错误
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool-flush-test.spooltmp");
    std::fs::write(&path, b"").unwrap();
    let file = tokio::fs::File::open(&path).await.unwrap();
    let mut buf = SpoolBuffer::Disk {
        file,
        path: SpoolPath(path.clone()),
        len: 0,
    };
    spool_chunk(&mut buf, b"final chunk", Some(dir.path())).await;
    assert!(
        matches!(buf, SpoolBuffer::Disk { len: 11, .. }),
        "前提：write_all 本身报 Ok，错误留到下一次 write/flush 才浮现"
    );

    let result = buf.finish(StreamErrorScanner::new()).await;
    let err = result.err().expect("收尾 flush 必须暴露最后一块的写失败");
    assert!(err.contains("spool 磁盘写入失败"), "{err}");
    assert!(!path.exists(), "失败的 spool 半截文件必须删除");
}

#[tokio::test]
async fn dropped_disk_spool_buffer_deletes_partial_file() {
    // 收集途中被整体丢弃（客户端断开让在途 forward_once 被 drop 即此情形）：
    // 半截 spool 文件必须当场删除，不能留到下次启动才由 clean_spool_dir 回收
    let dir = tempfile::tempdir().unwrap();
    let mut buf = SpoolBuffer::Memory(Vec::new());
    spool_chunk(&mut buf, &vec![b'x'; RESIDENT_LIMIT + 1], Some(dir.path())).await;
    assert!(matches!(buf, SpoolBuffer::Disk { .. }), "前提：已溢写磁盘");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    drop(buf);
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "被丢弃的 spool 缓冲必须删除自己的临时文件"
    );
}

#[tokio::test]
async fn spool_chunk_dropped_mid_disk_transition_leaves_no_file() {
    // 内存 → 磁盘迁移本身要好几次 await（建文件、写 1 MiB 内存前缀、写本块）；
    // future 恰在这期间被 drop（客户端断开）时，已建出的文件也必须删除。
    // 逐次 poll，文件一出现就丢弃 future——正落在迁移中途
    let dir = tempfile::tempdir().unwrap();
    let mut buf = SpoolBuffer::Memory(vec![b'x'; RESIDENT_LIMIT]);
    let chunk = vec![b'y'; 1024];
    let count = || std::fs::read_dir(dir.path()).unwrap().count();
    {
        let fut = spool_chunk(&mut buf, &chunk, Some(dir.path()));
        tokio::pin!(fut);
        loop {
            let polled = std::future::poll_fn(|cx| Poll::Ready(fut.as_mut().poll(cx))).await;
            assert!(polled.is_pending(), "前提：迁移不应在文件出现前就一次完成");
            if count() > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
    // 阻塞池里在途的写操作可能还持着句柄：删除已发出，等它收尾
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while count() > 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "迁移中途被丢弃的 spool 文件必须删除"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn request_body_dropped_mid_upload_leaves_no_file() {
    // 客户端上传到一半断开：hyper 直接 drop handler future，read_request_body
    // 停在「等下一块」的 await 上、走不到任何显式出错分支——已溢写的
    // req-*.spooltmp 也必须当场删除。首块超过驻留阈值触发溢写，之后流
    // 永远 pending（客户端不再发送）
    use futures_util::StreamExt as _;
    let dir = tempfile::tempdir().unwrap();
    let first = Bytes::from(vec![b'x'; RESIDENT_LIMIT + 1]);
    let stream = futures_util::stream::iter([Ok::<_, std::io::Error>(first)])
        .chain(futures_util::stream::pending());
    let count = || std::fs::read_dir(dir.path()).unwrap().count();
    {
        let fut = read_request_body(Body::from_stream(stream), usize::MAX, Some(dir.path()));
        tokio::pin!(fut);
        // 逐次 poll：文件出现后再推进一段时间，让溢写写完、future 停到
        // 「等下一块」上（这正是客户端上传中途停住再断开的形态）
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut settled_polls = 0;
        while settled_polls < 50 {
            let polled = std::future::poll_fn(|cx| Poll::Ready(fut.as_mut().poll(cx))).await;
            assert!(polled.is_pending(), "前提：请求体流未结束，读取不应完成");
            assert!(std::time::Instant::now() < deadline, "溢写文件始终未出现");
            if count() > 0 {
                settled_polls += 1;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while count() > 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "上传中途被丢弃的请求体临时文件必须删除"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn spool_finish_disk_success_keeps_full_content() {
    // 对照组：正常写句柄下 finish 成功，长度与文件内容一致（flush 不改变
    // 成功路径的语义）
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spool-flush-ok.spooltmp");
    let file = tokio::fs::File::create(&path).await.unwrap();
    let mut buf = SpoolBuffer::Disk {
        file,
        path: SpoolPath(path.clone()),
        len: 0,
    };
    spool_chunk(&mut buf, b"hello ", Some(dir.path())).await;
    spool_chunk(&mut buf, b"world", Some(dir.path())).await;
    let (body, scan) = buf
        .finish(StreamErrorScanner::new())
        .await
        .expect("正常写入的 finish 应成功");
    assert_eq!(body.len(), 11);
    assert!(scan.is_some(), "磁盘模式必带增量扫描结论");
    assert_eq!(std::fs::read(&path).unwrap(), b"hello world");
}

// ---- preview_body：二进制/压缩体不污染日志（回归：zstd 错误体曾被
// from_utf8_lossy 渲染成整片乱码）----

#[test]
fn preview_of_binary_shows_hex_not_mojibake() {
    // zstd magic 开头的压缩体（真实日志中出现过）
    let mut zstd_like = vec![0x28, 0xb5, 0x2f, 0xfd];
    zstd_like.extend((0..200u8).map(|i| i.wrapping_mul(37)));
    let p = preview_body(&zstd_like, 500, zstd_like.len());
    assert!(p.contains("二进制/压缩内容"), "{p}");
    assert!(p.contains("28 b5 2f fd"), "hex 应含 zstd magic: {p}");
    assert!(!p.contains('\u{FFFD}'), "不应输出替换符: {p}");
}

#[test]
fn preview_of_text_is_plain_and_truncated() {
    let text = "上游过载：请稍后重试".repeat(50);
    let p = preview_body(text.as_bytes(), 100, text.len());
    assert!(p.contains("上游过载"), "{p}");
    assert!(p.contains("截断"), "{p}");
    // 正常 JSON 错误体
    let json = br#"{"type":"error","error":{"type":"overloaded"}}"#;
    assert_eq!(
        preview_body(json, 500, json.len()),
        String::from_utf8_lossy(json)
    );
}

#[test]
fn preview_byte_count_is_the_response_size_not_the_snapshot_size() {
    // 磁盘模式回归：喂进来的是 1 KiB 头部快照，字节数必须报**响应真实大小**
    //（spool 长度），否则日志会把 10 MB 的响应说成 1 KiB，误导排障
    let snapshot = "上游过载：请稍后重试".repeat(60); // > limit，触发截断分支
    let p = preview_body(snapshot.as_bytes(), 100, 10 * 1024 * 1024);
    assert!(p.contains("截断"), "{p}");
    assert!(
        p.contains("共 10485760 字节"),
        "截断提示应报响应真实大小而非快照长度: {p}"
    );
    assert!(
        !p.contains(&format!("共 {} 字节", snapshot.len())),
        "不得把快照长度当成响应大小: {p}"
    );
    // 二进制分支同样按真实大小口径
    let bin = [0x28u8, 0xb5, 0x2f, 0xfd].repeat(500);
    let p = preview_body(&bin, 500, 7_000_000);
    assert!(p.contains("共 7000000 字节"), "{p}");
}

// ---- is_hop_header：hop-by-hop 头识别（大小写不敏感）----

#[test]
fn hop_headers_are_all_recognized() {
    for name in [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "host",
        "content-length",
    ] {
        assert!(is_hop_header(name), "{name} 应为 hop 头");
    }
}

#[test]
fn non_hop_headers_are_not_filtered() {
    for name in ["x-custom", "authorization", "content-type", "accept"] {
        assert!(!is_hop_header(name), "{name} 不应被当作 hop 头");
    }
}

#[test]
fn hop_matching_is_case_insensitive() {
    for name in [
        "Connection",
        "Keep-Alive",
        "HOST",
        "Content-Length",
        "Transfer-Encoding",
        "Proxy-Authorization",
        "TE",
        "Upgrade",
    ] {
        assert!(is_hop_header(name), "大小写不应影响 {name} 的 hop 判定");
    }
}

// ---- is_hop_response_header：响应侧过滤 = hop 头去掉 content-length
//（仅当上游未用 transfer-encoding 分帧时）----

#[test]
fn response_hop_filter_keeps_content_length() {
    // 字节未经变换，上游声明的长度仍精确：剥掉只会让定长响应退化为 chunked
    for name in ["content-length", "Content-Length", "CONTENT-LENGTH"] {
        assert!(
            !is_hop_response_header(name, false),
            "{name} 在响应侧必须保留（上游未分帧，字节未经变换）"
        );
        assert!(is_hop_header(name), "但它在请求侧仍是 hop 头");
    }
    // 其余 hop 头响应侧照旧过滤
    for name in [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "host",
    ] {
        assert!(
            is_hop_response_header(name, false),
            "{name} 在响应侧应被过滤"
        );
    }
    // 普通端到端头不受影响
    for name in ["content-type", "set-cookie", "x-custom"] {
        assert!(!is_hop_response_header(name, false), "{name} 不应被过滤");
    }
}

#[test]
fn response_hop_filter_drops_content_length_when_upstream_chunked() {
    // 上游同时带 CL 与 TE（协议违规但现实存在）：hyper 客户端按 TE 分帧，
    // 我们手里的 CL 只是上游的一句声明，与实际字节数可能不符——必须连同
    // transfer-encoding 一起剥掉，交由 hyper 重新分帧（RFC 7230 §3.3.3）。
    // CL 声称值大于实际时的后果是**静默**的：服务端按 CL 等缺额，客户端
    // 拿不到完整响应也看不出原因。
    for name in ["content-length", "Content-Length", "CONTENT-LENGTH"] {
        assert!(
            is_hop_response_header(name, true),
            "{name} 在上游用 TE 分帧时必须剥离"
        );
    }
    // 上游带 TE 时其余头的判定不受影响
    assert!(is_hop_response_header("transfer-encoding", true));
    assert!(!is_hop_response_header("content-type", true));
}

// ---- apply_header_overrides：api_key / override_headers / extra_headers 三层头策略 ----

#[test]
fn api_key_sets_bearer_authorization() {
    let cfg = Config {
        base_url: "https://api.example.com".to_string(),
        api_key: Some("sk-test".to_string()),
        ..Default::default()
    };
    let mut headers = HeaderMap::new();
    apply_header_overrides(&mut headers, &cfg);
    assert_eq!(headers.get("authorization").unwrap(), "Bearer sk-test");
}

#[test]
fn api_key_overwrites_existing_authorization() {
    let cfg = Config {
        base_url: "https://api.example.com".to_string(),
        api_key: Some("sk-test".to_string()),
        ..Default::default()
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_static("Basic dXNlcjpwYXNz"),
    );
    apply_header_overrides(&mut headers, &cfg);
    assert_eq!(headers.get("authorization").unwrap(), "Bearer sk-test");
}

#[test]
fn missing_api_key_adds_no_authorization() {
    let cfg = Config {
        base_url: "https://api.example.com".to_string(),
        ..Default::default()
    };
    let mut headers = HeaderMap::new();
    headers.insert("x-custom", HeaderValue::from_static("keep"));
    apply_header_overrides(&mut headers, &cfg);
    assert!(
        headers.get("authorization").is_none(),
        "未配置 api_key 不应新增 Authorization"
    );
    assert_eq!(headers.get("x-custom").unwrap(), "keep", "其余头应保持不变");
}

#[test]
fn override_headers_unconditionally_replace_existing() {
    // 配置键 "X-Foo" 应覆盖已有 "x-foo"（大小写不敏感），且同名多值全部收敛为单个新值
    let cfg = Config {
        base_url: "https://api.example.com".to_string(),
        override_headers: HashMap::from([("X-Foo".to_string(), "new".to_string())]),
        ..Default::default()
    };
    let mut headers = HeaderMap::new();
    headers.append("x-foo", HeaderValue::from_static("old1"));
    headers.append("x-foo", HeaderValue::from_static("old2"));
    apply_header_overrides(&mut headers, &cfg);
    assert_eq!(
        headers.get_all("x-foo").iter().count(),
        1,
        "覆盖后应只剩一个值"
    );
    assert_eq!(headers.get("x-foo").unwrap(), "new");
}

#[test]
fn override_headers_add_missing_headers() {
    let cfg = Config {
        base_url: "https://api.example.com".to_string(),
        override_headers: HashMap::from([("x-added".to_string(), "v".to_string())]),
        ..Default::default()
    };
    let mut headers = HeaderMap::new();
    apply_header_overrides(&mut headers, &cfg);
    assert_eq!(
        headers.get("x-added").unwrap(),
        "v",
        "override 对缺失头应直接新增"
    );
}

#[test]
fn extra_headers_only_fill_missing() {
    let cfg = Config {
        base_url: "https://api.example.com".to_string(),
        extra_headers: HashMap::from([
            ("x-extra".to_string(), "added".to_string()),
            ("x-present".to_string(), "ignored".to_string()),
        ]),
        ..Default::default()
    };
    let mut headers = HeaderMap::new();
    // 大小写不同的同名头也应保留原值，不被 extra_headers 覆盖
    headers.insert("X-PRESENT", HeaderValue::from_static("keep"));
    apply_header_overrides(&mut headers, &cfg);
    assert_eq!(headers.get("x-extra").unwrap(), "added", "缺失头应被追加");
    assert_eq!(
        headers.get("x-present").unwrap(),
        "keep",
        "已存在头不应被覆盖"
    );
}

#[test]
fn override_headers_beat_api_key_for_authorization() {
    // 优先级：api_key 先应用、override_headers 后应用 → authorization 最终取 override 值
    let cfg = Config {
        base_url: "https://api.example.com".to_string(),
        api_key: Some("sk-api".to_string()),
        override_headers: HashMap::from([(
            "Authorization".to_string(),
            "Bearer sk-override".to_string(),
        )]),
        ..Default::default()
    };
    let mut headers = HeaderMap::new();
    apply_header_overrides(&mut headers, &cfg);
    assert_eq!(headers.get("authorization").unwrap(), "Bearer sk-override");
}

#[test]
fn extra_headers_do_not_overwrite_existing_authorization() {
    // extra_headers 优先级最低（只补缺失）：api_key 已写入 authorization 时不再覆盖
    let cfg = Config {
        base_url: "https://api.example.com".to_string(),
        api_key: Some("sk-api".to_string()),
        extra_headers: HashMap::from([("Authorization".to_string(), "should-not-win".to_string())]),
        ..Default::default()
    };
    let mut headers = HeaderMap::new();
    apply_header_overrides(&mut headers, &cfg);
    assert_eq!(headers.get("authorization").unwrap(), "Bearer sk-api");
}

#[test]
fn invalid_names_and_values_are_silently_skipped() {
    // 非法头名（含空格/控制字符）与非法头值（含控制字符）应被静默跳过，不 panic
    let cfg = Config {
        base_url: "https://api.example.com".to_string(),
        api_key: Some("bad\nkey".to_string()), // 含换行控制符 → HeaderValue 非法，api_key 应被跳过
        override_headers: HashMap::from([
            ("bad name".to_string(), "v".to_string()), // 含空格 → HeaderName 非法
            ("x-good".to_string(), "ok".to_string()),
        ]),
        extra_headers: HashMap::from([("x-bad-val".to_string(), "bad\rval".to_string())]),
        ..Default::default()
    };
    let mut headers = HeaderMap::new();
    apply_header_overrides(&mut headers, &cfg);
    assert!(
        headers.get("authorization").is_none(),
        "非法 api_key 不应写入 Authorization"
    );
    assert!(headers.get("bad name").is_none(), "非法头名应跳过");
    assert_eq!(
        headers.get("x-good").unwrap(),
        "ok",
        "合法 override 应正常生效"
    );
    assert!(headers.get("x-bad-val").is_none(), "非法头值应跳过");
}

// ---- 压缩响应体：检查必须走解码副本 ----
//
// 上游按客户端 accept-encoding 压缩响应，而 reqwest 为保真透传刻意不解压，
// 于是判定与预览都跑在压缩字节上——JSON 解析必然失败、SSE 行扫描全失效、
// 预览只剩 hex。2026-09-14 由 opencode.ai 的 brotli 404 页实测暴露。

/// 构造带 content-encoding 的响应头（判定只看这一个头）
fn headers_with_encoding(encoding: &str) -> reqwest::header::HeaderMap {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert(reqwest::header::CONTENT_ENCODING, encoding.parse().unwrap());
    h
}

fn gzip_bytes(data: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut enc, data).unwrap();
    enc.finish().unwrap()
}

#[test]
fn compressed_error_json_with_200_is_retried() {
    // 回归：压缩后「200 携带 error JSON」曾完全检测不到——状态码 200 不触发
    // 重试，而 is_error_body 对压缩字节解析 JSON 必然失败
    let payload = br#"{"type":"error","error":{"type":"overloaded_error"}}"#;
    let headers = headers_with_encoding("gzip");
    assert!(
        should_retry_response(1, &StatusCode::OK, &headers, &gzip_bytes(payload)),
        "gzip 压缩的错误 JSON（HTTP 200）必须判定为需重试"
    );
    // 反向锚定：同一份 payload 不压缩时本就命中——证明样本有效，断言不是恒真
    assert!(
        should_retry_response(
            1,
            &StatusCode::OK,
            &reqwest::header::HeaderMap::new(),
            payload
        ),
        "未压缩的同一 payload 必须命中（样本有效性锚点）"
    );
}

#[test]
fn brotli_cloudflare_404_page_is_retried_and_readable() {
    // 事故原形：Cloudflare 对不存在的路径返回 **brotli** 压缩的 HTML 404 页。
    // brotli 没有 magic number，日志里只剩一串 hex，连「是压缩体」都认不出
    let page = b"<!DOCTYPE html><html><head><title>404 Not Found</title></head>\
<body><h1>404 Not Found</h1></body></html>";
    let mut enc = brotli::CompressorWriter::new(Vec::new(), 4096, 5, 22);
    std::io::Write::write_all(&mut enc, page).unwrap();
    let compressed = enc.into_inner();

    let mut headers = headers_with_encoding("br");
    headers.insert(reqwest::header::CONTENT_TYPE, "text/html".parse().unwrap());
    assert!(
        should_retry_response(1, &StatusCode::NOT_FOUND, &headers, &compressed),
        "404 必须重试"
    );

    // 修复目标本身：预览必须读得出正文（此前只剩 hex 摘要）
    let decoded =
        crate::decode::for_inspection(Some("br"), &compressed).expect("br 应可解码（检查副本）");
    let preview = preview_body(&decoded, 500, decoded.len());
    assert!(
        preview.contains("404 Not Found"),
        "预览应含 404 页正文，实际: {preview}"
    );
}

#[test]
fn uncompressed_responses_are_judged_without_decoding() {
    // 无 content-encoding 时不得解码（否则是一次无谓的整体拷贝）：
    // 传入「看起来像压缩体」的明文字节，判定仍按明文语义走
    let plain = br#"{"content":"hello"}"#;
    assert!(!should_retry_response(
        1,
        &StatusCode::OK,
        &reqwest::header::HeaderMap::new(),
        plain
    ));
}
