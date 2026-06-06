//! `mineral.log.*` 族级测试。

use crate::api::test_support::vm_with_push;

#[test]
fn log_calls_do_not_error() -> color_eyre::Result<()> {
    let (lua, _push_rx) = vm_with_push()?;
    lua.load(r#"mineral.log.info("i"); mineral.log.warn("w")"#)
        .exec()?;
    Ok(())
}
