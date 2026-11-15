//! 库不替调用方说话，也不替调用方结束进程。
//!
//! 库（`src/lib.rs` 及其子模块）的调用方有两个：命令行把操作的结果渲染成文字、决定退出码；
//! 桌面界面在自己的进程里调用同一批函数。库里的 `println!` 界面拿不到，`process::exit` 会
//! 把界面一起带走——所以这两样只许出现在二进制一侧（`src/main.rs`、`src/commands/`、
//! `src/server.rs`、`src/util.rs`）。这个测试逐个扫描库的源文件，守着这条线。aproxy-ipc 是
//! 更多程序（界面、第三方客户端）要链接的库，同样在扫描之内。
//!
//! 不在检查之内的：
//! - 测试代码（`#[cfg(test)]` 的条目）与注释；
//! - `tracing` 日志——它去哪里由进程入口决定，不是库在说话；
//! - 强制安装（`src/install/force.rs`）经 `outln!`、`errln!` 写出的文字。那是永久兼容契约
//!   C7 规定的输出，写失败不得改变退出码，所以它不用 `println!`，也就不在下面的模式里；
//!   它把退出码交还调用方，自己不退出。

use std::path::{Path, PathBuf};

/// 二进制一侧的源文件与目录（相对 `src/`）：这里可以打印、可以退出。
const BINARY_SIDE: &[&str] = &["main.rs", "server.rs", "util.rs", "commands"];

/// 库的非测试代码里不许出现的写法。
const FORBIDDEN: &[&str] = &[
    "println!",
    "eprintln!",
    "print!(",
    "eprint!(",
    "dbg!(",
    "process::exit",
];

/// `src/` 下属于库的全部 `.rs` 文件。
fn library_sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = root.join("src");
    let mut out = Vec::new();
    // aproxy-ipc 整个是库，没有二进制一侧
    walk(&root.join("aproxy-ipc").join("src"), &mut out);
    for entry in std::fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if BINARY_SIDE.contains(&name.as_str()) {
            continue;
        }
        if path.is_dir() {
            walk(&path, &mut out);
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// 一个源文件里的非测试、非注释代码行，连同行号。
///
/// 测试条目的识别：`#[cfg(test)]` 或 `#[cfg(all(test, ...))]` 这一行之后的那个条目，从它
/// 第一个 `{` 起到配对的 `}` 为止整个跳过（没有花括号、以 `;` 结束的条目跳到分号）。花括号
/// 按字符数，不解析字符串——格式串里的 `{}` 成对出现，不影响配对。
fn production_lines(source: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    // Some(depth)：正在跳过一个测试条目；depth 是已经进入的花括号层数，0 = 还没见到 `{`
    let mut skipping: Option<usize> = None;
    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(depth) = skipping.as_mut() {
            if trimmed.starts_with("//") || trimmed.starts_with("#[") {
                continue;
            }
            let opened = line.matches('{').count();
            let closed = line.matches('}').count();
            let entered = *depth > 0 || opened > 0;
            *depth = (*depth + opened).saturating_sub(closed);
            if (entered && *depth == 0) || (!entered && trimmed.ends_with(';')) {
                skipping = None;
            }
            continue;
        }
        if trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("#[cfg(all(test") {
            skipping = Some(0);
            continue;
        }
        if trimmed.starts_with("//") {
            continue;
        }
        out.push((index + 1, line));
    }
    out
}

#[test]
fn library_neither_prints_nor_exits() {
    let sources = library_sources();
    // 扫描范围要真的覆盖到库：这几个文件在，说明目录遍历没有落空
    for expected in ["lib.rs", "daemon.rs", "watchdog.rs"] {
        assert!(
            sources.iter().any(|p| p.ends_with(expected)),
            "没扫到 src/{expected}"
        );
    }
    assert!(
        sources.iter().any(|p| p.ends_with("ops/stop.rs")),
        "没扫到操作层"
    );
    assert!(
        sources
            .iter()
            .any(|p| p.ends_with("aproxy-ipc/src/ledger.rs")),
        "没扫到 aproxy-ipc"
    );
    assert!(
        !sources.iter().any(|p| p.ends_with("main.rs")),
        "二进制入口不该在扫描范围里"
    );

    let mut violations = Vec::new();
    for path in &sources {
        let source = std::fs::read_to_string(path).unwrap();
        for (number, line) in production_lines(&source) {
            for pattern in FORBIDDEN {
                if line.contains(pattern) {
                    violations.push(format!(
                        "{}:{number}: {}",
                        path.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                            .unwrap_or(path)
                            .display(),
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "库的非测试代码里出现了打印或 process::exit。把结果交回调用方，由命令行去说、去退出：\n{}",
        violations.join("\n")
    );
}

#[test]
fn scanner_skips_tests_and_comments_only() {
    let source = r#"
fn kept() {
    work(); // 尾注释不影响这一行
}
// println!("注释里的不算");
/// eprintln!("文档注释里的也不算");
#[cfg(test)]
mod tests {
    #[test]
    fn t() {
        println!("测试里的不算 {}", 1);
    }
}
#[cfg(test)]
use std::process::exit;
fn after() {
    std::process::exit(1);
}
"#;
    let lines: Vec<&str> = production_lines(source)
        .into_iter()
        .map(|(_, l)| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    assert_eq!(
        lines,
        [
            "fn kept() {",
            "work(); // 尾注释不影响这一行",
            "}",
            "fn after() {",
            "std::process::exit(1);",
            "}",
        ]
    );
}
