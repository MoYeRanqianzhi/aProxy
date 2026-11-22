//! 安装命令对终端或事件首行给出的说明。
use aproxy_ipc::message;
message!(failed(cause: &str), en: "Installation failed: {cause}", zh: "安装失败: {cause}");
message!(correct(), en: "Correct the problems above and run aproxy install again.", zh: "修正上面的问题后重新执行 aproxy install。");
