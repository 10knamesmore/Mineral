//! 验证模块入口允许的条目、文件范围和诊断字节位置。

use std::path::Path;

use color_eyre::eyre::{ContextCompat, WrapErr};
use rustc_session::lint::Level;

use super::rule::{MINERAL_MOD_RS_ITEMS, check};
use crate::source_files::{ParsedSource, Violation};

/// 解析一份指定路径的源码并执行模块入口检查。
///
/// # Params:
///   - `path`: 被检查文件的路径。
///   - `text`: 完整 Rust 源码。
///
/// # Return:
///   检查结果；解析失败时返回包含路径的错误。
fn check_text(path: &str, text: &str) -> color_eyre::Result<Vec<Violation>> {
    let syntax = syn::parse_file(text).wrap_err_with(|| format!("解析测试源码 {path}"))?;
    Ok(check(&ParsedSource {
        path: Path::new(path),
        text,
        syntax: &syntax,
    }))
}

/// 外置模块和各种显式可见性的重导出在生产及测试目录中均合法。
///
/// # Params:
///   无。
///
/// # Return:
///   解析成功且全部条目通过检查。
#[test]
fn permits_external_modules_and_visible_reexports() -> color_eyre::Result<()> {
    let text = r#"//! 模块入口。
#![allow(dead_code)]
mod private;
pub mod public;
pub(crate) mod crate_visible;
pub(super) mod parent_visible;
#[cfg(any())]
#[path = "another.rs"]
mod disabled;
#[cfg(test)]
mod tests;
pub use public::Entry;
pub(crate) use private::Internal;
pub(super) use private::Parent;
pub(self) use private::Own;
pub(in crate::catalog) use private::Scoped;
#[cfg(any())]
pub use disabled::Hidden;
"#;
    for path in [
        "mod.rs",
        "src/mod.rs",
        "tests/mod.rs",
        "tests/support/mod.rs",
    ] {
        assert!(check_text(path, text)?.is_empty(), "文件 {path}");
    }
    Ok(())
}

/// 所有实现条目和私有 use 都不能因所在目录或 cfg 属性获得豁免。
///
/// # Params:
///   无。
///
/// # Return:
///   每种违规条目在生产和测试目录中都产生一条完整范围诊断。
#[test]
fn rejects_implementation_items_in_source_and_test_directories() -> color_eyre::Result<()> {
    let items = [
        "fn run() {}",
        "type Value = u8;",
        "struct Value;",
        "enum Value { First }",
        "union Value { first: u8 }",
        "const VALUE: u8 = 1;",
        "static VALUE: u8 = 1;",
        "trait Value {}",
        "trait Value = Send;",
        "impl Value {}",
        "mod inline {}",
        "mod inline { pub use crate::Value; }",
        "macro_rules! value { () => {} }",
        "value!();",
        "use other::Value;",
        "extern crate other;",
        "unsafe extern \"C\" { fn run(); }",
        "#[test]\nfn test_value() {}",
        "#[cfg(test)]\nfn test_value() {}",
        "#[cfg(test)]\nmod tests {}",
        "#[cfg(any())]\nfn disabled() {}",
        "#[cfg(any())]\nmod disabled {}",
        "#[cfg(any())]\nuse other::Value;",
    ];
    let prefix = "mod implementation;\npub use implementation::Entry;\n";
    for path in ["src/mod.rs", "tests/mod.rs", "tests/support/mod.rs"] {
        for item in items {
            let text = format!("{prefix}{item}\n");
            let violations = check_text(path, &text)?;
            assert_eq!(violations.len(), 1, "文件 {path}，条目 {item}");
            let violation = violations.first().wrap_err("缺少违规条目诊断")?;
            assert_eq!(violation.bytes, prefix.len()..prefix.len() + item.len());
            assert_eq!(text.get(violation.bytes.clone()), Some(item));
            assert_eq!(
                violation.message,
                "mod.rs may only contain module declarations and re-exports; move this implementation into a named source file"
            );
        }
    }
    Ok(())
}

/// 文件名范围严格限定为 mod.rs，lib.rs 与普通源码不受该规则限制。
///
/// # Params:
///   无。
///
/// # Return:
///   所有非 mod.rs 路径均不产生诊断。
#[test]
fn ignores_lib_rs_and_other_filenames() -> color_eyre::Result<()> {
    let text = "fn run() {}\nstruct Value;\nmod inline {}\nuse other::Value;";
    for path in [
        "lib.rs",
        "src/lib.rs",
        "tests/lib.rs",
        "src/main.rs",
        "src/module.rs",
        "src/mod.rs.backup",
        "src/mod.rs/implementation.rs",
    ] {
        assert!(check_text(path, text)?.is_empty(), "文件 {path}");
    }
    Ok(())
}

/// 中文前缀不改变字节位置，诊断从条目的首条文档开始。
///
/// # Params:
///   无。
///
/// # Return:
///   每条诊断精确覆盖带文档与属性的违规条目。
#[test]
fn reports_complete_items_after_chinese_prefixes() -> color_eyre::Result<()> {
    let prefix = "//! 中文模块说明。\nmod implementation;\n// 中文前缀。\n";
    let item = "/// 中文函数说明。\n#[cfg(any())]\nfn 运行() {}";
    let text = format!("{prefix}{item}\n");
    for path in ["src/mod.rs", "tests/support/mod.rs"] {
        let violations = check_text(path, &text)?;
        assert_eq!(violations.len(), 1);
        let violation = violations.first().wrap_err("缺少中文条目诊断")?;
        assert_eq!(violation.bytes, prefix.len()..prefix.len() + item.len());
        assert_eq!(text.get(violation.bytes.clone()), Some(item));
    }
    Ok(())
}

/// 多个违规条目分别报告，合法条目不会打断检查。
///
/// # Params:
///   无。
///
/// # Return:
///   按源码顺序报告全部违规条目。
#[test]
fn reports_every_forbidden_item() -> color_eyre::Result<()> {
    let text = "fn first() {}\nmod child;\npub use child::Entry;\nstruct Second;";
    let violations = check_text("src/mod.rs", text)?;
    assert_eq!(violations.len(), 2);
    let fragments = violations
        .iter()
        .map(|violation| {
            text.get(violation.bytes.clone())
                .wrap_err("诊断范围超出源码")
        })
        .collect::<color_eyre::Result<Vec<_>>>()?;
    assert_eq!(fragments, ["fn first() {}", "struct Second;"]);
    Ok(())
}

/// 模块入口职责违规默认阻止编译。
///
/// # Params:
///   无。
///
/// # Return:
///   默认诊断级别为 Deny。
#[test]
fn defaults_to_deny() -> color_eyre::Result<()> {
    assert_eq!(MINERAL_MOD_RS_ITEMS.default_level, Level::Deny);
    Ok(())
}
