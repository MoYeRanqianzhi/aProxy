//! 迁移报告的终端标题与运行中实例的说明。
use aproxy_ipc::message;
message!(current_file(), en: "Already current", zh: "已是当前格式");
message!(migrated(), en: "Migrated", zh: "已迁移");
message!(preview(), en: "Would migrate", zh: "将迁移");
message!(review(), en: "Left unchanged; review needed", zh: "未改动，需要人工确认");
message!(failed(), en: "Failed", zh: "失败");
message!(running(), en: "Running instances keep the configuration they loaded; restart them to use the migrated files.", zh: "运行中的实例仍用启动时加载的配置，重启后才会用迁移后的文件。");
