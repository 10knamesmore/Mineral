//! 读取脚本返回值的字段，为类型错误补充实体与字段上下文。

/// 从脚本返回值 table 取一个字段,错误带「哪个实体的哪个字段」上下文
/// (mlua 原始错误链保留为 cause)。
///
/// # Params:
///   - `table`: 返回值 table
///   - `entity`: 实体描述(如 `"hook 返回值"` / `"curate 返回的第 2 条"`)
///   - `field`: 字段名
///
/// # Return:
///   字段值,类型不符时 `Err` 带上下文。
pub(crate) fn lua_field<T: mlua::FromLua>(
    table: &mlua::Table,
    entity: &str,
    field: &str,
) -> mlua::Result<T> {
    use mlua::ErrorContext;
    table
        .get::<T>(field)
        .with_context(|_cause| format!("{entity}的 {field} 字段非法"))
}
