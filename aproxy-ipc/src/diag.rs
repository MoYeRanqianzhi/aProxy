//! 配置诊断：config.toml 与 settings.json 里的一个问题。
//!
//! 诊断出现在命令行（start、doctor、`config show`）、`--json` 的 `config_invalid` 的
//! `details.diagnostics`，以及界面里。码取自码表的诊断族（[`crate::code`]）；`message` 是
//! 一句英文，显示端按码与 `details` 去说各自语言的话。

use serde::Serialize;
use serde_json::{Map, Value};

/// 诊断的级别。只有 `Error` 让配置无效（启动失败、退出码 3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Error,
    Warning,
    Info,
}

/// 一条诊断。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Diagnostic {
    /// 码表里诊断族的名字
    pub code: &'static str,
    pub severity: Level,
    /// 出问题的文件
    pub file: String,
    /// 出问题的键的路径，例如 `timeouts.connect_secs`、`format[1].stages`、`retry.rules[0].path`；
    /// 问题不属于某个键时（语法错误）没有
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// 行号，从 1 起
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// 列号，从 1 起，按字符数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    /// 一句英文说明
    pub message: String,
    /// 码表登记的 `details`
    pub details: Map<String, Value>,
}

impl Diagnostic {
    pub fn is_error(&self) -> bool {
        self.severity == Level::Error
    }
}

/// 一组诊断里有没有 error 级的。
pub fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics.iter().any(Diagnostic::is_error)
}

/// 文本里字节偏移 `offset` 处的行号与列号（都从 1 起，列按字符数）。偏移越界时落在末尾。
pub fn line_column(text: &str, offset: usize) -> (u32, u32) {
    let mut end = offset.min(text.len());
    // 偏移落在多字节字符中间时退到它的开头
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let before = &text[..end];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    (line as u32, column as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_and_column_are_one_based_and_count_characters() {
        let text = "a = 1\n键 = \"值\"\nlast";
        assert_eq!(line_column(text, 0), (1, 1));
        assert_eq!(line_column(text, 4), (1, 5));
        // 第二行开头，以及多字节字符之后的列
        let second = text.find('键').unwrap();
        assert_eq!(line_column(text, second), (2, 1));
        assert_eq!(line_column(text, second + '键'.len_utf8()), (2, 2));
        // 落在多字节字符中间：退到它的开头
        assert_eq!(line_column(text, second + 1), (2, 1));
        assert_eq!(line_column(text, text.len() + 10), (3, 5));
    }

    #[test]
    fn serializes_without_absent_position() {
        let diagnostic = Diagnostic {
            code: "config_syntax",
            severity: Level::Error,
            file: "config.toml".into(),
            key: None,
            line: Some(3),
            column: None,
            message: "x".into(),
            details: Map::new(),
        };
        assert_eq!(
            serde_json::to_value(&diagnostic).unwrap(),
            serde_json::json!({
                "code": "config_syntax",
                "severity": "error",
                "file": "config.toml",
                "line": 3,
                "message": "x",
                "details": {},
            })
        );
        assert!(has_errors(&[diagnostic]));
    }
}
