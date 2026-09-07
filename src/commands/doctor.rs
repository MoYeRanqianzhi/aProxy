//! `aproxy doctor`：配置体检。

use aproxy::doctor;
use aproxy::settings;

/// `aproxy doctor`：配置体检。settings.json 的 error 级检查每次软件运行都会
/// 执行；别名配置深入审查与目录 toml 扫描（warning 级）只在此处执行。
pub(crate) async fn handle_doctor_cmd() {
    println!("aProxy 配置体检");
    println!("════════════════");
    let report = doctor::run(&settings::settings_path());
    if report.is_clean() {
        println!("全部配置正常，未发现问题。");
        return;
    }
    report.print();
    if report.error_count() > 0 {
        println!(
            "\nerror 级问题会影响按名字的 start/stop 等操作，建议先修复；warning 级仅供参考。"
        );
    }
}
