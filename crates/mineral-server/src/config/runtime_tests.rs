//! daemon 音乐 API、异步回执、hook 与配置函数的线程往返测试。

use std::time::Duration;

use mineral_model::{BitRate, PlaylistId, SongId, SourceKind};
use mineral_protocol::{Event, FailureNotice, StoreValue};
use mineral_script::mlua::{self, Lua};
use mineral_test::{song, with_duration};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use mineral_script::ScriptRuntime;
use mineral_script::{
    BeforeDownloadCtx, BeforeStreamCtx, CurateOutcome, HookDecision, HookMode, PlaylistBrief,
    ResolveValue, ScriptCmd, ScriptHost, ScriptSender, SourceWebUrls, install_daemon_api,
    seed_web_url_templates,
};

/// 真实 daemon 脚本线程与命令、失败通知的接收端。
struct Rig {
    /// Drop 时停机并 join。
    runtime: ScriptRuntime,

    /// daemon 回投查询与请求回调的入口。
    sender: ScriptSender,

    /// 音乐命令出口。
    commands: UnboundedReceiver<ScriptCmd>,

    /// 仅接收 daemon 回调失败类别。
    failures: UnboundedReceiver<Event>,
}

impl Rig {
    /// 将音乐操作放进 daemon.lua 的 setup，经真实配置加载管线启动回调线程。
    fn script(source: &str) -> color_eyre::Result<Self> {
        Self::configured(source, |_lua| Ok(()))
    }

    /// 在配置加载后、线程启动前保存边界测试专用函数与网页链接模板。
    fn configured(
        source: &str,
        configure: impl FnOnce(&Lua) -> mlua::Result<()>,
    ) -> color_eyre::Result<Self> {
        let config = format!("return {{ setup = function(mineral) {source} end }}");
        Self::load(&config, configure)
    }

    /// 加载完整 daemon.lua 内容，以验证配置提取与线程调度的衔接。
    fn load(
        source: &str,
        configure: impl FnOnce(&Lua) -> mlua::Result<()>,
    ) -> color_eyre::Result<Self> {
        let (cmd_tx, commands) = unbounded_channel();
        let (push_tx, failures) = unbounded_channel();
        let host = ScriptHost::new(cmd_tx, push_tx);
        let lua = daemon_vm(source, &host)?;
        configure(&lua)?;
        let sender = ScriptSender::detached();
        let runtime = ScriptRuntime::spawn(lua, host, lax_watchdog(), &sender)?;
        Ok(Self {
            runtime,
            sender,
            commands,
            failures,
        })
    }

    /// join 后排干命令与失败通知，保证回调已完成。
    fn finish(mut self) -> (Vec<ScriptCmd>, Vec<Event>) {
        drop(self.runtime);
        let commands = drain_cmds(&mut self.commands);
        let mut failures = Vec::new();
        while let Ok(event) = self.failures.try_recv() {
            failures.push(event);
        }
        (commands, failures)
    }
}

/// 通过临时 daemon.lua 加载真实配置与回调；不访问用户配置。
fn daemon_vm(source: &str, host: &ScriptHost) -> color_eyre::Result<Lua> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("daemon.lua");
    std::fs::write(&path, source)?;
    let loaded = crate::config::load_daemon_with_vm(&path, |lua| install_daemon_api(lua, host))?;
    if !loaded.warnings.is_empty() {
        color_eyre::eyre::bail!("daemon fixture rejected: {:?}", loaded.warnings);
    }
    loaded
        .vm
        .ok_or_else(|| color_eyre::eyre::eyre!("daemon fixture VM missing"))
}

/// Lua callback 成功后的 StoreSet 回执。
fn stored(song_id: &str, key: &str, value: StoreValue) -> ScriptCmd {
    ScriptCmd::StoreSet {
        song: SongId::new(SourceKind::NETEASE, song_id),
        key: key.to_owned(),
        value,
    }
}

#[test]
fn store_get_resolves_once_and_callback_can_write() -> color_eyre::Result<()> {
    let mut rig = Rig::script(
        r#"
        mineral.store.get("netease:1", "plugin.x", function(v, err)
            assert(err == nil)
            mineral.store.set("netease:1", "plugin.x", v + 1)
        end)
        "#,
    )?;
    let command = rig.commands.try_recv()?;
    let ScriptCmd::StoreGet { song, key, query } = command else {
        color_eyre::eyre::bail!("expected StoreGet, got {command:?}");
    };
    assert_eq!(song.qualified(), "netease:1");
    assert_eq!(key, "plugin.x");
    rig.sender
        .resolve(query, ResolveValue::Store(StoreValue::Int(7)));
    rig.sender
        .resolve(query, ResolveValue::Store(StoreValue::Int(99)));
    let (commands, failures) = rig.finish();
    assert_eq!(commands, vec![stored("1", "plugin.x", StoreValue::Int(8))]);
    assert!(failures.is_empty());
    Ok(())
}

#[test]
fn store_set_maps_scalars_and_nil_deletion() -> color_eyre::Result<()> {
    let rig = Rig::script(
        r#"
        mineral.store.set("netease:2", "plugin.s", "文本")
        mineral.store.set("netease:2", "plugin.b", true)
        mineral.store.set("netease:2", "plugin.f", 2.5)
        mineral.store.set("netease:2", "plugin.gone", nil)
        "#,
    )?;
    let (commands, failures) = rig.finish();
    assert_eq!(
        commands,
        vec![
            stored("2", "plugin.s", StoreValue::Text("文本".to_owned())),
            stored("2", "plugin.b", StoreValue::Bool(true)),
            stored("2", "plugin.f", StoreValue::Real(2.5)),
            stored("2", "plugin.gone", StoreValue::Nil),
        ]
    );
    assert!(failures.is_empty());
    Ok(())
}

#[test]
fn query_failure_preserves_source_until_lua_boundary() -> color_eyre::Result<()> {
    let mut rig = Rig::script(
        r#"
        mineral.store.get("netease:3", "plugin.x", function(v, err)
            assert(v == nil and type(err) == "string" and #err > 0)
            mineral.player.next()
        end)
        "#,
    )?;
    let command = rig.commands.try_recv()?;
    let ScriptCmd::StoreGet { query, .. } = command else {
        color_eyre::eyre::bail!("expected StoreGet, got {command:?}");
    };
    let reply = ResolveValue::Error(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "query rejected",
    )));
    assert!(matches!(
        &reply,
        ResolveValue::Error(source)
            if matches!(source.as_ref().downcast_ref::<std::io::Error>(),
                Some(cause) if cause.kind() == std::io::ErrorKind::InvalidData)
    ));
    rig.sender.resolve(query, reply);
    let (commands, failures) = rig.finish();
    assert_eq!(commands, vec![ScriptCmd::Next]);
    assert!(failures.is_empty());
    Ok(())
}

#[test]
fn failing_query_callback_does_not_block_other_queries() -> color_eyre::Result<()> {
    let mut rig = Rig::script(
        r#"
        mineral.queue.list(function() error("failed") end)
        mineral.queue.list(function() mineral.player.next() end)
        "#,
    )?;
    for _ in 0..2 {
        let command = rig.commands.try_recv()?;
        let ScriptCmd::QueueList { query } = command else {
            color_eyre::eyre::bail!("expected QueueList, got {command:?}");
        };
        rig.sender.resolve(query, ResolveValue::Songs(Vec::new()));
    }
    let (commands, failures) = rig.finish();
    assert_eq!(commands, vec![ScriptCmd::Next]);
    assert_eq!(
        failures,
        vec![Event::Failure(FailureNotice::ScriptCallbackFailed {
            callback: "query".to_owned(),
        })]
    );
    Ok(())
}

#[test]
fn queue_query_projects_song_metadata_and_seeded_urls() -> color_eyre::Result<()> {
    let mut rig = Rig::configured(
        r#"
        mineral.queue.list(function(q, err)
            assert(err == nil and #q == 2)
            assert(q[1].id == "netease:1" and q[2].id == "netease:2")
            assert(q[1].title == "one" and q[1].duration_ms == 1500)
            assert(q[2].duration_ms == nil)
            assert(q[1].artists[1] == "Mineral" and q[1].album == "EndSerenading")
            assert(q[1].cover_url == nil and q[1].source_url == nil)
            assert(q[1].source == "netease" and q[1].url == "https://x.example/song?id=1")
            assert(q[1].index == nil)
            mineral.queue.set({ q[2], q[1] })
        end)
        "#,
        |lua| {
            seed_web_url_templates(
                lua,
                vec![SourceWebUrls {
                    source: "netease".to_owned(),
                    song: Some("https://x.example/song?id={id}".to_owned()),
                    playlist: None,
                    album: None,
                    artist: None,
                }],
            )
        },
    )?;
    let command = rig.commands.try_recv()?;
    let ScriptCmd::QueueList { query } = command else {
        color_eyre::eyre::bail!("expected QueueList, got {command:?}");
    };
    let mut first = with_duration(song("1"), 1500);
    first.name = "one".to_owned();
    first.artists = vec![mineral_model::ArtistRef {
        id: mineral_model::ArtistId::new(SourceKind::NETEASE, "mineral"),
        name: "Mineral".to_owned(),
    }];
    first.album = Some(mineral_model::AlbumRef {
        id: mineral_model::AlbumId::new(SourceKind::NETEASE, "es"),
        name: "EndSerenading".to_owned(),
    });
    rig.sender
        .resolve(query, ResolveValue::Songs(vec![first, song("2")]));
    let (commands, failures) = rig.finish();
    assert_eq!(
        commands,
        vec![ScriptCmd::QueueSet {
            ids: vec![song("2").id, song("1").id],
        }]
    );
    assert!(failures.is_empty());
    Ok(())
}

#[test]
fn library_queries_preserve_membership_and_source_filters() -> color_eyre::Result<()> {
    let mut rig = Rig::script(
        r#"
        mineral.library.playlists(function(ps, err)
            assert(err == nil and ps[1].id == "netease:p1" and ps[1].track_count == 42)
            assert(ps[1].description == "desc" and ps[1].source == "netease")
            assert(ps[1].play_count == nil and ps[1].subscriber_count == nil)
            mineral.player.next()
        end)
        mineral.library.tracks("netease:p9", function(entries, err)
            assert(err == nil and entries[1].index == 7)
            assert(entries[1].song.id == "netease:entry" and entries[1].song.index == nil)
            assert(entries[1].song.url == nil)
            mineral.player.prev()
        end)
        mineral.library.search("雨", function(songs, err)
            assert(err == nil and #songs == 2 and songs[1].index == nil)
            mineral.player.stop()
        end)
        mineral.library.search("雪", { source = "netease", offset = 10, limit = 5 },
            function() end)
        mineral.library.love("netease:5", true)
        mineral.library.love("netease:6", false)
        "#,
    )?;
    let first = rig.commands.try_recv()?;
    let ScriptCmd::LibraryPlaylists { query: playlists } = first else {
        color_eyre::eyre::bail!("expected LibraryPlaylists, got {first:?}");
    };
    let second = rig.commands.try_recv()?;
    let ScriptCmd::LibraryTracks {
        playlist,
        query: tracks,
    } = second
    else {
        color_eyre::eyre::bail!("expected LibraryTracks, got {second:?}");
    };
    assert_eq!(playlist.qualified(), "netease:p9");
    let third = rig.commands.try_recv()?;
    let ScriptCmd::LibrarySearch {
        term,
        source,
        offset,
        limit,
        query: search,
    } = third
    else {
        color_eyre::eyre::bail!("expected LibrarySearch, got {third:?}");
    };
    assert_eq!((term.as_str(), source, offset, limit), ("雨", None, 0, 30));
    let fourth = rig.commands.try_recv()?;
    let ScriptCmd::LibrarySearch {
        term,
        source,
        offset,
        limit,
        ..
    } = fourth
    else {
        color_eyre::eyre::bail!("expected LibrarySearch, got {fourth:?}");
    };
    assert_eq!(
        (term.as_str(), source, offset, limit),
        ("雪", Some(SourceKind::NETEASE), 10, 5)
    );
    for (id, loved) in [("5", true), ("6", false)] {
        assert_eq!(
            rig.commands.try_recv()?,
            ScriptCmd::SetLoved {
                song: song(id).id,
                loved
            }
        );
    }
    let mut playlist = brief(SourceKind::NETEASE, "p1", "list", 42);
    playlist.description = "desc".to_owned();
    rig.sender
        .resolve(playlists, ResolveValue::Playlists(vec![playlist]));
    rig.sender.resolve(
        tracks,
        ResolveValue::PlaylistEntries(vec![
            mineral_model::PlaylistEntry::builder()
                .index(mineral_model::CollectionIndex::new(7))
                .song(song("entry"))
                .build(),
        ]),
    );
    rig.sender
        .resolve(search, ResolveValue::Songs(vec![song("1"), song("2")]));
    let (commands, failures) = rig.finish();
    assert_eq!(
        commands,
        vec![ScriptCmd::Next, ScriptCmd::Prev, ScriptCmd::Stop]
    );
    assert!(failures.is_empty());
    Ok(())
}

#[test]
fn song_url_query_projects_direct_media() -> color_eyre::Result<()> {
    let mut rig = Rig::script(
        r#"
        mineral.library.song_url("bilibili:BV1xx:1", function(r, err)
            assert(err == nil)
            assert(r.song_id == "bilibili:BV1xx:1" and r.url == "https://cdn.example/a.m4s")
            assert(r.quality == "exhigh" and r.bitrate_bps == 192000 and r.format == "aac")
            assert(r.layout == "chunked" and r.size == nil)
            assert(r.headers[1][1] == "Referer" and r.headers[1][2] == "https://www.bilibili.com/")
            mineral.player.next()
        end)
        "#,
    )?;
    let command = rig.commands.try_recv()?;
    let ScriptCmd::LibrarySongUrl { song, query } = command else {
        color_eyre::eyre::bail!("expected LibrarySongUrl, got {command:?}");
    };
    assert_eq!(song.qualified(), "bilibili:BV1xx:1");
    let direct = mineral_model::DirectMedia::remote(
        mineral_model::PlaybackMediaInfo {
            song_id: song,
            bitrate_bps: Some(192_000),
            size: None,
            format: Some(mineral_model::AudioFormat::Aac),
            bit_depth: None,
            substituted: false,
        },
        "https://cdn.example/a.m4s".parse()?,
        vec![("Referer".to_owned(), "https://www.bilibili.com/".to_owned())],
        mineral_model::StreamLayout::Chunked,
    );
    rig.sender.resolve(
        query,
        ResolveValue::DirectMedia {
            media: Box::new(direct),
            requested_quality: BitRate::Exhigh,
        },
    );
    let (commands, failures) = rig.finish();
    assert_eq!(commands, vec![ScriptCmd::Next]);
    assert!(failures.is_empty());
    Ok(())
}

#[test]
fn named_queue_transforms_execute_in_daemon() -> color_eyre::Result<()> {
    let rig = Rig::load(
        r#"
        return {
            queue = { transforms = {
                { name = "Selected first", transform = function(q, ctx)
                    assert(ctx.current == 2 and ctx.selected == 1)
                    return {q[3], q[1]}
                end },
                { name = "Invalid ID", transform = function()
                    return {{id = "invalid"}}
                end },
            } },
            setup = function(api)
                assert(api.ui == nil)
                assert(not pcall(require, "mineral.tui"))
            end
        }
        "#,
        |_lua| Ok(()),
    )?;
    let queue = vec![song("1"), song("2"), song("3")];
    let transformed = rig
        .sender
        .queue_transform("Selected first".to_owned(), queue.clone(), 1, Some(0))
        .blocking_recv()??;
    assert_eq!(transformed, vec![song("3").id, song("1").id]);
    let invalid = rig
        .sender
        .queue_transform("Invalid ID".to_owned(), queue.clone(), 0, None)
        .blocking_recv()?;
    assert!(matches!(
        invalid,
        Err(mineral_script::Error::InvalidSongId { index: 1, .. })
    ));
    let missing = rig
        .sender
        .queue_transform("Removed operation".to_owned(), queue, 0, None)
        .blocking_recv()?;
    assert!(
        matches!(missing, Err(mineral_script::Error::MissingQueueTransform { name, .. })
        if name == "Removed operation")
    );
    let (commands, failures) = rig.finish();
    assert!(commands.is_empty() && failures.is_empty());
    Ok(())
}

/// curate 测试的轻量歌单。
fn brief(source: SourceKind, id: &str, name: &str, track_count: u64) -> PlaylistBrief {
    PlaylistBrief {
        id: PlaylistId::new(source, id),
        name: name.to_owned(),
        track_count,
        description: String::new(),
        play_count: None,
        subscriber_count: None,
    }
}

#[tokio::test]
async fn curate_filters_renames_reorders_and_merges_sources() -> color_eyre::Result<()> {
    let rig = Rig::configured("", |lua| {
        let per_source = lua.create_table()?;
        per_source.set(
            "bilibili",
            lua.load(
                "function(lists)
            local keep = {}
            for i = #lists, 1, -1 do
                local p = lists[i]
                if p.track_count > 0 then
                    p.name = 'renamed:' .. p.name
                    keep[#keep + 1] = p
                end
            end
            return keep
        end",
            )
            .eval::<mlua::Function>()?,
        )?;
        lua.set_named_registry_value(
            mineral_script::registry::CURATE_PLAYLISTS_SOURCE_FNS,
            per_source,
        )?;
        lua.set_named_registry_value(
            mineral_script::registry::CURATE_PLAYLISTS_MERGED_FN,
            lua.load(
                "function(all)
                table.sort(all, function(a, b) return a.source < b.source end)
                for _, p in ipairs(all) do p.name = p.source .. '/' .. p.description end
                return all
            end",
            )
            .eval::<mlua::Function>()?,
        )
    })?;
    assert_eq!(
        rig.sender.curate_source_keys().await?,
        vec!["bilibili".to_owned()]
    );
    let outcome = rig
        .sender
        .curate_playlists(
            Some(SourceKind::BILIBILI),
            vec![
                brief(SourceKind::BILIBILI, "f1", "first", 3),
                brief(SourceKind::BILIBILI, "f2", "empty", 0),
                brief(SourceKind::BILIBILI, "f3", "third", 7),
            ],
            Duration::from_secs(5),
        )
        .await;
    let CurateOutcome::Curated(entries) = outcome else {
        color_eyre::eyre::bail!("expected Curated, got {outcome:?}");
    };
    let got = entries
        .iter()
        .map(|e| (e.id.qualified(), e.name.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        got,
        vec![
            ("bilibili:f3".to_owned(), Some("renamed:third".to_owned())),
            ("bilibili:f1".to_owned(), Some("renamed:first".to_owned())),
        ]
    );
    let mut netease = brief(SourceKind::NETEASE, "p1", "list", 5);
    netease.description = "net".to_owned();
    let mut bilibili = brief(SourceKind::BILIBILI, "f1", "list", 3);
    bilibili.description = "bili".to_owned();
    let outcome = rig
        .sender
        .curate_playlists(None, vec![netease, bilibili], Duration::from_secs(5))
        .await;
    let CurateOutcome::Curated(entries) = outcome else {
        color_eyre::eyre::bail!("expected Curated, got {outcome:?}");
    };
    assert_eq!(
        entries.into_iter().map(|e| e.name).collect::<Vec<_>>(),
        vec![
            Some("bilibili/bili".to_owned()),
            Some("netease/net".to_owned()),
        ]
    );
    let (_, failures) = rig.finish();
    assert!(failures.is_empty());
    Ok(())
}

#[tokio::test]
async fn curate_missing_and_bad_functions_fall_back_to_identity() -> color_eyre::Result<()> {
    let rig = Rig::configured("", |lua| {
        let per_source = lua.create_table()?;
        for (source, code) in [
            ("netease", "function() return 42 end"),
            ("bilibili", "function() return {{name = 'missing id'}} end"),
            ("local", "function() error('failed') end"),
        ] {
            per_source.set(source, lua.load(code).eval::<mlua::Function>()?)?;
        }
        lua.set_named_registry_value(
            mineral_script::registry::CURATE_PLAYLISTS_SOURCE_FNS,
            per_source,
        )
    })?;
    for source in [SourceKind::NETEASE, SourceKind::BILIBILI, SourceKind::LOCAL] {
        assert_eq!(
            rig.sender
                .curate_playlists(
                    Some(source),
                    vec![brief(source, "p1", "list", 1)],
                    Duration::from_secs(5)
                )
                .await,
            CurateOutcome::Identity
        );
    }
    assert_eq!(
        rig.sender
            .curate_playlists(None, Vec::new(), Duration::from_secs(5))
            .await,
        CurateOutcome::Identity
    );
    let (_, failures) = rig.finish();
    assert_eq!(
        failures,
        vec![
            Event::Failure(FailureNotice::ScriptCallbackFailed {
                callback: "curate_playlists".to_owned(),
            });
            3
        ]
    );
    let detached = ScriptSender::detached();
    assert!(detached.curate_source_keys().await?.is_empty());
    assert_eq!(
        detached
            .curate_playlists(None, Vec::new(), Duration::from_secs(5))
            .await,
        CurateOutcome::Identity
    );
    Ok(())
}

/// 有原始直链的 before_stream 快照。
fn stream_ctx() -> color_eyre::Result<BeforeStreamCtx> {
    let target = song("1");
    let direct = mineral_model::DirectMedia::remote(
        mineral_model::PlaybackMediaInfo {
            song_id: target.id.clone(),
            bitrate_bps: None,
            size: None,
            format: Some(mineral_model::AudioFormat::Flac),
            bit_depth: None,
            substituted: false,
        },
        "https://example.com/a.flac".parse()?,
        Vec::new(),
        mineral_model::StreamLayout::Contiguous,
    );
    Ok(BeforeStreamCtx::playable(
        target,
        BitRate::Exhigh,
        HookMode::Immediate,
        Some(direct),
    ))
}

#[tokio::test]
async fn hook_rewrite_projects_stream_context_and_preserves_media_fields() -> color_eyre::Result<()>
{
    let rig = Rig::script(
        r#"
        mineral.hook("before_stream", function(ctx)
            assert(ctx.song.id == "netease:1" and ctx.quality == "exhigh")
            assert(ctx.kind == "before_stream" and ctx.mode == "immediate" and not ctx.unplayable)
            assert(ctx.url == "https://example.com/a.flac")
            return {
                url = "https://fallback.example/b.m4s", quality = "standard",
                headers = {{"Referer", "https://example.com/"}}, layout = "chunked",
                bitrate_bps = 132000, format = "m4a",
            }
        end)
        "#,
    )?;
    let outcome = rig
        .sender
        .intercept_stream(stream_ctx()?, Duration::from_secs(5))
        .await;
    let HookDecision::Rewrite(spec) = outcome else {
        color_eyre::eyre::bail!("expected Rewrite, got {outcome:?}");
    };
    assert_eq!(
        spec.new_url().map(ToString::to_string),
        Some("https://fallback.example/b.m4s".to_owned())
    );
    assert_eq!(spec.new_quality(), Some(BitRate::Standard));
    assert_eq!(spec.bitrate_bps(), Some(132_000));
    assert_eq!(spec.format(), Some(&mineral_model::AudioFormat::Mp4));
    assert_eq!(spec.layout(), Some(mineral_model::StreamLayout::Chunked));
    let headers = vec![("Referer".to_owned(), "https://example.com/".to_owned())];
    assert_eq!(spec.stream_headers(), Some(headers.as_slice()));
    let (_, failures) = rig.finish();
    assert!(failures.is_empty());
    Ok(())
}

#[tokio::test]
async fn download_context_omits_stream_mode_and_skip_short_circuits() -> color_eyre::Result<()> {
    let rig = Rig::script(
        r#"
        mineral.hook("before_download", function(ctx)
            assert(ctx.mode == nil and ctx.url == nil and ctx.unplayable)
            return {skip = ctx.kind}
        end)
        mineral.hook("before_download", function() error("must not run") end)
        "#,
    )?;
    let ctx = BeforeDownloadCtx::unavailable(song("1"), BitRate::Exhigh);
    assert_eq!(
        rig.sender
            .intercept_download(ctx, Duration::from_secs(5))
            .await,
        HookDecision::Skip {
            reason: "before_download".to_owned()
        }
    );
    let (_, failures) = rig.finish();
    assert!(failures.is_empty());
    Ok(())
}

#[tokio::test]
async fn invalid_hooks_fall_through_and_false_skips() -> color_eyre::Result<()> {
    let rig = Rig::script(
        r#"
        mineral.hook("before_stream", function() error("failed") end)
        mineral.hook("before_stream", function() return 42 end)
        mineral.hook("before_stream", function(ctx)
            assert(ctx.url == nil and ctx.unplayable and ctx.mode == "prefetch")
            return false
        end)
        "#,
    )?;
    let ctx = BeforeStreamCtx::unavailable(song("1"), BitRate::Exhigh, HookMode::Prefetch);
    assert!(matches!(
        rig.sender
            .intercept_stream(ctx, Duration::from_secs(5))
            .await,
        HookDecision::Skip { .. }
    ));
    let (_, failures) = rig.finish();
    assert_eq!(
        failures,
        vec![
            Event::Failure(FailureNotice::ScriptCallbackFailed {
                callback: "before_stream".to_owned(),
            });
            2
        ]
    );
    Ok(())
}

#[tokio::test]
async fn deferred_hook_resolves_music_query_once_and_short_circuits() -> color_eyre::Result<()> {
    let mut rig = Rig::script(
        r#"
        mineral.hook("before_stream", function(ctx)
            mineral.store.get(ctx.song.id, "plugin.rewrite", function(v, err)
                assert(err == nil)
                ctx.resolve({url = v, quality = "standard"})
                ctx.resolve({skip = "second resolve must not win"})
            end)
            return mineral.DEFER
        end)
        mineral.hook("before_stream", function() error("must not run") end)
        "#,
    )?;
    let ctx = stream_ctx()?;
    let intercept = rig.sender.intercept_stream(ctx, Duration::from_secs(5));
    let completion = async {
        let command = tokio::time::timeout(Duration::from_secs(5), rig.commands.recv())
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("command channel closed"))?;
        let ScriptCmd::StoreGet { song, key, query } = command else {
            color_eyre::eyre::bail!("expected StoreGet, got {command:?}");
        };
        assert_eq!(song.qualified(), "netease:1");
        assert_eq!(key, "plugin.rewrite");
        rig.sender.resolve(
            query,
            ResolveValue::Store(StoreValue::Text(
                "https://fallback.example/b.flac".to_owned(),
            )),
        );
        Ok::<_, color_eyre::Report>(())
    };
    let (decision, completed) = tokio::join!(intercept, completion);
    completed?;
    let HookDecision::Rewrite(spec) = decision else {
        color_eyre::eyre::bail!("expected deferred Rewrite, got {decision:?}");
    };
    assert_eq!(
        spec.new_url().map(ToString::to_string),
        Some("https://fallback.example/b.flac".to_owned())
    );
    assert_eq!(spec.new_quality(), Some(BitRate::Standard));
    let (_, failures) = rig.finish();
    assert!(failures.is_empty());
    Ok(())
}

#[tokio::test]
async fn deferred_music_query_can_resolve_continue() -> color_eyre::Result<()> {
    let mut rig = Rig::script(
        r#"
        mineral.hook("before_stream", function(ctx)
            mineral.queue.list(function() ctx.resolve(nil) end)
            return mineral.DEFER
        end)
        "#,
    )?;
    let ctx = stream_ctx()?;
    let intercept = rig.sender.intercept_stream(ctx, Duration::from_secs(5));
    let completion = async {
        let command = tokio::time::timeout(Duration::from_secs(5), rig.commands.recv())
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("command channel closed"))?;
        let ScriptCmd::QueueList { query } = command else {
            color_eyre::eyre::bail!("expected QueueList, got {command:?}");
        };
        rig.sender.resolve(query, ResolveValue::Songs(Vec::new()));
        Ok::<_, color_eyre::Report>(())
    };
    let (decision, completed) = tokio::join!(intercept, completion);
    completed?;
    assert_eq!(decision, HookDecision::Continue);
    let (_, failures) = rig.finish();
    assert!(failures.is_empty());
    Ok(())
}

#[tokio::test]
async fn deferred_hook_without_completion_times_out_to_continue() -> color_eyre::Result<()> {
    let rig = Rig::script(
        r#"
        mineral.hook("before_stream", function() return mineral.DEFER end)
        "#,
    )?;
    let started = std::time::Instant::now();
    let decision = rig
        .sender
        .intercept_stream(stream_ctx()?, Duration::from_millis(50))
        .await;
    assert_eq!(decision, HookDecision::Continue);
    assert!(started.elapsed() >= Duration::from_millis(50));
    let (_, failures) = rig.finish();
    assert!(failures.is_empty());
    Ok(())
}

#[tokio::test]
async fn missing_hooks_fail_open_after_daemon_load() -> color_eyre::Result<()> {
    let rig = Rig::script("")?;
    assert_eq!(
        rig.sender
            .intercept_stream(stream_ctx()?, Duration::from_secs(5))
            .await,
        HookDecision::Continue
    );
    rig.finish();
    Ok(())
}

#[test]
fn failed_setup_discards_buffered_commands_and_keeps_old_sender() -> color_eyre::Result<()> {
    let old = Rig::load(
        r#"return { queue = { transforms = {
            { name = "Keep", transform = function(q) return q end },
        } } }"#,
        |_lua| Ok(()),
    )?;
    let (cmd_tx, mut commands) = unbounded_channel();
    let (push_tx, _failures) = unbounded_channel();
    let host = ScriptHost::new(cmd_tx, push_tx);
    let lua = daemon_vm(
        r#"
        local api = require("mineral.daemon")
        api.player.prev()
        return { setup = function(host)
            assert(host == api and host.ui == nil)
            host.player.next()
            error("setup failed")
        end }
        "#,
        &host,
    )?;
    assert!(commands.try_recv().is_err());
    let failed = ScriptRuntime::spawn(lua, host, lax_watchdog(), &old.sender);
    assert!(matches!(failed, Err(mineral_script::Error::Lua { .. })));
    assert!(commands.try_recv().is_err());
    let ids = old
        .sender
        .queue_transform("Keep".to_owned(), vec![song("1")], 0, None)
        .blocking_recv()??;
    assert_eq!(ids, vec![song("1").id]);
    let (commands, failures) = old.finish();
    assert!(commands.is_empty() && failures.is_empty());
    Ok(())
}

#[test]
fn daemon_setup_loop_is_guarded_before_activation() -> color_eyre::Result<()> {
    let (cmd_tx, mut commands) = unbounded_channel();
    let (push_tx, _failures) = unbounded_channel();
    let host = ScriptHost::new(cmd_tx, push_tx);
    let lua = daemon_vm(
        r#"return { setup = function(api)
            api.player.next()
            while true do end
        end }"#,
        &host,
    )?;
    let watchdog = mineral_script::WatchdogConfig::builder()
        .instruction_interval(1_000)
        .soft_wall(Duration::from_millis(10))
        .hard_wall(Duration::from_millis(50))
        .build();
    let sender = ScriptSender::detached();
    let started = std::time::Instant::now();
    assert!(ScriptRuntime::spawn(lua, host, watchdog, &sender).is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(!sender.is_attached());
    assert!(commands.try_recv().is_err());
    Ok(())
}

#[test]
fn query_handles_are_not_reused_across_runtime_reloads() -> color_eyre::Result<()> {
    let mut old = Rig::script("mineral.queue.list(function() mineral.player.next() end)")?;
    let old_command = old.commands.try_recv()?;
    let ScriptCmd::QueueList { query: old_query } = old_command else {
        color_eyre::eyre::bail!("expected QueueList, got {old_command:?}");
    };
    old.finish();
    let mut new = Rig::script("mineral.queue.list(function() mineral.player.prev() end)")?;
    let new_command = new.commands.try_recv()?;
    let ScriptCmd::QueueList { query: new_query } = new_command else {
        color_eyre::eyre::bail!("expected QueueList, got {new_command:?}");
    };
    assert_ne!(old_query, new_query);
    new.sender
        .resolve(old_query, ResolveValue::Songs(Vec::new()));
    new.sender
        .resolve(new_query, ResolveValue::Songs(Vec::new()));
    let (commands, failures) = new.finish();
    assert_eq!(commands, vec![ScriptCmd::Prev]);
    assert!(failures.is_empty());
    Ok(())
}

#[test]
fn unknown_hook_name_is_rejected() -> color_eyre::Result<()> {
    let (lua, _host) = vm_with_host()?;
    assert!(lua.load(r#"local mineral = require("mineral.daemon"); mineral.hook("after_play", function() end)"#)
        .exec().is_err());
    Ok(())
}

#[test]
fn drop_joins_thread_and_late_query_is_ignored() -> color_eyre::Result<()> {
    let mut rig = Rig::script(
        r#"
        mineral.queue.list(function() mineral.player.next() end)
        "#,
    )?;
    let command = rig.commands.try_recv()?;
    let ScriptCmd::QueueList { query } = command else {
        color_eyre::eyre::bail!("expected QueueList, got {command:?}");
    };
    let sender = rig.sender.clone();
    let (commands, failures) = rig.finish();
    assert!(commands.is_empty() && failures.is_empty());
    sender.resolve(query, ResolveValue::Songs(Vec::new()));
    let result = sender
        .queue_transform("Removed operation".to_owned(), vec![song("1")], 0, None)
        .blocking_recv()?;
    assert!(matches!(result, Err(mineral_script::Error::Unavailable)));
    Ok(())
}

/// 装好音乐 API 的测试 VM;不加载个人配置或创建脚本线程。
fn vm_with_host() -> color_eyre::Result<(Lua, ScriptHost)> {
    let (cmd_tx, _commands) = unbounded_channel();
    let (push_tx, _failures) = unbounded_channel();
    let host = ScriptHost::new(cmd_tx, push_tx);
    let lua = Lua::new();
    install_daemon_api(&lua, &host)?;
    Ok((lua, host))
}

/// 排干线程已经产生的音乐命令。
fn drain_cmds(rx: &mut UnboundedReceiver<ScriptCmd>) -> Vec<ScriptCmd> {
    let mut commands = Vec::new();
    while let Ok(command) = rx.try_recv() {
        commands.push(command);
    }
    commands
}

/// 短回调的测试预算;生产默认值仍只来自 Lua。
fn lax_watchdog() -> mineral_script::WatchdogConfig {
    mineral_script::WatchdogConfig::builder()
        .instruction_interval(10_000)
        .soft_wall(Duration::from_millis(200))
        .hard_wall(Duration::from_secs(1))
        .build()
}
