//! 体检报告面向终端的文字。机器报告保留等级、错误码及其结构化参数。

use aproxy_ipc::message;

message!(title(), en: "aProxy configuration check", zh: "aProxy 配置体检");

message!(healthy(), en: "All configurations passed. No problems found.", zh: "全部配置正常，未发现问题。");

message!(unset_setting(key: &str), en: "    Remove this key: aproxy settings unset {key}", zh: "    删除这个键: aproxy settings unset {key}");

message!(unset_config(file: &str, key: &str), en: "    Remove this key: aproxy config --config {file} unset {key}", zh: "    删除这个键: aproxy config --config {file} unset {key}");

message!(paused(), en: "Log cleanup is paused: run/ contains unreadable records.", zh: "日志清理已暂停：run/ 里有读不懂的记录。");

message!(summary(errors: usize, warnings: usize, infos: usize), en: "Results: {errors} error, {warnings} warning, {infos} info", zh: "检查结果: {errors} error, {warnings} warning, {infos} info");

message!(cannot_start(), en: "Instances cannot start with configuration errors. Most commands cannot run with errors in settings.json. Warnings and informational messages do not block startup.", zh: "有 error 的配置不能用来启动；settings.json 有 error 时多数命令不能运行。warning 与 info 仅供参考。");
