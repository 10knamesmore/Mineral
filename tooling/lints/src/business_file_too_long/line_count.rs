//! 统计保留空行、排除纯注释和明确测试条目后的源文件行数。

use proc_macro2::Span;
use rustc_lexer::{FrontmatterAllowed, TokenKind};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Attribute, ImplItemFn, Item, Meta, Token};

use crate::source_files::ParsedSource;

/// 合并纯注释行和测试条目的整行范围，再统计其余物理行。
///
/// # Params:
///   - `source`: 原始文本及其语法树，二者必须来自同一个文件。
///
/// # Return:
///   业务行数，包含空行和未启用 cfg 的非测试代码。
pub(super) fn business_lines(source: &ParsedSource<'_>) -> usize {
    let mut visitor = TestLines {
        excluded: comment_only_lines(source.text),
    };
    visitor.visit_file(source.syntax);
    visitor
        .excluded
        .iter()
        .filter(|excluded| !**excluded)
        .count()
}

/// 根据词法 token 标记纯注释行，字符串内的注释符属于代码。
///
/// # Params:
///   - `text`: 完整源文件文本。
///
/// # Return:
///   每个物理行对应一个标记；纯注释行为 true，空行和含代码的行为 false。
fn comment_only_lines(text: &str) -> Vec<bool> {
    let mut excluded = Vec::with_capacity(text.lines().count());
    let (mut has_code, body) = match rustc_lexer::strip_shebang(text) {
        Some(length) => (true, text.split_at(length).1),
        None => (false, text),
    };
    let mut has_comment = false;
    let mut bytes = body.bytes();

    for token in rustc_lexer::tokenize(body, FrontmatterAllowed::No) {
        let is_comment = matches!(
            token.kind,
            TokenKind::LineComment { .. } | TokenKind::BlockComment { .. }
        );
        let is_code = !is_comment && token.kind != TokenKind::Whitespace;
        for (_, byte) in (0..token.len).zip(bytes.by_ref()) {
            // 换行本身也归属 token，使块注释内部的空行仍被识别为注释。
            has_comment |= is_comment;
            has_code |= is_code;
            if byte == b'\n' {
                excluded.push(has_comment && !has_code);
                has_comment = false;
                has_code = false;
            }
        }
    }

    if !text.is_empty() && !text.ends_with('\n') {
        excluded.push(has_comment && !has_code);
    }
    excluded
}

/// 保存每个物理行是否属于纯注释或明确测试条目的合并结果。
struct TestLines {
    /// 按零基行号排列的排除标记，同一行重复排除不影响计数。
    excluded: Vec<bool>,
}

impl TestLines {
    /// 将测试条目从最早属性行到末尾行的范围全部排除。
    ///
    /// # Params:
    ///   - `attributes`: 条目的属性，包含文档注释对应的属性。
    ///   - `span`: 条目语法树的源文件范围。
    ///
    /// # Return:
    ///   仅测试条目返回 true，并更新其覆盖行的排除标记。
    fn exclude_test(&mut self, attributes: &[Attribute], span: Span) -> bool {
        if !test_only(attributes) {
            return false;
        }
        let start = attributes
            .iter()
            .map(|attribute| attribute.span().start().line)
            .fold(span.start().line, usize::min);
        let end = span.end().line;
        for excluded in self.excluded.iter_mut().take(end).skip(start - 1) {
            *excluded = true;
        }
        true
    }
}

impl<'ast> Visit<'ast> for TestLines {
    /// 排除明确的测试条目，并继续访问普通条目内的嵌套定义。
    ///
    /// # Params:
    ///   - `item`: 当前访问的条目。
    ///
    /// # Return:
    ///   将测试条目占用的整行并入排除标记。
    fn visit_item(&mut self, item: &'ast Item) {
        if !self.exclude_test(item_attributes(item), item.span()) {
            visit::visit_item(self, item);
        }
    }

    /// 排除 impl 内明确的测试方法，保留 impl 本身的业务行。
    ///
    /// # Params:
    ///   - `method`: 当前访问的 impl 方法。
    ///
    /// # Return:
    ///   将测试方法占用的整行并入排除标记。
    fn visit_impl_item_fn(&mut self, method: &'ast ImplItemFn) {
        if !self.exclude_test(&method.attrs, method.span()) {
            visit::visit_impl_item_fn(self, method);
        }
    }
}

/// 读取各类条目的属性，无法解释的 token 不推导为测试条目。
///
/// # Params:
///   - `item`: 待检查的条目。
///
/// # Return:
///   条目的属性列表；不支持的语法返回空列表。
fn item_attributes(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        Item::Verbatim(_) => &[],
        _ => &[],
    }
}

/// 判断属性是否明确将条目限定为测试代码。
///
/// # Params:
///   - `attributes`: 文件或条目的全部属性。
///
/// # Return:
///   存在 test、tokio::test 或必然要求 test 的 cfg 属性时返回 true。
pub(super) fn test_only(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        let path = attribute.path();
        path.is_ident("test")
            || (path.segments.len() == 2
                && path
                    .segments
                    .iter()
                    .zip(["tokio", "test"])
                    .all(|(segment, name)| segment.ident == name))
            || (path.is_ident("cfg")
                && attribute
                    .parse_args::<Meta>()
                    .is_ok_and(|predicate| cfg_requires_test(&predicate)))
    })
}

/// 推导 cfg 条件成立时是否必须启用 test。
///
/// # Params:
///   - `predicate`: cfg 内的条件表达式。
///
/// # Return:
///   test 为真；all 任一项要求 test 即为真，非空 any 每项都要求 test 才为真。
///   not、未知条件和无法解析的条件不作推导。
fn cfg_requires_test(predicate: &Meta) -> bool {
    match predicate {
        Meta::Path(path) => path.is_ident("test"),
        Meta::List(list) => {
            let Ok(nested) = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            else {
                return false;
            };
            if list.path.is_ident("all") {
                nested.iter().any(cfg_requires_test)
            } else if list.path.is_ident("any") {
                !nested.is_empty() && nested.iter().all(cfg_requires_test)
            } else {
                false
            }
        }
        Meta::NameValue(_) => false,
    }
}
