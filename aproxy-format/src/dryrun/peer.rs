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

/// 一次只允许一条未应答消息；超时的管道不能用于下一阶段。
pub(super) struct ChildPeer {
    /// 显式启动的进程句柄，超时只终止它。
    child: Child,
    /// 关闭发送端会在正常退出路径上关闭子进程的 stdin。
    input: Option<mpsc::Sender<(Vec<u8>, usize)>>,
    /// 只接收有上限的单帧，未封口的行不是应答。
    reply: mpsc::Receiver<Result<Value, ()>>,
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
                    stdin.write_all(&line).ok()?;
                    stdin.write_all(b"\n").ok()?;
                    stdin.flush().ok()?;
                    let mut out = Vec::new();
                    stdout
                        .by_ref()
                        .take(limit as u64 + 1)
                        .read_until(b'\n', &mut out)
                        .ok()?;
                    (out.len() <= limit && out.last() == Some(&b'\n')).then_some(())?;
                    serde_json::from_slice(&out).ok()
                })()
                .ok_or(());
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
    pub fn frame(&self, msg: &Value, limit: usize, timeout: u64) -> Result<Value, String> {
        let bytes = serde_json::to_vec(msg).map_err(|_| "无法编码试跑消息")?;
        self.input
            .as_ref()
            .expect("子进程未退役")
            .send((bytes, limit))
            .map_err(|_| "外部程序已退出")?;
        self.reply
            .recv_timeout(Duration::from_secs(if timeout == 0 {
                30
            } else {
                timeout.min(86400)
            }))
            .ok()
            .and_then(Result::ok)
            .ok_or_else(|| "外部程序未按时给出有效回信".into())
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
