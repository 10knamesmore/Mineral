//! 执行歌单整理、复制模板和队列变换，解释脚本返回值。

use mlua::Lua;

use super::callbacks::report_callback_failure;
use super::projection::{album_table, artist_table, briefs_table, playlist_table, song_table};
use super::return_value::lua_field;
use crate::host::ScriptHost;
use crate::watchdog::{WatchdogConfig, call_guarded};

/// 跑一级 curate transform:registry 取函数(缺席 = 常态透传),投影入参,
/// 看门狗保护执行,返回值解释成采纳条目。函数失败 / 返回非法形态一律
/// [`CurateOutcome::Identity`](crate::message::CurateOutcome::Identity)
/// (fail-open,歌单不因脚本 bug 消失)+ 记日志
/// + error toast。
pub(super) fn run_curate(
    lua: &Lua,
    host: &ScriptHost,
    watchdog: &WatchdogConfig,
    source: Option<&mineral_model::SourceKind>,
    briefs: &[crate::message::PlaylistBrief],
) -> crate::message::CurateOutcome {
    use crate::message::CurateOutcome;
    let Some(func) = curate_fn(lua, source) else {
        return CurateOutcome::Identity;
    };
    let outcome = briefs_table(lua, briefs)
        .and_then(|list| call_guarded::<_, mlua::Value>(lua, watchdog, &func, list))
        .and_then(|value| interpret_curate_return(&value));
    match outcome {
        Ok(entries) => CurateOutcome::Curated(entries),
        Err(e) => {
            report_callback_failure(host, "curate_playlists", &e);
            CurateOutcome::Identity
        }
    }
}

/// 按级取 curate 函数:`Some(kind)` = per-source 表按源名索引;`None` = 跨源
/// 独立键。registry 值缺席 / 该源无函数 → `None`(常态,不是错误)。
fn curate_fn(lua: &Lua, source: Option<&mineral_model::SourceKind>) -> Option<mlua::Function> {
    match source {
        Some(kind) => lua
            .named_registry_value::<mlua::Table>(mineral_config::CURATE_PLAYLISTS_SOURCE_FNS)
            .ok()?
            .get::<mlua::Function>(kind.name())
            .ok(),
        None => lua
            .named_registry_value::<mlua::Function>(mineral_config::CURATE_PLAYLISTS_MERGED_FN)
            .ok(),
    }
}

/// per-source curate 函数的源名键集(daemon 对无对应 channel 的键打 warn 用)。
pub(super) fn curate_source_keys(lua: &Lua) -> Vec<String> {
    let Ok(fns) =
        lua.named_registry_value::<mlua::Table>(mineral_config::CURATE_PLAYLISTS_SOURCE_FNS)
    else {
        return Vec::new();
    };
    fns.pairs::<String, mlua::Value>()
        .filter_map(|pair| pair.map(|(key, _value)| key).ok())
        .collect::<Vec<String>>()
}

/// 把 curate 函数的 Lua 返回值解释成采纳条目;非法形态整体报 `Err`(按透传
/// 处理)——逐条跳过会把「写错一条」放大成「歌单静默消失」,宁可全量透传。
fn interpret_curate_return(value: &mlua::Value) -> mlua::Result<Vec<crate::message::CuratedEntry>> {
    use mlua::ErrorContext;
    let mlua::Value::Table(list) = value else {
        return Err(mlua::Error::runtime(format!(
            "curate_playlists 须返回歌单数组,实得 {}",
            value.type_name()
        )));
    };
    let mut entries = Vec::new();
    for i in 1..=list.raw_len() {
        let entity = format!("curate 返回的第 {i} 条");
        let item = list
            .get::<mlua::Table>(i)
            .with_context(|_cause| format!("{entity}不是 table"))?;
        let raw_id = item
            .get::<String>("id")
            .with_context(|_cause| format!("{entity}缺 id(隐藏请省略整条)"))?;
        let id = crate::api::value::parse_playlist_id(&raw_id)
            .with_context(|_cause| format!("{entity}的 id 非法"))?;
        let name = lua_field::<Option<String>>(&item, &entity, "name")?;
        let description = lua_field::<Option<String>>(&item, &entity, "description")?;
        entries.push(crate::message::CuratedEntry {
            id,
            name,
            description,
        });
    }
    Ok(entries)
}

/// 渲染一个复制模板:registry 函数表按下标取函数,实体投影成表喂入,看门狗
/// 保护执行,返回剪贴板文本。错误侧是人读首行短文(回执给 client toast),
/// 完整链在这里进日志。
pub(super) fn render_copy_template(
    lua: &Lua,
    watchdog: &WatchdogConfig,
    index: usize,
    ctx: &mineral_protocol::CopyTemplateCtx,
) -> Result<String, String> {
    use mineral_protocol::CopyTemplateCtx;
    let fns: mlua::Table = lua
        .named_registry_value(mineral_config::COPY_TEMPLATE_FNS)
        .map_err(|e| format!("模板函数表缺失:{e}"))?;
    // protocol 下标 0-based,Lua 数组 1-based。
    let func: mlua::Function = fns
        .get(index.saturating_add(1))
        .map_err(|_not_a_function| format!("模板 #{index} 没有可调用的 template 函数"))?;
    let arg = match ctx {
        CopyTemplateCtx::Song(song) => song_table(lua, song),
        CopyTemplateCtx::Playlist(playlist) => playlist_table(lua, playlist),
        CopyTemplateCtx::Album(album) => album_table(lua, album),
        CopyTemplateCtx::Artist(artist) => artist_table(lua, artist),
    }
    .map_err(|e| format!("实体投影失败:{e}"))?;
    call_guarded::<_, String>(lua, watchdog, &func, arg).map_err(|e| {
        mineral_log::error!(
            target: "script",
            index,
            error = mineral_log::chain(&e),
            "copy template failed"
        );
        // 回执只给首行(toast 的人读信息);mlua 错误的 traceback 多行。
        mineral_log::chain(&e)
            .lines()
            .next()
            .unwrap_or("脚本错误(详见日志)")
            .to_owned()
    })
}

/// 跑一个具名队列变换,回执新的队列顺序(只取 id,实体由 daemon 从原队列回捞)。
///
/// 只读 id 是刻意的:脚本手里的 song 表是有损投影(艺人 / 专辑只有名字,没有 id),让它
/// 直接构造 `Song` 会造出半残实体。凭空引入新歌因而不被支持——变换的能力边界是删减与排序。
///
/// # Params:
///   - `lua`: 脚本 VM
///   - `watchdog`: 看门狗阈值
///   - `index`: 变换下标(0-based)
///   - `queue`: 当前队列(有序)
///   - `current`: 在播条目下标(0-based)
///   - `selected`: 光标下标(0-based),无则 `None`
///
/// # Return:
///   新顺序的 id 序列;函数缺失 / 报错 / 返回值不是歌表数组时返回错误串(调用方 fail-open)。
pub(super) fn run_queue_transform(
    lua: &Lua,
    watchdog: &WatchdogConfig,
    index: usize,
    queue: &[mineral_model::Song],
    current: usize,
    selected: Option<usize>,
) -> Result<Vec<mineral_model::SongId>, String> {
    let fns: mlua::Table = lua
        .named_registry_value(mineral_config::QUEUE_TRANSFORM_FNS)
        .map_err(|e| format!("变换函数表缺失:{e}"))?;
    // protocol 下标 0-based,Lua 数组 1-based。
    let func: mlua::Function = fns
        .get(index.saturating_add(1))
        .map_err(|_not_a_function| format!("变换 #{index} 没有可调用的 transform 函数"))?;
    let songs = lua
        .create_sequence_from(
            queue
                .iter()
                .map(|s| song_table(lua, s))
                .collect::<mlua::Result<Vec<_>>>()
                .map_err(|e| format!("队列投影失败:{e}"))?,
        )
        .map_err(|e| format!("队列投影失败:{e}"))?;
    let ctx = lua
        .create_table()
        .map_err(|e| format!("上下文投影失败:{e}"))?;
    // Lua 侧一律 1-based,与 songs 数组下标同口径。
    ctx.set("current", current.saturating_add(1))
        .and_then(|()| ctx.set("selected", selected.map(|at| at.saturating_add(1))))
        .map_err(|e| format!("上下文投影失败:{e}"))?;
    let returned: mlua::Table = call_guarded(lua, watchdog, &func, (songs, ctx)).map_err(|e| {
        mineral_log::error!(
            target: "script",
            index,
            error = mineral_log::chain(&e),
            "queue transform failed"
        );
        mineral_log::chain(&e)
            .lines()
            .next()
            .unwrap_or("脚本错误(详见日志)")
            .to_owned()
    })?;
    let mut ids = Vec::with_capacity(returned.raw_len());
    for at in 1..=returned.raw_len() {
        let entry: mlua::Table = returned
            .get(at)
            .map_err(|_not_a_table| format!("返回的第 {at} 项不是歌表"))?;
        let qualified: String = entry
            .get("id")
            .map_err(|_missing| format!("返回的第 {at} 项缺 id 字段"))?;
        ids.push(
            crate::api::value::parse_song_id(&qualified)
                .map_err(|e| format!("返回的第 {at} 项 id 无法解析:{e}"))?,
        );
    }
    Ok(ids)
}
