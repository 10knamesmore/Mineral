//! 验证业务行数、测试豁免和超限诊断的可观察行为。

use std::path::Path;

use color_eyre::eyre::{WrapErr, eyre};

use crate::source_files::{ParsedSource, Violation};

use super::line_count::business_lines;
use super::rule::check;

/// 构造含指定物理行数的函数，函数体使用空行。
///
/// # Params:
///   - `lines`: 包含函数首尾两行的总行数，至少为两行。
///
/// # Return:
///   带结尾换行的有效 Rust 源文件。
fn business_source(lines: usize) -> String {
    format!("fn business() {{\n{}}}\n", "\n".repeat(lines - 2))
}

/// 解析源文件并执行文件检查入口。
///
/// # Params:
///   - `path`: 用于测试文件豁免的路径。
///   - `text`: 有效 Rust 源文件文本。
///
/// # Return:
///   检查产生的诊断，语法解析失败时保留错误上下文。
fn check_text(path: &str, text: &str) -> color_eyre::Result<Vec<Violation>> {
    let syntax = syn::parse_file(text).wrap_err("解析业务行数测试源文件失败")?;
    Ok(check(&ParsedSource {
        path: Path::new(path),
        text,
        syntax: &syntax,
    }))
}

/// 同时验证业务行数、800 行边界及超限诊断的定位和文本。
///
/// # Params:
///   - `text`: 有效 Rust 业务源文件文本。
///   - `expected`: 预期保留的业务行数。
///
/// # Return:
///   解析及断言成功时返回空值。
fn assert_business_lines(text: &str, expected: usize) -> color_eyre::Result<()> {
    let syntax = syn::parse_file(text).wrap_err("解析业务行数测试源文件失败")?;
    let source = ParsedSource {
        path: Path::new("src/business.rs"),
        text,
        syntax: &syntax,
    };
    assert_eq!(business_lines(&source), expected);
    let violations = check(&source);
    if expected <= 800 {
        assert!(violations.is_empty());
    } else {
        assert_eq!(violations.len(), 1);
        let violation = violations
            .first()
            .ok_or_else(|| eyre!("缺少超限文件诊断"))?;
        assert_eq!(violation.bytes, 0..0);
        assert_eq!(
            violation.message,
            format!(
                "business source file has {expected} lines excluding tests and comment-only lines (maximum 800)"
            )
        );
    }
    Ok(())
}

/// 空行计入 800 行边界，结尾换行不额外产生一行。
///
/// # Return:
///   800 行通过、801 行拒绝且有无结尾换行均一致。
#[test]
fn blank_lines_and_final_newline_preserve_the_limit() -> color_eyre::Result<()> {
    for lines in [800, 801] {
        let text = business_source(lines);
        assert_business_lines(&text, lines)?;
        assert_business_lines(text.trim_end_matches('\n'), lines)?;
    }
    Ok(())
}

/// 大量文档与嵌套块注释不改变实际业务代码的边界。
///
/// # Return:
///   注释扣除后仍分别按 800 和 801 行判断。
#[test]
fn documentation_and_nested_comments_do_not_consume_the_budget() -> color_eyre::Result<()> {
    let docs = "/// 业务入口的说明。\n".repeat(/* n */ 900);
    let blocks = "/* 外层\n\n/* 内层\n\n*/\n*/\n".repeat(/* n */ 300);
    for lines in [800, 801] {
        let text = format!("{docs}{blocks}{}", business_source(lines));
        assert_business_lines(&text, lines)?;
    }
    Ok(())
}

/// 纯注释按物理行排除，同行代码、空行及 shebang 继续计数。
///
/// # Return:
///   空文件、CRLF、无末尾换行及混合注释均符合计数约定。
#[test]
fn physical_line_and_comment_boundaries_are_preserved() -> color_eyre::Result<()> {
    let cases = [
        ("", 0),
        ("\n\n", 2),
        (" \t\r\n\r\n", 2),
        ("// 说明\r\n\r\nfn business() {}\r\n", 2),
        ("// 没有结尾换行", 0),
        ("fn business() {} // 没有结尾换行", 1),
        ("/* 开头 */ fn business() {} // 结尾\n", 1),
        ("/* 外层\n\n/* 内层\n\n*/\n\n*/\n", 0),
        ("/* 注释 */\n\n/* 注释 */", 1),
        ("fn business() { /* 开头\n\n结尾 */ }\n", 2),
        ("/* 注释 */ // 另一条注释\n", 0),
        ("#!/usr/bin/env rustx\nfn business() {}\n", 2),
        ("#!/usr/bin/env /* runner\nfn business() {}\n", 2),
        ("#!/usr/bin/env rustx", 1),
        ("#!/usr/bin/env rustx\r\n// 说明\r\n", 1),
    ];
    for (text, expected) in cases {
        assert_business_lines(text, expected)?;
    }
    Ok(())
}

/// 普通、raw、byte 和 raw byte 字符串内的注释符及空行都是代码。
///
/// # Return:
///   各种字符串的物理行全部保留。
#[test]
fn comment_markers_inside_literals_are_not_comments() -> color_eyre::Result<()> {
    let cases = [
        ("const TEXT: &str = \"// /* 中文 */\";\n", 1),
        ("const TEXT: &str = \"\\\" // /* */\";\n", 1),
        ("const TEXT: &str = \"// first\n\n/* last */\n\";\n", 4),
        (
            "const TEXT: &str = r###\"// first\n\n/* \\\" */\n\"###;\n",
            4,
        ),
        ("const BYTES: &[u8] = b\"// first\n\n/* last */\n\";\n", 4),
        (
            "const BYTES: &[u8] = br##\"// first\n\n/* last */\n\"##;\n",
            4,
        ),
        ("const SLASH: char = '/';\n", 1),
    ];
    for (text, expected) in cases {
        assert_business_lines(text, expected)?;
    }
    Ok(())
}

/// 超长测试模块和测试函数连同文档、属性、内部空行一并豁免。
///
/// # Return:
///   900 行测试不会占用额度，相邻 801 行业务仍产生诊断。
#[test]
fn large_test_items_leave_adjacent_business_subject_to_the_limit() -> color_eyre::Result<()> {
    let cases = [
        ("#[cfg(test)]", "mod tests"),
        ("#[test]", "fn verifies_behavior()"),
        ("#[tokio::test]", "async fn verifies_behavior()"),
        (
            "#[tokio::test(flavor = \"current_thread\")]",
            "async fn verifies_behavior()",
        ),
    ];
    let body = "\n".repeat(/* n */ 900);
    for (attribute, declaration) in cases {
        let tests = format!(
            "/// 测试说明。\n#[allow(dead_code)]\n\n{attribute}\n{declaration} {{\n{body}}}\n"
        );
        assert_business_lines(&tests, /* expected */ 0)?;
        for lines in [800, 801] {
            let business = business_source(lines);
            assert_business_lines(&format!("{tests}{business}"), lines)?;
            assert_business_lines(&format!("{business}{tests}"), lines)?;
        }
    }
    Ok(())
}

/// cfg 仅在其成立必然要求 test 时排除对应条目。
///
/// # Return:
///   all、非空 any 递归推导，feature、not 和未知条件保留业务行。
#[test]
fn cfg_predicates_only_exclude_provably_test_only_items() -> color_eyre::Result<()> {
    let cases = [
        ("test", true),
        ("all(test, feature = \"extra\")", true),
        ("all(feature = \"extra\", test)", true),
        ("all(test, not(feature = \"extra\"))", true),
        ("any(test)", true),
        ("any(test, all(test, feature = \"extra\"))", true),
        ("all(any(test, all(test)), feature = \"extra\")", true),
        ("any(test, feature = \"extra\")", false),
        ("not(test)", false),
        ("not(not(test))", false),
        ("feature = \"extra\"", false),
        ("all(feature = \"extra\")", false),
        ("any(all(test, feature = \"extra\"), unix)", false),
        ("all()", false),
        ("any()", false),
        ("custom(test)", false),
    ];
    let body = "\n".repeat(/* n */ 900);
    for (predicate, excluded) in cases {
        let text = format!("#[cfg({predicate})]\nfn conditional() {{\n{body}}}\n");
        let expected = if excluded { 0 } else { 903 };
        assert_business_lines(&text, expected)?;
    }
    Ok(())
}

/// 名称相似的属性和 cfg_attr 不足以证明条目仅用于测试。
///
/// # Return:
///   cfg_attr 和其他路径的 test 属性不会缩减业务行数。
#[test]
fn unrelated_attributes_do_not_exempt_business_code() -> color_eyre::Result<()> {
    for attribute in [
        "#[other::test]",
        "#[tokio::other::test]",
        "#[cfg_attr(test, test)]",
        "#[cfg_attr(test, cfg(test))]",
    ] {
        let text = format!("{attribute}\n{}", business_source(/* lines */ 800));
        assert_business_lines(&text, /* expected */ 801)?;
    }
    Ok(())
}

/// 各类条目的 cfg(test) 属性都排除整个条目。
///
/// # Return:
///   函数之外的声明、实现、宏和导入同样遵守测试豁免。
#[test]
fn test_attributes_apply_to_every_supported_item_kind() -> color_eyre::Result<()> {
    let items = [
        "const VALUE: usize = 1;",
        "enum Choice { One }",
        "extern crate core;",
        "fn helper() {}",
        "extern \"C\" { fn helper(); }",
        "impl Business { fn helper() {} }",
        "macro_rules! helper { () => {} }",
        "mod helpers {}",
        "static VALUE: usize = 1;",
        "struct Business;",
        "trait Business {}",
        "trait Business = Send;",
        "type Value = usize;",
        "union Value { integer: usize, float: f64 }",
        "use std::fmt;",
    ];
    let business = business_source(/* lines */ 801);
    for item in items {
        let tests = format!("#[cfg(test)]\n{item}\n");
        assert_business_lines(&tests, /* expected */ 0)?;
        assert_business_lines(&format!("{tests}{business}"), /* expected */ 801)?;
    }
    Ok(())
}

/// impl 内的方法与函数内的嵌套测试单独豁免，外层代码仍然计数。
///
/// # Return:
///   900 行测试方法只扣自身范围，不扣除外层 impl 或函数。
#[test]
fn nested_test_functions_preserve_their_business_containers() -> color_eyre::Result<()> {
    let body = "\n".repeat(/* n */ 900);
    for attribute in ["#[cfg(test)]", "#[test]", "#[tokio::test]"] {
        let method = format!(
            "struct Business;\nimpl Business {{\n/// 测试说明。\n#[allow(dead_code)]\n{attribute}\nfn helper() {{\n{body}}}\n}}\n"
        );
        assert_business_lines(&method, /* expected */ 3)?;
        let business = business_source(/* lines */ 801);
        assert_business_lines(&format!("{method}{business}"), /* expected */ 804)?;
    }
    let nested = format!("fn business() {{\n#[test]\nfn nested() {{\n{body}}}\n}}\n");
    assert_business_lines(&nested, /* expected */ 2)?;
    Ok(())
}

/// 测试条目与注释的范围取并集，重叠部分不重复扣除。
///
/// # Return:
///   测试块内部的注释不改变相邻业务的 800 行边界。
#[test]
fn comments_inside_test_items_are_excluded_only_once() -> color_eyre::Result<()> {
    let comments = "// 行注释\n/* 块注释\n\n/* 内层 */\n*/\n".repeat(/* n */ 200);
    let tests = format!("/// 测试说明。\n#[cfg(test)]\nmod tests {{\n{comments}}}\n");
    for lines in [800, 801] {
        let business = business_source(lines);
        assert_business_lines(&format!("{tests}{business}"), lines)?;
    }
    Ok(())
}

/// tests 路径组件和 tests.rs 文件名豁免整份文件，近似名称不豁免。
///
/// # Return:
///   仅约定的测试路径通过超长文件检查。
#[test]
fn test_file_paths_are_exempt_without_matching_similar_names() -> color_eyre::Result<()> {
    let text = business_source(/* lines */ 900);
    for path in [
        "tests/integration.rs",
        "crates/player/tests/nested/integration.rs",
        "src/tests/helpers.rs",
        "tests.rs",
        "src/tests.rs",
    ] {
        assert!(check_text(path, &text)?.is_empty(), "路径应豁免：{path}");
    }
    for path in [
        "src/contests/business.rs",
        "src/tests_helper.rs",
        "src/tests.rs/business.rs",
    ] {
        assert_eq!(check_text(path, &text)?.len(), 1, "路径不应豁免：{path}");
    }
    Ok(())
}

/// 文件级 cfg(test) 及必然要求 test 的组合条件豁免整份文件。
///
/// # Return:
///   仅测试文件通过，可能在非测试构建启用的文件仍受行数限制。
#[test]
fn file_level_cfg_uses_the_same_test_requirement() -> color_eyre::Result<()> {
    let business = business_source(/* lines */ 900);
    for predicate in [
        "test",
        "all(test, feature = \"extra\")",
        "any(test, all(test, feature = \"extra\"))",
    ] {
        let text = format!("#![cfg({predicate})]\n{business}");
        assert!(check_text("src/business.rs", &text)?.is_empty());
    }
    for predicate in [
        "any(test, feature = \"extra\")",
        "not(test)",
        "feature = \"extra\"",
    ] {
        let text = format!("#![cfg({predicate})]\n{business}");
        assert_business_lines(&text, /* expected */ 901)?;
    }
    Ok(())
}
