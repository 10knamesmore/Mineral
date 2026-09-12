//! 为超过业务行数上限的源文件生成诊断。

use rustc_ast::Crate;
use rustc_lint::{EarlyContext, EarlyLintPass};
use rustc_session::{declare_lint, impl_lint_pass};

use crate::source_files::{ParsedSource, Violation, check_sources};

use super::line_count::{business_lines, test_only};

/// 单个源文件允许保留的业务行数。
const MAX_BUSINESS_LINES: usize = 800;

declare_lint! {
    /// 禁止扣除测试条目和纯注释后仍超过 800 行的业务源文件。
    pub(crate) MINERAL_BUSINESS_FILE_TOO_LONG,
    Deny,
    "business source file exceeds 800 lines excluding tests and comment-only lines"
}

/// 在 crate 检查入口按源文件统计业务行数。
pub(crate) struct BusinessFileTooLong;

impl_lint_pass!(BusinessFileTooLong => [MINERAL_BUSINESS_FILE_TOO_LONG]);

impl EarlyLintPass for BusinessFileTooLong {
    /// 检查本次编译涉及的源文件。
    ///
    /// # Params:
    ///   - `cx`: 编译器的早期 lint 上下文。
    ///   - `_krate`: 本次编译的 crate 语法树。
    ///
    /// # Return:
    ///   将超限文件的诊断提交给编译器。
    fn check_crate(&mut self, cx: &EarlyContext<'_>, _krate: &Crate) {
        check_sources(cx, MINERAL_BUSINESS_FILE_TOO_LONG, check);
    }
}

/// 检查业务行数；测试目录、测试文件和仅测试时启用的文件全部豁免。
///
/// # Params:
///   - `source`: 路径、原始文本及其未按 cfg 裁剪的语法树。
///
/// # Return:
///   未超限或属于测试文件时返回空列表，否则返回定位到文件开头的一条诊断。
pub(super) fn check(source: &ParsedSource<'_>) -> Vec<Violation> {
    if source
        .path
        .components()
        .any(|component| component.as_os_str() == "tests")
        || source
            .path
            .file_name()
            .is_some_and(|name| name == "tests.rs")
        || test_only(&source.syntax.attrs)
    {
        return Vec::new();
    }

    let lines = business_lines(source);
    if lines <= MAX_BUSINESS_LINES {
        return Vec::new();
    }

    vec![Violation {
        bytes: 0..0,
        message: format!(
            "business source file has {lines} lines excluding tests and comment-only lines (maximum {MAX_BUSINESS_LINES})"
        ),
    }]
}
