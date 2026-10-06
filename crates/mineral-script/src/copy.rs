//! TUI 本地复制模板的音乐实体与受看门狗保护的渲染。

use mineral_model::{Album, Artist, Playlist, Song};
use mlua::Lua;

use crate::projection::{album_table, artist_table, playlist_table, song_table};
use crate::watchdog::{WatchdogConfig, call_guarded};
use crate::{Error, Result};

/// 本地复制模板的输入；使用 TUI 已加载的实体，不向 daemon 发渲染请求。
#[derive(Clone, Debug, PartialEq)]
pub enum CopyTemplateCtx {
    /// 一首歌曲，不携带 collection 下标。
    Song(Box<Song>),

    /// 歌单及已经加载的曲目。
    Playlist(Box<Playlist>),

    /// 专辑及已经加载的曲目。
    Album(Box<Album>),

    /// 艺人及已经加载的代表曲。
    Artist(Box<Artist>),
}

/// 按 TUI 配置的零基下标渲染复制文本；错误只供诊断，不作为界面文案。
pub(crate) fn render(
    lua: &Lua,
    watchdog: &WatchdogConfig,
    index: usize,
    ctx: CopyTemplateCtx,
) -> Result<String> {
    let fns = lua
        .named_registry_value::<mlua::Table>(crate::registry::COPY_TEMPLATE_FNS)
        .map_err(|source| Error::Lua {
            operation: "读取模板函数表",
            source,
        })?;
    let func = fns
        .get::<mlua::Function>(index.saturating_add(1))
        .map_err(|source| Error::MissingFunction {
            kind: "模板",
            index,
            source,
        })?;
    let arg = match ctx {
        CopyTemplateCtx::Song(song) => song_table(lua, &song),
        CopyTemplateCtx::Playlist(playlist) => playlist_table(lua, &playlist),
        CopyTemplateCtx::Album(album) => album_table(lua, &album),
        CopyTemplateCtx::Artist(artist) => artist_table(lua, &artist),
    }
    .map_err(|source| Error::Lua {
        operation: "实体投影",
        source,
    })?;
    call_guarded::<_, String>(lua, watchdog, &func, arg).map_err(|source| {
        mineral_log::error!(
            target: "script",
            index,
            error = mineral_log::chain(&source),
            "local copy template failed"
        );
        Error::Lua {
            operation: "执行复制模板",
            source,
        }
    })
}
