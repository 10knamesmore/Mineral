//! 按表达式所属执行体检查直接调用的阻塞操作。

use rustc_hir::Expr;
use rustc_lint::{LateContext, LateLintPass, LintContext};

use super::{calls, config::Config};

rustc_session::declare_lint! {
    /// 禁止在异步执行体中直接调用配置列出的阻塞操作。
    pub(crate) MINERAL_BLOCKING_IN_ASYNC,
    Deny,
    "blocking call in an async body"
}

/// 保存本次编译使用的阻塞调用清单。
pub(crate) struct BlockingInAsync {
    /// 已校验的完整配置；每条记录按定义路径精确匹配。
    config: Config,
}

rustc_session::impl_lint_pass!(BlockingInAsync => [MINERAL_BLOCKING_IN_ASYNC]);

impl BlockingInAsync {
    /// 使用已校验的配置创建检查器。
    ///
    /// # Params:
    ///   - `config`: 本次编译使用的完整阻塞调用清单。
    ///
    /// # Return:
    ///   按该清单检查异步执行体的检查器。
    pub(crate) fn new(config: Config) -> Self {
        Self { config }
    }
}

impl<'tcx> LateLintPass<'tcx> for BlockingInAsync {
    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        let owner = cx.tcx.hir_enclosing_body_owner(expr.hir_id);
        if !cx
            .tcx
            .coroutine_kind(owner.to_def_id())
            .is_some_and(|kind| kind.is_async_desugaring())
        {
            return;
        }

        let Some(call) = calls::resolve(cx, expr) else {
            return;
        };
        let Some(blocking) = self
            .config
            .calls
            .iter()
            .find(|blocking| blocking.path == call)
        else {
            return;
        };

        cx.emit_span_lint(
            MINERAL_BLOCKING_IN_ASYNC,
            expr.span,
            rustc_errors::DiagDecorator(|diag| {
                diag.primary_message(format!("blocking call `{call}` in an async body"));
                diag.note(blocking.reason.clone());
            }),
        );
    }
}
