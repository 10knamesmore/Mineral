//! 会话协议进程级 e2e:真 `mineral serve` 子进程 + 真 `mineral_client::Client`,
//! 验证操作提交顺序、查询归属、订阅推送与配置热更在完整链路上的行为。
//!
//! 音频走 `MINERAL_AUDIO_NULL` 降级,headless 稳跑;每个测试隔离一套 XDG 目录与
//! 独立 socket 目录,与 `daemon_lifecycle` / `script_hooks` 同属 `daemon-e2e` 串行组。

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use color_eyre::eyre::{WrapErr, bail, eyre};
use mineral_client::Client;
use mineral_client::connection::ClientConfig;
use mineral_client::operation::Outcome;
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
    /// 起一个隔离环境、null 音频后端的 daemon;`config_lua` 为 `Some` 时预埋。
    ///
    /// # Params:
    ///   - `tag`: 临时目录名里的测试标识
    ///   - `config_lua`: 预埋的用户 config.lua(须以 `return {}` 结尾)
    fn spawn(tag: &str, config_lua: Option<&str>) -> color_eyre::Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "mineral-session-e2e-{}-{}-{}",
            tag,
            std::process::id(),
            unique_suffix()
        ));
        let sock_dir =
            std::env::temp_dir().join(format!("mnls-{}-{}", std::process::id(), unique_suffix()));
        std::fs::create_dir_all(&root).wrap_err("create isolated root dir")?;
        if let Some(src) = config_lua {
            let cfg_dir = root.join("config/mineral");
            std::fs::create_dir_all(&cfg_dir).wrap_err("create config dir")?;
            std::fs::write(cfg_dir.join("config.lua"), src).wrap_err("seed config.lua")?;
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
        bail!("daemon did not become ready in time")
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

    /// 重写用户 config.lua(热更测试用)。
    ///
    /// # Params:
    ///   - `content`: 新的完整脚本内容
    fn write_config(&self, content: &str) -> color_eyre::Result<()> {
        let path = self.root.join("config/mineral/config.lua");
        std::fs::write(path, content).wrap_err("rewrite config.lua")
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

/// 慢脚本动作在脚本线程执行,不阻塞同一会话的控制查询。
#[tokio::test]
async fn slow_action_does_not_block_control() -> color_eyre::Result<()> {
    let daemon = Daemon::spawn(
        "slow",
        Some(
            r#"
            mineral.action("e2e.slow", function(ctx) os.execute("sleep 0.6") end)
            return {}
            "#,
        ),
    )?;
    daemon.wait_ready()?;
    let client = daemon.connect("slow").await?;
    let slow = client.invoke_action("e2e.slow", /*ctx*/ None, Vec::new());
    tokio::pin!(slow);
    // 给 daemon 一点时间真正开始执行慢动作。
    tokio::time::sleep(Duration::from_millis(100)).await;
    let started = Instant::now();
    let pid = client.daemon_info().await;
    assert!(
        started.elapsed() < Duration::from_millis(400),
        "控制查询不应被慢脚本阻塞,实际 {:?}",
        started.elapsed()
    );
    assert!(matches!(pid, Outcome::Applied(_)), "daemon_info 应成功");
    assert!(slow.await.is_success(), "慢动作本身仍应完成");
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
        client.mirror().read_player(|player| player.play_mode()) == PlayMode::Shuffle
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
            client.mirror().read_player(|player| player.play_mode()) == expected
        })
        .await?;
    }
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

/// 配置热更:改写 config.lua 后 daemon 推新配置事件。
#[tokio::test]
async fn config_change_is_pushed() -> color_eyre::Result<()> {
    let daemon = Daemon::spawn("config", Some("return {}\n"))?;
    daemon.wait_ready()?;
    let client = daemon.connect("config").await?;
    client.subscribe(SubscriptionTopic::Events(Subscription::Config));
    // 订阅重放当前配置:首帧 ConfigChanged 到达事件流。
    let mut seen = 0_usize;
    wait_until("订阅重放当前配置", || {
        seen += config_changes(&client);
        seen > 0
    })
    .await?;
    daemon.write_config("return { tui = { animation = { frame_tick_ms = 33 } } }\n")?;
    wait_until("配置变更推送", || {
        seen += config_changes(&client);
        seen > 1
    })
    .await?;
    Ok(())
}

/// 取走事件流里的 `ConfigChanged` 数量(其余事件一并消费,本用例只关心配置推送)。
///
/// # Params:
///   - `client`: 已订阅配置事件的会话 client
fn config_changes(client: &mineral_client::Client) -> usize {
    let mut count = 0;
    for event in client.mirror().drain_events() {
        match event {
            mineral_protocol::Event::ConfigChanged { .. } => count += 1,
            mineral_protocol::Event::Toast { .. }
            | mineral_protocol::Event::Card { .. }
            | mineral_protocol::Event::PropertyChanged { .. }
            | mineral_protocol::Event::TrackFinished { .. }
            | mineral_protocol::Event::DownloadCompleted { .. }
            | mineral_protocol::Event::StoreChanged { .. }
            | mineral_protocol::Event::ScriptReloaded
            | mineral_protocol::Event::BusMessage { .. }
            | mineral_protocol::Event::WindowTitleOverride { .. }
            | mineral_protocol::Event::DismissToast { .. }
            | mineral_protocol::Event::Task(_) => {}
        }
    }
    count
}
