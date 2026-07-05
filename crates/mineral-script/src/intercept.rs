//! 同步拦截的脚本侧执行面:跑回调链、解释返回值成裁决,以及 DEFER 延迟裁决的
//! 回执槽管理。
//!
//! 回调链机制(短路 / DEFER / fail-open)是拦截点无关的,共享;ctx table 的
//! 字段装配是拦截点私有的,每个拦截点各写各的装配函数——新增拦截点 = 新入口 +
//! 新装配,不改既有装配。与类型面([`crate::hooks`])分离:这里是消息循环内的
//! 执行逻辑,由 dispatch 层的拦截消息臂调用。

use std::sync::Arc;

use mlua::Lua;

use crate::dispatch::{lua_field, report_callback_failure, song_table};
use crate::hooks::{BeforeDownloadCtx, BeforeStreamCtx, HookDecision, HookKind};
use crate::host::ScriptHost;
use crate::watchdog::{WatchdogConfig, call_guarded};

/// 待补交的拦截回执槽:`Some` = 裁决未交(同步返回或 `ctx.resolve` 补交时 take),
/// `None` = 已裁决(后续 resolve 一律 no-op)。
type PendingReply = Arc<parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<HookDecision>>>>;

/// 跑一次 `before_stream` 拦截并送出裁决。
///
/// ctx table:`song` / `url` / `quality`(URL 缺席时为 nil)/ `kind` / `mode`
/// (提交点口味)/ `unplayable` / `resolve`。
pub(crate) fn run_stream(
    lua: &Lua,
    host: &ScriptHost,
    watchdog: &WatchdogConfig,
    ctx: &BeforeStreamCtx,
    reply: tokio::sync::oneshot::Sender<HookDecision>,
) {
    run_intercept(
        lua,
        host,
        watchdog,
        HookKind::BeforeStream,
        reply,
        |lua, pending| {
            let table = lua.create_table()?;
            table.set("song", song_table(lua, ctx.song())?)?;
            if let Some(original) = ctx.original() {
                table.set("url", original.url.to_string())?;
                table.set("quality", original.quality.as_str())?;
            }
            table.set("kind", HookKind::BeforeStream.as_str())?;
            table.set("mode", ctx.mode().as_str())?;
            table.set("unplayable", ctx.unplayable())?;
            install_resolve(lua, &table, host, HookKind::BeforeStream, pending)?;
            Ok(table)
        },
    );
}

/// 跑一次 `before_download` 拦截并送出裁决。
///
/// ctx table:`song` / `url` / `quality`(直链缺席时为 nil)/ `kind` /
/// `unplayable` / `resolve`。
pub(crate) fn run_download(
    lua: &Lua,
    host: &ScriptHost,
    watchdog: &WatchdogConfig,
    ctx: &BeforeDownloadCtx,
    reply: tokio::sync::oneshot::Sender<HookDecision>,
) {
    run_intercept(
        lua,
        host,
        watchdog,
        HookKind::BeforeDownload,
        reply,
        |lua, pending| {
            let table = lua.create_table()?;
            table.set("song", song_table(lua, ctx.song())?)?;
            if let Some(original) = ctx.original() {
                table.set("url", original.url.to_string())?;
                table.set("quality", original.quality.as_str())?;
            }
            table.set("kind", HookKind::BeforeDownload.as_str())?;
            table.set("unplayable", ctx.unplayable())?;
            install_resolve(lua, &table, host, HookKind::BeforeDownload, pending)?;
            Ok(table)
        },
    );
}

/// 跑一次拦截并送出裁决;回调返回 [`DEFER`](crate::api::hook::DEFER_REGISTRY_KEY) 时
/// 不送——回执留在共享槽里,由脚本稍后经 `ctx.resolve(...)` 补交(daemon 侧软超时兜底)。
fn run_intercept(
    lua: &Lua,
    host: &ScriptHost,
    watchdog: &WatchdogConfig,
    kind: HookKind,
    reply: tokio::sync::oneshot::Sender<HookDecision>,
    build_ctx: impl Fn(&Lua, &PendingReply) -> mlua::Result<mlua::Table>,
) {
    let pending: PendingReply = Arc::new(parking_lot::Mutex::new(Some(reply)));
    if let Some(decision) = run_hooks(lua, host, watchdog, kind, &pending, build_ctx) {
        // 回执接收端 drop(daemon 侧超时放弃)时静默丢。
        if let Some(tx) = pending.lock().take() {
            let _ = tx.send(decision);
        }
    }
    // None = 有回调 DEFER:槽的所有权已在 ctx.resolve 闭包里,这里不送。
}

/// 跑一类同步拦截 hook:按注册顺序调用,首个非放行裁决(或 DEFER)短路生效。
///
/// ctx table 由 `build_ctx` 按拦截点装配(每个回调一张新表,resolve 闭包共享同一
/// 回执槽);返回值解释:`nil`(或 `true`)= 放行;`false` / `{ skip = 原因 }` =
/// 跳过;`{ url = ?, quality = ? }` = 改写;`mineral.DEFER` = 裁决稍后经
/// `ctx.resolve(...)` 补交(返回 `None`,回执留在 `pending` 槽)。Lua 错误 /
/// 非法返回值按放行处理(拦截失败不致命),记日志 + error toast。
fn run_hooks(
    lua: &Lua,
    host: &ScriptHost,
    watchdog: &WatchdogConfig,
    kind: HookKind,
    pending: &PendingReply,
    build_ctx: impl Fn(&Lua, &PendingReply) -> mlua::Result<mlua::Table>,
) -> Option<HookDecision> {
    // 锁内只克隆 Arc 列表,锁外调回调(回调里再注册不撞锁)。
    let callbacks = host
        .events
        .lock()
        .hooks
        .get(&kind)
        .cloned()
        .unwrap_or_default();
    for key in &callbacks {
        let outcome = build_ctx(lua, pending).and_then(|args| {
            let func = lua.registry_value::<mlua::Function>(key)?;
            call_guarded::<_, mlua::Value>(lua, watchdog, &func, args)
        });
        let value = match outcome {
            Ok(value) => value,
            Err(e) => {
                report_callback_failure(host, kind.as_str(), &e);
                continue;
            }
        };
        if is_defer_sentinel(lua, &value) {
            // 本回调认领裁决(稍后 resolve),短路后续回调。
            return None;
        }
        match interpret_hook_return(&value) {
            Ok(HookDecision::Continue) => {}
            Ok(decision) => return Some(decision),
            Err(e) => report_callback_failure(host, kind.as_str(), &e),
        }
    }
    Some(HookDecision::Continue)
}

/// 返回值是否 `mineral.DEFER` 哨兵(注册表存的唯一 table,按指针比对——值相等不算)。
fn is_defer_sentinel(lua: &Lua, value: &mlua::Value) -> bool {
    let Ok(defer) = lua.named_registry_value::<mlua::Table>(crate::api::hook::DEFER_REGISTRY_KEY)
    else {
        return false;
    };
    matches!(value, mlua::Value::Table(t) if t.to_pointer() == defer.to_pointer())
}

/// 往 ctx table 装 `resolve`(延迟补交裁决,配合返回 `mineral.DEFER` 用)。
///
/// 只认第一次调用(共享槽 take 语义;同步裁决已送出后再调同样 no-op),
/// 参数与同步返回值同一套协议(nil 放行 / table 改写或跳过);晚于 daemon 侧
/// 超时到达时回执接收端已 drop,静默丢。
fn install_resolve(
    lua: &Lua,
    table: &mlua::Table,
    host: &ScriptHost,
    kind: HookKind,
    pending: &PendingReply,
) -> mlua::Result<()> {
    let slot = Arc::clone(pending);
    let reporter = host.clone();
    let hook_name = kind.as_str();
    table.set(
        "resolve",
        lua.create_function(move |_lua, value: mlua::Value| {
            let Some(tx) = slot.lock().take() else {
                mineral_log::debug!(target: "script", hook = hook_name, "resolve 重复调用/已裁决,忽略");
                return Ok(());
            };
            let decision = match interpret_hook_return(&value) {
                Ok(decision) => decision,
                Err(e) => {
                    // 非法补交按放行(与同步路径同一条 fail-open 线),但裁决必须送出
                    // ——扣住回执只会白等到超时。
                    report_callback_failure(&reporter, hook_name, &e);
                    HookDecision::Continue
                }
            };
            // 回执接收端 drop(daemon 侧超时放弃)时静默丢。
            let _ = tx.send(decision);
            Ok(())
        })?,
    )
}

/// 把 hook 回调的 Lua 返回值解释成裁决;非法形态报 `Err`(按放行处理)。
fn interpret_hook_return(value: &mlua::Value) -> mlua::Result<crate::hooks::HookDecision> {
    use crate::hooks::RewriteSpec;
    const ENTITY: &str = "hook 返回值";
    match value {
        mlua::Value::Nil | mlua::Value::Boolean(true) => Ok(HookDecision::Continue),
        mlua::Value::Boolean(false) => Ok(HookDecision::Skip {
            reason: "脚本跳过".to_owned(),
        }),
        mlua::Value::Table(table) => {
            if let Some(reason) = lua_field::<Option<String>>(table, ENTITY, "skip")? {
                return Ok(HookDecision::Skip { reason });
            }
            let new_url = lua_field::<Option<String>>(table, ENTITY, "url")?
                .map(|raw| {
                    raw.parse::<mineral_model::MediaUrl>()
                        .map_err(|e| mlua::Error::runtime(format!("hook 返回的 url 解析失败: {e}")))
                })
                .transpose()?;
            let new_quality = lua_field::<Option<String>>(table, ENTITY, "quality")?
                .map(|raw| parse_bitrate(&raw))
                .transpose()?;
            // Lua 侧 `headers = { {name, value}, ... }`(数组的 {name,value} 对);缺项的行丢弃。
            let stream_headers = lua_field::<Option<Vec<Vec<String>>>>(table, ENTITY, "headers")?
                .map(|rows| {
                    rows.into_iter()
                        .filter_map(|row| {
                            let mut it = row.into_iter();
                            match (it.next(), it.next()) {
                                (Some(name), Some(value)) => Some((name, value)),
                                _ => None,
                            }
                        })
                        .collect::<Vec<(String, String)>>()
                });
            let layout = lua_field::<Option<String>>(table, ENTITY, "layout")?
                .map(|raw| parse_layout(&raw))
                .transpose()?;
            let bitrate_bps = lua_field::<Option<u32>>(table, ENTITY, "bitrate_bps")?;
            // 格式与 wire 边界同一套归一化:未知名保留原文落 Other,不报错。
            let format = lua_field::<Option<String>>(table, ENTITY, "format")?
                .map(mineral_model::AudioFormat::from);
            if new_url.is_none()
                && new_quality.is_none()
                && stream_headers.is_none()
                && layout.is_none()
                && bitrate_bps.is_none()
                && format.is_none()
            {
                return Err(mlua::Error::runtime(
                    "hook 返回 table 但无 url / quality / headers / layout / bitrate_bps / format / skip 字段",
                ));
            }
            Ok(HookDecision::Rewrite(RewriteSpec {
                new_url,
                new_quality,
                stream_headers,
                layout,
                bitrate_bps,
                format,
            }))
        }
        other => Err(mlua::Error::runtime(format!(
            "hook 返回值须是 nil / boolean / table,实得 {}",
            other.type_name()
        ))),
    }
}

/// 按音质名解析 [`mineral_model::BitRate`](与 `as_str` 对偶);未知名报错。
fn parse_bitrate(raw: &str) -> mlua::Result<mineral_model::BitRate> {
    mineral_model::BitRate::ALL
        .into_iter()
        .find(|q| q.as_str() == raw)
        .ok_or_else(|| {
            mlua::Error::runtime(format!(
                "未知音质名 `{raw}`(可选:standard/higher/exhigh/lossless/hires)"
            ))
        })
}

/// 按容器布局名解析 [`mineral_model::StreamLayout`](与 serde snake_case 对偶);未知名报错。
fn parse_layout(raw: &str) -> mlua::Result<mineral_model::StreamLayout> {
    match raw {
        "contiguous" => Ok(mineral_model::StreamLayout::Contiguous),
        "chunked" => Ok(mineral_model::StreamLayout::Chunked),
        other => Err(mlua::Error::runtime(format!(
            "未知 layout `{other}`(可选:contiguous / chunked)"
        ))),
    }
}
