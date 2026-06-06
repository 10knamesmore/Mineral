//! `mineral.player.*` 命令族与 `mineral.download`:脚本 → daemon 的
//! 播放器控制出口。全部 fire-and-forget(命令进 channel,daemon 侧
//! 独立 task drain;PR-4 接线前另一头无消费者,发送即丢)。

use mineral_model::{SongId, SourceKind};
use mineral_protocol::PlayMode;
use mlua::{Lua, Table};
use tokio::sync::mpsc::UnboundedSender;

use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `player` 子表与顶层 `download` 挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄(闭包捕获其命令出口)
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let player = lua.create_table()?;

    install_nullary(lua, &player, &host.commands, "toggle", ScriptCmd::Toggle)?;
    install_nullary(lua, &player, &host.commands, "next", ScriptCmd::Next)?;
    install_nullary(lua, &player, &host.commands, "prev", ScriptCmd::Prev)?;
    install_nullary(lua, &player, &host.commands, "stop", ScriptCmd::Stop)?;

    let commands = host.commands.clone();
    player.set(
        "seek_rel",
        lua.create_function(move |_lua, secs: f64| {
            let _ = commands.send(ScriptCmd::SeekRel(secs));
            Ok(())
        })?,
    )?;

    let commands = host.commands.clone();
    player.set(
        "seek_to",
        lua.create_function(move |_lua, secs: f64| {
            // 负数压回 0(与音量 clamp 同一容忍风格)。
            let _ = commands.send(ScriptCmd::SeekTo(secs.max(0.0)));
            Ok(())
        })?,
    )?;

    let commands = host.commands.clone();
    player.set(
        "set_volume",
        lua.create_function(move |_lua, pct: i64| {
            // 越界 clamp 到 0..=100(用户裁决:容忍,不报错)。
            let clamped = u8::try_from(pct.clamp(0, 100)).unwrap_or(100);
            let _ = commands.send(ScriptCmd::SetVolume(clamped));
            Ok(())
        })?,
    )?;

    let commands = host.commands.clone();
    player.set(
        "set_mode",
        lua.create_function(move |_lua, mode: String| {
            let Some(mode) = PlayMode::from_script_name(&mode) else {
                return Err(mlua::Error::RuntimeError(format!(
                    "unknown play mode {mode:?}, expected \"sequential\" | \"shuffle\" | \"repeat_all\" | \"repeat_one\""
                )));
            };
            let _ = commands.send(ScriptCmd::SetMode(mode));
            Ok(())
        })?,
    )?;

    let commands = host.commands.clone();
    player.set(
        "play",
        lua.create_function(move |_lua, song_id: String| {
            let _ = commands.send(ScriptCmd::Play(parse_song_id(&song_id)?));
            Ok(())
        })?,
    )?;

    mineral.set("player", player)?;

    let commands = host.commands.clone();
    mineral.set(
        "download",
        lua.create_function(move |_lua, song_id: String| {
            let _ = commands.send(ScriptCmd::Download(parse_song_id(&song_id)?));
            Ok(())
        })?,
    )
}

/// 挂一个无参命令(toggle/next/prev/stop 同构,收编样板)。
fn install_nullary(
    lua: &Lua,
    player: &Table,
    commands: &UnboundedSender<ScriptCmd>,
    name: &str,
    cmd: ScriptCmd,
) -> mlua::Result<()> {
    let commands = commands.clone();
    player.set(
        name,
        lua.create_function(move |_lua, ()| {
            let _ = commands.send(cmd.clone());
            Ok(())
        })?,
    )
}

/// 解析 qualified 形式的歌曲 id(`"namespace:value"`,即事件回调里
/// `args.song.id` 给出的格式)。
///
/// # Params:
///   - `raw`: 脚本侧输入
///
/// # Return:
///   结构化 [`SongId`];缺冒号 / 两段有空者报 Lua 错。
fn parse_song_id(raw: &str) -> mlua::Result<SongId> {
    let bad = || {
        mlua::Error::RuntimeError(format!(
            "invalid song id {raw:?}, expected \"namespace:value\" (e.g. \"netease:123\")"
        ))
    };
    let (namespace, value) = raw.split_once(':').ok_or_else(bad)?;
    if namespace.is_empty() || value.is_empty() {
        return Err(bad());
    }
    // namespace 开放(插件源),未知名经 intern 铸造 —— 与模型层哲学一致。
    Ok(SongId::new(SourceKind::from_name(namespace), value))
}

#[cfg(test)]
mod tests {
    use mineral_model::{SongId, SourceKind};
    use mineral_protocol::PlayMode;
    use mlua::Lua;
    use tokio::sync::mpsc::unbounded_channel;

    use crate::host::{ScriptHost, install_api};
    use crate::message::ScriptCmd;

    /// 装好 API 的 VM + 命令接收端。
    fn vm_with_commands()
    -> color_eyre::Result<(Lua, tokio::sync::mpsc::UnboundedReceiver<ScriptCmd>)> {
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (push_tx, _push_rx) = unbounded_channel();
        let host = ScriptHost::new(cmd_tx, push_tx);
        let lua = Lua::new();
        install_api(&lua, &host)?;
        Ok((lua, cmd_rx))
    }

    /// 排干命令通道。
    fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<ScriptCmd>) -> Vec<ScriptCmd> {
        let mut cmds = Vec::new();
        while let Ok(cmd) = rx.try_recv() {
            cmds.push(cmd);
        }
        cmds
    }

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
            drain(&mut cmd_rx),
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
            drain(&mut cmd_rx),
            vec![
                ScriptCmd::SetVolume(100),
                ScriptCmd::SetVolume(0),
                ScriptCmd::SeekTo(0.0),
            ]
        );
        Ok(())
    }

    #[test]
    fn meta_stub_play_mode_alias_matches_rust() -> color_eyre::Result<()> {
        use color_eyre::eyre::WrapErr;
        // `mineral.PlayMode` 枚举必须与 Rust `PlayMode::script_name` 的
        // 全部取值逐字一致(顺序也钉死)。
        let meta_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../mineral-config/src/lua/meta/mineral.lua"
        );
        let meta = std::fs::read_to_string(meta_path).wrap_err("read meta/mineral.lua")?;
        let literals = [
            PlayMode::Sequential,
            PlayMode::Shuffle,
            PlayMode::RepeatAll,
            PlayMode::RepeatOne,
        ]
        .map(|mode| format!("\"{}\"", mode.script_name()))
        .join("|");
        let alias = format!("---@alias mineral.PlayMode {literals}");
        assert!(
            meta.contains(&alias),
            "meta stub 缺少与 Rust 一致的别名行:`{alias}`"
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
            lua.load(r#"mineral.player.play("just-a-number")"#)
                .exec()
                .is_err(),
            "缺 namespace 的 id 必须报 Lua 错"
        );
        assert!(
            lua.load(r#"mineral.download(":123")"#).exec().is_err(),
            "空 namespace 必须报 Lua 错"
        );
        assert!(drain(&mut cmd_rx).is_empty(), "报错时不得发出命令");
        Ok(())
    }
}
