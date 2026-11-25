//! 迁移的终端和报告用语。JSON 读者始终选择英文。
use aproxy_ipc::message;
message!(manual(), en: "Choose the format program, stages and arguments before migrating this file.", zh: "请先确定 format 的程序、阶段与参数，再迁移此文件。");
message!(invalid(), en: "This value or file is not valid in the target format.", zh: "此值或文件不符合目标格式。");
message!(unknown(), en: "Unrecognized key; it is not a documented 0.1.x rename.", zh: "无法识别此键；它不在 0.1.x 的改名表中。");
message!(credential(), en: "Set defaults.base_url first, so it is clear which upstream this credential belongs to.", zh: "请先设置 defaults.base_url，明确此凭据所属的上游。");
message!(collision(key:&str), en: "Cannot move this key without overwriting {key}.", zh: "迁移此键会覆盖 {key}。");
message!(different_backup(), en: "Cannot reuse the existing .0.1.bak as an identical private backup; neither file was replaced.", zh: "无法将现有 .0.1.bak 用作内容相同的私有备份；两份文件都未被替换。");
message!(changed(), en: "The file changed during migration; the external edit was not overwritten.", zh: "文件在迁移期间被修改，未覆盖外部改动。");
message!(restored(), en: "Startup validation failed; restored the original from .0.1.bak.", zh: "启动校验未通过，已从 .0.1.bak 恢复原文。");
message!(io(error:std::io::ErrorKind), en: "File operation failed: {error}", zh: "文件操作失败：{error}");
message!(reserved(), en: "This file is a migration backup or a runtime record; migrate does not rewrite it.", zh: "这是迁移备份或运行记录，迁移不会改写它。");
message!(invalid_settings(), en: "Repair settings.json first so startup can validate this configuration.", zh: "请先修复 settings.json，才能按启动规则校验此配置。");
message!(duplicate(), en: "This member occurs more than once; choose which value to keep.", zh: "此成员出现了多次，请选择要保留的值。");
#[cfg(unix)]
message!(linked(), en: "This file has other hard links; migrating it would separate them.", zh: "此文件还有其他硬链接，迁移会把它们分开。");
message!(settings_target(), en: "To migrate settings.json, run migrate without paths.", zh: "要迁移 settings.json，请运行不带路径的 migrate。");
message!(missing_target(), en: "This alias or default_config points to a file that does not exist; remove the alias or restore the file.", zh: "这个别名或 default_config 指向的文件不存在；请删除别名或恢复文件。");
message!(missing_dir(), en: "This config_dirs entry does not exist; remove it from settings.json or create it.", zh: "config_dirs 中的这个目录不存在；请从 settings.json 删除它或创建这个目录。");
