//! 读取当前包的 Rust 源码，将违规字节区间映射为编译器诊断。

use std::{env, ops::Range, path::Path};

use rustc_errors::DiagDecorator;
use rustc_lint::{EarlyContext, LintContext};
use rustc_session::lint::Lint;
use rustc_span::{BytePos, Span, SyntaxContext};
use walkdir::WalkDir;

/// 原始源文件的借用视图，保留未启用的 cfg 分支和未展开的宏。
pub(crate) struct ParsedSource<'a> {
    /// 文件路径，用于识别模块入口和测试目录。
    pub path: &'a Path,

    /// 与语法树及违规区间对应的 UTF-8 原文。
    pub text: &'a str,

    /// 从原文解析的语法树。
    pub syntax: &'a syn::File,
}

/// 一条规则产生的违规位置和说明。
pub(crate) struct Violation {
    /// 相对原文的半开字节区间；文件级诊断使用文件开头的零宽区间。
    pub bytes: Range<usize>,

    /// 面向维护者的原因与修正方向。
    pub message: String,
}

/// 扫描当前包的 src/tests，读取或解析失败时使编译失败。
///
/// # Params:
///   - `cx`: 编译器语法检查上下文。
///   - `lint`: 当前检查所使用的 lint 身份。
///   - `check`: 对原始源码进行检查的纯函数。
///
/// # Return:
///   为每条违规发出源码位置诊断，不扫描目录之外的自定义 target 或路径模块。
pub(crate) fn check_sources(
    cx: &EarlyContext<'_>,
    lint: &'static Lint,
    check: fn(&ParsedSource<'_>) -> Vec<Violation>,
) {
    let Some(manifest) = env::var_os("CARGO_MANIFEST_DIR") else {
        cx.sess()
            .dcx()
            .err("Mineral lints require CARGO_MANIFEST_DIR");
        return;
    };
    for directory in ["src", "tests"] {
        let root = Path::new(&manifest).join(directory);
        if !root.exists() {
            continue;
        }
        for entry in WalkDir::new(root).sort_by_file_name() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    cx.sess()
                        .dcx()
                        .err(format!("Mineral source scan failed: {error}"));
                    continue;
                }
            };
            if entry.file_type().is_file()
                && entry.path().extension().is_some_and(|ext| ext == "rs")
            {
                check_source(cx, lint, check, entry.path());
            }
        }
    }
}

/// 读取并解析单个文件，再以对应 lint 身份报告违规。
///
/// # Params:
///   - `cx`: 提供 source map 与诊断入口的编译上下文。
///   - `lint`: 用于设置诊断级别的规则。
///   - `check`: 读取原文和语法树的检查函数。
///   - `path`: 当前 Rust 文件路径。
///
/// # Return:
///   完成文件检查，无法读取、解析或表示源码位置时报告编译错误。
fn check_source(
    cx: &EarlyContext<'_>,
    lint: &'static Lint,
    check: fn(&ParsedSource<'_>) -> Vec<Violation>,
    path: &Path,
) {
    let file = match cx.sess().source_map().load_file(path) {
        Ok(file) => file,
        Err(error) => {
            cx.sess()
                .dcx()
                .err(format!("cannot read {}: {error}", path.display()));
            return;
        }
    };
    let Some(source) = &file.src else {
        cx.sess()
            .dcx()
            .err(format!("missing source text: {}", path.display()));
        return;
    };
    let syntax = match syn::parse_file(source) {
        Ok(syntax) => syntax,
        Err(error) => {
            cx.sess()
                .dcx()
                .err(format!("cannot check {}: {error}", path.display()));
            return;
        }
    };
    let parsed = ParsedSource {
        path,
        text: source,
        syntax: &syntax,
    };
    for violation in check(&parsed) {
        let (Ok(start), Ok(end)) = (
            u32::try_from(violation.bytes.start),
            u32::try_from(violation.bytes.end),
        ) else {
            cx.sess()
                .dcx()
                .err(format!("source span is too large: {}", path.display()));
            continue;
        };
        let span = Span::new(
            file.start_pos + BytePos(start),
            file.start_pos + BytePos(end),
            SyntaxContext::root(),
            /* parent */ None,
        );
        cx.emit_span_lint(
            lint,
            span,
            DiagDecorator(|diagnostic| {
                diagnostic.primary_message(violation.message);
            }),
        );
    }
}
