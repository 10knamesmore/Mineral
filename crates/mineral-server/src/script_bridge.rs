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

    /// 配置底树落点:重载成功后把新合成树交给配置宿主(重算有效树 +
    /// 推送订阅 client)。
    pub(crate) apply_config_base: ApplyConfigBase,

    /// 埋点句柄:重载成功 / 失败记 script_lifecycle。
    pub(crate) stats: crate::StatsRecorder,
}

/// 配置底树落点(重载任务 → 配置宿主的间接层,同 [`PropsSnapshot`] 模式)。
pub(crate) type ApplyConfigBase = Arc<dyn Fn(serde_json::Value) + Send + Sync>;

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
        // player 随后被泵任务 move 走,先留一份埋点句柄给重载器。
        let stats = player.inner.stats.clone();
        let props_snapshot: PropsSnapshot = {
            let player = player.clone();
            Arc::new(move || player.props_snapshot())
        };
        let apply_config_base: ApplyConfigBase = {
            let player = player.clone();
            Arc::new(move |tree| player.set_config_base(tree))
        };
        // 拼错的 curate 源名在 config 层无从校验(config crate 不知运行期
        // channel 集),这里对无对应 channel 的键打 warn 兜诊断。
        {
            let player = player.clone();
            tokio::spawn(async move {
                let Some(sender) = player.script_sender() else {
                    return;
                };
                let Ok(keys) = sender.curate_source_keys().await else {
                    return;
                };
                for key in keys {
                    let source = mineral_model::SourceKind::from_name(&key);
                    if player.channel_for(source).is_none() {
                        mineral_log::warn!(
                            target: "script",
                            source = key,
                            "curate_playlists 配了未知源(无对应 channel),该函数不会生效"
                        );
                    }
                }
            });
        }
        let stats_bus = player.inner.stats.clone();
        tokio::spawn(async move {
            while let Some(event) = push_rx.recv().await {
                // 埋点:脚本事件总线消息(bus_messages;actor=Script)。只记名,载荷不入库。
                if let Event::BusMessage { name, .. } = &event {
                    stats_bus.event(mineral_stats::StatsEvent::Behavior {
                        actor: mineral_stats::Actor::Script,
                        event: mineral_stats::BehaviorEvent::BusMessage { name: name.clone() },
                    });
                }
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
            apply_config_base,
            stats,
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
    // 埋点页码 = offset / limit(Page 是 Copy,search_songs 后仍可读)。
    let page_no = i64::from(page.offset.checked_div(page.limit).unwrap_or(0));
    match source {
        Some(source) => {
            let Some(channel) = player.channel_for(source).cloned() else {
                let e = color_eyre::eyre::eyre!("no channel for source {}", source.name());
                resolve_err(player, query, &e);
                return;
            };
            match channel.search_songs(&term, page).await {
                Ok(hits) => {
                    record_script_search(player, &term, source, page_no, Ok(hits.items.len()));
                    resolve_ok(player, query, ResolveValue::Songs(hits.items));
                }
                Err(e) => {
                    record_script_search(player, &term, source, page_no, Err(()));
                    resolve_err(player, query, &color_eyre::eyre::eyre!("{e}"));
                }
            }
        }
        None => {
            let mut songs = Vec::new();
            for channel in player.channels() {
                match channel.search_songs(&term, page).await {
                    Ok(hits) => {
                        record_script_search(
                            player,
                            &term,
                            channel.source(),
                            page_no,
                            Ok(hits.items.len()),
                        );
                        songs.extend(hits.items);
                    }
                    Err(e) => {
                        record_script_search(player, &term, channel.source(), page_no, Err(()));
                        mineral_log::warn!(
                            target: "script",
                            source = channel.source().name(),
                            error = mineral_log::chain(&e),
                            "library.search: 该源搜索失败,跳过"
                        );
                    }
                }
            }
            resolve_ok(player, query, ResolveValue::Songs(songs));
        }
    }
}

/// 记一次脚本发起的歌曲搜索(searches;actor=script,kind=song——`mineral.search`
/// / `library.search` 只搜曲)。`result` 为 `Ok(条数)` / `Err(())`(失败无条数)。
fn record_script_search(
    player: &PlayerCore,
    term: &str,
    source: mineral_model::SourceKind,
    page: i64,
    result: Result<usize, ()>,
) {
    let (count, outcome) = match result {
        Ok(n) => (
            Some(i64::try_from(n).unwrap_or(i64::MAX)),
            mineral_stats::SearchOutcome::Ok,
        ),
        Err(()) => (None, mineral_stats::SearchOutcome::Failed),
    };
    player.inner.stats.record_search(
        mineral_stats::Actor::Script,
        term,
        mineral_stats::SearchTargetKind::Song,
        source,
        page,
        count,
        outcome,
    );
}

/// 记一次脚本 KV 写事件(actor=script)。
fn record_store_write(
    player: &PlayerCore,
    song: &mineral_model::SongId,
    key: &str,
    op: mineral_stats::StoreOp,
) {
    player
        .inner
        .stats
        .event(mineral_stats::StatsEvent::Behavior {
            actor: mineral_stats::Actor::Script,
            event: mineral_stats::BehaviorEvent::StoreWrite {
                song: song.clone(),
                key: key.to_owned(),
                op,
            },
        });
}

/// 记一次脚本子进程 spawn 收束(actor=script;spawns 表)。
fn record_spawn(
    player: &PlayerCore,
    program: String,
    outcome: mineral_stats::SpawnOutcome,
    exit_code: Option<i64>,
) {
    player
        .inner
        .stats
        .event(mineral_stats::StatsEvent::Behavior {
            actor: mineral_stats::Actor::Script,
            event: mineral_stats::BehaviorEvent::Spawn {
                program,
                outcome,
                exit_code,
            },
        });
}

/// 把一条脚本命令落到 player 执行面(与 client Request 同一些方法)。
fn apply_cmd(player: &PlayerCore, cmd: ScriptCmd, spawns: &SpawnTable) {
    // 传输类命令(暂停 / 跳转 / 音量 / 模式)一律走 PlayerCore 的 transport 方法,与
    // client Handler 同一执行 + 埋点出口——脚本操作以 actor=Script 入库,不再漏记。
    match cmd {
        ScriptCmd::Toggle => player.toggle_playback(mineral_stats::Actor::Script),
        ScriptCmd::Next => player.next_song(mineral_stats::Actor::Script),
        ScriptCmd::Prev => player.prev_or_restart(mineral_stats::Actor::Script),
        ScriptCmd::Stop => player.stop_playback(),
        ScriptCmd::SeekRel(secs) => {
            let delta_ms = (secs * 1000.0).round().to_i64().unwrap_or(0);
            let pos = i64::try_from(player.audio_snapshot().position_ms).unwrap_or(i64::MAX);
            let target = pos.saturating_add(delta_ms).max(0);
            player.seek_playback(
                u64::try_from(target).unwrap_or(0),
                mineral_stats::Actor::Script,
            );
        }
        ScriptCmd::SeekTo(secs) => {
            let target_ms = (secs * 1000.0).round().to_u64().unwrap_or(0);
            player.seek_playback(target_ms, mineral_stats::Actor::Script);
        }
        ScriptCmd::SetVolume(pct) => player.set_playback_volume(pct, mineral_stats::Actor::Script),
        ScriptCmd::SetMode(mode) => player.set_play_mode(mode, mineral_stats::Actor::Script),
        ScriptCmd::Play(id) => {
            let song = player.with_state(|st| st.queue.iter().find(|s| s.id == id).cloned());
            match song {
                Some(song) => {
                    // 直接改播:先结算被顶掉的在播曲(与 client PlaySong 同规矩)。
                    player.settle_interrupted();
                    player.play_song(
                        &song,
                        mineral_stats::PlayOrigin::Script,
                        mineral_stats::Actor::Script,
                    );
                }
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
            record_store_write(player, &song, &key, mineral_stats::StoreOp::Set);
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
            record_store_write(player, &song, &key, mineral_stats::StoreOp::Inc);
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
            // 读聚合快照,不再逐源真拉:与 client 严格同一份出口变换结果。
            // 初始完备前(daemon 启动早期)query 停靠,完备时刻由管线统一 resolve。
            if let Some(snapshot) = player.library_snapshot_or_park(query) {
                let briefs = snapshot
                    .iter()
                    .map(PlaylistBrief::from)
                    .collect::<Vec<PlaylistBrief>>();
                resolve_ok(player, query, ResolveValue::Playlists(briefs));
            }
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
                match channel.playlist_detail(&playlist).await {
                    Ok(pl) => resolve_ok(&player, query, ResolveValue::Songs(pl.songs)),
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
        ScriptCmd::LibrarySongUrl { song, query } => {
            let Some(channel) = player.channel_for(song.namespace()).cloned() else {
                let e =
                    color_eyre::eyre::eyre!("no channel for source {}", song.namespace().name());
                resolve_err(player, query, &e);
                return;
            };
            let player = player.clone();
            tokio::spawn(async move {
                let ids = [song.clone()];
                match channel.song_urls(&ids, player.playback_quality()).await {
                    Ok(mut urls) => match urls.pop() {
                        Some(play_url) => {
                            resolve_ok(&player, query, ResolveValue::PlayUrl(Box::new(play_url)));
                        }
                        None => {
                            let e = color_eyre::eyre::eyre!("{} 无可播 URL", song.qualified());
                            resolve_err(&player, query, &e);
                        }
                    },
                    Err(e) => {
                        resolve_err(&player, query, &color_eyre::eyre::eyre!("{e}"));
                    }
                }
            });
        }
        ScriptCmd::Spawn { id, spec, query } => {
            // spec 随即被 run_child 移走,先留程序名给埋点。
            let program = spec.program().to_owned();
            let over_limit = spawns.max != 0 && spawns.running.lock().len() >= spawns.max;
            if over_limit {
                // 埋点:并发超限即起进程失败(spawns;outcome=SpawnFailed,无退出码)。
                record_spawn(
                    player,
                    program,
                    mineral_stats::SpawnOutcome::SpawnFailed,
                    None,
                );
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
                    // 埋点:子进程收束(kill / 正常退出 / 起进程失败)。退出码仅正常退出有。
                    let outcome = match &result {
                        Ok(done) if done.killed => mineral_stats::SpawnOutcome::Killed,
                        Ok(_) => mineral_stats::SpawnOutcome::Exited,
                        Err(_) => mineral_stats::SpawnOutcome::SpawnFailed,
                    };
                    let exit_code = result.as_ref().ok().and_then(|d| d.code).map(i64::from);
                    record_spawn(&player, program, outcome, exit_code);
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
        ScriptCmd::ConfigOverride { path, value } => {
            player
                .inner
                .stats
                .event(mineral_stats::StatsEvent::Behavior {
                    actor: mineral_stats::Actor::Script,
                    event: mineral_stats::BehaviorEvent::ConfigOverride { path: path.clone() },
                });
            player.apply_config_override(path, value);
        }
        ScriptCmd::WindowTitle { text } => player.apply_window_title_override(text),
        ScriptCmd::SetLoved { song, loved } => {
            let player = player.clone();
            tokio::spawn(async move {
                // 走 server 统一路径:锁内写本地 persist(事实来源)+ 推 canonical(脚本路径无
                // client 乐观翻转,靠这条让装饰即时更新),锁外尽力镜像远端。
                if let Err(e) = player
                    .set_favorite(&song, loved, mineral_stats::Actor::Script)
                    .await
                {
                    mineral_log::warn!(
                        target: "script",
                        song_id = song.qualified(),
                        error = mineral_log::chain(&e),
                        "love 失败"
                    );
                }
            });
        }
    }
}
