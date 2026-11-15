//! AGENTS.md 与 CLAUDE.md 里讲永久兼容契约的那一段：两份文件是各自独立的副本，这里核对
//! 它们逐字一致、列出的冻结文件都还在。冻结的代码搬了位置而这一段没跟上，读它的人（与
//! agent）就会去一个不存在的文件里找契约。

use std::path::Path;

/// 取出以这句话开头的那一段（到行尾为止，Markdown 里一段就是一行）。
fn contract_paragraph(file: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{file}: {e}"));
    text.lines()
        .find(|line| line.starts_with("The force-install contract (C1-C8, frozen in "))
        .unwrap_or_else(|| panic!("{file} 里找不到讲永久兼容契约的那一段"))
        .to_string()
}

#[test]
fn the_contract_paragraph_is_identical_and_names_existing_files() {
    let agents = contract_paragraph("AGENTS.md");
    assert_eq!(
        agents,
        contract_paragraph("CLAUDE.md"),
        "AGENTS.md 与 CLAUDE.md 讲永久兼容契约的那一段必须逐字一致"
    );

    // 「frozen in … ) is permanent」之间用反引号列出的文件
    let listed = agents
        .split_once("frozen in ")
        .and_then(|(_, rest)| rest.split_once(") is permanent"))
        .map(|(files, _)| files)
        .expect("那一段应以「frozen in <文件清单>) is permanent」开头");
    let files: Vec<&str> = listed.split('`').skip(1).step_by(2).collect();
    assert_eq!(
        files,
        [
            "aproxy-ipc/src/home.rs",
            "aproxy-ipc/src/ledger.rs",
            "src/install/force.rs",
            "tests/force_contract.rs",
        ],
        "冻结代码搬了位置的话，两份文件与这里的清单一起改"
    );
    for file in files {
        assert!(
            Path::new(env!("CARGO_MANIFEST_DIR")).join(file).is_file(),
            "契约段列出的 {file} 不存在"
        );
    }
    assert!(
        agents.ends_with(
            "The contract tests freeze their assertions, not where the code lives: a `use` path may follow code that moves."
        ),
        "末句要写明冻结的是断言、`use` 路径可以随代码移动"
    );
}
