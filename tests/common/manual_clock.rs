//! 把代理路由和 IPC 放在独立的测试运行时中，主测试只在看见真实的 HTTP 或进程事件后才推进节拍。
//! 与数据链共用真正的管道与套接字；时钟命令只走测试进程的 stdin，不进入安装的可执行文件。
use super::common::{ChildGuard, TestHome, free_port};
use std::{
    io::{BufRead as _, Write as _},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

/// 持有本测试启动的代理及其时钟输入。关闭输入先退役进程，不给正式协议增加测试操作。
pub(super) struct Clock {
    /// 仅作异常退出的最后保障；正常退出通过控制管道 EOF。
    child: ChildGuard,
    /// 子进程完成推进后发回确认，避免将操作系统调度时间算作一拍。
    replies: std::sync::mpsc::Receiver<String>,
}
impl Clock {
    /// 和 CLI 一样解析配置、握手；父测试不改变自己的环境或时钟。
    pub(super) fn start(
        home: &TestHome,
        upstream: u16,
        config: &str,
    ) -> (reqwest::Client, String, Self) {
        let port = free_port();
        let file = home.path().join("clock.toml");
        let text = format!(
            "base_url='http://127.0.0.1:{upstream}'\nlisten_addr='127.0.0.1:{port}'\nretry.max_backoff_secs=0\n{config}"
        );
        std::fs::write(&file, text).unwrap();
        let mut child = ChildGuard(
            home.cmd_with(std::env::current_exe().unwrap())
                .args(["--ignored", "--exact", "manual_clock::serve", "--nocapture"])
                .env("APROXY_TEST_CLOCK_CONFIG", file)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        let pipe = std::io::BufReader::new(child.0.stdout.take().unwrap());
        let (send, replies) = std::sync::mpsc::sync_channel(2);
        std::thread::spawn(move || {
            for line in pipe.lines().map_while(Result::ok) {
                if (line.trim_end().ends_with("clock ready")
                    || line.trim_end().ends_with("clock advanced")
                    || line.trim_end().ends_with("clock drained"))
                    && send.send(line).is_err()
                {
                    break;
                }
            }
        });
        let mut clock = Self { child, replies };
        clock.ack("clock ready");
        (
            reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap(),
            format!("http://127.0.0.1:{port}"),
            clock,
        )
    }
    /// libtest 也会往同一管道写测试名；只用约定的行尾作屏障。
    fn ack(&mut self, marker: &str) {
        let line = self
            .replies
            .recv_timeout(Duration::from_secs(30))
            .unwrap_or_else(|e| panic!("no {marker} acknowledgement from clock fixture: {e}"));
        assert!(
            line.trim_end().ends_with(marker),
            "expected {marker}, got {line}"
        );
    }
    /// 一个时刻最多显式推进到下一拍；不会在等操作系统读写期间追赶另一个截止点。
    pub(super) fn advance(&mut self, secs: u64) {
        self.step(secs, false);
    }
    /// 最后一拨等服务自身的强退分支收回进程，不用测试夹具析构造成的退出冒充停机成功。
    pub(super) fn step(&mut self, secs: u64, finish: bool) {
        if !finish {
            writeln!(self.child.0.stdin.as_mut().unwrap(), "{secs}").unwrap();
            self.ack("clock advanced");
            return;
        }
        let mut pipe = self.child.0.stdin.take().unwrap();
        writeln!(pipe, "finish {secs}").unwrap();
        drop(pipe);
        self.ack("clock drained");
        let guard = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.0.try_wait().unwrap() {
                assert!(status.success(), "{status}");
                break;
            }
            assert!(
                std::time::Instant::now() < guard,
                "drained server did not exit"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    /// 请求已到达尚未放行的上游；之后的推进必定落在这条请求的生命期内。
    pub(super) async fn first_head(
        &mut self,
        request: reqwest::RequestBuilder,
        calls: &std::sync::atomic::AtomicUsize,
        secs: u64,
    ) -> reqwest::Response {
        let response = tokio::spawn(request.send());
        tokio::time::timeout(Duration::from_secs(15), async {
            while calls.load(std::sync::atomic::Ordering::Relaxed) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await
            }
        })
        .await
        .unwrap();
        self.advance(secs);
        response.await.unwrap().unwrap()
    }
}
impl Drop for Clock {
    fn drop(&mut self) {
        drop(self.child.0.stdin.take());
        // 启动卡死时即使 stdin 已关也可能无法自行退役；不能让失败断言的析构再次无限等待。
        let end = std::time::Instant::now() + Duration::from_secs(15);
        while matches!(self.child.0.try_wait(), Ok(None)) && std::time::Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        // 仍未退出时由 ChildGuard 按本测试持有的子进程句柄收回。
    }
}

/// 仅由 Clock 在 TestHome 内以明确的路径调用。独立阻塞任务等待命令期间 Tokio 不会自动跳时。
#[tokio::test]
#[ignore = "the parent test controls this process through stdin"]
async fn serve() {
    use aproxy::{daemon, proxy};
    let file = std::path::PathBuf::from(
        std::env::var_os("APROXY_TEST_CLOCK_CONFIG").expect("fixture configuration"),
    );
    assert_eq!(file.parent().unwrap(), aproxy_ipc::home::home());
    let config = aproxy::config::resolve::resolve_text(
        Some(&std::fs::read_to_string(&file).unwrap()),
        &file,
        &Default::default(),
        &Default::default(),
        false,
    );
    assert!(config.is_valid(), "{:?}", config.diagnostics);
    // 记录实际可选层跳过的警告，不把日志混到时钟的确认管道。
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(std::sync::Mutex::new(
            std::fs::File::create(file.with_extension("log")).unwrap(),
        ))
        .init();
    let state = proxy::AppState::new(config.config);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let reader = tokio::task::spawn_blocking(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    tokio::time::pause();
    state.handshake_formats().await.unwrap();
    let addr = state.config.listen_addr.clone();
    let port = daemon::port_of(&addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    std::fs::create_dir_all(daemon::run_dir()).unwrap();
    let endpoint = daemon::bind_ipc(port).await.unwrap();
    let (stop, mut stopped) = tokio::sync::watch::channel(false);
    let control = Arc::new(daemon::ControlState {
        record: daemon::InstanceRecord {
            pid: std::process::id(),
            start: aproxy_ipc::ledger::start_marker(std::process::id()).unwrap_or_default(),
            instance_id: state.instance_id.to_string(),
            version: env!("CARGO_PKG_VERSION").into(),
            listen_addr: addr,
            config_path: Some(file.display().to_string()),
            base_url: state.config.base_url.clone(),
            started_at: daemon::now_millis(),
            log_path: None,
            foreground: true,
        },
        behavior: daemon::behavior_of(&state.config),
        run_dir: daemon::run_dir().display().to_string(),
        stats: state.stats.clone(),
        history: state.history.clone(),
        on_shutdown: stop,
        stop_requested: Default::default(),
    });
    daemon::write_instance_file(&control.record).unwrap();
    let ipc = tokio::spawn(daemon::serve_ipc(endpoint, control));
    let router = proxy::router(state.clone());
    use futures_util::FutureExt as _;
    let serving = state.clone();
    let http = tokio::spawn(async move {
        serving
            .serve_until_stopped(
                listener,
                router,
                async move {
                    // 普通时钟夹具通过控制管道 EOF 收场，IPC 发送端也会随之关闭。
                    let _ = stopped.wait_for(|v| *v).await;
                }
                .shared(),
            )
            .await
    });
    println!("clock ready");
    while let Some(line) = rx.recv().await {
        if let Some(secs) = line.strip_prefix("finish ") {
            tokio::time::advance(
                Duration::from_secs(secs.parse().unwrap()) + Duration::from_millis(2),
            )
            .await;
            // 截止点已由父测试越过；只把之后的进程退出确认放回真实时钟。
            tokio::time::resume();
            assert!(http.await.unwrap(), "server did not take its deadline exit");
            ipc.abort();
            daemon::remove_registry_files(&state.config.listen_addr);
            reader.await.unwrap();
            println!("clock drained");
            return;
        }
        let secs = line.parse().unwrap();
        // 定时轮把子毫秒截止点向上取整；跨过边界而不是卡在取整前的那个瞬间。
        tokio::time::advance(Duration::from_secs(secs) + Duration::from_millis(2)).await;
        tokio::task::yield_now().await;
        println!("clock advanced");
    }
    tokio::time::resume();
    http.abort();
    ipc.abort();
    state.retire_formats().await;
    reader.await.unwrap();
}
