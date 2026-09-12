//! 按原始词法边界检查类型成员之间的空行。

use rustc_lexer::{FrontmatterAllowed, TokenKind, tokenize};
use rustc_lint::{EarlyContext, EarlyLintPass};
use rustc_session::{declare_lint, impl_lint_pass};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};

use crate::source_files::{ParsedSource, Violation, check_sources};

declare_lint! {
    /// 要求具名字段和枚举变体之间留出空行。
    ///
    /// 元组结构体和元组变体的字段不检查间距，因为 rustfmt 会移除其字段间的空行。
    pub(crate) MINERAL_MEMBER_SPACING,
    Warn,
    "type members must be separated by a blank line"
}

/// 检查类型成员间距的编译器入口。
pub(crate) struct MemberSpacing;

impl_lint_pass!(MemberSpacing => [MINERAL_MEMBER_SPACING]);

impl EarlyLintPass for MemberSpacing {
    /// 检查当前 crate 的原始源码文件。
    ///
    /// # Params:
    ///   - `cx`: 编译器检查上下文。
    ///   - `_krate`: 当前 crate 的语法树。
    ///
    /// # Return:
    ///   诊断通过编译器上下文发出。
    fn check_crate(&mut self, cx: &EarlyContext<'_>, _krate: &rustc_ast::Crate) {
        check_sources(cx, MINERAL_MEMBER_SPACING, check);
    }
}

/// 检查语法树中所有结构体和枚举定义的成员间距。
///
/// # Params:
///   - `source`: 原始文件路径、文本与尚未展开宏的语法树。
///
/// # Return:
///   缺少前置空行的成员范围，包含该成员的属性和文档。
pub(super) fn check(source: &ParsedSource<'_>) -> Vec<Violation> {
    let mut visitor = MemberVisitor {
        text: source.text,
        violations: Vec::new(),
    };
    visitor.visit_file(source.syntax);
    visitor.violations
}

/// 保存原始源码与遍历过程中发现的成员间距问题。
struct MemberVisitor<'source> {
    /// 提供成员间隔的原始文本。
    text: &'source str,

    /// 每个缺少前置空行的成员对应一条诊断。
    violations: Vec<Violation>,
}

impl MemberVisitor<'_> {
    /// 检查同一类型或变体中相邻成员的原文间隔。
    ///
    /// # Params:
    ///   - `members`: 按源码顺序排列、范围包含属性与文档的成员。
    ///
    /// # Return:
    ///   将缺少空行的成员追加到诊断列表。
    fn check_members<'member, T: Spanned + 'member>(
        &mut self,
        members: impl IntoIterator<Item = &'member T>,
    ) {
        let mut previous_end = None;
        for member in members {
            let bytes = member.span().byte_range();
            if let Some(end) = previous_end
                && !self.text.get(end..bytes.start).is_some_and(has_blank_line)
            {
                self.violations.push(Violation {
                    bytes: bytes.clone(),
                    message: "insert a blank line before this member, including its attributes and documentation".to_owned(),
                });
            }
            previous_end = Some(bytes.end);
        }
    }
}

impl<'ast> Visit<'ast> for MemberVisitor<'_> {
    /// 检查结构体的具名字段，跳过 tuple struct 的字段间距并继续访问嵌套类型。
    ///
    /// # Params:
    ///   - `item`: 当前结构体定义。
    ///
    /// # Return:
    ///   收集结构体及其嵌套定义的诊断。
    fn visit_item_struct(&mut self, item: &'ast syn::ItemStruct) {
        if let syn::Fields::Named(fields) = &item.fields {
            self.check_members(&fields.named);
        }
        visit::visit_item_struct(self, item);
    }

    /// 检查枚举变体之间的空行，并继续访问每个变体。
    ///
    /// # Params:
    ///   - `item`: 当前枚举定义。
    ///
    /// # Return:
    ///   收集枚举及其嵌套定义的诊断。
    fn visit_item_enum(&mut self, item: &'ast syn::ItemEnum) {
        self.check_members(&item.variants);
        visit::visit_item_enum(self, item);
    }

    /// 检查枚举变体内部的具名字段，并继续访问所有字段中的嵌套类型。
    ///
    /// # Params:
    ///   - `variant`: 当前枚举变体。
    ///
    /// # Return:
    ///   收集变体字段及其嵌套定义的诊断。
    fn visit_variant(&mut self, variant: &'ast syn::Variant) {
        if let syn::Fields::Named(fields) = &variant.fields {
            self.check_members(&fields.named);
        }
        visit::visit_variant(self, variant);
    }
}

/// 判断原文间隔中是否存在包含至少两个换行的单个空白 token。
///
/// # Params:
///   - `gap`: 前一成员结束至当前成员属性或文档起点之间的原文。
///
/// # Return:
///   存在真正的空白行时返回 true；注释或字符串内部换行不计入。
fn has_blank_line(gap: &str) -> bool {
    let mut offset = 0;
    for token in tokenize(gap, FrontmatterAllowed::No) {
        let Ok(length) = usize::try_from(token.len) else {
            return false;
        };
        let end = offset + length;
        if token.kind == TokenKind::Whitespace
            && gap
                .get(offset..end)
                .is_some_and(|text| text.bytes().filter(|byte| *byte == b'\n').count() >= 2)
        {
            return true;
        }
        offset = end;
    }
    false
}
