//! 会话协议进程级 e2e:真 `mineral serve` 子进程 + 真 `mineral_client::Client`,
//! 验证操作提交顺序、查询归属、订阅推送与 daemon 能力热更在完整链路上的行为。
//!
//! 音频走 `MINERAL_AUDIO_NULL` 降级,headless 稳跑;每个测试隔离一套 XDG 目录与
//! 独立 socket 目录,与 `daemon_lifecycle` / `script_hooks` 同属 `daemon-e2e` 串行组。

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use color_eyre::eyre::{WrapErr, bail, eyre};
use mineral_client::Client;
use mineral_client::connection::ClientConfig;
use mineral_client::state::PlayerMirror;
use mineral_model::{Song, SongId, SourceKind};
use mineral_protocol::{
    PlayMode, QueueContextWire, Request, SocketWire, Subscription, SubscriptionTopic,
};
use mineral_task::{ChannelFetchKind, Priority, TaskKind};
use tokio::time::timeout;

/// 订阅推送 / 收敛的通用等待上限。
const WAIT: Duration = Duration::from_secs(10);

/// 隔离环境里的一个 daemon 子进程;Drop 时 kill 子进程并清临时目录。
struct Daemon {
    /// `mineral serve` 子进程。
    child: Child,

    /// 隔离用的临时根目录(XDG 配置/数据/缓存全指到这下面)。
    root: PathBuf,

    /// socket 目录(经 `MINERAL_SOCKET_DIR` 注入;刻意短,压在 `sun_path` 内)。
    sock_dir: PathBuf,

    /// daemon 监听的 socket 路径。
    socket: PathBuf,
}

impl Daemon {
    /// 起一个隔离环境、null 音频后端的 daemon;可预埋返回配置表的 daemon.lua。
    ///
    /// # Params:
    ///   - `tag`: 临时目录名里的测试标识。
    ///   - `daemon_lua`: daemon 配置与可选 setup 回调。
    fn spawn(tag: &str, daemon_lua: Option<&str>) -> color_eyre::Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "mineral-session-e2e-{}-{}-{}",
            tag,
            std::process::id(),
            unique_suffix()
        ));
        let sock_dir =
            std::env::temp_dir().join(format!("mnls-{}-{}", std::process::id(), unique_suffix()));
        std::fs::create_dir_all(&root).wrap_err("create isolated root dir")?;
        if let Some(src) = daemon_lua {
            let cfg_dir = root.join("config/mineral");
            std::fs::create_dir_all(&cfg_dir).wrap_err("create config dir")?;
            std::fs::write(cfg_dir.join("daemon.lua"), src).wrap_err("seed daemon.lua")?;
        }
        let child = Command::new(env!("CARGO_BIN_EXE_mineral"))
            .arg("serve")
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("MINERAL_SOCKET_DIR", &sock_dir)
            .env("MINERAL_AUDIO_NULL", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .wrap_err("spawn `mineral serve`")?;
        let socket = sock_dir.join("mineral.sock");
        Ok(Self {
            child,
            root,
            sock_dir,
            socket,
        })
    }

    /// 轮询直到 socket 可连(daemon ready),超时则报错。
    fn wait_ready(&self) -> color_eyre::Result<()> {
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            if std::os::unix::net::UnixStream::connect(&self.socket).is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        bail!("daemon did not become ready in time");
    }

    /// 连一条新会话(每次调用是一个独立 client)。
    ///
    /// # Params:
    ///   - `name`: client 自报名
    async fn connect(&self, name: &str) -> color_eyre::Result<Client> {
        let wire = SocketWire::connect(&self.socket).await?;
        Client::from_wire(Box::new(wire), name, ClientConfig::default())
            .await
            .map_err(color_eyre::Report::new)
    }

    /// 重写 daemon.lua,触发 daemon 的配置与脚本重载。
    ///
    /// # Params:
    ///   - `content`: 新的完整文件内容。
    fn write_daemon(&self, content: &str) -> color_eyre::Result<()> {
        let path = self.root.join("config/mineral/daemon.lua");
        std::fs::write(path, content).wrap_err("rewrite daemon.lua")
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_dir_all(&self.sock_dir);
    }
}

/// 短随机后缀,避免并发测试撞目录。
fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{nanos}-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// 造 `len` 首测试歌曲。
fn songs(len: usize) -> Vec<Song> {
    (0..len)
        .map(|index| mineral_test::song(&format!("e2e{index}")))
        .collect()
}

/// 轮询直到条件成立。
async fn wait_until(label: &str, mut ready: impl FnMut() -> bool) -> color_eyre::Result<()> {
    timeout(WAIT, async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .map_err(|_elapsed| eyre!("等待超时:{label}"))
}

/// 同一 client 的队列操作按到达次序提交:连续两次 next 落到第三首。
#[tokio::test]
async fn queue_ops_apply_in_arrival_order() -> color_eyre::Result<()> {
    let daemon = Daemon::spawn("order", None)?;
    daemon.wait_ready()?;
    let client = daemon.connect("order").await?;
    client.subscribe(SubscriptionTopic::Player);
    let outcome = client
        .play_queue(songs(5), /*target*/ 0, QueueContextWire::Manual)?
        .outcome()
        .await;
    assert!(outcome.is_success(), "起播应成功: {outcome:?}");
    client.wait_player_ready(WAIT).await;
    // 连续两条命令:writer 合批,daemon read loop 内联按序提交。
    client.fire(Request::NextSong);
    client.fire(Request::NextSong);
    wait_until("cursor 落到第 3 首", || {
        client
            .mirror()
            .read_player(|player| player.cursor().anchor())
            == 2
    })
    .await?;
    let queue_len = client.mirror().read_player(|player| player.queue().len());
    assert_eq!(queue_len, 5);
    Ok(())
}

/// 查询结果只回发起者:两个 client 各自 store 键互不串线。
#[tokio::test]
async fn queries_are_request_scoped() -> color_eyre::Result<()> {
    let daemon = Daemon::spawn("scope", None)?;
    daemon.wait_ready()?;
    let first = daemon.connect("scope-a").await?;
    let second = daemon.connect("scope-b").await?;
    let song = SongId::new(SourceKind::NETEASE, "e2e-scope");
    let outcome = first
        .store_set(song.clone(), "a", mineral_protocol::StoreValue::Int(11))
        .await;
    assert!(outcome.is_success(), "写 A 应成功: {outcome:?}");
    let outcome = second
        .store_set(song.clone(), "b", mineral_protocol::StoreValue::Int(22))
        .await;
    assert!(outcome.is_success(), "写 B 应成功: {outcome:?}");
    let (a, b) = tokio::join!(
        first.store_get(song.clone(), "a"),
        second.store_get(song.clone(), "b")
    );
    assert_eq!(
        a.into_success(),
        Some(mineral_protocol::StoreValue::Int(11)),
        "A 的查询结果不应串给 B"
    );
    assert_eq!(
        b.into_success(),
        Some(mineral_protocol::StoreValue::Int(22)),
        "B 的查询结果不应串给 A"
    );
    Ok(())
}

/// 跨 client 的队列编辑都由 daemon 接受:两边追加的歌都在最终队列里。
#[tokio::test]
async fn multi_client_edits_converge() -> color_eyre::Result<()> {
    let daemon = Daemon::spawn("multi", None)?;
    daemon.wait_ready()?;
    let first = daemon.connect("multi-a").await?;
    let second = daemon.connect("multi-b").await?;
    first.subscribe(SubscriptionTopic::Player);
    second.subscribe(SubscriptionTopic::Player);
    let outcome = first
        .play_queue(songs(1), /*target*/ 0, QueueContextWire::Manual)?
        .outcome()
        .await;
    assert!(outcome.is_success());
    first.wait_player_ready(WAIT).await;
    first.fire(Request::QueueAppend {
        songs: vec![mineral_test::song("from-a")],
        context: QueueContextWire::Manual,
    });
    second.fire(Request::QueueAppend {
        songs: vec![mineral_test::song("from-b")],
        context: QueueContextWire::Manual,
    });
    wait_until("两个 client 的追加都可见", || {
        let queue = first.mirror().read_player(|player| {
            player
                .queue()
                .iter()
                .map(|s| s.id.qualified())
                .collect::<Vec<_>>()
        });
        queue.iter().any(|id| id.ends_with("from-a"))
            && queue.iter().any(|id| id.ends_with("from-b"))
    })
    .await?;
    wait_until("第二个 client 收敛到同一队列", || {
        let a = first.mirror().read_player(|player| player.versions().queue);
        let b = second
            .mirror()
            .read_player(|player| player.versions().queue);
        a == b
    })
    .await?;
    Ok(())
}

/// 大队列经真实 socket 分片下发:镜像只在收齐后整体更替。
#[tokio::test]
async fn player_queue_fragments_over_socket() -> color_eyre::Result<()> {
    let daemon = Daemon::spawn("fragment", None)?;
    daemon.wait_ready()?;
    let client = daemon.connect("fragment").await?;
    client.subscribe(SubscriptionTopic::Player);
    let outcome = client
        .play_queue(songs(600), /*target*/ 0, QueueContextWire::Manual)?
        .outcome()
        .await;
    assert!(outcome.is_success());
    wait_until("600 首队列整体到位", || {
        client.mirror().read_player(|player| player.queue().len()) == 600
    })
    .await?;
    Ok(())
}

/// 500 首队列整组追加或插播 1000 首，经真实 socket 后完整保序，重叠歌曲不去重。
#[tokio::test]
async fn large_queue_batches_arrive_complete_and_in_order() -> color_eyre::Result<()> {
    let daemon = Daemon::spawn("queue-batch", None)?;
    daemon.wait_ready()?;
    let client = daemon.connect("queue-batch").await?;
    client.subscribe(SubscriptionTopic::Player);
    let original = songs(500);
    let batch = songs(1000);
    for insert_next in [false, true] {
        let outcome = client
            .play_queue(
                original.clone(),
                /*target*/ 200,
                QueueContextWire::Manual,
            )?
            .outcome()
            .await;
        assert!(outcome.is_success(), "建立原队列应成功: {outcome:?}");
        wait_until("原队列 500 首已同步", || {
            client.mirror().read_player(|player| player.queue().len()) == 500
        })
        .await?;
        let request = if insert_next {
            Request::QueueInsertNext {
                songs: batch.clone(),
                context: QueueContextWire::Manual,
            }
        } else {
            Request::QueueAppend {
                songs: batch.clone(),
                context: QueueContextWire::Manual,
            }
        };
        client.fire(request);
        wait_until("批量入队后应有 1500 首", || {
            client.mirror().read_player(|player| player.queue().len()) == 1500
        })
        .await?;
        let expected = if insert_next {
            original
                .iter()
                .take(201)
                .chain(batch.iter())
                .chain(original.iter().skip(201))
                .map(|song| song.id.clone())
                .collect::<Vec<_>>()
        } else {
            original
                .iter()
                .chain(batch.iter())
                .map(|song| song.id.clone())
                .collect()
        };
        client.mirror().read_player(|player| {
            assert_eq!(
                player
                    .queue()
                    .iter()
                    .map(|song| song.id.clone())
                    .collect::<Vec<_>>(),
                expected,
                "插播={insert_next}: 所有歌曲、顺序和重复项都应保留"
            );
            assert_eq!(player.cursor(), mineral_protocol::PlayCursor::InQueue(200));
        });
    }
    Ok(())
}

/// 播放锚点订阅推送音量变化;停止态位置不自行推进。
#[tokio::test]
async fn playback_subscription_pushes_anchor() -> color_eyre::Result<()> {
    let daemon = Daemon::spawn("playback", None)?;
    daemon.wait_ready()?;
    let client = daemon.connect("playback").await?;
    client.subscribe(SubscriptionTopic::Playback);
    wait_until("首个播放锚点", || {
        client.playback_snapshot().volume_pct > 0
    })
    .await?;
    client.fire(Request::SetVolume(42));
    wait_until("音量推送到位", || {
        client.playback_snapshot().volume_pct == 42
    })
    .await?;
    let anchor = client.playback_snapshot();
    assert!(!anchor.playing, "null 后端未起播时应为停止态");
    assert_eq!(
        client.playback_position_ms(),
        anchor.position_ms,
        "停止态位置不应本地推进"
    );
    Ok(())
}

/// `m` 键(cycle play mode)每一步都推送到 client。
///
/// 覆盖两种不动队列版本的切换:两个非 Shuffle 档之间,以及空队列进 Shuffle
/// (洗牌提前返回)。这两种情况都等不到队列变更顺带唤醒。
#[tokio::test]
async fn play_mode_cycle_pushes_every_step() -> color_eyre::Result<()> {
    let daemon = Daemon::spawn("play-mode", None)?;
    daemon.wait_ready()?;
    let client = daemon.connect("play-mode").await?;
    client.subscribe(SubscriptionTopic::Player);

    // 空队列:进 Shuffle 不洗牌(不动版本),仍须推送。
    client.fire(Request::CyclePlayMode);
    wait_until("空队列下模式推送", || {
        client.mirror().read_player(PlayerMirror::play_mode) == PlayMode::Shuffle
    })
    .await?;

    // 队列非空后走完剩下三档,其中两次只在两个非 Shuffle 档之间切换。
    let outcome = client
        .play_queue(songs(3), /*target*/ 0, QueueContextWire::Manual)?
        .outcome()
        .await;
    assert!(outcome.is_success(), "建立队列应成功: {outcome:?}");
    wait_until("队列同步", || {
        client.mirror().read_player(|player| player.queue().len()) == 3
    })
    .await?;

    for expected in [
        PlayMode::RepeatAll,
        PlayMode::RepeatOne,
        PlayMode::Sequential,
    ] {
        client.fire(Request::CyclePlayMode);
        let label = format!("模式推送到位 {expected:?}");
        wait_until(&label, || {
            client.mirror().read_player(PlayerMirror::play_mode) == expected
        })
        .await?;
    }
    Ok(())
}

/// 直设播放模式经 IPC 生效;设成当前同档是 no-op(队列版本不动,不重洗)。
#[tokio::test]
async fn set_play_mode_applies_directly() -> color_eyre::Result<()> {
    let daemon = Daemon::spawn("set-play-mode", None)?;
    daemon.wait_ready()?;
    let client = daemon.connect("set-play-mode").await?;
    client.subscribe(SubscriptionTopic::Player);

    let outcome = client
        .play_queue(songs(3), /*target*/ 0, QueueContextWire::Manual)?
        .outcome()
        .await;
    assert!(outcome.is_success(), "建立队列应成功: {outcome:?}");
    wait_until("队列同步", || {
        client.mirror().read_player(|player| player.queue().len()) == 3
    })
    .await?;

    let outcome = client.set_play_mode(PlayMode::Shuffle).await;
    assert!(outcome.is_success(), "直设 Shuffle 应成功: {outcome:?}");
    wait_until("模式推送到位", || {
        client.mirror().read_player(PlayerMirror::play_mode) == PlayMode::Shuffle
    })
    .await?;

    // 同档再设一次:不重洗队列,队列版本保持不动。用一条随后到达的查询定序,
    // 不靠 sleep 断言「没有发生」。
    let queue_version = client
        .mirror()
        .read_player(|player| player.versions().queue);
    let outcome = client.set_play_mode(PlayMode::Shuffle).await;
    assert!(outcome.is_success(), "重复设置应成功: {outcome:?}");
    let _ = client.daemon_info().await;
    assert_eq!(
        client
            .mirror()
            .read_player(|player| player.versions().queue),
        queue_version,
        "同档直设不应重洗队列"
    );

    // 换到非 Shuffle 档(退出洗牌恢复原序)同样推送。
    let outcome = client.set_play_mode(PlayMode::RepeatAll).await;
    assert!(outcome.is_success(), "直设 RepeatAll 应成功: {outcome:?}");
    wait_until("换档推送到位", || {
        client.mirror().read_player(PlayerMirror::play_mode) == PlayMode::RepeatAll
    })
    .await?;
    Ok(())
}

/// 任务摘要订阅:订阅即得当前快照(变更推送由 daemon 的 change-only watch 承担)。
#[tokio::test]
async fn tasks_subscription_delivers_snapshot() -> color_eyre::Result<()> {
    let daemon = Daemon::spawn("tasks", None)?;
    daemon.wait_ready()?;
    let client = daemon.connect("tasks").await?;
    client.subscribe(SubscriptionTopic::Tasks);
    wait_until("首个任务摘要", || client.tasks_snapshot().is_some()).await?;
    // 提交一批任务:会话不被摘要订阅阻塞,任务事件照常回流。
    for index in 0..20 {
        client.fire(Request::SubmitTask(
            TaskKind::ChannelFetch(ChannelFetchKind::Lyrics {
                song_id: SongId::new(SourceKind::MINERAL, format!("missing{index}")),
            }),
            Priority::Background,
        ));
    }
    assert!(client.connected(), "提交任务不应影响会话");
    Ok(())
}

/// 订阅重放业务能力;daemon.lua 重载后推送具名队列操作与播放计数可用性。
#[tokio::test]
async fn service_info_replays_and_changes_after_daemon_reload() -> color_eyre::Result<()> {
    let daemon = Daemon::spawn(
        "service-info",
        Some(
            r#"return {
                queue = { transforms = {
                    { name = "Keep order", transform = function(queue) return queue end },
                } },
                stats = { level = "off" },
            }"#,
        ),
    )?;
    daemon.wait_ready()?;
    let client = daemon.connect("service-info").await?;
    let initial = client
        .service_info()
        .await
        .into_success()
        .ok_or_else(|| eyre!("service info query failed"))?;
    assert_eq!(initial.queue_transforms, vec!["Keep order"]);
    assert!(!initial.play_counts.enabled);
    client.subscribe(SubscriptionTopic::Events(Subscription::ServiceInfo));
    wait_until("订阅重放当前能力", || {
        client.mirror().drain_events().into_iter().any(|event| {
            matches!(event, mineral_protocol::Event::ServiceInfoChanged { info }
                if info.queue_transforms == initial.queue_transforms
                    && info.play_counts.enabled == initial.play_counts.enabled)
        })
    })
    .await?;
    daemon.write_daemon(
        r#"return {
            queue = { transforms = {
                { name = "Reverse queue", transform = function(queue)
                    local out = {}
                    for i = #queue, 1, -1 do out[#out + 1] = queue[i] end
                    return out
                end },
            } },
            stats = { level = "core", exclude_sources = { "bilibili" } },
        }"#,
    )?;
    wait_until("业务能力变更推送", || {
        client.mirror().drain_events().into_iter().any(|event| {
            matches!(event, mineral_protocol::Event::ServiceInfoChanged { info }
                if info.queue_transforms == ["Reverse queue"]
                    && info.play_counts.enabled
                    && info.play_counts.excluded_sources == ["bilibili"])
        })
    })
    .await?;
    let current = client
        .service_info()
        .await
        .into_success()
        .ok_or_else(|| eyre!("reloaded service info query failed"))?;
    assert_eq!(current.queue_transforms, vec!["Reverse queue"]);
    assert!(current.play_counts.enabled);
    assert_eq!(current.play_counts.excluded_sources, vec!["bilibili"]);
    Ok(())
}

/// A real daemon reads and plays local media through ordinary channel requests; reconnects reuse it.
#[tokio::test]
async fn local_library_load_playback_and_reconnect() -> color_eyre::Result<()> {
    /// Read the local playlist identity from the public library snapshot.
    async fn local_playlist_id(client: &Client) -> color_eyre::Result<mineral_model::PlaylistId> {
        client.fire(Request::SubmitTask(
            TaskKind::ChannelFetch(ChannelFetchKind::MyPlaylists {
                source: SourceKind::LOCAL,
            }),
            Priority::User,
        ));
        let mut found = None;
        wait_until("local library snapshot", || {
            for event in client.mirror().drain_events() {
                if let mineral_protocol::Event::Task(event) = event
                    && let mineral_task::TaskEvent::LibrarySnapshot { playlists } = *event
                {
                    found = playlists
                        .into_iter()
                        .find(|playlist| playlist.source() == SourceKind::LOCAL)
                        .map(|playlist| playlist.id);
                }
            }
            found.is_some()
        })
        .await?;
        found.ok_or_else(|| eyre!("missing local playlist in library snapshot"))
    }
    let music = tempfile::tempdir()?;
    mineral_test::write_wav(
        &music.path().join("first.wav"),
        &vec![100; 480_000],
        1,
        48_000,
    )?;
    let root = music.path().canonicalize()?;
    let root_lua = serde_json::to_string(&root)?;
    let config =
        format!("return {{ sources = {{ [\"local\"] = {{ roots = {{ {root_lua} }} }} }} }}");
    let daemon = Daemon::spawn("local-library", Some(&config))?;
    daemon.wait_ready()?;
    let client = daemon.connect("local-library").await?;
    client.subscribe(SubscriptionTopic::Player);
    client.subscribe(SubscriptionTopic::Events(Subscription::Task));
    client.subscribe(SubscriptionTopic::Events(Subscription::Lifecycle));
    let playlist_id = local_playlist_id(&client).await?;
    client.fire(Request::SubmitTask(
        TaskKind::ChannelFetch(ChannelFetchKind::PlaylistDetail {
            id: playlist_id.clone(),
            load: mineral_channel_core::PlaylistLoad::Complete,
        }),
        Priority::User,
    ));
    let mut selected = None;
    wait_until("local playlist detail", || {
        for event in client.mirror().drain_events() {
            if let mineral_protocol::Event::Task(event) = event
                && let mineral_task::TaskEvent::PlaylistDetailFetched { id, detail, .. } = *event
                && id == playlist_id
            {
                selected = detail
                    .playlist
                    .entries
                    .first()
                    .map(|entry| entry.song.clone());
            }
        }
        selected.is_some()
    })
    .await?;
    let song = selected.ok_or_else(|| eyre!("missing local song"))?;
    assert!(
        client
            .toggle_love(song.clone())?
            .outcome()
            .await
            .into_success()
            .is_some_and(|loved| loved)
    );
    assert!(
        client
            .play_queue(vec![song.clone()], 0, QueueContextWire::Manual)?
            .outcome()
            .await
            .is_success()
    );
    wait_until("direct local playback and facts", || {
        client.mirror().read_player(|player| {
            player.play_origin() == Some(mineral_protocol::PlaybackOrigin::Remote)
                && player.current().is_some_and(|current| {
                    current
                        .media_info
                        .as_ref()
                        .is_some_and(|info| info.format == Some(mineral_model::AudioFormat::Wav))
                })
        })
    })
    .await?;
    // Files added after the first load do not appear merely because a client reconnects.
    mineral_test::write_wav(&root.join("later.wav"), &[100; 4_800], 1, 48_000)?;
    let other = daemon.connect("local-library-reconnect").await?;
    other.subscribe(SubscriptionTopic::Player);
    wait_until("reconnect retains local identity", || {
        other.mirror().read_player(|player| {
            player
                .queue()
                .first()
                .is_some_and(|queued| queued.id == song.id)
        })
    })
    .await?;
    other.subscribe(SubscriptionTopic::Events(
        mineral_protocol::Subscription::Task,
    ));
    // 重连后公开库列表仍解析到同一本地歌单 identity。
    let reconnected_id = local_playlist_id(&other).await?;
    assert_eq!(
        reconnected_id, playlist_id,
        "重连后库列表应给出同一本地歌单 id"
    );
    other.fire(Request::SubmitTask(
        TaskKind::ChannelFetch(ChannelFetchKind::PlaylistDetail {
            id: reconnected_id,
            load: mineral_channel_core::PlaylistLoad::Complete,
        }),
        Priority::User,
    ));
    let mut reconnected_songs = None;
    wait_until("reconnect reads the same local catalog", || {
        for event in other.mirror().drain_events() {
            if let mineral_protocol::Event::Task(event) = event
                && let mineral_task::TaskEvent::PlaylistDetailFetched { id, detail, .. } = *event
                && id == playlist_id
            {
                reconnected_songs = Some(detail.playlist.entries);
            }
        }
        reconnected_songs.is_some()
    })
    .await?;
    let reconnected_songs = reconnected_songs.ok_or_else(|| eyre!("missing local playlist"))?;
    assert_eq!(reconnected_songs.len(), 1);
    assert_eq!(
        reconnected_songs.first().map(|entry| &entry.song.id),
        Some(&song.id)
    );
    assert!(client.stop().await.is_success());
    Ok(())
}
