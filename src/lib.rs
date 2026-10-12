pub mod config;
pub mod daemon;
pub mod decode;
pub mod doctor;
pub mod find;
pub mod install;
pub mod ledger;
pub mod proxy;
pub mod retry;
pub mod settings;
pub mod transform;
pub mod watchdog;

// `--version` 那一行的变体段与指令集段。写成宏而不是 const：concat! 只接受
// 字面量（以及展开成字面量的宏），不接受 const。每种取值各占一个宏，build_variant
// 与 build_cpu_level 按 cfg 选用其中一个；单元测试拿全部取值逐一断言，所以无论
// 这次构建选的是哪一个，改了任何一个取值都逃不过测试。在非测试构建里没被选中的
// 取值宏用不上，故允许 unused_macros。

/// 变体段：desktop（带桌面界面，默认）
#[cfg_attr(not(test), allow(unused_macros))]
macro_rules! variant_desktop {
    () => {
        "desktop"
    };
}
/// 变体段：headless（`--no-default-features`，无界面）
#[cfg_attr(not(test), allow(unused_macros))]
macro_rules! variant_headless {
    () => {
        "headless"
    };
}
/// 指令集段：baseline 产物与非 x86_64 目标没有这一段
#[cfg_attr(not(test), allow(unused_macros))]
macro_rules! cpu_level_baseline {
    () => {
        ""
    };
}
/// 指令集段：发布的 -v3 产物以 `-C target-cpu=x86-64-v3` 构建，编译期就开着
/// avx2、fma、bmi2
#[cfg_attr(not(test), allow(unused_macros))]
macro_rules! cpu_level_x86_64_v3 {
    () => {
        "; x86-64-v3"
    };
}

#[cfg(feature = "desktop")]
macro_rules! build_variant {
    () => {
        variant_desktop!()
    };
}
#[cfg(not(feature = "desktop"))]
macro_rules! build_variant {
    () => {
        variant_headless!()
    };
}

#[cfg(all(
    target_arch = "x86_64",
    target_feature = "avx2",
    target_feature = "fma",
    target_feature = "bmi2"
))]
macro_rules! build_cpu_level {
    () => {
        cpu_level_x86_64_v3!()
    };
}
#[cfg(not(all(
    target_arch = "x86_64",
    target_feature = "avx2",
    target_feature = "fma",
    target_feature = "bmi2"
)))]
macro_rules! build_cpu_level {
    () => {
        cpu_level_baseline!()
    };
}

/// `aproxy --version` 输出的那一行（标准输出第一行），例如
/// `aproxy v0.2.0 (desktop; x86_64-pc-windows-msvc; x86-64-v3)`：
/// `aproxy v<semver> (<变体>; <目标三元组>[; x86-64-v3])`。
///
/// 永久兼容契约 C5：所有版本的安装器都靠这一行认版本，格式无论如何都不得改动。
/// 安装器下载或拿到一个二进制，先在本机执行它的 `--version`，取首行 `aproxy v`
/// 之后、到空格为止的部分当作它的版本（见 install::staging::parse_version_line）
/// ——今后任何版本的安装器都这样读任何版本的二进制。变体段与目标段让安装器升级
/// 时选对同一种产物。`)` 之后不得追加任何内容；读者只取版本号之后的第一个括号组、
/// 忽略其后的内容（见 install::force::VersionLine），那是容错，不是许可。退出码 0。
///
/// 以空白分隔的每个词都不得以数字开头，括号里的变体段、目标段、指令集段也一样。
/// 0.1.0 的安装器把 `--version` 输出里第一个以数字开头的词当版本：一个都取不到，
/// 它就报「--version 输出无版本号」，在交换二进制之前中止、不改动任何东西——0.2.0
/// 起不支持从 0.1.0 原地升级，靠的就是这一点。只要有一个词以数字开头，它就会把
/// 那个词当成版本号：`--from` 直接拿它当目标版本装下去，一路走到交换；在线安装时
/// 那个词碰巧等于目标版本也一样。现在版本前面有 `v`，变体段与指令集段是上面写死的
/// 字面量，rustc 的目标三元组都以字母开头；下面的单元测试对两种变体、rustc 支持的
/// 全部目标三元组、有无指令集段的每种组合逐一断言。
///
/// 单独的 `--version` / `-V`（安装器就是这样调用的）在 main 的第一句就输出并返回，
/// 先于一切副作用：不切控制台代码页、不读写 home、不联网、不拉起任何进程。以后在
/// 解析参数之前要做的事（例如读 settings.json 判定界面语言）都必须排在它后面：
/// 安装器要在任何机器、任何状态下读到这一行，settings.json 损坏或 home 不可写都
/// 不能挡住它，探测版本本身也不该改动用户的环境。
pub const VERSION_LINE: &str = concat!(
    "aproxy v",
    env!("CARGO_PKG_VERSION"),
    " (",
    build_variant!(),
    "; ",
    env!("APROXY_BUILD_TARGET"),
    build_cpu_level!(),
    ")"
);

#[cfg(test)]
mod tests {
    /// 按 VERSION_LINE 的格式拼出任意一种产物的版本行
    fn version_line(variant: &str, target: &str, cpu_level: &str) -> String {
        format!(
            "aproxy v{} ({variant}; {target}{cpu_level})",
            env!("CARGO_PKG_VERSION")
        )
    }

    /// 任何产物的版本行里，以空白分隔的词都不以数字开头（为什么见 VERSION_LINE），
    /// 新安装器也都读得回版本。变体段与指令集段取上面每个取值宏，不管这次构建选了
    /// 哪个；目标三元组取 rustc 支持的全部目标，不只发布矩阵里那几个：cargo 渠道与
    /// 源码构建可以为任何目标编译，目标段就是构建时的 TARGET。
    #[test]
    fn no_word_of_any_version_line_starts_with_a_digit() {
        // 先确认这里拼出的就是常量的格式，下面对各种组合的断言才作数
        assert_eq!(
            version_line(
                build_variant!(),
                env!("APROXY_BUILD_TARGET"),
                build_cpu_level!()
            ),
            crate::VERSION_LINE
        );
        let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
        let out = std::process::Command::new(&rustc)
            .args(["--print", "target-list"])
            .output()
            .unwrap_or_else(|e| panic!("无法运行 {rustc} --print target-list: {e}"));
        assert!(out.status.success(), "{rustc} --print target-list 退出非零");
        let list = String::from_utf8(out.stdout).expect("rustc 的目标列表是 UTF-8");
        let targets: Vec<&str> = list.split_whitespace().collect();
        // 列表取得不对（空的、截断的）在这里暴露：构建本身的目标一定在里面
        assert!(targets.contains(&env!("APROXY_BUILD_TARGET")), "{list}");
        for target in targets {
            for variant in [variant_desktop!(), variant_headless!()] {
                for cpu_level in [cpu_level_baseline!(), cpu_level_x86_64_v3!()] {
                    let line = version_line(variant, target, cpu_level);
                    // 与 0.1.0 的 probe_version 取词的方式一模一样
                    let probed = line
                        .split_whitespace()
                        .find(|word| word.starts_with(|c: char| c.is_ascii_digit()));
                    assert_eq!(probed, None, "{line}");
                    assert_eq!(
                        crate::install::staging::parse_version_line(&line).as_deref(),
                        Some(env!("CARGO_PKG_VERSION")),
                        "{line}"
                    );
                }
            }
        }
    }
}
