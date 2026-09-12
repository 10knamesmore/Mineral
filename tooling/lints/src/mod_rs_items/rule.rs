//! 检查模块入口文件中的顶层条目。

use std::ffi::OsStr;

use rustc_lint::{EarlyContext, EarlyLintPass};
use rustc_session::{declare_lint, impl_lint_pass};
use syn::spanned::Spanned;
use syn::{Item, Visibility};

use crate::source_files::{ParsedSource, Violation, check_sources};

declare_lint! {
    /// 禁止在 mod.rs 中放置外置模块声明与重导出以外的条目。
    pub(crate) MINERAL_MOD_RS_ITEMS,
    Deny,
    "mod.rs may only contain module declarations and re-exports"
}

/// 检查模块入口文件职责的编译器入口。
pub(crate) struct ModRsItems;

impl_lint_pass!(ModRsItems => [MINERAL_MOD_RS_ITEMS]);

impl EarlyLintPass for ModRsItems {
    /// 检查当前 crate 的原始源码文件。
    ///
    /// # Params:
    ///   - `cx`: 编译器检查上下文。
    ///   - `_krate`: 当前 crate 的语法树。
    ///
    /// # Return:
    ///   诊断通过编译器上下文发出。
    fn check_crate(&mut self, cx: &EarlyContext<'_>, _krate: &rustc_ast::Crate) {
        check_sources(cx, MINERAL_MOD_RS_ITEMS, check);
    }
}

/// 找出 mod.rs 中既非外置模块声明、也非带可见性重导出的条目。
///
/// # Params:
///   - `source`: 原始文件路径、文本与尚未展开宏的语法树。
///
/// # Return:
///   每个违规条目的完整字节范围与诊断文案。
pub(super) fn check(source: &ParsedSource<'_>) -> Vec<Violation> {
    if source.path.file_name() != Some(OsStr::new("mod.rs")) {
        return Vec::new();
    }

    source
        .syntax
        .items
        .iter()
        .filter(|item| match item {
            Item::Mod(module) => module.content.is_some(),
            Item::Use(import) => matches!(import.vis, Visibility::Inherited),
            _ => true,
        })
        .map(|item| Violation {
            bytes: item.span().byte_range(),
            message: "mod.rs may only contain module declarations and re-exports; move this implementation into a named source file".to_owned(),
        })
        .collect()
}
