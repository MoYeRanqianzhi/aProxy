//! 按期望状态记录恢复实例的命令输出。
use aproxy_ipc::message;
message!(failed(port:&str,error:&str,excerpt:&str), en:"Restore failed on port {port}: {error}{excerpt}", zh:"端口 {port} 恢复失败：{error}{excerpt}");
