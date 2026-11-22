//! 只根据已验证的进程身份说明终止结果，不从台账的原始说明猜测类别。

use aproxy_ipc::message;

message!(unmarked(pid: u32), en: "Pid {pid} has no registered start marker. Its identity cannot be verified. It was not terminated.", zh: "pid {pid} 没有登记启动标记，无法核验身份，拒绝终止以防误杀");

message!(gone(pid: u32), en: "Pid {pid} no longer identifies the recorded process (it exited or the pid was reused). It was not terminated.", zh: "pid {pid} 已不是记录里的那个进程（已退出，或 pid 已被复用），未终止");

message!(other(pid: u32, owner: &str), en: "Pid {pid} belongs to another user ({owner}). It was not terminated.", zh: "pid {pid} 属于别的用户（{owner}），未终止");

message!(failed(pid: u32, why: &str), en: "Could not terminate pid {pid} ({why})", zh: "终止 pid {pid} 失败（{why}）");
