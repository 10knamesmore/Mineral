//! TUI 文件加载、默认值、schema 拒绝和本地回调摘取的独立边界。

use super::loader::tui_from_source;
use super::{default_tui_tree, load_tui, tui_from_tree};
use mineral_config::{ConfigWarning, Error};
use mineral_script::mlua::{Function, Lua, Table, Value};
use mineral_script::registry::{COPY_TEMPLATE_FNS, QUEUE_TRANSFORM_FNS, TUI_SETUP_FN};

/// TUI 只加载自己的文件与默认,不求值 daemon 或共享配置。
#[test]
fn tui_loader_ignores_daemon_and_shared_files() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("tui.lua"), "return { heartbeat_secs = 42 }")?;
    std::fs::write(dir.path().join("daemon.lua"), "error('must not run')")?;
    std::fs::write(dir.path().join("config.lua"), "error('must not run')")?;
    let (config, warnings) = load_tui(&dir.path().join("tui.lua"))?;
    assert!(warnings.is_empty());
    assert_eq!(*config.heartbeat_secs(), 42);
    Ok(())
}

/// TUI 源码加载保留闭包状态与模板函数,不执行 setup,不创建 daemon 回调表。
#[test]
fn tui_source_extracts_local_setup_and_persistent_templates() -> color_eyre::Result<()> {
    let lua = Lua::new();
    let (config, tree) = tui_from_source(
        &lua,
        r#"
        local count = 0
        return {
          setup = function(api) setup_identity = api.identity end,
          copy = { templates = {
            { label = "identity", template = function(song)
              count = count + 1
              return song.id .. "/" .. count
            end },
          } },
        }
    "#,
        "tui.lua",
    )?;
    assert_eq!(config.copy().templates().len(), 1);
    assert!(tree.pointer("/setup").is_none());
    assert!(tree.pointer("/copy/templates/0/template").is_none());
    assert!(matches!(
        lua.globals().get::<Value>("setup_identity")?,
        Value::Nil
    ));
    let setup = lua.named_registry_value::<Function>(TUI_SETUP_FN)?;
    let module = lua.create_table()?;
    module.set("identity", "tui")?;
    setup.call::<()>(module)?;
    assert_eq!(lua.globals().get::<String>("setup_identity")?, "tui");
    let templates = lua.named_registry_value::<Table>(COPY_TEMPLATE_FNS)?;
    let template = templates.get::<Function>(1)?;
    let song = lua.create_table()?;
    song.set("id", "local:42")?;
    assert_eq!(template.call::<String>(song.clone())?, "local:42/1");
    assert_eq!(template.call::<String>(song)?, "local:42/2");
    assert!(matches!(
        lua.named_registry_value::<Value>(QUEUE_TRANSFORM_FNS)?,
        Value::Nil
    ));
    Ok(())
}

/// 源码求值失败是结构化错误,不把非法 TUI 字段回落成一次成功重载。
#[test]
fn tui_source_returns_structured_validation_errors() {
    let lua = Lua::new();
    let error = tui_from_source(&lua, "return { audio = {} }", "tui.lua");
    assert!(matches!(error, Err(Error::InvalidConfig { .. })));
    let error = tui_from_source(&lua, "return { setup = false }", "tui.lua");
    assert!(matches!(error, Err(Error::InvalidConfig { .. })));
    let error = tui_from_source(&lua, "return {", "tui.lua");
    assert!(matches!(error, Err(Error::Lua { .. })));
    let error = tui_from_source(
        &lua,
        "return { copy = { templates = { { label = 'Missing callback' } } } }",
        "tui.lua",
    );
    assert!(matches!(error, Err(Error::Lua { .. })));
}

/// 内置 TUI 默认直接落型,没有根包装或 daemon 字段。
#[test]
fn defaults_have_tui_root() -> color_eyre::Result<()> {
    let tui = default_tui_tree()?;
    assert!(tui.get("tui").is_none());
    assert!(tui.get("theme").is_some());
    assert!(tui.get("heartbeat_secs").is_some());
    tui_from_tree(&tui)?;
    Ok(())
}

/// 缺失 TUI 文件只用本地默认,不读取共享文件。
#[test]
fn missing_file_uses_tui_defaults() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(
        dir.path().join("config.lua"),
        "return { audio = { volume = 1 } }",
    )?;
    let (tui, warnings) = load_tui(&dir.path().join("tui.lua"))?;
    assert!(warnings.is_empty());
    assert_eq!(*tui.heartbeat_secs(), 180);
    Ok(())
}

/// 普通校验只摘取 setup;其中的运行时错误不发生,语法错误仍须报告。
#[test]
fn check_does_not_execute_setup() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("tui.lua");
    std::fs::write(
        &path,
        "return { setup = function(api) error('runtime only') end }",
    )?;
    assert!(load_tui(&path)?.1.is_empty());
    std::fs::write(
        &path,
        "return { setup = function(api) local invalid = ) end }",
    )?;
    assert!(matches!(
        load_tui(&path)?.1.as_slice(),
        [ConfigWarning::Eval { .. }]
    ));
    Ok(())
}

/// 本地数据树拒绝 daemon 字段和旧包装,避免覆盖跨过宿主边界。
#[test]
fn trees_reject_daemon_fields_and_wrappers() -> color_eyre::Result<()> {
    let tree = mineral_config::merge_tree(default_tui_tree()?, serde_json::json!({ "audio": {} }));
    assert!(tui_from_tree(&tree).is_err());
    for namespace in ["daemon", "tui"] {
        let patch = serde_json::json!({ namespace: {} });
        assert!(tui_from_tree(&mineral_config::merge_tree(default_tui_tree()?, patch)).is_err());
    }
    Ok(())
}

/// 旧包装、其他宿主字段和非法 setup 都使整次 TUI 文件被拒。
#[test]
fn file_rejects_wrappers_and_invalid_fields() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let tui_path = dir.path().join("tui.lua");
    for source in [
        "return { heartbeat_secs = 42, audio = {} }",
        "return { tui = {} }",
        "return { tui = { setup = function(api) end } }",
        "return { daemon = {} }",
        "return { setup = false }",
        "return { setup = 42 }",
        "return { setup = 'invalid' }",
        "return { setup = {} }",
        "return { sources = {} }",
        "return { queue = {} }",
        "return { stats = {} }",
        "return { cache = {} }",
        "return { download = {} }",
        "return { script = { hook_timeout_ms = 100 } }",
    ] {
        std::fs::write(&tui_path, source)?;
        let (tui, warnings) = load_tui(&tui_path)?;
        assert_eq!(*tui.heartbeat_secs(), 180);
        assert!(matches!(
            warnings.as_slice(),
            [ConfigWarning::Serialize { .. } | ConfigWarning::Deserialize { .. }]
        ));
    }
    Ok(())
}
