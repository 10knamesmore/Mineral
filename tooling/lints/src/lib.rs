//! Mineral 自定义源码与 async 阻塞调用检查。

#![feature(rustc_private)]

extern crate rustc_ast;
extern crate rustc_errors;
extern crate rustc_hir;
extern crate rustc_lexer;
extern crate rustc_lint;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;

mod blocking_in_async;
mod business_file_too_long;
mod member_spacing;
mod mod_rs_items;
mod registration;
mod source_files;

pub use registration::register_lints;

dylint_linting::dylint_library!();
