//! 验证成员空行的词法边界、遍历范围和完整诊断位置。

use std::path::Path;

use color_eyre::eyre::{ContextCompat, WrapErr};
use rustc_session::lint::Level;

use super::rule::{MINERAL_MEMBER_SPACING, check};
use crate::source_files::{ParsedSource, Violation};

/// 解析原始文本并执行成员间距检查。
///
/// # Params:
///   - `text`: 完整 Rust 源码。
///
/// # Return:
///   检查结果；语法错误携带测试解析上下文。
fn check_text(text: &str) -> color_eyre::Result<Vec<Violation>> {
    let syntax = syn::parse_file(text).wrap_err("解析成员间距测试源码")?;
    Ok(check(&ParsedSource {
        path: Path::new("src/types.rs"),
        text,
        syntax: &syntax,
    }))
}

/// 断言源码只报告指定的一个成员，并覆盖完整成员原文。
///
/// # Params:
///   - `text`: 完整 Rust 源码。
///   - `member`: 应被报告的完整成员，包含其属性和文档。
///
/// # Return:
///   诊断数量、字节范围和文案全部符合预期。
fn assert_member_diagnostic(text: &str, member: &str) -> color_eyre::Result<()> {
    let violations = check_text(text)?;
    assert_eq!(violations.len(), 1, "源码：{text}");
    let violation = violations.first().wrap_err("缺少成员间距诊断")?;
    let start = text.find(member).wrap_err("测试源码中缺少预期成员")?;
    assert_eq!(violation.bytes, start..start + member.len(), "源码：{text}");
    assert_eq!(text.get(violation.bytes.clone()), Some(member));
    assert_eq!(
        violation.message,
        "insert a blank line before this member, including its attributes and documentation"
    );
    Ok(())
}

/// 结构体具名字段、枚举变体及变体字段即使没有文档或属性，也必须以空行分隔。
///
/// # Params:
///   无。
///
/// # Return:
///   紧邻的具名字段、元组字段和变体均被准确报告。
#[test]
fn rejects_adjacent_members_with_or_without_docs_and_attributes() -> color_eyre::Result<()> {
    let cases = [
        ("struct Value { first: u8,\nsecond: u16 }", "second: u16"),
        (
            "struct Value { first: u8,\n/// 第二字段。\nsecond: u16 }",
            "/// 第二字段。\nsecond: u16",
        ),
        (
            "struct Value { first: u8,\n#[cfg(any())]\nsecond: u16 }",
            "#[cfg(any())]\nsecond: u16",
        ),
        ("enum Value { First,\nSecond }", "Second"),
        (
            "enum Value { First,\n/// 第二变体。\nSecond }",
            "/// 第二变体。\nSecond",
        ),
        (
            "enum Value { First,\n#[cfg(any())]\nSecond }",
            "#[cfg(any())]\nSecond",
        ),
        (
            "enum Value { Named { first: u8,\nsecond: u16 } }",
            "second: u16",
        ),
        (
            "enum Value { Named { first: u8,\n/// 第二字段。\nsecond: u16 } }",
            "/// 第二字段。\nsecond: u16",
        ),
        (
            "enum Value { Named { first: u8,\n#[cfg(any())]\nsecond: u16 } }",
            "#[cfg(any())]\nsecond: u16",
        ),
        ("enum Value { Tuple(u8,\nu16) }", "u16"),
        (
            "enum Value { Tuple(u8,\n/// 第二字段。\nu16) }",
            "/// 第二字段。\nu16",
        ),
        (
            "enum Value { Tuple(u8,\n#[cfg(any())]\nu16) }",
            "#[cfg(any())]\nu16",
        ),
    ];
    for (text, member) in cases {
        assert_member_diagnostic(text, member)?;
    }
    Ok(())
}

/// tuple struct 的字段间距不受检查，兼容 rustfmt 合并后的格式。
///
/// # Return:
///   紧邻字段、字段文档与属性、局部及未启用的 tuple struct 均通过。
#[test]
fn skips_tuple_struct_field_spacing() -> color_eyre::Result<()> {
    for text in [
        "struct Pair(u8, u16);",
        "struct Pair(u8,\nu16);",
        "struct Pair(u8,\n/// 第二字段。\nu16);",
        "struct Pair(u8,\n#[cfg(any())]\nu16);",
        "#[cfg(any())]\nstruct Pair(u8, u16);",
        "fn run() { struct Local(u8,\nu16); }",
        "//! 中文前缀。\nstruct 记录(u8,\n/// 第二成员。\n#[cfg(any())]\nu16);",
    ] {
        assert!(check_text(text)?.is_empty(), "源码：{text}");
    }
    Ok(())
}

/// 空定义和只有一个成员的定义不需要前置空行。
///
/// # Params:
///   无。
///
/// # Return:
///   不存在相邻成员时不产生诊断。
#[test]
fn permits_empty_and_single_member_definitions() -> color_eyre::Result<()> {
    for text in [
        "",
        "struct Unit;",
        "struct Empty {}",
        "struct Empty();",
        "enum Empty {}",
        "struct Single { first: u8 }",
        "struct Single(#[cfg(any())] u8);",
        "enum Single { First }",
        "enum Single { Named { first: u8 } }",
        "enum Single { Tuple(u8) }",
        "struct Single { /// 唯一字段。\nfirst: u8 }",
    ] {
        assert!(check_text(text)?.is_empty(), "源码：{text}");
    }
    Ok(())
}

/// 真正空白行支持不同换行符、空格、制表符和尾随注释。
///
/// # Params:
///   无。
///
/// # Return:
///   所有成员形态在空白 token 中具有两个换行时通过。
#[test]
fn permits_blank_lines_with_crlf_tabs_and_trailing_comments() -> color_eyre::Result<()> {
    for gap in [
        "\n\n",
        "\r\n\r\n",
        "\n  \n  ",
        "\n\t\n\t",
        " // 中文尾注。\n\n",
        " // 中文尾注。\r\n\t\r\n",
        " /* 中文注释。 */\n\n",
        "\n/* 中文注释。 */\n\n",
    ] {
        for text in [
            format!("struct Named {{ first: u8,{gap}second: u16 }}"),
            format!("enum Value {{ First,{gap}Second }}"),
            format!("enum Value {{ Named {{ first: u8,{gap}second: u16 }} }}"),
            format!("enum Value {{ Tuple(u8,{gap}u16) }}"),
        ] {
            assert!(check_text(&text)?.is_empty(), "源码：{text}");
        }
    }
    Ok(())
}

/// 空行应放在整个文档和属性块之前。
///
/// # Params:
///   无。
///
/// # Return:
///   正确分隔的文档、属性及它们的组合均通过。
#[test]
fn permits_blank_lines_before_documented_and_attributed_members() -> color_eyre::Result<()> {
    for member in [
        "/// 第二字段。\nsecond: u16",
        "#[cfg(any())]\nsecond: u16",
        "/// 第二字段。\n#[cfg(any())]\nsecond: u16",
        "#[cfg(any())]\n/// 第二字段。\nsecond: u16",
    ] {
        let text = format!("struct Value {{ first: u8,\n\n{member} }}");
        assert!(check_text(&text)?.is_empty(), "源码：{text}");
    }
    Ok(())
}

/// 文档、属性及字符串中的空行属于成员内容，不能充当成员间隔。
///
/// # Params:
///   无。
///
/// # Return:
///   所有只有成员内部空行的情况均报告完整的第二成员。
#[test]
fn rejects_blank_lines_inside_docs_attributes_and_strings() -> color_eyre::Result<()> {
    for member in [
        "/// 第二字段。\n///\n/// 更多说明。\nsecond: u16",
        "/** 第二字段。\n\n更多说明。 */\nsecond: u16",
        "/// 第二字段。\n\nsecond: u16",
        "/// 第二字段。\n\n#[cfg(any())]\nsecond: u16",
        "#[cfg(any())]\n\nsecond: u16",
        "#[cfg_attr(\n\nany(), allow(dead_code))]\nsecond: u16",
        "#[allow(dead_code)]\n\n#[cfg(any())]\nsecond: u16",
        "#[doc = \"中文文档。\n\n更多说明。\"]\nsecond: u16",
        "#[doc = r#\"中文文档。\n\n更多说明。\"#]\nsecond: u16",
    ] {
        let text = format!("struct Value {{ first: u8,\n{member} }}");
        assert_member_diagnostic(&text, member)?;
    }
    assert_member_diagnostic(
        "struct Value { first: [u8; { let _ = \"中文。\n\n文本。\"; 1 }],\nsecond: u16 }",
        "second: u16",
    )?;
    Ok(())
}

/// 普通注释里的空行和被注释隔开的换行都不算空白行。
///
/// # Params:
///   无。
///
/// # Return:
///   注释 token 不会被当作 Whitespace，CRLF 单行也不会重复计数。
#[test]
fn rejects_comment_only_spacing_and_single_line_gaps() -> color_eyre::Result<()> {
    for gap in [
        " ",
        "\n",
        "\r\n",
        "\n\t",
        " // 中文尾注。\n",
        "\n// 中文说明。\n",
        "\n/* 中文说明。\n\n更多说明。 */\n",
        "\n/* 外层。\n/* 内层。\n\n*/\n*/\n",
        "\n/* 第一段。 */\n/* 第二段。 */\n",
    ] {
        let text = format!("struct Value {{ first: u8,{gap}second: u16 }}");
        assert_member_diagnostic(&text, "second: u16")?;
    }
    Ok(())
}

/// cfg 不会过滤原始语法树，局部类型和内联模块中的类型也要检查。
///
/// # Params:
///   无。
///
/// # Return:
///   所有被编译器条件过滤或嵌套在函数、模块中的违规类型仍被报告。
#[test]
fn checks_disabled_cfg_items_members_and_local_types() -> color_eyre::Result<()> {
    let cases = [
        (
            "#[cfg(any())]\nstruct Hidden { first: u8,\nsecond: u16 }",
            "second: u16",
        ),
        (
            "struct Value { #[cfg(any())] first: u8,\nsecond: u16 }",
            "second: u16",
        ),
        ("#[cfg(any())]\nenum Hidden { First,\nSecond }", "Second"),
        (
            "#[cfg(any())]\nmod hidden { struct Value { first: u8,\nsecond: u16 } }",
            "second: u16",
        ),
        (
            "#[cfg(test)]\nmod tests { struct Value { first: u8,\nsecond: u16 } }",
            "second: u16",
        ),
        (
            "fn run() { struct Local { first: u8,\nsecond: u16 } }",
            "second: u16",
        ),
        ("fn run() { enum Local { First,\nSecond } }", "Second"),
        (
            "fn run() { enum Local { Named { first: u8,\nsecond: u16 } } }",
            "second: u16",
        ),
        ("fn run() { enum Local { Tuple(u8,\nu16) } }", "u16"),
    ];
    for (text, member) in cases {
        assert_member_diagnostic(text, member)?;
    }
    Ok(())
}

/// 宏 token、构造表达式和解构模式不属于被检查的类型成员定义。
///
/// # Params:
///   无。
///
/// # Return:
///   这些位置没有空行时也不产生诊断。
#[test]
fn ignores_macro_bodies_invocations_constructors_and_patterns() -> color_eyre::Result<()> {
    let text = r#"
macro_rules! define_types {
    () => {
        struct Generated { first: u8, second: u16 }
        enum GeneratedEnum { First, Second }
    };
}
define_types!();
custom! {
    struct Tokens { first: u8, second: u16 }
    enum TokenEnum { First, Second }
}
fn run(value: Named, tuple: Tuple) {
    let Named { first, second } = value;
    let Tuple(left, right) = tuple;
    let _ = Named { first: 1, second: 2 };
    let _ = Tuple(1, 2);
    let _ = Value::Named { first: 1, second: 2 };
    let _ = Value::Tuple(1, 2);
    match value {
        Named { first, second } => (),
    }
}
"#;
    assert!(check_text(text)?.is_empty());
    Ok(())
}

/// 中文文档和属性都包含在诊断中，偏移以字节计算。
///
/// # Params:
///   无。
///
/// # Return:
///   具名字段、元组字段和变体均从首条文档开始报告。
#[test]
fn reports_chinese_members_from_their_first_doc_and_attributes() -> color_eyre::Result<()> {
    let documentation = "/// 第二成员的中文说明。\n#[cfg(any())]\n";
    for (prefix, name, suffix) in [
        (
            "//! 中文前缀。\nstruct 记录 { 首项: u8,\n",
            "次项: u16",
            " }",
        ),
        ("//! 中文前缀。\nenum 记录 { 首项,\n", "次项", " }"),
        (
            "//! 中文前缀。\nenum 记录 { 内容 { 首项: u8,\n",
            "次项: u16",
            " } }",
        ),
        ("//! 中文前缀。\nenum 记录 { 内容(u8,\n", "u16", ") }"),
    ] {
        let member = format!("{documentation}{name}");
        let text = format!("{prefix}{member}{suffix}");
        assert_member_diagnostic(&text, &member)?;
        let violations = check_text(&text)?;
        let violation = violations.first().wrap_err("缺少中文成员诊断")?;
        assert_eq!(violation.bytes.start, prefix.len());
    }
    Ok(())
}

/// 同一定义中的每个违规边界都分别诊断。
///
/// # Params:
///   无。
///
/// # Return:
///   诊断只覆盖第二、第三成员，顺序与源码一致。
#[test]
fn reports_every_missing_member_separator() -> color_eyre::Result<()> {
    let text = "struct Value { first: u8, second: u16, third: u32 }";
    let violations = check_text(text)?;
    assert_eq!(violations.len(), 2);
    let fragments = violations
        .iter()
        .map(|violation| {
            text.get(violation.bytes.clone())
                .wrap_err("诊断范围超出源码")
        })
        .collect::<color_eyre::Result<Vec<_>>>()?;
    assert_eq!(fragments, ["second: u16", "third: u32"]);
    Ok(())
}

/// 成员间距问题默认提醒维护者。
///
/// # Params:
///   无。
///
/// # Return:
///   默认诊断级别为 Warn。
#[test]
fn defaults_to_warn() -> color_eyre::Result<()> {
    assert_eq!(MINERAL_MEMBER_SPACING.default_level, Level::Warn);
    Ok(())
}
