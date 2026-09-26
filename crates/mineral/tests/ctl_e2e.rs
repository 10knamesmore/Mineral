//! `mineral ctl` 进程级回归:真 daemon + 真 `mineral` 二进制,验证命令结论、退出码与 JSON 契约。
//!
//! 音频走 `MINERAL_AUDIO_NULL` 降级;每个用例隔离一套 XDG 目录与独立 socket 目录,与
//! `session_e2e` / `daemon_lifecycle` 同属 `daemon-e2e` 串行组。
//!
//! daemon 侧状态由并行的 `mineral_client::Client` 读真值,CLI 的结论只作对照——两边合起来才
//! 能证明「命令确实落到了 daemon 上」。

use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use color_eyre::eyre::{WrapErr, bail};
use mineral_client::Client;
use mineral_client::connection::ClientConfig;
use mineral_client::state::PlayerMirror;
use mineral_model::Song;
use mineral_protocol::{PlayMode, QueueContextWire, SocketWire, SubscriptionTopic};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use serde_json::Value;
use tokio::time::timeout;

/// 订阅推送 / 命令收敛的通用等待上限。
const WAIT: Duration = Duration::from_secs(10);

/// 预埋配置:一个唯一 label、一组同名 label 与一条反转变换(用来看队列真的被改了)。
const CONFIG_WITH_TRANSFORMS: &str = r#"
return {
  queue = {
    transforms = {
      { label = "reverse", transform = function(queue)
          local out = {}
          for i = #queue, 1, -1 do out[#out + 1] = queue[i] end
          return out
        end },
      { label = "dup", transform = function(queue) return queue end },
      { label = "dup", transform = function(queue) return queue end },
      { label = "bogus", transform = function(queue)
          return { { id = "netease:not-in-queue" } }
        end },
      { label = "keep-selected", transform = function(queue, ctx)
          if not ctx.selected then return queue end
          return { queue[ctx.selected] }
        end },
    },
  },
}
"#;

/// 一次 `mineral ctl` 的产出。
struct CtlRun {
    /// 进程退出状态。
    status: ExitStatus,

    /// stdout 全文(`--json` 时应恰好一行)。
    stdout: String,

    /// stderr 全文。
    stderr: String,
}

impl CtlRun {
    /// stdout 里唯一一行 JSON。
    fn json(&self) -> color_eyre::Result<Value> {
        let lines = self
            .stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<&str>>();
        let Some(line) = lines.first().filter(|_| lines.len() == 1) else {
            bail!(
                "`--json` 应恰好一行,实际 {} 行:{}",
                lines.len(),
                self.stdout
            );
        };
        serde_json::from_str(line).wrap_err("解析 JSON 结论")
    }
}

/// 取 JSON 的字符串字段。
///
/// # Params:
///   - `json`: 结论信封
///   - `key`: 字段名
fn field<'a>(json: &'a Value, key: &str) -> color_eyre::Result<&'a str> {
    json.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| color_eyre::eyre::eyre!("结论缺字符串字段 {key}:{json}"))
}

/// 取 JSON 的数字字段。
///
/// # Params:
///   - `json`: 结论信封
///   - `key`: 字段名
fn number(json: &Value, key: &str) -> color_eyre::Result<u64> {
    json.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| color_eyre::eyre::eyre!("结论缺数字字段 {key}:{json}"))
}

/// 隔离环境里的 daemon 子进程;Drop 时 kill 并清临时目录。
struct Harness {
    /// `mineral serve` 子进程。
    child: Child,

    /// 隔离用的临时根目录(XDG 配置 / 数据 / 缓存都指到这下面)。
    root: PathBuf,

    /// socket 目录(`MINERAL_SOCKET_DIR`,刻意短,压在 `sun_path` 内)。
    sock_dir: PathBuf,

    /// daemon socket 路径。
    socket: PathBuf,
}

impl Harness {
    /// 起一个隔离环境、null 音频后端的 daemon;`config_lua` 为 `Some` 时预埋。
    ///
    /// # Params:
    ///   - `tag`: 临时目录名里的测试标识
    ///   - `config_lua`: 预埋的用户 config.lua
    fn spawn(tag: &str, config_lua: Option<&str>) -> color_eyre::Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "mineral-ctl-e2e-{}-{}-{}",
            tag,
            std::process::id(),
            unique_suffix()
        ));
        let sock_dir =
            std::env::temp_dir().join(format!("mnlc-{}-{}", std::process::id(), unique_suffix()));
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

    /// 让 daemon 优雅收尾(SIGTERM,与 `mineral stop` 同一条路)并等 socket 消失。
    fn stop_daemon(&mut self) -> color_eyre::Result<()> {
        let pid = i32::try_from(self.child.id()).wrap_err("daemon pid 超出 i32")?;
        kill(Pid::from_raw(pid), Signal::SIGTERM).wrap_err("send SIGTERM to daemon")?;
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            if !self.socket.exists() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        bail!("daemon did not exit in time")
    }

    /// 在同一个隔离环境里跑一条 `mineral ctl` 命令(与 daemon 同一套 XDG / socket)。
    ///
    /// # Params:
    ///   - `args`: `ctl` 之后的参数
    fn ctl(&self, args: &[&str]) -> color_eyre::Result<CtlRun> {
        let output = Command::new(env!("CARGO_BIN_EXE_mineral"))
            .arg("ctl")
            .args(args)
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("MINERAL_SOCKET_DIR", &self.sock_dir)
            .env("MINERAL_AUDIO_NULL", "1")
            .stdin(Stdio::null())
            .output()
            .wrap_err("run `mineral ctl`")?;
        Ok(CtlRun {
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// 连一条新会话,用于读 daemon 真值(CLI 命令不改动这条会话)。
    ///
    /// # Params:
    ///   - `name`: client 自报名
    async fn connect(&self, name: &str) -> color_eyre::Result<Client> {
        let wire = SocketWire::connect(&self.socket).await?;
        Client::from_wire(Box::new(wire), name, ClientConfig::cli())
            .await
            .map_err(color_eyre::Report::new)
    }
}

impl Drop for Harness {
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
        .map(|index| mineral_test::song(&format!("ctl{index}")))
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
    .map_err(|_elapsed| color_eyre::eyre::eyre!("等待超时:{label}"))
}

/// 当前队列的歌曲 id 序列。
fn queue_ids(client: &Client) -> Vec<String> {
    client.mirror().read_player(|player| {
        player
            .queue()
            .iter()
            .map(|song| song.id.to_string())
            .collect::<Vec<String>>()
    })
}

/// 起一条 3 首的队列并等镜像同步。
async fn start_queue(client: &Client) -> color_eyre::Result<()> {
    let outcome = client
        .play_queue(songs(3), /*target*/ 0, QueueContextWire::Manual)?
        .outcome()
        .await;
    assert!(outcome.is_success(), "起播应成功: {outcome:?}");
    wait_until("队列同步", || queue_ids(client).len() == 3).await
}

/// 不需读状态的命令:结论 applied、退出 0、一行 JSON、命令专属字段落位;参数错误退出 2。
#[tokio::test(flavor = "multi_thread")]
async fn transport_reports_and_exit_codes() -> color_eyre::Result<()> {
    let h = Harness::spawn("transport", None)?;
    h.wait_ready()?;

    for args in [
        vec!["--json", "pause"],
        vec!["--json", "resume"],
        vec!["--json", "stop"],
        vec!["--json", "next"],
        vec!["--json", "prev"],
        vec!["--json", "mode"],
        vec!["--json", "seek", "1:30"],
        vec!["--json", "volume", "40"],
    ] {
        let run = h.ctl(&args)?;
        assert_eq!(
            run.status.code(),
            Some(0),
            "`ctl {args:?}` 应成功;stderr={}",
            run.stderr
        );
        let json = run.json()?;
        assert_eq!(field(&json, "outcome")?, "applied", "`ctl {args:?}`");
    }

    // 命令专属字段跟在核心字段之后,值就是解析后的目标。
    let seek = h.ctl(&["--json", "seek", "1:30"])?.json()?;
    assert_eq!(field(&seek, "command")?, "seek");
    assert_eq!(number(&seek, "position_ms")?, 90_000);
    let volume = h.ctl(&["--json", "volume", "40"])?.json()?;
    assert_eq!(field(&volume, "command")?, "volume");
    assert_eq!(number(&volume, "volume_pct")?, 40);

    // 人读模式仍使用相同退出码，并把成功输出送到 stdout。
    let human = h.ctl(&["pause"])?;
    assert_eq!(human.status.code(), Some(0));
    assert!(!human.stdout.trim().is_empty());
    assert!(human.stderr.trim().is_empty());

    // 参数错误由 clap 拦截:退出 2,不产出结论。
    for args in [
        vec!["--json", "volume", "101"],
        vec!["--json", "mode", "bogus"],
        vec!["--json", "seek", "1:99"],
    ] {
        let run = h.ctl(&args)?;
        assert_eq!(
            run.status.code(),
            Some(2),
            "`ctl {args:?}` 应报参数错误;stderr={}",
            run.stderr
        );
        assert!(
            run.stdout.trim().is_empty(),
            "参数错误不产出结论:{}",
            run.stdout
        );
    }
    Ok(())
}

/// 播放模式直设经 CLI 落到 daemon:镜像里的档位真的变了。
#[tokio::test(flavor = "multi_thread")]
async fn mode_reaches_daemon() -> color_eyre::Result<()> {
    let h = Harness::spawn("mode", None)?;
    h.wait_ready()?;
    let client = h.connect("ctl-mode").await?;
    client.subscribe(SubscriptionTopic::Player);
    client.wait_subscriptions_ready(WAIT).await;

    let run = h.ctl(&["--json", "mode", "shuffle"])?;
    assert_eq!(run.status.code(), Some(0), "stderr={}", run.stderr);
    assert_eq!(field(&run.json()?, "command")?, "mode");
    wait_until("档位变 Shuffle", || {
        client.mirror().read_player(PlayerMirror::play_mode) == PlayMode::Shuffle
    })
    .await?;

    let run = h.ctl(&["--json", "mode"])?;
    assert_eq!(run.status.code(), Some(0), "stderr={}", run.stderr);
    wait_until("循环到 RepeatAll", || {
        client.mirror().read_player(PlayerMirror::play_mode) == PlayMode::RepeatAll
    })
    .await?;
    Ok(())
}

/// 读播放镜像的三条命令:无当前曲跳过、相对音量按镜像增减、相对跳在时长未知时跳过。
#[tokio::test(flavor = "multi_thread")]
async fn mirror_backed_commands() -> color_eyre::Result<()> {
    let h = Harness::spawn("mirror", None)?;
    h.wait_ready()?;
    let client = h.connect("ctl-mirror").await?;
    client.subscribe(SubscriptionTopic::Player);
    client.subscribe(SubscriptionTopic::Playback);
    client.wait_subscriptions_ready(WAIT).await;

    // 没有当前曲:跳过且退出 0,不产生请求。
    let run = h.ctl(&["--json", "play-pause"])?;
    assert_eq!(run.status.code(), Some(0));
    let json = run.json()?;
    assert_eq!(field(&json, "outcome")?, "skipped");
    assert_eq!(field(&json, "reason")?, "no-current-track");

    // 有当前曲后不再跳过(null 后端不出声,方向由镜像的 playing 决定)。
    start_queue(&client).await?;
    wait_until("当前曲到位", || {
        client
            .mirror()
            .read_player(|player| player.current().is_some_and(|c| c.current_song.is_some()))
    })
    .await?;
    let run = h.ctl(&["--json", "play-pause"])?;
    assert_eq!(run.status.code(), Some(0), "stderr={}", run.stderr);
    assert_eq!(field(&run.json()?, "outcome")?, "applied");

    // 相对音量:按镜像当前值增减,结果钳在 0..=100 且能被 daemon 真值观察到。
    let run = h.ctl(&["--json", "volume", "40"])?;
    assert_eq!(number(&run.json()?, "volume_pct")?, 40);
    wait_until("音量 40", || client.playback_snapshot().volume_pct == 40).await?;
    let run = h.ctl(&["--json", "volume", "+5"])?;
    assert_eq!(number(&run.json()?, "volume_pct")?, 45);
    wait_until("音量 45", || client.playback_snapshot().volume_pct == 45).await?;
    let run = h.ctl(&["--json", "volume", "-200"])?;
    assert_eq!(number(&run.json()?, "volume_pct")?, 0);
    wait_until("音量 0", || client.playback_snapshot().volume_pct == 0).await?;

    // 相对跳:fixture 曲目没有真实媒体,时长未知 → 跳过而不是瞎钳。
    let run = h.ctl(&["--json", "seek", "+10s"])?;
    assert_eq!(run.status.code(), Some(0));
    let json = run.json()?;
    assert_eq!(field(&json, "outcome")?, "skipped");
    assert_eq!(field(&json, "reason")?, "duration-unknown");
    Ok(())
}

/// 队列命令:唯一 label 的变换真的改到队列、可撤销;同名 / 未注册明确失败且不动队列。
#[tokio::test(flavor = "multi_thread")]
async fn queue_commands() -> color_eyre::Result<()> {
    let h = Harness::spawn("queue", Some(CONFIG_WITH_TRANSFORMS))?;
    h.wait_ready()?;
    let client = h.connect("ctl-queue").await?;
    client.subscribe(SubscriptionTopic::Player);
    client.wait_subscriptions_ready(WAIT).await;
    start_queue(&client).await?;
    let original = queue_ids(&client);

    // 唯一 label:队列真的被反转,结论 applied。
    let run = h.ctl(&["--json", "queue", "transform", "reverse"])?;
    assert_eq!(run.status.code(), Some(0), "stderr={}", run.stderr);
    let json = run.json()?;
    assert_eq!(field(&json, "command")?, "queue transform");
    assert_eq!(field(&json, "outcome")?, "applied");
    let mut reversed = original.clone();
    reversed.reverse();
    wait_until("队列反转", || queue_ids(&client) == reversed).await?;

    // 撤销回到原序。
    let run = h.ctl(&["--json", "queue", "undo"])?;
    assert_eq!(field(&run.json()?, "outcome")?, "applied");
    wait_until("撤销回原序", || queue_ids(&client) == original).await?;

    // 没有历史再撤销:跳过而不是报错。
    let run = h.ctl(&["--json", "queue", "undo"])?;
    assert_eq!(run.status.code(), Some(0));
    let json = run.json()?;
    assert_eq!(field(&json, "outcome")?, "skipped");
    assert_eq!(field(&json, "reason")?, "no-change");

    // `--at` 落到脚本的 `ctx.selected`(wire 0-based → Lua 1-based):保留第三首。
    let run = h.ctl(&["--json", "queue", "transform", "keep-selected", "--at", "2"])?;
    assert_eq!(run.status.code(), Some(0), "stderr={}", run.stderr);
    assert_eq!(field(&run.json()?, "outcome")?, "applied");
    let expected = original
        .get(2)
        .map(|id| vec![id.clone()])
        .ok_or_else(|| color_eyre::eyre::eyre!("原队列不足三首:{original:?}"))?;
    wait_until("只留第三首", || queue_ids(&client) == expected).await?;
    let run = h.ctl(&["--json", "queue", "undo"])?;
    assert_eq!(field(&run.json()?, "outcome")?, "applied");
    wait_until("撤销回原序", || queue_ids(&client) == original).await?;

    // 变换返回队列外的 id:daemon 拒整次变换,CLI 报 failed(stale) 且队列不动。
    let run = h.ctl(&["--json", "queue", "transform", "bogus"])?;
    assert_eq!(run.status.code(), Some(1), "stderr={}", run.stderr);
    assert_eq!(field(&run.json()?, "kind")?, "stale");
    assert_eq!(queue_ids(&client), original, "stale 不该动队列");

    // 同名 label → ambiguous;未注册 → unknown;两者都被拒且不动队列。
    let run = h.ctl(&["--json", "queue", "transform", "dup"])?;
    assert_eq!(run.status.code(), Some(1), "stderr={}", run.stderr);
    assert_eq!(field(&run.json()?, "kind")?, "ambiguous-transform");
    let run = h.ctl(&["--json", "queue", "transform", "nope"])?;
    assert_eq!(run.status.code(), Some(1), "stderr={}", run.stderr);
    assert_eq!(field(&run.json()?, "kind")?, "unknown-transform");
    assert_eq!(queue_ids(&client), original, "失败不该动队列");
    Ok(())
}

/// daemon 不在跑:结论 unknown、退出 3、人读走 stderr。
#[tokio::test(flavor = "multi_thread")]
async fn missing_daemon_is_unknown() -> color_eyre::Result<()> {
    let mut h = Harness::spawn("no-daemon", None)?;
    h.wait_ready()?;
    h.stop_daemon()?;

    let run = h.ctl(&["--json", "pause"])?;
    assert_eq!(run.status.code(), Some(3), "stderr={}", run.stderr);
    let json = run.json()?;
    assert_eq!(field(&json, "command")?, "pause");
    assert_eq!(field(&json, "outcome")?, "unknown");
    assert!(!field(&json, "detail")?.is_empty());

    let human = h.ctl(&["pause"])?;
    assert_eq!(human.status.code(), Some(3));
    assert!(
        human.stdout.trim().is_empty(),
        "没执行不写 stdout:{}",
        human.stdout
    );
    assert!(
        !human.stderr.trim().is_empty(),
        "人读结论走 stderr:{}",
        human.stderr
    );
    Ok(())
}
