//! 独立 skill 命令的终端提示；文档正文与机器文档不经过此处。
use aproxy_ipc::message;

message!(installed(version: &str, file_count: usize, location: &str), en: "Installed skill documentation for {version} ({file_count} files): {location}", zh: "已安装 {version} 的 skill 文档（{file_count} 个文件）：{location}");

message!(show(), en: "Run aproxy skill show <name> to read the documentation.", zh: "执行 aproxy skill show <name> 阅读文档。");

message!(headings(), en: "Name\tVersion\tMatches current version\tLocation\tFiles", zh: "名称\t版本\t与当前版本一致\t位置\t文件");

message!(yes(), en: "yes", zh: "是");

message!(no(), en: "no", zh: "否");
