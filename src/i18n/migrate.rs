//! 迁移的终端和报告用语。JSON 读者始终选择英文。
use aproxy_ipc::message;
message!(manual(), en: "Choose the format program, stages and arguments before migrating this file.", zh: "请先确定 format 的程序、阶段与参数，再迁移此文件。");
message!(invalid(), en: "This value or file is not valid in the target format.", zh: "此值或文件不符合目标格式。");
message!(unknown(), en: "Unrecognized key; it is not a documented 0.1.x rename.", zh: "无法识别此键；它不在 0.1.x 的改名表中。");
message!(credential(), en: "Set defaults.base_url to the intended credential target first.", zh: "请先设置 defaults.base_url，明确此凭据所属的上游。");
message!(collision(key:&str), en: "Cannot move this key without overwriting {key}.", zh: "迁移此键会覆盖 {key}。");
message!(different_backup(), en: "The existing .0.1.bak is not an identical regular file; neither file was replaced.", zh: "现有 .0.1.bak 不是内容相同的普通文件；两份文件都未被替换。");
message!(changed(), en: "The file changed during migration; the external edit was not overwritten.", zh: "文件在迁移期间被修改，未覆盖外部改动。");
message!(restored(), en: "Startup validation failed; restored the original from .0.1.bak.", zh: "启动校验未通过，已从 .0.1.bak 恢复原文。");
message!(io(error:std::io::ErrorKind), en: "File operation failed: {error}", zh: "文件操作失败：{error}");
message!(reserved(), en: "Migration does not write runtime records or migration backups.", zh: "迁移不会写入运行记录或迁移备份。");
message!(invalid_settings(), en: "Repair settings.json first so startup can validate this configuration.", zh: "请先修复 settings.json，才能按启动规则校验此配置。");
