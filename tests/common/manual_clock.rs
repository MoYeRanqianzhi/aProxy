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
    replies: std::io::BufReader<std::process::ChildStdout>,
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
        let replies = std::io::BufReader::new(child.0.stdout.take().unwrap());
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
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(
                self.replies.read_line(&mut line).unwrap(),
                0,
                "clock fixture exited before {marker}"
            );
            if line.trim_end().ends_with(marker) {
                return;
            }
        }
    }
    /// 一个时刻最多显式推进到下一拍；不会在等操作系统读写期间追赶另一个截止点。
    pub(super) fn advance(&mut self, secs: u64) {
        writeln!(self.child.0.stdin.as_mut().unwrap(), "{secs}").unwrap();
        self.ack("clock advanced");
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
        let _ = self.child.0.wait();
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
    let state = proxy::AppState::new(config.config);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let reader = tokio::task::spawn_blocking(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(line.parse::<u64>().unwrap()).is_err() {
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
    let (stop, _rx) = tokio::sync::watch::channel(false);
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
        behavior: daemon::Behavior::of(&state.config),
        run_dir: daemon::run_dir().display().to_string(),
        stats: state.stats.clone(),
        history: state.history.clone(),
        on_shutdown: stop,
        stop_requested: Default::default(),
    });
    let ipc = tokio::spawn(daemon::serve_ipc(endpoint, control));
    let router = proxy::router(state.clone());
    let http = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    println!("clock ready");
    while let Some(secs) = rx.recv().await {
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
