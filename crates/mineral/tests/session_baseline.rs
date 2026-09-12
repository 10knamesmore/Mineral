//! 会话协议性能采样:真 daemon + 真 client,量化控制往返、队列同步、多 client 并发与合批。
//!
//! `#[ignore]`:手动运行,输出 JSON 到
//! `specs/26-09-07-client-daemon-sessions/artifacts/baseline-ipc-new-protocol.json`。
//! 控制往返与各 client 请求记录延迟分布和吞吐,队列同步计时覆盖请求提交至镜像可见。
//! 采样规模见报告的 `scale` 字段,实际合批量见 `batch_probe`;跨次比较应使用同一机器与构建 profile。
//! 采样命令:
//! `cargo nextest run -p mineral --release --run-ignored ignored-only -E 'test(session_baseline)' --no-capture`

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use color_eyre::eyre::WrapErr;
use mineral_client::Client;
use mineral_client::connection::ClientConfig;
use mineral_client::operation::Outcome;
use mineral_model::Song;
use mineral_protocol::{QueueContextWire, SocketWire, SubscriptionTopic};
use serde::Serialize;

/// 控制往返样本数(顺序等待,交替 pause / resume)。
const CONTROL_SAMPLES: usize = 20_000;

/// 合批探针提交目标(撞上在途上限即止,实际入队数以探针实测为准)。
const BATCH_PROBE_REQUESTS: usize = 4_096;

/// 多 client 并发规模。
const MULTI_CLIENTS: usize = 32;

/// 每 client 的请求数。
const PER_CLIENT_REQUESTS: usize = 1_000;

/// 小队列端到端同步的行数。
const QUEUE_ROWS_SMALL: usize = 2_000;

/// 大队列端到端同步的行数。
const QUEUE_ROWS_LARGE: usize = 9_999;

/// `usize` → `f64`(样本上限在 u32 范围内)。
fn usize_f64(value: usize) -> f64 {
    f64::from(u32::try_from(value).unwrap_or(u32::MAX))
}

/// 从已排序耗时序列算全套统计(ms)。
#[derive(Serialize)]
struct Stats {
    /// 样本数。
    count: usize,

    /// 最小。
    min_ms: f64,

    /// 中位数。
    p50_ms: f64,

    /// 95 分位。
    p95_ms: f64,

    /// 99 分位。
    p99_ms: f64,

    /// 最大。
    max_ms: f64,

    /// 均值。
    mean_ms: f64,

    /// 标准差。
    stddev_ms: f64,
}

/// 从已排序耗时序列算全套统计。
fn summarize(sorted: &[f64]) -> Stats {
    let count = sorted.len();
    let count_f64 = usize_f64(count);
    let sum = sorted.iter().sum::<f64>();
    let mean = if count == 0 { 0.0 } else { sum / count_f64 };
    let pick = |permille: usize| -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        let index = (count.saturating_mul(permille) / 1000).min(count.saturating_sub(1));
        sorted.get(index).copied().unwrap_or(0.0)
    };
    let squared = sorted
        .iter()
        .map(|sample| {
            let delta = sample - mean;
            delta * delta
        })
        .sum::<f64>();
    let variance = if count == 0 { 0.0 } else { squared / count_f64 };
    Stats {
        count,
        min_ms: sorted.first().copied().unwrap_or(0.0),
        p50_ms: pick(500),
        p95_ms: pick(950),
        p99_ms: pick(990),
        max_ms: sorted.last().copied().unwrap_or(0.0),
        mean_ms: mean,
        stddev_ms: variance.sqrt(),
    }
}

/// 一段计时采样的统计与吞吐。
#[derive(Serialize)]
struct Segment {
    /// 延迟统计。
    stats: Stats,

    /// 吞吐(笔/秒)。
    ops_per_sec: f64,
}

/// 队列端到端同步计时。
#[derive(Serialize)]
struct QueueSync {
    /// 2000 首队列从请求提交到镜像可见的耗时(ms)。
    rows_2000_ms: f64,

    /// 9999 首队列从请求提交到镜像可见的耗时(ms)。
    rows_9999_ms: f64,
}

/// 多 client 并发结果。
#[derive(Serialize)]
struct MultiClient {
    /// client 数。
    clients: usize,

    /// 每 client 请求数。
    per_client_requests: usize,

    /// 总 wall time。
    wall_ms: f64,

    /// 总吞吐(笔/秒)。
    ops_per_sec: f64,

    /// 每 client 的完整统计。
    per_client: Vec<Stats>,
}

/// 会话级计数器。
#[derive(Serialize)]
struct SessionCounters {
    /// 已发送批次数。
    batches_sent: u64,

    /// 已发送请求数。
    requests_sent: u64,

    /// 平均批大小(requests / batches)。
    avg_batch_size: f64,

    /// 因容量丢弃的更新数。
    updates_dropped: u64,
}

/// 合批探针结果。
#[derive(Serialize)]
struct BatchProbe {
    /// 探针请求数。
    requests: u64,

    /// 探针期间实际发送的批数。
    batches: u64,

    /// 平均批大小。
    avg_batch_size: f64,
}

/// 采样结果。
#[derive(Serialize)]
struct Report {
    /// 协议标识。
    protocol: String,

    /// 采样规模说明。
    scale: String,

    /// 订阅是否启用(真实负载)。
    subscribed: bool,

    /// 控制往返(2 万次,交替 pause / resume;顺序等待)。
    control_round_trip: Segment,

    /// 队列端到端同步。
    queue_sync: QueueSync,

    /// 多 client 并发 daemon_info。
    multi_client: MultiClient,

    /// 合批探针(持续提交至撞在途上限)。
    batch_probe: BatchProbe,

    /// 主会话累计计数器。
    metrics: SessionCounters,
}

/// 隔离环境里的 daemon。
struct Daemon {
    /// 子进程。
    child: Child,
    /// 隔离根目录(随进程清理)。
    root: PathBuf,
    /// socket 目录(随进程清理)。
    sock_dir: PathBuf,
    /// socket 路径。
    socket: PathBuf,
}

impl Daemon {
    /// 起 daemon(null 音频)。
    fn spawn() -> color_eyre::Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "mineral-session-baseline-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        let sock_dir = std::env::temp_dir().join(format!(
            "mnlb-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        std::fs::create_dir_all(&root).wrap_err("create root")?;
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
            .wrap_err("spawn daemon")?;
        let socket = sock_dir.join("mineral.sock");
        Ok(Self {
            child,
            root,
            sock_dir,
            socket,
        })
    }

    /// 等 socket 可连。
    fn wait_ready(&self) -> color_eyre::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if std::os::unix::net::UnixStream::connect(&self.socket).is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(color_eyre::eyre::eyre!("daemon not ready"))
    }

    /// 连一条会话。
    async fn connect(&self, name: &str) -> color_eyre::Result<Client> {
        let wire = SocketWire::connect(&self.socket).await?;
        Client::from_wire(Box::new(wire), name, ClientConfig::default())
            .await
            .map_err(color_eyre::Report::new)
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

/// 造 `len` 首测试歌曲。
fn songs(len: usize) -> Vec<Song> {
    (0..len)
        .map(|index| mineral_test::song(&format!("base{index}")))
        .collect()
}

/// 队列端到端同步:play_queue 到镜像可见。
async fn queue_sync_ms(client: &Client, rows: usize) -> color_eyre::Result<f64> {
    let start = Instant::now();
    let outcome = client
        .play_queue(songs(rows), /*target*/ 0, QueueContextWire::Manual)?
        .outcome()
        .await;
    match outcome {
        Outcome::Applied(()) => {}
        other => {
            return Err(color_eyre::eyre::eyre!(
                "play_queue({rows}) 未应用:{other:?}(业务失败或断连)"
            ));
        }
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while client.mirror().read_player(|player| player.queue().len()) != rows {
        if Instant::now() >= deadline {
            let metrics = client.metrics();
            let seen = client.mirror().read_player(|player| player.queue().len());
            let versions = client
                .mirror()
                .read_player(mineral_client::state::PlayerMirror::versions);
            return Err(color_eyre::eyre::eyre!(
                "{rows} 首队列未在期限内可见:镜像队列 {seen},版本 {versions:?},丢弃更新 {},\n批 {} 消息 {}",
                metrics.updates_dropped,
                metrics.batches_sent,
                metrics.messages_sent
            ));
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    Ok(start.elapsed().as_secs_f64() * 1_000.0)
}

/// 在隔离 daemon 上采样会话性能并写入 JSON 报告。
#[tokio::test]
#[ignore = "采样基线时手动跑,见文件头命令"]
async fn session_baseline() -> color_eyre::Result<()> {
    let daemon = Daemon::spawn()?;
    daemon.wait_ready()?;
    let client = daemon.connect("baseline").await?;
    client.subscribe(SubscriptionTopic::Player);
    client.subscribe(SubscriptionTopic::Playback);
    client.subscribe(SubscriptionTopic::Tasks);
    client.subscribe(SubscriptionTopic::DownloadsSummary);

    // 保持状态订阅开启,测量交替暂停 / 恢复的顺序请求延迟。
    let mut control = Vec::with_capacity(CONTROL_SAMPLES);
    let control_wall_start = Instant::now();
    for index in 0..CONTROL_SAMPLES {
        let request = if index % 2 == 0 {
            mineral_protocol::Request::Pause
        } else {
            mineral_protocol::Request::Resume
        };
        let start = Instant::now();
        let outcome = client
            .submit(request, |_result, _request_name| Outcome::Applied(()))?
            .outcome()
            .await;
        assert!(matches!(outcome, Outcome::Applied(())));
        control.push(start.elapsed().as_secs_f64() * 1_000.0);
    }
    let control_wall = control_wall_start.elapsed().as_secs_f64();
    control.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));

    let rows_2000_ms = queue_sync_ms(&client, QUEUE_ROWS_SMALL).await?;
    let rows_9999_ms = queue_sync_ms(&client, QUEUE_ROWS_LARGE).await?;

    // 连续提交以观察合批;达到提交目标或本地容量上限即止,按实际入队量计算批大小。
    let metrics_before = client.metrics();
    let mut pending = Vec::new();
    let mut submitted = 0_u64;
    loop {
        let handle = client.submit(
            mineral_protocol::Request::DaemonInfo,
            |_result, _request_name| Outcome::Applied(()),
        );
        match handle {
            Ok(handle) => {
                pending.push(handle);
                submitted = submitted.saturating_add(1);
                if submitted >= u64::try_from(BATCH_PROBE_REQUESTS).unwrap_or(u64::MAX) {
                    break;
                }
            }
            Err(mineral_client::operation::SubmitError::InFlightLimit) => break,
            Err(other) => {
                return Err(color_eyre::eyre::eyre!("合批探针提交失败: {other}"));
            }
        }
    }
    for handle in pending {
        assert!(matches!(handle.outcome().await, Outcome::Applied(_)));
    }
    let metrics_after = client.metrics();
    let probe_requests = metrics_after
        .requests_sent
        .saturating_sub(metrics_before.requests_sent);
    let probe_batches = metrics_after
        .batches_sent
        .saturating_sub(metrics_before.batches_sent);

    // 各 client 并发查询 daemon_info,分别记录延迟分布。
    let wall_start = Instant::now();
    let mut tasks = Vec::new();
    for index in 0..MULTI_CLIENTS {
        let socket = daemon.socket.clone();
        tasks.push(tokio::spawn(async move {
            let wire = SocketWire::connect(&socket).await?;
            let client = Client::from_wire(
                Box::new(wire),
                &format!("baseline-{index}"),
                ClientConfig::default(),
            )
            .await
            .map_err(color_eyre::Report::new)?;
            let mut samples = Vec::with_capacity(PER_CLIENT_REQUESTS);
            for _ in 0..PER_CLIENT_REQUESTS {
                let start = Instant::now();
                let outcome = client.daemon_info().await;
                assert!(matches!(outcome, Outcome::Applied(_)));
                samples.push(start.elapsed().as_secs_f64() * 1_000.0);
            }
            samples.sort_by(|left, right| {
                left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
            });
            Ok::<Stats, color_eyre::Report>(summarize(&samples))
        }));
    }
    let mut per_client = Vec::new();
    for task in tasks {
        per_client.push(task.await??);
    }
    let multi_wall = wall_start.elapsed().as_secs_f64();

    // 聚合报告。
    let total_multi_ops = MULTI_CLIENTS.saturating_mul(PER_CLIENT_REQUESTS);
    let final_metrics = client.metrics();
    let avg_batch = if final_metrics.batches_sent == 0 {
        0.0
    } else {
        usize_f64(usize::try_from(final_metrics.requests_sent).unwrap_or(usize::MAX))
            / usize_f64(usize::try_from(final_metrics.batches_sent).unwrap_or(usize::MAX))
    };
    let probe_avg_batch = if probe_batches == 0 {
        0.0
    } else {
        usize_f64(usize::try_from(probe_requests).unwrap_or(usize::MAX))
            / usize_f64(usize::try_from(probe_batches).unwrap_or(usize::MAX))
    };
    let report = Report {
        protocol: "session MessageBatch/OperationResult".to_owned(),
        scale: format!(
            "control={CONTROL_SAMPLES}, queue={QUEUE_ROWS_SMALL}/{QUEUE_ROWS_LARGE}, \
             multi={MULTI_CLIENTS}x{PER_CLIENT_REQUESTS}, probe={BATCH_PROBE_REQUESTS}"
        ),
        subscribed: true,
        control_round_trip: Segment {
            stats: summarize(&control),
            ops_per_sec: usize_f64(CONTROL_SAMPLES) / control_wall,
        },
        queue_sync: QueueSync {
            rows_2000_ms,
            rows_9999_ms,
        },
        multi_client: MultiClient {
            clients: MULTI_CLIENTS,
            per_client_requests: PER_CLIENT_REQUESTS,
            wall_ms: multi_wall * 1_000.0,
            ops_per_sec: usize_f64(total_multi_ops) / multi_wall,
            per_client,
        },
        batch_probe: BatchProbe {
            requests: probe_requests,
            batches: probe_batches,
            avg_batch_size: probe_avg_batch,
        },
        metrics: SessionCounters {
            batches_sent: final_metrics.batches_sent,
            requests_sent: final_metrics.requests_sent,
            avg_batch_size: avg_batch,
            updates_dropped: final_metrics.updates_dropped,
        },
    };
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "../../specs/26-09-07-client-daemon-sessions/artifacts/baseline-ipc-new-protocol.json",
    );
    std::fs::write(&path, serde_json::to_string_pretty(&report)?).wrap_err("write report")?;
    println!("wrote {}", path.display());
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
