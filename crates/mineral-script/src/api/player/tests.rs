//! `mineral.player.*` 命令族的族级测试:Lua 调用 → 结构化 [`ScriptCmd`] 的
//! 完整映射、入参 clamp 与校验。

use mineral_model::{SongId, SourceKind};
use mineral_protocol::PlayMode;

use crate::api::test_support::{drain_cmds, vm_with_commands};
use crate::message::ScriptCmd;

#[test]
fn command_family_maps_to_script_cmds() -> color_eyre::Result<()> {
    let (lua, mut cmd_rx) = vm_with_commands()?;
    lua.load(
        r#"
        mineral.player.toggle()
        mineral.player.next()
        mineral.player.prev()
        mineral.player.stop()
        mineral.player.seek_rel(-5.5)
        mineral.player.seek_to(30)
        mineral.player.set_mode("repeat_all")
        mineral.player.play("netease:42")
        mineral.download("local:abc")
        "#,
    )
    .exec()?;
    assert_eq!(
        drain_cmds(&mut cmd_rx),
        vec![
            ScriptCmd::Toggle,
            ScriptCmd::Next,
            ScriptCmd::Prev,
            ScriptCmd::Stop,
            ScriptCmd::SeekRel(-5.5),
            ScriptCmd::SeekTo(30.0),
            ScriptCmd::SetMode(PlayMode::RepeatAll),
            ScriptCmd::Play(SongId::new(SourceKind::NETEASE, "42")),
            ScriptCmd::Download(SongId::new(SourceKind::LOCAL, "abc")),
        ]
    );
    Ok(())
}

#[test]
fn volume_and_seek_clamp_out_of_range() -> color_eyre::Result<()> {
    let (lua, mut cmd_rx) = vm_with_commands()?;
    lua.load(
        r#"
        mineral.player.set_volume(150)
        mineral.player.set_volume(-3)
        mineral.player.seek_to(-10)
        "#,
    )
    .exec()?;
    assert_eq!(
        drain_cmds(&mut cmd_rx),
        vec![
            ScriptCmd::SetVolume(100),
            ScriptCmd::SetVolume(0),
            ScriptCmd::SeekTo(0.0),
        ],
        "音量与负 seek 越界 clamp,不报错"
    );
    Ok(())
}

#[test]
fn unknown_mode_and_bad_song_id_are_lua_errors() -> color_eyre::Result<()> {
    let (lua, mut cmd_rx) = vm_with_commands()?;
    assert!(
        lua.load(r#"mineral.player.set_mode("random")"#)
            .exec()
            .is_err(),
        "未知模式名必须报 Lua 错"
    );
    assert!(
        lua.load(r#"mineral.player.play("42")"#).exec().is_err(),
        "缺 namespace 的 song id 必须报 Lua 错"
    );
    assert!(drain_cmds(&mut cmd_rx).is_empty(), "报错时不得发出命令");
    Ok(())
}
