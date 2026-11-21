// manifest 和实际 invoke 分派共用这里的标识符。宏只拼接固定的 Rust 函数名，
// 不接受页面传入的操作名；最后一组只在 Windows 注册，但各平台发布相同的权限文件。
macro_rules! with_commands {
    ($use_list:ident) => { $use_list!([desktop_info], [rescan,attach,detach,ack,snapshot,query_logs,request_detail,body_chunk,cancel_detail,list_configs,start_instance,stop_instance,restart_instance,force_stop_instance,restore_instance,forget_restore,read_config,validate_config,write_config,read_settings,set_watchdog,read_desktop_state,set_preferences,set_pinned,about,check_update,install_update,read_autostart,set_autostart], [titlebar_insets,set_caption_color]) };
}
