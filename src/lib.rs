pub mod config;
pub mod daemon;
pub mod decode;
pub mod doctor;
pub mod find;
pub mod install;
pub mod proxy;
pub mod retry;
pub mod settings;
pub mod transform;
pub mod watchdog;

// `--version` 那一行的变体段与指令集段。写成宏而不是 const：concat! 只接受
// 字面量（以及展开成字面量的宏），不接受 const。

/// 变体段：desktop（带桌面界面，默认）或 headless（`--no-default-features`）
#[cfg(feature = "desktop")]
macro_rules! build_variant {
    () => {
        "desktop"
    };
}
#[cfg(not(feature = "desktop"))]
macro_rules! build_variant {
    () => {
        "headless"
    };
}

/// 指令集段：发布的 -v3 产物以 `-C target-cpu=x86-64-v3` 构建，编译期就开着
/// avx2、fma、bmi2；baseline 产物与非 x86_64 目标没有这一段
#[cfg(all(
    target_arch = "x86_64",
    target_feature = "avx2",
    target_feature = "fma",
    target_feature = "bmi2"
))]
macro_rules! build_cpu_level {
    () => {
        "; x86-64-v3"
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
        ""
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
/// 时选对同一种产物。
///
/// 这一格式还让 0.1.0 的安装器在交换二进制之前失败：0.1.0 取 `--version` 输出里
/// 第一个以数字开头的词当版本，而这一行的每个词都不以数字开头（`v0.2.0…`、变体、
/// 目标三元组都以字母开头），它取不到版本就在交换之前中止，不改动任何东西。
/// 所以任何一段都不能以数字开头。
///
/// 输出它的代码在 main.rs 最前面，先于一切副作用（不读写 home、不联网、不拉起
/// 任何进程）。
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
