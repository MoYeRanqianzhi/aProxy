//! 只在带窗口的版本报告菜单集成。
use aproxy_ipc::message;
message!(shortcut_changed(files: &str), en: "Updated the application menu entry: {files}.", zh: "已更新应用菜单项：{files}。");
message!(shortcut_present(), en: "The application menu entry is already configured.", zh: "应用菜单项已配置，无需更改。");
message!(shortcut_failed(reason: &str, exe: impl std::fmt::Display, home: impl std::fmt::Display), en: "Warning: application menu setup failed: {reason}. Create a launcher for {exe} with argument desktop and APROXY_HOME set to {home}.", zh: "警告：应用菜单项配置失败：{reason}。请手动创建启动项，程序为 {exe}，参数为 desktop，环境变量 APROXY_HOME 设为 {home}。");
message!(shortcut_skipped(), en: "Skipped the application menu entry (--no-shortcut).", zh: "已跳过应用菜单项（--no-shortcut）。");
