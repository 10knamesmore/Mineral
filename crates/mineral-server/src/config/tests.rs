//! daemon 文件加载、默认值、schema 拒绝和回调摘取的独立边界。

use super::{daemon_from_tree, default_daemon_tree, load_daemon, load_daemon_with_vm};
use mineral_config::ConfigWarning;
use mineral_script::mlua::{Function, Table, Value};
use mineral_script::registry::{
    COPY_TEMPLATE_FNS, CURATE_PLAYLISTS_MERGED_FN, CURATE_PLAYLISTS_SOURCE_FNS, DAEMON_SETUP_FN,
    QUEUE_TRANSFORM_FNS,
};

/// 即使旧共享文件与其他宿主文件损坏,loader 也只读取指定文件。
#[test]
fn daemon_loader_ignores_tui_and_shared_files() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(
        dir.path().join("daemon.lua"),
        "return { audio = { volume = 83 }, heartbeat_secs = 42 }",
    )?;
    std::fs::write(dir.path().join("tui.lua"), "error('must not run')")?;
    std::fs::write(dir.path().join("config.lua"), "error('must not run')")?;
    let loaded = load_daemon_with_vm(&dir.path().join("daemon.lua"), |_| Ok(()))?;
    assert!(loaded.warnings.is_empty());
    assert_eq!(*loaded.config.audio().volume(), 83);
    assert_eq!(*loaded.config.heartbeat_secs(), 42);
    assert!(loaded.vm.is_some());
    Ok(())
}

/// 错误的 daemon 文件只交还默认与诊断,不交还可能已注册回调的 VM。
#[test]
fn failed_daemon_file_returns_no_vm() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("daemon.lua");
    std::fs::write(&path, "return { audio = { volume = 'invalid' } }")?;
    let loaded = load_daemon_with_vm(&path, |_| Ok(()))?;
    assert!(loaded.vm.is_none());
    assert!(matches!(
        loaded.warnings.as_slice(),
        [ConfigWarning::Deserialize { .. }]
    ));
    std::fs::write(&path, "error('top-level failure')")?;
    let loaded = load_daemon_with_vm(&path, |_| Ok(()))?;
    assert!(loaded.vm.is_none());
    assert!(
        matches!(loaded.warnings.as_slice(), [ConfigWarning::Eval { path: failed, .. }] if failed == &path)
    );
    Ok(())
}

/// daemon.lua 的 setup 与音乐回调摘进同一 VM,setup 不在加载阶段调用。
#[test]
fn daemon_extracts_setup_and_music_callbacks_by_name() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("daemon.lua");
    std::fs::write(
        &path,
        r#"
        assert(api_installed)
        return {
          setup = function(api) setup_identity = api.identity end,
          queue = { transforms = {
            { name = "Reverse", transform = function(songs) return songs end },
          } },
          sources = {
            curate_playlists = function(lists) return lists end,
            bilibili = { curate_playlists = function(lists) return lists end },
          },
        }
    "#,
    )?;
    let loaded = load_daemon_with_vm(&path, |lua| lua.globals().set("api_installed", true))?;
    assert!(loaded.warnings.is_empty());
    assert_eq!(
        loaded
            .config
            .queue()
            .transforms()
            .first()
            .map(|operation| operation.name().as_str()),
        Some("Reverse")
    );
    assert!(loaded.tree.pointer("/setup").is_none());
    assert!(
        loaded
            .tree
            .pointer("/queue/transforms/0/transform")
            .is_none()
    );
    let lua = loaded
        .vm
        .ok_or_else(|| color_eyre::eyre::eyre!("missing daemon VM"))?;
    assert!(matches!(
        lua.globals().get::<Value>("setup_identity")?,
        Value::Nil
    ));
    let setup = lua.named_registry_value::<Function>(DAEMON_SETUP_FN)?;
    let module = lua.create_table()?;
    module.set("identity", "daemon")?;
    setup.call::<()>(module)?;
    assert_eq!(lua.globals().get::<String>("setup_identity")?, "daemon");
    let transforms = lua.named_registry_value::<Table>(QUEUE_TRANSFORM_FNS)?;
    transforms.get::<Function>("Reverse")?;
    let source_curate = lua.named_registry_value::<Table>(CURATE_PLAYLISTS_SOURCE_FNS)?;
    source_curate.get::<Function>("bilibili")?;
    lua.named_registry_value::<Function>(CURATE_PLAYLISTS_MERGED_FN)?;
    assert!(matches!(
        lua.named_registry_value::<Value>(COPY_TEMPLATE_FNS)?,
        Value::Nil
    ));
    Ok(())
}

/// 队列操作使用唯一名称,不接受旧 key/label 形态,文件与覆盖落型规则一致。
#[test]
fn queue_transform_names_are_required_and_unique() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("daemon.lua");
    for source in [
        "return { queue = { transforms = { { name = 'Missing callback' } } } }",
        "return { queue = { transforms = { { label = 'Reverse', transform = function(q) return q end } } } }",
        "return { queue = { transforms = { { name = 'Reverse', key = 'r', transform = function(q) return q end } } } }",
        "return { queue = { transforms = { { name = 'Same', transform = function(q) return q end }, { name = 'Same', transform = function(q) return q end } } } }",
        "return { queue = { transforms = { { name = ' ', transform = function(q) return q end } } } }",
    ] {
        std::fs::write(&path, source)?;
        let loaded = load_daemon_with_vm(&path, |_| Ok(()))?;
        assert!(loaded.vm.is_none());
        assert!(!loaded.warnings.is_empty());
    }
    let tree = mineral_config::merge_tree(
        default_daemon_tree()?,
        serde_json::json!({ "queue": { "transforms": [{ "name": "Same" }, { "name": "Same" }] } }),
    );
    assert!(daemon_from_tree(&tree).is_err());
    Ok(())
}
/// 内置 daemon 默认直接落型,没有任何根包装或 TUI 字段。
#[test]
fn defaults_have_daemon_root() -> color_eyre::Result<()> {
    let daemon = default_daemon_tree()?;
    assert!(daemon.get("tui").is_none());
    assert!(daemon.get("daemon").is_none());
    assert!(daemon.get("gapless_prefetch_ms").is_some());
    daemon_from_tree(&daemon)?;
    Ok(())
}

/// 缺失 daemon 文件只用自己的默认,不激活 VM 或读取共享文件。
#[test]
fn missing_file_uses_daemon_defaults() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(
        dir.path().join("config.lua"),
        "return { audio = { volume = 1 } }",
    )?;
    let daemon = load_daemon_with_vm(&dir.path().join("daemon.lua"), |_| Ok(()))?;
    assert!(daemon.warnings.is_empty());
    assert!(daemon.vm.is_none());
    assert_eq!(*daemon.config.audio().volume(), 100);
    Ok(())
}

/// 普通校验只摘取 setup,不会执行其中的运行时命令。
#[test]
fn check_does_not_execute_setup() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("daemon.lua");
    std::fs::write(
        &path,
        "return { setup = function(api) error('runtime only') end }",
    )?;
    assert!(load_daemon(&path)?.1.is_empty());
    std::fs::write(&path, "mineral.player.toggle(); return {}")?;
    assert!(matches!(
        load_daemon(&path)?.1.as_slice(),
        [ConfigWarning::Eval { .. }]
    ));
    Ok(())
}

/// daemon 数据树不接受来源颜色或旧根包装;覆盖与文件规则一致。
#[test]
fn trees_reject_tui_colors_and_wrappers() -> color_eyre::Result<()> {
    let tree = mineral_config::merge_tree(
        default_daemon_tree()?,
        serde_json::json!({ "sources": { "netease": { "color": "red" } } }),
    );
    assert!(daemon_from_tree(&tree).is_err());
    for namespace in ["daemon", "tui"] {
        let patch = serde_json::json!({ namespace: {} });
        assert!(
            daemon_from_tree(&mineral_config::merge_tree(default_daemon_tree()?, patch)).is_err()
        );
    }
    Ok(())
}

/// 旧包装、其他宿主字段和非法 setup 都使整次 daemon 文件被拒。
#[test]
fn file_rejects_wrappers_and_invalid_fields() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let daemon_path = dir.path().join("daemon.lua");
    for source in [
        "return { audio = { volume = 83 }, tui = {} }",
        "return { audio = { volume = 83 }, daemon = {} }",
        "return { daemon = { setup = function(api) end } }",
        "return { setup = false }",
        "return { setup = 42 }",
        "return { setup = 'invalid' }",
        "return { setup = {} }",
    ] {
        std::fs::write(&daemon_path, source)?;
        let daemon = load_daemon_with_vm(&daemon_path, |_| Ok(()))?;
        assert!(daemon.vm.is_none());
        assert_eq!(*daemon.config.audio().volume(), 100);
        assert!(matches!(
            daemon.warnings.as_slice(),
            [ConfigWarning::Serialize { .. } | ConfigWarning::Deserialize { .. }]
        ));
    }

    Ok(())
}
