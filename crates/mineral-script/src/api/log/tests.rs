//! `mineral.log.*` 族级测试。

use crate::api::test_support::vm_with_host;

#[test]
fn log_calls_do_not_error() -> color_eyre::Result<()> {
    let (lua, _host) = vm_with_host()?;
    lua.load(r#"local mineral = require("mineral.daemon"); mineral.log.info("i"); mineral.log.warn("w")"#)
        .exec()?;
    Ok(())
}
