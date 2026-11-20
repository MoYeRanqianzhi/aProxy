//! 只在操作者明示同意后启动第三方程序。停止的是本对象持有的子进程，不按名称查找进程。
use aproxy_ipc::config_file::FormatEntry;
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

/// 静态不兼容不能因条目可选而忽略；没拿到协议行的进程死亡或超时才是不可用。
#[derive(Clone, Copy, Debug)]
pub(super) enum PeerFault {
    /// 未启动、未收到任何完整协议行就失联，或收发超时。
    Unavailable,
    /// 行超限、回信未经请求、JSON 损坏或握手与声明冲突。
    Invalid,
}
impl From<PeerFault> for String {
    fn from(fault: PeerFault) -> Self {
        match fault {
            PeerFault::Unavailable => "外部程序未按时回答",
            PeerFault::Invalid => "外部程序回信无效",
        }
        .into()
    }
}
/// 一次只允许一条未应答消息；超时的管道不能用于下一阶段。
pub(super) struct ChildPeer {
    /// 显式启动的进程句柄，超时只终止它。
    child: Child,
    /// 关闭发送端会在正常退出路径上关闭子进程的 stdin。
    input: Option<mpsc::Sender<(Vec<u8>, usize)>>,
    /// 只接收有上限的单帧，未封口的行不是应答。
    reply: mpsc::Receiver<Result<Value, PeerFault>>,
}
impl ChildPeer {
    /// 不经过 shell。私有参数与子进程的 stderr 都不出现在试跑的诊断中。
    pub fn start(entry: &FormatEntry, dir: &Path) -> Result<Self, String> {
        let cmd = aproxy_ipc::program::resolve(&entry.command, dir, &entry.env)
            .ok_or("无法定位外部程序")?;
        let mut child = Command::new(cmd)
            .current_dir(dir)
            .args(&entry.args)
            .envs(&entry.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "无法启动外部程序")?;
        let mut stdin = child.stdin.take().expect("管道输入");
        let mut stdout = BufReader::new(child.stdout.take().expect("管道输出"));
        let (input, recv) = mpsc::channel::<(Vec<u8>, usize)>();
        let (send, reply) = mpsc::channel();
        std::thread::spawn(move || {
            for (line, limit) in recv {
                let result = (|| {
                    // 子进程可能已留下诊断行再退出；写管道先报错也必须检查 stdout，
                    // 不能把已经收到的非协议输出按可选程序不可用跳过。
                    let written = stdin
                        .write_all(&line)
                        .and_then(|_| stdin.write_all(b"\n"))
                        .and_then(|_| stdin.flush());
                    let mut out = Vec::new();
                    stdout
                        .by_ref()
                        .take(limit as u64)
                        .read_until(b'\n', &mut out)
                        .map_err(|_| PeerFault::Unavailable)?;
                    if out.last() != Some(&b'\n') {
                        return Err(if out.len() == limit {
                            PeerFault::Invalid
                        } else {
                            PeerFault::Unavailable
                        });
                    }
                    // 请求未完整交付时，哪怕对端留下的是 JSON，也不是本帧的应答。
                    written.map_err(|_| PeerFault::Invalid)?;
                    serde_json::from_slice(&out).map_err(|_| PeerFault::Invalid)
                })();
                let failed = result.is_err();
                if send.send(result).is_err() || failed {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            input: Some(input),
            reply,
        })
    }
    /// 写入、刷新与读取共享时限，外部进程不能通过不读 stdin 让主线程卡死。
    pub fn frame(&self, msg: &Value, limit: usize, timeout: u64) -> Result<Value, PeerFault> {
        let bytes = serde_json::to_vec(msg).map_err(|_| PeerFault::Invalid)?;
        self.input
            .as_ref()
            .expect("子进程未退役")
            .send((bytes, limit))
            .map_err(|_| PeerFault::Unavailable)?;
        self.reply
            .recv_timeout(Duration::from_secs(if timeout == 0 {
                30
            } else {
                timeout.min(86400)
            }))
            .map_err(|_| PeerFault::Unavailable)?
    }
}
impl Drop for ChildPeer {
    /// 给正常 EOF 一个短暂的退出机会；不把持有终端的外部程序遗留给下一次试跑。
    fn drop(&mut self) {
        drop(self.input.take());
        for _ in 0..10 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 先回收子进程，固定「写 hello 前对端已关闭 stdin」的顺序，不靠操作系统调度碰运气。
#[cfg(test)]
#[test]
fn exited_peer_distinguishes_buffered_output_from_silent_eof() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = aproxy_ipc::config_file::parse(
        "[[format]]\ncommand='fixture'\nstages=['request']",
        &dir.path().join("config.toml"),
    );
    let mut entry = config.format.unwrap().remove(0);
    entry.command = std::env::current_exe().unwrap().to_str().unwrap().into();
    // libtest 列出一个测试后正常退出；不认识的选项只向 stderr 报错。两次都不会读 stdin。
    for (args, banner) in [
        (&["--list", "exited_peer_distinguishes"][..], true),
        (&["--unknown-fixture-option"][..], false),
    ] {
        entry.args = args.iter().map(|s| (*s).to_owned()).collect();
        let mut peer = ChildPeer::start(&entry, dir.path()).unwrap();
        assert_eq!(peer.child.wait().unwrap().success(), banner);
        let result = peer.frame(&serde_json::json!({"stage":"hello"}), 64 * 1024, 10);
        assert!(
            matches!(
                (&result, banner),
                (Err(PeerFault::Invalid), true) | (Err(PeerFault::Unavailable), false)
            ),
            "banner={banner}: {result:?}"
        );
    }
}
