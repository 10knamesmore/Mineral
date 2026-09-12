//! 将直接调用解析为可供配置匹配的定义路径。

use rustc_hir::{Expr, ExprKind, def_id::DefId};
use rustc_lint::LateContext;
use rustc_middle::ty::{self, TyCtxt};

/// 解析函数调用或方法调用的定义路径，不跟踪函数指针与其他间接调用。
///
/// # Params:
///   - `cx`: 提供类型检查结果与定义信息的 lint 上下文。
///   - `expr`: 待识别的调用表达式。
///
/// # Return:
///   可静态解析的调用路径；非调用或间接调用返回 `None`。
pub(super) fn resolve(cx: &LateContext<'_>, expr: &Expr<'_>) -> Option<String> {
    let definition = match expr.kind {
        ExprKind::MethodCall(..) => cx.typeck_results().type_dependent_def_id(expr.hir_id)?,
        ExprKind::Call(callee, _) => {
            let ty::FnDef(definition, _) = cx.typeck_results().expr_ty(callee).kind() else {
                return None;
            };
            *definition
        }
        _ => return None,
    };
    Some(definition_path(cx.tcx, definition))
}

/// 将关联项归到其 trait 或固有实现类型，避免配置依赖匿名 impl 编号。
///
/// # Params:
///   - `tcx`: 提供关联项归属与实现类型的类型上下文。
///   - `definition`: 调用的函数或方法定义。
///
/// # Return:
///   包含 crate 名的函数路径，或所属 trait、类型与关联项名组成的路径。
fn definition_path(tcx: TyCtxt<'_>, definition: DefId) -> String {
    if let Some(trait_definition) = tcx.trait_of_assoc(definition) {
        let owner = item_path(tcx, trait_definition);
        let name = tcx.item_name(definition);
        return format!("{owner}::{name}");
    }

    if let Some(implementation) = tcx.impl_of_assoc(definition) {
        if let Some(trait_ref) = tcx.impl_opt_trait_ref(implementation) {
            let owner = item_path(tcx, trait_ref.skip_binder().def_id);
            let name = tcx.item_name(definition);
            return format!("{owner}::{name}");
        }

        let receiver = tcx
            .type_of(implementation)
            .instantiate_identity()
            .skip_normalization()
            .peel_refs();
        let owner = match receiver.kind() {
            ty::Adt(definition, _) => item_path(tcx, definition.did()),
            ty::Str => String::from("str"),
            ty::Slice(_) => String::from("[T]"),
            _ => return item_path(tcx, definition),
        };
        let name = tcx.item_name(definition);
        return format!("{owner}::{name}");
    }

    item_path(tcx, definition)
}

/// 拼接定义所属 crate 名与编译器记录的完整项路径。
///
/// # Params:
///   - `tcx`: 提供 crate 名和定义路径的类型上下文。
///   - `definition`: 需要命名的定义。
///
/// # Return:
///   不受调用处导入别名影响的定义路径。
fn item_path(tcx: TyCtxt<'_>, definition: DefId) -> String {
    format!(
        "{}{}",
        tcx.crate_name(definition.krate),
        tcx.def_path(definition).to_string_no_crate_verbose()
    )
}
