//! 进程入口的诊断。保护协议中的摘要、原因和终端上的修复说明有不同的读者。

use aproxy_ipc::message;

message!(relative_home(home_env: &str, path: &str), en: "{home_env} must be an absolute path, not {path:?}. A relative path selects a different home in each working directory. Set an absolute path or unset the variable to use <user directory>/.aproxy.", zh: "{home_env} 必须是绝对路径，现在是 {path:?}：相对路径会随工作目录解析成不同的目录。改成绝对路径，或者删掉这个环境变量（默认用 <用户目录>/.aproxy）。");
message!(missing_home(user_env: &str, home_env: &str), en: "Cannot find the user directory: {user_env} is empty and the system did not return a directory. Set {home_env} to an absolute path.", zh: "取不到用户目录（{user_env} 为空，系统里也查不到），无法确定 aproxy 的主目录；请把 {home_env} 设为绝对路径。");
message!(settings_errors(), en: "settings.json has errors:", zh: "settings.json 有错误:");
message!(using_defaults(), en: "Using built-in defaults for this command. Aliases are unavailable; select instances by port or all. Inspect with aproxy settings show and repair with aproxy settings unset <key>.", zh: "本次按内置默认运行（别名不可用，请按端口或 all 指定实例）。用 aproxy settings show 查看、aproxy settings unset <键> 修复");
message!(missing_alias(name: &str, path: &str), en: "Alias \"{name}\" points to a missing configuration file: {path} (start and stop cannot use this alias)", zh: "别名 \"{name}\" 指向的配置文件不存在: {path}（start/stop 该别名会失败）");
message!(repair_keys(), en: "Remove the invalid keys below or correct their values with aproxy settings set <key> <value>:", zh: "逐项修复（删掉出问题的键，或用 aproxy settings set <键> <值> 改对）:");
message!(remove_settings(path: std::path::Display<'_>), en: "To return to built-in defaults, delete the file. This also removes aliases and global defaults: {path}", zh: "也可以删除整个文件回到内置默认（别名与全局默认会一并丢失）: {path}");
