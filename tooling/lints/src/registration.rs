//! 注册编译规则，在开始检查前报告非法配置。

use crate::blocking_in_async::{BlockingInAsync, Config, MINERAL_BLOCKING_IN_ASYNC};
use crate::business_file_too_long::{BusinessFileTooLong, MINERAL_BUSINESS_FILE_TOO_LONG};
use crate::member_spacing::{MINERAL_MEMBER_SPACING, MemberSpacing};
use crate::mod_rs_items::{MINERAL_MOD_RS_ITEMS, ModRsItems};

/// 注册四条规则；阻塞清单无效时令编译失败。
///
/// # Params:
///   - `sess`: 本次编译会话，用于读取配置与报告错误。
///   - `store`: rustc 的 lint 注册表，接收规则和检查 pass。
///
/// # Return:
///   注册语法规则，并在配置有效时注册阻塞调用检查。
// Dylint 通过符号名加载此入口；no_mangle 是其动态库 ABI 的要求。
#[unsafe(no_mangle)]
pub fn register_lints(sess: &rustc_session::Session, store: &mut rustc_lint::LintStore) {
    store.register_lints(&[
        MINERAL_BUSINESS_FILE_TOO_LONG,
        MINERAL_MOD_RS_ITEMS,
        MINERAL_MEMBER_SPACING,
    ]);
    store.register_early_lint_pass(Box::new(|| Box::new(BusinessFileTooLong)));
    store.register_early_lint_pass(Box::new(|| Box::new(ModRsItems)));
    store.register_early_lint_pass(Box::new(|| Box::new(MemberSpacing)));
    dylint_linting::init_config(sess);
    store.register_lints(&[MINERAL_BLOCKING_IN_ASYNC]);
    if let Some(config) = Config::load(sess) {
        store.register_late_lint_pass(Box::new(move |_| {
            Box::new(BlockingInAsync::new(config.clone()))
        }));
    }
}
