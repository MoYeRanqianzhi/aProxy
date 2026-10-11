//! 根包（aproxy）的构建脚本：把只有构建期才知道的信息导出成编译期常量。
//!
//! 以后桌面界面（Tauri）的构建步骤也加在 `main` 里，各步互不依赖，按需追加即可。

fn main() {
    // 目标三元组（如 x86_64-pc-windows-msvc）：cargo 只把 TARGET 交给构建脚本，
    // 代码本身拿不到，cfg 也拼不出完整的三元组。导出后代码用
    // env!("APROXY_BUILD_TARGET") 读取，`aproxy --version` 的目标段就是它
    // （src/lib.rs 的 VERSION_LINE）。
    let target = std::env::var("TARGET").expect("cargo 总会给构建脚本设置 TARGET");
    println!("cargo::rustc-env=APROXY_BUILD_TARGET={target}");
    // 只在本脚本变化时重跑。不声明的话，包里任何文件一改，cargo 都会重跑它
    println!("cargo::rerun-if-changed=build.rs");
}
