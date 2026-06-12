//! daemon 侧脚本桥:装配件透传([`ScriptParts`])+ 双泵(脚本命令 →
//! player 执行面;脚本推送 → event hub)。
//!
//! 启动顺序(见 [`ScriptParts::spawn_runtime`]):脚本线程先于
//! [`Server`](crate::Server) 起(只需 VM + host),其投递句柄再喂给
//! Server 的事件出口 —— 无环。

use std::sync::Arc;

use mineral_protocol::{DownloadTarget, Event};
use mineral_script::mlua::Lua;
use mineral_script::{
    PlaylistBrief, PropKey, PropValue, QueryId, ResolveValue, ScriptCmd, ScriptHost, ScriptRuntime,
    ScriptSender, WatchdogConfig,
};
use num_traits::ToPrimitive;
use tokio::sync::broadcast;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::player::PlayerCore;

/// daemon 入口(main)装配、`serve` 层消费的脚本部件包。
///
/// `vm` 为 `None` 表示无用户脚本(文件缺失 / eval 失败已降级),此时只有
/// 泵在跑;热重载发现 config.lua 后仍可升级为有脚本。
pub struct ScriptParts {
    /// eval 过用户脚本的 VM(无脚本为 `None`)。
    vm: Option<Lua>,

    /// 宿主句柄(与 VM 内 API 闭包共享注册表)。
    host: ScriptHost,

    /// 脚本 → daemon 的命令出口发送端(热重载给新 host 复用同一通道)。
    cmd_tx: UnboundedSender<ScriptCmd>,

    /// 脚本 → daemon 的命令出口接收端。
    cmd_rx: UnboundedReceiver<ScriptCmd>,

    /// 脚本 → client 的推送出口发送端(热重载给新 host 复用同一通道)。
    push_tx: UnboundedSender<Event>,

    /// 脚本 → client 的推送出口接收端。
    push_rx: UnboundedReceiver<Event>,
}

impl ScriptParts {
    /// 打包装配件(由 daemon 入口在 `load_with_vm` 后构造)。
    ///
    /// # Params:
    ///   - `vm`: `load_with_vm` 交还的 VM
    ///   - `host`: 与 `install_api` 同一个宿主句柄
    ///   - `cmd_tx`: 命令通道发送端(与 `host` 内同源,热重载复用)
    ///   - `cmd_rx`: 命令通道接收端
    ///   - `push_tx`: 推送通道发送端(同上)
    ///   - `push_rx`: 推送通道接收端
    #[must_use]
    pub fn new(
        vm: Option<Lua>,
        host: ScriptHost,
        cmd_tx: UnboundedSender<ScriptCmd>,
        cmd_rx: UnboundedReceiver<ScriptCmd>,
        push_tx: UnboundedSender<Event>,
        push_rx: UnboundedReceiver<Event>,
    ) -> Self {
        Self {
            vm,
            host,
            cmd_tx,
            cmd_rx,
            push_tx,
            push_rx,
        }
    }

    /// 起脚本线程(若有 VM),消息入口挂进 `sender`。须在
    /// [`Server::spawn`](crate::Server::spawn) **之前**调用;spawn 失败降级无脚本(warn)。
    ///
    /// # Params:
    ///   - `watchdog`: 回调看门狗参数(配置 `script` 段派生)
    ///   - `sender`: daemon 侧投递句柄(装配期 `ScriptSender::detached()` 先建)
    ///   - `channels`: 已注册音乐源(收集各源网页链接模板,seed 进 VM 供
    ///     Song/Playlist 投影拼 `url`;热重载的新 VM 复用同一份)
    ///
    /// # Return:
    ///   `(runtime, rest)`:runtime 为 `None` 表示无脚本;rest 是泵 + 热重载接线件。
    #[must_use]
    pub fn spawn_runtime(
        self,
        watchdog: WatchdogConfig,
        sender: &ScriptSender,
        channels: &[Arc<dyn mineral_channel_core::MusicChannel>],
    ) -> (Option<ScriptRuntime>, ScriptPumps) {
        let web_urls = channels
            .iter()
            .map(|ch| {
                let caps = ch.caps();
                (
                    ch.source().name().to_owned(),
                    caps.song_web_url().clone(),
                    caps.playlist_web_url().clone(),
                )
            })
            .collect::<Vec<(String, Option<String>, Option<String>)>>();
        let runtime = self.vm.and_then(|lua| {
            seed_web_urls(&lua, &web_urls);
            match ScriptRuntime::spawn(lua, self.host.clone(), watchdog, sender) {
                Ok(runtime) => Some(runtime),
                Err(e) => {
                    mineral_log::warn!(
                        target: "script",
                        error = mineral_log::chain(&e),
                        "脚本线程启动失败,降级无脚本"
                    );
                    None
                }
            }
        });
        (
            runtime,
            ScriptPumps {
                cmd_rx: self.cmd_rx,
                push_rx: self.push_rx,
                cmd_tx: self.cmd_tx,
                push_tx: self.push_tx,
                watchdog,
                web_urls,
            },
        )
    }
}

/// 把各源网页链接模板 seed 进 VM;失败只降级(`url` 为 nil),不阻断脚本启动。
pub(crate) fn seed_web_urls(lua: &Lua, web_urls: &[(String, Option<String>, Option<String>)]) {
    if let Err(e) = mineral_script::seed_web_url_templates(lua, web_urls.iter().cloned()) {
        mineral_log::warn!(
            target: "script",
            error = mineral_log::chain(&e),
            "网页链接模板 seed 失败,实体 url 字段降级为 nil"
        );
    }
}

/// [`ScriptParts::spawn_runtime`] 拆出的泵接线件,等 Server 就绪后接上。
pub struct ScriptPumps {
    /// 命令通道接收端。
    cmd_rx: UnboundedReceiver<ScriptCmd>,

    /// 推送通道接收端。
    push_rx: UnboundedReceiver<Event>,

    /// 命令通道发送端(透传给热重载件,新 host 复用同一通道,泵不动)。
    cmd_tx: UnboundedSender<ScriptCmd>,

    /// 推送通道发送端(同上;重载结果 toast 也走它)。
    push_tx: UnboundedSender<Event>,

    /// 看门狗参数(热重载起新线程复用)。
    watchdog: WatchdogConfig,

    /// 各源网页链接模板(热重载的新 VM 重新 seed 用;caps 启动后不变)。
    web_urls: Vec<(String, Option<String>, Option<String>)>,
}

/// 属性值快照源:重载起新 VM 前取 daemon 当前属性,播种其缓存
/// (经 [`ScriptHost::seed_props`];daemon 只下发 diff,不播种则新 VM
/// 的 observe 回放 / 顶层 get 要等属性下次真变更)。
pub(crate) type PropsSnapshot = Arc<dyn Fn() -> Vec<(PropKey, PropValue)> + Send + Sync>;

/// [`ScriptPumps::start`] 拆出的热重载接线件(交给
/// [`crate::script_reload::spawn_script_reloader`])。
pub struct ScriptReloadParts {
    /// 命令通道发送端(重载的新 host 用)。
    pub(crate) cmd_tx: UnboundedSender<ScriptCmd>,

    /// 推送通道发送端(重载的新 host 用 + 结果 toast 出口)。
    pub(crate) push_tx: UnboundedSender<Event>,

    /// 看门狗参数(重载起新线程用)。
    pub(crate) watchdog: WatchdogConfig,

    /// 属性值快照源(重载播种新 VM 的属性缓存)。
    pub(crate) props_snapshot: PropsSnapshot,

    /// 各源网页链接模板(重载的新 VM 重新 seed 用)。
    pub(crate) web_urls: Vec<(String, Option<String>, Option<String>)>,
}

impl ScriptPumps {
    /// 接上两条泵:脚本命令 → player 执行面;脚本推送 → event hub。
    ///
    /// # Params:
    ///   - `player`: 命令执行面
    ///   - `sink`: event hub 发送端
    ///
    /// # Return:
    ///   热重载接线件(daemon 入口交给 reloader)。
    pub(crate) fn start(
        self,
        player: PlayerCore,
        sink: broadcast::Sender<Event>,
    ) -> ScriptReloadParts {
        let Self {
            mut cmd_rx,
            mut push_rx,
            cmd_tx,
            push_tx,
            watchdog,
            web_urls,
        } = self;
        let props_snapshot: PropsSnapshot = {
            let player = player.clone();
            Arc::new(move || player.props_snapshot())
        };
        tokio::spawn(async move {
            while let Some(event) = push_rx.recv().await {
                // 无订阅者 send 失败即丢(advisory)。
                let _ = sink.send(event);
            }
        });
        tokio::spawn(async move {
            // 在跑子进程表归泵任务所有,随泵同生命周期。
            let spawns = SpawnTable::new(player.spawn_max_concurrent());
            while let Some(cmd) = cmd_rx.recv().await {
                apply_cmd(&player, cmd, &spawns);
            }
        });
        ScriptReloadParts {
            cmd_tx,
            push_tx,
            watchdog,
            props_snapshot,
            web_urls,
        }
    }
}

/// `mineral.spawn` 的在跑子进程表:id → kill 信号发送端。
///
/// 完成 / 被杀即移除;并发闸按表长判断(`max == 0` 不限)。
struct SpawnTable {
    /// 在跑子进程。
    running: Arc<
        parking_lot::Mutex<
            rustc_hash::FxHashMap<mineral_script::SpawnId, tokio::sync::oneshot::Sender<()>>,
        >,
    >,

    /// 并发上限(配置 `script.spawn_max_concurrent`)。
    max: usize,
}

impl SpawnTable {
    /// 建空表。
    ///
    /// # Params:
    ///   - `max`: 并发上限(0 = 不限)
    fn new(max: usize) -> Self {
        Self {
            running: Arc::new(parking_lot::Mutex::new(rustc_hash::FxHashMap::default())),
            max,
        }
    }
}

/// 一次查询失败的统一收口:回投 `Error`(脚本回调收 `(nil, err)`)。
fn resolve_err(player: &PlayerCore, query: QueryId, e: &color_eyre::Report) {
    if let Some(sender) = player.script_sender() {
        sender.resolve(query, ResolveValue::Error(mineral_log::chain(e)));
    }
}

/// 把一条查询结果回投脚本线程(无脚本时静默丢——查询本就发不出来)。
fn resolve_ok(player: &PlayerCore, query: QueryId, value: ResolveValue) {
    if let Some(sender) = player.script_sender() {
        sender.resolve(query, value);
    }
}

/// 跑一次 `library.search` 并回投结果。
///
/// 限定源:查不到对应 channel / 该源失败都回 `(nil, err)`。
/// 不限定:跨全部源聚合,单源失败 warn 跳过(与 `library.playlists` 同语义)。
///
/// # Params:
///   - `term`: 关键词
///   - `source`: 限定源(`None` = 全源聚合)
///   - `page`: 分页
///   - `query`: 回投句柄
async fn resolve_search(
    player: &PlayerCore,
    term: String,
    source: Option<mineral_model::SourceKind>,
    page: mineral_channel_core::Page,
    query: QueryId,
) {
    match source {
        Some(source) => {
            let Some(channel) = player.channel_for(source).cloned() else {
                let e = color_eyre::eyre::eyre!("no channel for source {}", source.name());
                resolve_err(player, query, &e);
                return;
            };
            match channel.search_songs(&term, page).await {
                Ok(songs) => resolve_ok(player, query, ResolveValue::Songs(songs)),
                Err(e) => resolve_err(player, query, &color_eyre::eyre::eyre!("{e}")),
            }
        }
        None => {
            let mut songs = Vec::new();
            for channel in player.channels() {
                match channel.search_songs(&term, page).await {
                    Ok(hits) => songs.extend(hits),
                    Err(e) => mineral_log::warn!(
                        target: "script",
                        source = channel.source().name(),
                        error = %e,
                        "library.search: 该源搜索失败,跳过"
                    ),
                }
            }
            resolve_ok(player, query, ResolveValue::Songs(songs));
        }
    }
}

/// 把一条脚本命令落到 player 执行面(与 client Request 同一些方法)。
fn apply_cmd(player: &PlayerCore, cmd: ScriptCmd, spawns: &SpawnTable) {
    match cmd {
        ScriptCmd::Toggle => {
            if player.audio_snapshot().playing {
                player.audio().pause();
            } else {
                player.audio().resume();
            }
        }
        ScriptCmd::Next => player.next_song(),
        ScriptCmd::Prev => player.prev_or_restart(),
        ScriptCmd::Stop => player.stop_playback(),
        ScriptCmd::SeekRel(secs) => {
            let delta_ms = (secs * 1000.0).round().to_i64().unwrap_or(0);
            let pos = i64::try_from(player.audio_snapshot().position_ms).unwrap_or(i64::MAX);
            let target = pos.saturating_add(delta_ms).max(0);
            player.audio().seek(u64::try_from(target).unwrap_or(0));
        }
        ScriptCmd::SeekTo(secs) => {
            let target_ms = (secs * 1000.0).round().to_u64().unwrap_or(0);
            player.audio().seek(target_ms);
        }
        ScriptCmd::SetVolume(pct) => player.audio().set_volume(pct),
        ScriptCmd::SetMode(mode) => player.set_play_mode(mode),
        ScriptCmd::Play(id) => {
            let song = player.with_state(|st| st.queue.iter().find(|s| s.id == id).cloned());
            match song {
                Some(song) => player.play_song(&song),
                // 队列外跳播是后续能力;当前 warn 丢弃,不拉详情。
                None => mineral_log::warn!(
                    target: "script",
                    song_id = id.qualified(),
                    "play: 不在当前队列,忽略"
                ),
            }
        }
        ScriptCmd::Download(id) => {
            let song = player.with_state(|st| {
                st.queue
                    .iter()
                    .find(|s| s.id == id)
                    .or(st.current_song.as_ref())
                    .filter(|s| s.id == id)
                    .cloned()
            });
            match song {
                Some(song) => player.download(DownloadTarget::Song(Box::new(song))),
                None => mineral_log::warn!(
                    target: "script",
                    song_id = id.qualified(),
                    "download: 不在当前队列,忽略"
                ),
            }
        }
        ScriptCmd::QueueList { query } => {
            // 内存快照,同步取;经 resolve 回投保持与其他查询一致的回调路径。
            let songs = player.with_state(|st| st.queue.clone());
            resolve_ok(player, query, ResolveValue::Songs(songs));
        }
        ScriptCmd::StoreGet { song, key, query } => {
            let player = player.clone();
            tokio::spawn(async move {
                let scope = player.persist().scope(song.namespace());
                match scope.kv_get(&song, &key).await {
                    Ok(value) => resolve_ok(&player, query, ResolveValue::Store(value)),
                    Err(e) => resolve_err(&player, query, &e),
                }
            });
        }
        ScriptCmd::StoreSet { song, key, value } => {
            let player = player.clone();
            tokio::spawn(async move {
                let scope = player.persist().scope(song.namespace());
                match scope.kv_set(&song, &key, &value).await {
                    Ok(()) => player.notify().store_changed(&song, &key),
                    Err(e) => mineral_log::warn!(
                        target: "script",
                        song_id = song.qualified(),
                        key,
                        error = mineral_log::chain(&e),
                        "store.set 失败"
                    ),
                }
            });
        }
        ScriptCmd::StoreInc {
            song,
            key,
            delta,
            query,
        } => {
            let player = player.clone();
            tokio::spawn(async move {
                let scope = player.persist().scope(song.namespace());
                match scope.kv_inc(&song, &key, delta).await {
                    Ok(value) => {
                        player.notify().store_changed(&song, &key);
                        if let Some(query) = query {
                            resolve_ok(&player, query, ResolveValue::Store(value));
                        }
                    }
                    Err(e) => match query {
                        Some(query) => resolve_err(&player, query, &e),
                        None => mineral_log::warn!(
                            target: "script",
                            song_id = song.qualified(),
                            key,
                            error = mineral_log::chain(&e),
                            "store.inc 失败"
                        ),
                    },
                }
            });
        }
        ScriptCmd::LibraryPlaylists { query } => {
            // channel 调用可能打网络,spawn 不卡泵(后续 player 命令照常)。
            let player = player.clone();
            tokio::spawn(async move {
                let mut briefs = Vec::new();
                for channel in player.channels() {
                    match channel.my_playlists().await {
                        Ok(playlists) => {
                            briefs.extend(playlists.into_iter().map(|p| PlaylistBrief {
                                id: p.id,
                                name: p.name,
                                track_count: p.track_count,
                            }))
                        }
                        Err(e) => mineral_log::warn!(
                            target: "script",
                            source = channel.source().name(),
                            error = %e,
                            "library.playlists: 该源拉取失败,跳过"
                        ),
                    }
                }
                resolve_ok(&player, query, ResolveValue::Playlists(briefs));
            });
        }
        ScriptCmd::LibraryTracks { playlist, query } => {
            let player = player.clone();
            tokio::spawn(async move {
                let Some(channel) = player.channel_for(playlist.namespace()).cloned() else {
                    let e = color_eyre::eyre::eyre!(
                        "no channel for source {}",
                        playlist.namespace().name()
                    );
                    resolve_err(&player, query, &e);
                    return;
                };
                match channel.songs_in_playlist(&playlist).await {
                    Ok(songs) => resolve_ok(&player, query, ResolveValue::Songs(songs)),
                    Err(e) => {
                        resolve_err(&player, query, &color_eyre::eyre::eyre!("{e}"));
                    }
                }
            });
        }
        ScriptCmd::LibrarySearch {
            term,
            source,
            offset,
            limit,
            query,
        } => {
            let player = player.clone();
            tokio::spawn(async move {
                let page = mineral_channel_core::Page::new(offset, limit);
                resolve_search(&player, term, source, page, query).await;
            });
        }
        ScriptCmd::Spawn { id, spec, query } => {
            let over_limit = spawns.max != 0 && spawns.running.lock().len() >= spawns.max;
            if over_limit {
                let e = color_eyre::eyre::eyre!(
                    "spawn 并发超限(script.spawn_max_concurrent = {})",
                    spawns.max
                );
                resolve_err(player, query, &e);
            } else {
                let (kill_tx, kill_rx) = tokio::sync::oneshot::channel();
                spawns.running.lock().insert(id, kill_tx);
                let player = player.clone();
                let running = Arc::clone(&spawns.running);
                tokio::spawn(async move {
                    let result = mineral_script::run_child(spec, kill_rx).await;
                    running.lock().remove(&id);
                    match result {
                        Ok(done) => resolve_ok(&player, query, ResolveValue::Spawn(done)),
                        Err(e) => resolve_err(&player, query, &e),
                    }
                });
            }
        }
        ScriptCmd::SpawnKill { id } => {
            // 已退出 / 未知 id:发送端缺席,no-op。
            if let Some(kill) = spawns.running.lock().remove(&id) {
                let _ = kill.send(());
            }
        }
        ScriptCmd::UiOverride { key, value } => player.apply_ui_override(key, value),
        ScriptCmd::SetLoved { song, loved } => {
            let player = player.clone();
            tokio::spawn(async move {
                let Some(channel) = player.channel_for(song.namespace()).cloned() else {
                    mineral_log::warn!(
                        target: "script",
                        song_id = song.qualified(),
                        "love: 无对应 channel,忽略"
                    );
                    return;
                };
                if let Err(e) = channel.set_loved(&song, loved).await {
                    mineral_log::warn!(
                        target: "script",
                        song_id = song.qualified(),
                        error = %e,
                        "love 失败"
                    );
                }
            });
        }
    }
}
