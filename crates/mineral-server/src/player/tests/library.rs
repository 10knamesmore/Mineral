//! playlist 写收敛 · library 快照 curate · toggle_favorite 聚合。

use super::*;
use pretty_assertions::assert_eq;

/// 只支持建单/列单的写桩 channel(写收敛链路测试用)。
struct WritableChannel;

#[async_trait]
impl MusicChannel for WritableChannel {
    fn source(&self) -> SourceKind {
        SourceKind::NETEASE
    }

    fn caps(&self) -> ChannelCaps {
        ChannelCaps::builder()
            .searchable(Vec::new())
            .playlist_edit(true)
            .artist_sections(mineral_channel_core::ArtistSections::new(vec![
                mineral_channel_core::ArtistSectionKind::TopSongs,
                mineral_channel_core::ArtistSectionKind::Albums,
            ]))
            .build()
    }

    async fn search_songs(&self, _q: &str, _p: Page) -> ChannelResult<SearchHits<Song>> {
        Err(Error::NotSupported)
    }
    async fn search_albums(&self, _q: &str, _p: Page) -> ChannelResult<SearchHits<Album>> {
        Err(Error::NotSupported)
    }
    async fn search_playlists(&self, _q: &str, _p: Page) -> ChannelResult<SearchHits<Playlist>> {
        Err(Error::NotSupported)
    }
    async fn songs_detail(&self, _ids: &[SongId]) -> ChannelResult<Vec<Song>> {
        Err(Error::NotSupported)
    }
    async fn album_detail(&self, _id: &AlbumId) -> ChannelResult<Album> {
        Err(Error::NotSupported)
    }
    async fn playlist_detail(&self, _id: &PlaylistId) -> ChannelResult<Playlist> {
        Err(Error::NotSupported)
    }
    async fn song_urls(&self, _ids: &[SongId], _q: BitRate) -> ChannelResult<Vec<PlayUrl>> {
        Err(Error::NotSupported)
    }
    async fn lyrics(&self, _id: &SongId) -> ChannelResult<Lyrics> {
        Err(Error::NotSupported)
    }

    async fn create_playlist(&self, name: &str) -> ChannelResult<Playlist> {
        Ok(Playlist::builder()
            .id(PlaylistId::new(SourceKind::NETEASE, "created-1"))
            .name(name.to_owned())
            .build())
    }

    async fn my_playlists(&self) -> ChannelResult<Vec<Playlist>> {
        Ok(vec![
            Playlist::builder()
                .id(PlaylistId::new(SourceKind::NETEASE, "created-1"))
                .name(String::from("新歌单"))
                .build(),
        ])
    }
}

/// 写成功 → PlaylistWriteDone 转发给 client,且自动触发 MyPlaylists 重拉
/// (缓存收敛走读管线,不直接改数据)。
#[tokio::test]
async fn playlist_write_done_forwards_and_triggers_refetch() -> color_eyre::Result<()> {
    let ch: Arc<dyn MusicChannel> = Arc::new(WritableChannel);
    let core = core_with_channels(
        vec![ch],
        ServerStore::disabled(),
        /*music_dir*/ None,
        MediaCache::disabled(),
    )?;
    let h = core.inner.scheduler.submit(
        mineral_task::TaskKind::PlaylistWrite(mineral_task::PlaylistWriteOp::Create {
            source: SourceKind::NETEASE,
            name: String::from("新歌单"),
        }),
        mineral_task::Priority::User,
    );
    assert_eq!(h.done().await, mineral_task::TaskOutcome::Ok);
    core.consume_events_once();
    let evs = core.drain_client_events();
    assert!(
        evs.iter().any(|e| matches!(
            e,
            mineral_task::TaskEvent::PlaylistWriteDone { error: None, .. }
        )),
        "写完结事件应转发给 client,got {evs:?}"
    );

    // 收敛重拉(MyPlaylists)由 consume 时提交,异步执行;逐源列表进聚合态,
    // client 收到的是出口变换后的合并快照。轮询等它落地。
    let mut found = false;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(5)).await;
        core.consume_events_once();
        if core.drain_client_events().iter().any(|e| {
            matches!(
                e,
                mineral_task::TaskEvent::LibrarySnapshot { playlists }
                    if playlists.iter().any(|p| p.id.value() == "created-1")
            )
        }) {
            found = true;
            break;
        }
    }
    assert!(found, "写成功后应触发 MyPlaylists 重拉并推合并快照");
    Ok(())
}

/// 极简歌单(netease 源)。
fn named_playlist(id: &str, name: &str) -> Playlist {
    Playlist::builder()
        .id(PlaylistId::new(SourceKind::NETEASE, id))
        .name(name.to_owned())
        .build()
}

/// 轮询 consume + drain 直到收到一条 `LibrarySnapshot`,返回其载荷。
async fn wait_snapshot(core: &PlayerCore) -> color_eyre::Result<Vec<Playlist>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        core.consume_events_once();
        for ev in core.drain_client_events() {
            if let mineral_task::TaskEvent::LibrarySnapshot { playlists } = ev {
                return Ok(playlists);
            }
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    color_eyre::eyre::bail!("超时未收到 LibrarySnapshot")
}

/// 组装带 curate registry 函数的 core(模拟 config 管线摘取结果):
/// per-source 函数按源名入表、跨源函数入独立键,channel 返回给定歌单。
fn core_with_curate(
    per_source: &[(&str, &str)],
    merged: Option<&str>,
    playlists: Vec<Playlist>,
) -> color_eyre::Result<(PlayerCore, mineral_script::ScriptRuntime)> {
    use mineral_script::{ScriptHost, ScriptRuntime, ScriptSender, install_api};
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (push_tx, _push_rx) = tokio::sync::mpsc::unbounded_channel();
    let host = ScriptHost::new(cmd_tx, push_tx);
    let lua = mineral_script::mlua::Lua::new();
    install_api(&lua, &host)?;
    let fns = lua.create_table()?;
    for (source, src) in per_source {
        fns.set(
            *source,
            lua.load(*src).eval::<mineral_script::mlua::Function>()?,
        )?;
    }
    lua.set_named_registry_value(mineral_config::CURATE_PLAYLISTS_SOURCE_FNS, fns)?;
    if let Some(src) = merged {
        lua.set_named_registry_value(
            mineral_config::CURATE_PLAYLISTS_MERGED_FN,
            lua.load(src).eval::<mineral_script::mlua::Function>()?,
        )?;
    }
    let sender = ScriptSender::detached();
    let watchdog = mineral_script::WatchdogConfig::builder()
        .instruction_interval(10_000)
        .soft_wall(Duration::from_millis(200))
        .hard_wall(Duration::from_secs(1))
        .build();
    let runtime = ScriptRuntime::spawn(lua, host, watchdog, &sender)?;
    let core = core_with_events(
        vec![Arc::new(RecordingChannel {
            calls: Arc::default(),
            url_delay: None,
            liked_ids: None,
            playlists: Some(playlists),
        })],
        ServerStore::disabled(),
        /*music_dir*/ None,
        MediaCache::disabled(),
        tokio::sync::broadcast::channel(/*capacity*/ 8).0,
        Some(sender),
    )?;
    Ok((core, runtime))
}

/// 无脚本:各源列表进聚合态,client 收 identity 透传的合并快照。
#[tokio::test(flavor = "multi_thread")]
async fn library_snapshot_identity_without_script() -> color_eyre::Result<()> {
    let core = core_with_channels(
        vec![Arc::new(RecordingChannel {
            calls: Arc::default(),
            url_delay: None,
            liked_ids: None,
            playlists: Some(vec![
                named_playlist("p1", "日常"),
                named_playlist("p2", "稍后再看"),
            ]),
        })],
        ServerStore::disabled(),
        /*music_dir*/ None,
        MediaCache::disabled(),
    )?;
    core.submit_my_playlists(SourceKind::NETEASE);
    let snapshot = wait_snapshot(&core).await?;
    let names = snapshot
        .iter()
        .map(|p| p.name.as_str())
        .collect::<Vec<&str>>();
    assert_eq!(names, vec!["日常", "稍后再看"], "无 transform 原样透传");
    Ok(())
}

/// curate 全链:per-source 过滤 + 跨源改名都落到快照。
#[tokio::test(flavor = "multi_thread")]
async fn library_snapshot_applies_curate_functions() -> color_eyre::Result<()> {
    let (core, _runtime) = core_with_curate(
        &[(
            "netease",
            r#"function(lists)
                    local keep = {}
                    for _, p in ipairs(lists) do
                        if p.name ~= "稍后再看" then keep[#keep + 1] = p end
                    end
                    return keep
                end"#,
        )],
        Some(
            r#"function(all)
                    for _, p in ipairs(all) do p.name = "[" .. p.name .. "]" end
                    return all
                end"#,
        ),
        vec![
            named_playlist("p1", "日常"),
            named_playlist("p2", "稍后再看"),
        ],
    )?;
    core.submit_my_playlists(SourceKind::NETEASE);
    let snapshot = wait_snapshot(&core).await?;
    let names = snapshot
        .iter()
        .map(|p| p.name.as_str())
        .collect::<Vec<&str>>();
    assert_eq!(
        names,
        vec!["[日常]"],
        "per-source 滤掉稍后再看,跨源函数改名"
    );
    Ok(())
}

/// 拉取失败(NotSupported)也给结论:空贡献快照照常推,初始完备不卡死。
#[tokio::test(flavor = "multi_thread")]
async fn library_failure_concludes_with_empty_snapshot() -> color_eyre::Result<()> {
    // RecordingChannel playlists: None → my_playlists NotSupported → 任务 Failed。
    let core = core_with(Arc::default())?;
    core.submit_my_playlists(SourceKind::NETEASE);
    let snapshot = wait_snapshot(&core).await?;
    assert!(snapshot.is_empty(), "失败源空贡献,快照为空但必须到达");
    Ok(())
}

/// 脚本 `library.playlists` 在初始完备前停靠,完备时刻统一 resolve
/// (config.lua 顶层调用是常态场景;快照与 client 同为出口变换结果)。
#[tokio::test(flavor = "multi_thread")]
async fn library_playlists_query_parks_until_complete() -> color_eyre::Result<()> {
    use mineral_script::{ScriptHost, ScriptSender, install_api};
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (push_tx, push_rx) = tokio::sync::mpsc::unbounded_channel();
    let host = ScriptHost::new(cmd_tx.clone(), push_tx.clone());
    let lua = mineral_script::mlua::Lua::new();
    install_api(&lua, &host)?;
    // 顶层调用:此刻聚合态必然未完备 → daemon 侧停靠。
    lua.load(
        r#"
            mineral.library.playlists(function(ps, err)
                mineral.ui.toast("got:" .. #ps .. ":" .. ps[1].name)
            end)
            "#,
    )
    .exec()?;
    let parts =
        crate::script_bridge::ScriptParts::new(Some(lua), host, cmd_tx, cmd_rx, push_tx, push_rx);
    let sender = ScriptSender::detached();
    let watchdog = mineral_script::WatchdogConfig::builder()
        .instruction_interval(10_000)
        .soft_wall(Duration::from_millis(200))
        .hard_wall(Duration::from_secs(1))
        .build();
    let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
        calls: Arc::default(),
        url_delay: None,
        liked_ids: None,
        playlists: Some(vec![named_playlist("p1", "日常")]),
    })];
    let (runtime, pumps) = parts.spawn_runtime(watchdog, &sender, &channels);
    let _runtime = runtime.ok_or_else(|| color_eyre::eyre::eyre!("应有脚本线程"))?;
    let (hub_tx, mut hub_rx) = tokio::sync::broadcast::channel(/*capacity*/ 8);
    let core = core_with_events(
        channels,
        ServerStore::disabled(),
        /*music_dir*/ None,
        MediaCache::disabled(),
        hub_tx.clone(),
        Some(sender),
    )?;
    let _reload = pumps.start(core.clone(), hub_tx);
    core.submit_my_playlists(SourceKind::NETEASE);
    // 测试 core 不跑 background loop,手动 tick 驱动事件消化。
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        core.consume_events_once();
        match hub_rx.try_recv() {
            Ok(mineral_protocol::Event::Toast { content, .. }) => {
                let text = content.iter().map(|s| s.text.as_str()).collect::<String>();
                assert_eq!(text, "got:1:日常", "停靠 query 在完备时刻收到快照");
                return Ok(());
            }
            Ok(_other) => {}
            Err(_empty) => {
                if std::time::Instant::now() > deadline {
                    color_eyre::eyre::bail!("超时未收到脚本回调 toast");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
}

/// toggle_favorite 以本地 persist 为事实来源:即使该源 channel 的远端镜像
/// (`set_loved`)返回 NotSupported(如 bilibili / 未登录),本地也必写。
/// 回归:曾把写绕道 channel.set_loved,NotSupported 时本地一个字没写 → 按 f 假反馈。
#[tokio::test]
async fn toggle_favorite_persists_locally_even_when_remote_unsupported() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let persist = ServerStore::open(&dir.path().join("t.db")).await?;
    // RecordingChannel(源 NETEASE)的 set_loved 用 trait 默认 → NotSupported。
    let core = core_with_persist(Arc::default(), persist.clone())?;
    let track = song("42");
    let id = track.id.clone();
    let scope = persist.scope(SourceKind::NETEASE);
    assert!(!scope.is_loved(&id).await?, "初始未收藏");

    let new = core
        .toggle_favorite(&track, mineral_stats::Actor::User)
        .await?;
    assert!(new, "toggle 返回新态 true");
    assert!(
        scope.is_loved(&id).await?,
        "远端镜像 NotSupported,本地 persist 仍必写 loved"
    );
    // 携带整首 Song 的意义:love 落地时 meta 顺手入库,聚合视图离线可重建。
    assert!(
        scope.get_meta(&id).await?.is_some(),
        "toggle 应顺手 upsert_meta"
    );
    // toggle 后推 canonical(供装饰自愈,不只靠 client 乐观翻转)。
    let events = core.drain_client_events();
    let pushed = events
        .iter()
        .rev()
        .find_map(|e| match e {
            mineral_task::TaskEvent::LikedSongIdsFetched { source, ids }
                if *source == SourceKind::NETEASE =>
            {
                Some(ids)
            }
            _ => None,
        })
        .ok_or_else(|| color_eyre::eyre::eyre!("toggle 后应推 canonical favorited 集"))?;
    assert!(pushed.contains(&id), "推的 canonical 应含刚收藏的歌");

    let new2 = core
        .toggle_favorite(&track, mineral_stats::Actor::User)
        .await?;
    assert!(!new2, "再 toggle 回 false");
    assert!(!scope.is_loved(&id).await?, "本地 persist 已取消");
    Ok(())
}

/// toggle 后聚合收藏歌单(mineral 源)被重合成并重推:曲目集合走 `PlaylistDetailFetched`
/// (正在看聚合歌单的 client 实时增删),sidebar 计数走 client **认得**的 `LibrarySnapshot`
/// (经 library_concluded 出口管线,而非被 client no-op 丢弃的 `PlaylistsFetched`)。
#[tokio::test(flavor = "multi_thread")]
async fn toggle_favorite_repushes_aggregate_playlist() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let persist = ServerStore::open(&dir.path().join("t.db")).await?;
    let channels: Vec<Arc<dyn MusicChannel>> = vec![
        Arc::new(RecordingChannel {
            calls: Arc::default(),
            url_delay: None,
            liked_ids: None,
            playlists: None,
        }),
        Arc::new(mineral_channel_mineral::MineralChannel::new(
            persist.clone(),
        )),
    ];
    let core = core_with_channels(
        channels,
        persist,
        /*music_dir*/ None,
        MediaCache::disabled(),
    )?;
    let track = song("agg1");
    core.toggle_favorite(&track, mineral_stats::Actor::User)
        .await?;

    // detail 同步推;LibrarySnapshot 经 library_concluded 出口管线异步落地——轮询累积到它为止
    // (不能只 drain 一次:snapshot 可能尚未产出,或与首次 drain 竞争被吞)。
    let mut events = core.drain_client_events();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !events
        .iter()
        .any(|e| matches!(e, mineral_task::TaskEvent::LibrarySnapshot { .. }))
    {
        if std::time::Instant::now() >= deadline {
            color_eyre::eyre::bail!("超时未收到聚合 LibrarySnapshot");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
        core.consume_events_once();
        events.extend(core.drain_client_events());
    }

    let detail = events
        .iter()
        .rev()
        .find_map(|e| match e {
            mineral_task::TaskEvent::PlaylistDetailFetched { id, playlist }
                if id.namespace() == SourceKind::MINERAL =>
            {
                Some(playlist)
            }
            _ => None,
        })
        .ok_or_else(|| color_eyre::eyre::eyre!("toggle 后应重推聚合歌单 detail"))?;
    assert_eq!(detail.songs.len(), 1);
    assert_eq!(
        detail.songs.first().map(|s| &s.id),
        Some(&track.id),
        "聚合曲目保留原源 id"
    );

    // F1 回归:sidebar 计数经 client 认得的 LibrarySnapshot 更新,快照里 mineral 聚合歌单的
    // track_count 跟随收藏数。直推 PlaylistsFetched 会被 client apply 丢弃,故这里改断言这条。
    let snapshot = events
        .iter()
        .rev()
        .find_map(|e| match e {
            mineral_task::TaskEvent::LibrarySnapshot { playlists } => Some(playlists),
            _ => None,
        })
        .ok_or_else(|| color_eyre::eyre::eyre!("toggle 后应产出 LibrarySnapshot"))?;
    let fav = snapshot
        .iter()
        .find(|p| p.id.namespace() == SourceKind::MINERAL)
        .ok_or_else(|| color_eyre::eyre::eyre!("快照应含 mineral 聚合歌单"))?;
    assert_eq!(fav.track_count, 1, "sidebar 计数跟随收藏数");
    Ok(())
}

/// set_favorite(script/CLI love,只握 id)收藏一首 persist 里没 meta 的歌:触发后台补 meta
/// 任务向其源 channel 拉 detail 回填,聚合视图才能离线重建它(sync / script 共用这条后台任务)。
#[tokio::test(flavor = "multi_thread")]
async fn set_favorite_backfills_missing_meta() -> color_eyre::Result<()> {
    use mineral_test::mock::DetailChannel;
    use mineral_test::with_name;

    let dir = tempfile::tempdir()?;
    let persist = ServerStore::open(&dir.path().join("t.db")).await?;
    let id = SongId::new(SourceKind::NETEASE, "lua1");
    let full = with_name(song("lua1"), "From Detail");
    let channels: Vec<Arc<dyn MusicChannel>> = vec![
        Arc::new(DetailChannel::new(SourceKind::NETEASE, vec![full])),
        Arc::new(mineral_channel_mineral::MineralChannel::new(
            persist.clone(),
        )),
    ];
    let core = core_with_channels(
        channels,
        persist.clone(),
        /*music_dir*/ None,
        MediaCache::disabled(),
    )?;

    let scope = persist.scope(SourceKind::NETEASE);
    assert!(scope.get_meta(&id).await?.is_none(), "初始无 meta");

    core.set_favorite(&id, /*loved*/ true, mineral_stats::Actor::User)
        .await?;

    // 补 meta 是后台单飞任务(异步 spawn),轮询到它把 meta 写进 persist。
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let meta = loop {
        if let Some(meta) = scope.get_meta(&id).await? {
            break meta;
        }
        if std::time::Instant::now() >= deadline {
            color_eyre::eyre::bail!("后台补 meta 超时未写入");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert_eq!(meta.name, "From Detail", "补的是 channel detail 返回的歌名");
    Ok(())
}

/// 后台补 meta 是 source-neutral 的:一次 spawn 扫全部源,netease + bilibili 各自缺 meta 的
/// 歌都经**各自** channel 的 songs_detail 补上(单飞一个 worker 覆盖多源,无源特判)。
#[tokio::test(flavor = "multi_thread")]
async fn meta_backfill_covers_all_sources() -> color_eyre::Result<()> {
    use mineral_test::mock::DetailChannel;
    use mineral_test::with_name;

    let dir = tempfile::tempdir()?;
    let persist = ServerStore::open(&dir.path().join("t.db")).await?;
    let n_id = SongId::new(SourceKind::NETEASE, "n1");
    let b_id = SongId::new(SourceKind::BILIBILI, "b1");
    let n_full = with_name(song("n1"), "Netease Song");
    let mut b_full = with_name(song("b1"), "Bili Song");
    b_full.id = b_id.clone();
    let channels: Vec<Arc<dyn MusicChannel>> = vec![
        Arc::new(DetailChannel::new(SourceKind::NETEASE, vec![n_full])),
        Arc::new(DetailChannel::new(SourceKind::BILIBILI, vec![b_full])),
        Arc::new(mineral_channel_mineral::MineralChannel::new(
            persist.clone(),
        )),
    ];
    let core = core_with_channels(
        channels,
        persist.clone(),
        /*music_dir*/ None,
        MediaCache::disabled(),
    )?;

    // 两源各 love 一首、均无 meta。
    persist
        .scope(SourceKind::NETEASE)
        .set_loved(&n_id, true)
        .await?;
    persist
        .scope(SourceKind::BILIBILI)
        .set_loved(&b_id, true)
        .await?;

    core.spawn_meta_backfill();

    // 轮询到两源的 meta 都被后台任务补上。
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let n = persist.scope(SourceKind::NETEASE).get_meta(&n_id).await?;
        let b = persist.scope(SourceKind::BILIBILI).get_meta(&b_id).await?;
        if n.is_some() && b.is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            color_eyre::eyre::bail!("后台补 meta 超时:两源未都补上");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    Ok(())
}

/// sync_favorites 把远端红心导入本地 persist(add-only,不删本地),并向 client_events
/// 推 canonical(persist)favorited 集。回归:导入不得删掉本地独有的收藏(本地为准)。
#[tokio::test]
async fn sync_favorites_imports_remote_add_only_and_emits() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let persist = ServerStore::open(&dir.path().join("t.db")).await?;
    let local_only = SongId::new(SourceKind::NETEASE, "B");
    persist
        .scope(SourceKind::NETEASE)
        .set_loved(&local_only, /*loved*/ true)
        .await?;
    let core = core_with_persist(Arc::default(), persist.clone())?;

    let remote_only = SongId::new(SourceKind::NETEASE, "A");
    let remote: rustc_hash::FxHashSet<SongId> = [remote_only.clone()].into_iter().collect();
    let channel: Arc<dyn MusicChannel> = Arc::new(RecordingChannel {
        calls: Arc::default(),
        url_delay: None,
        liked_ids: Some(remote),
        playlists: None,
    });
    core.sync_favorites(SourceKind::NETEASE, channel).await;

    let ids = persist.scope(SourceKind::NETEASE).loved_ids().await?;
    assert!(ids.contains(&remote_only), "远端 A 应导入本地");
    assert!(ids.contains(&local_only), "本地独有 B 不被删(本地为准)");
    assert_eq!(ids.len(), 2, "persist 应为 A ∪ B");

    let events = core.drain_client_events();
    let last = events
        .iter()
        .rev()
        .find_map(|e| match e {
            mineral_task::TaskEvent::LikedSongIdsFetched { source, ids }
                if *source == SourceKind::NETEASE =>
            {
                Some(ids)
            }
            _ => None,
        })
        .ok_or_else(|| color_eyre::eyre::eyre!("应向 client 推 canonical favorited 集"))?;
    assert!(
        last.contains(&remote_only) && last.contains(&local_only),
        "推给 client 的应是 persist canonical 集(A ∪ B)"
    );
    Ok(())
}
