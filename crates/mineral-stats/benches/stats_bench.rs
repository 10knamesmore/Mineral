//! stats 热路径与查询压测(criterion;spec §9.1)。
//!
//! 纯 measurement:热路径(gating / 散列 / 落库)在采集侧跑,查询在 CLI / 报告层跑。
//! 种子数据走 [`mineral_stats::fixture`] 的确定性生成器(与聚合正确性测试同源),量级
//! 可调、跨运行可复现。优化仍以实测驱动,这里只提供可复现的量纲基线。
//!
//! 手动触发:`cargo bench -p mineral-stats --features fixture`(fixture 是种子生成器的
//! feature 开关,`[[bench]]` 的 required-features 已声明)。
//!
//! bench 是一次性测量代码,豁免 unwrap / criterion 宏生成的无文档 pub fn;数值强转等其余
//! workspace lint 仍生效。
#![allow(clippy::unwrap_used, missing_docs)]

use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use mineral_stats::{
    BucketBy, Level, ReportOptions, Retention, SearchQueryMode, StatsStore, TopBy, fixture,
    query_hash,
};
use rustc_hash::{FxHashMap, FxHashSet};
use tokio::runtime::Runtime;

/// full 档参数(gating 恒 baseline=true)。
fn params() -> mineral_stats::StatsParams {
    mineral_stats::StatsParams::builder()
        .level(Level::Full)
        .collect(FxHashMap::default())
        .search_queries(SearchQueryMode::Raw)
        .exclude_sources(FxHashSet::default())
        .gap_ms(30 * 60_000)
        .retention(Retention::Forever)
        .build()
}

/// 预填 `plays` 行 + `events` 条的落盘库(查询 bench 的共用夹具,走确定性 fixture)。
fn seeded(rt: &Runtime, plays: i64, events: i64) -> (tempfile::TempDir, StatsStore) {
    rt.block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let store = StatsStore::open(&dir.path().join("bench.db"))
            .await
            .unwrap();
        fixture::seed(&store, plays, events).await.unwrap();
        (dir, store)
    })
}

/// 累加临时目录内所有文件字节数(stats.db + -wal + -shm = 真实磁盘占用)。
fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

/// 报告全套聚合(近似 CLI `report` 一次体感:多维聚合串跑)。
async fn report_suite(store: &StatsStore, opts: &ReportOptions) {
    store.totals(0..i64::MAX).await.unwrap();
    store.distributions(0..i64::MAX).await.unwrap();
    store
        .listen_buckets(0..i64::MAX, BucketBy::Hour)
        .await
        .unwrap();
    store
        .listen_buckets(0..i64::MAX, BucketBy::Weekday)
        .await
        .unwrap();
    store
        .top_songs(0..i64::MAX, TopBy::Plays, opts)
        .await
        .unwrap();
    store.top_contexts(0..i64::MAX, None, 0, 20).await.unwrap();
    store.discoveries(0..i64::MAX, 100).await.unwrap();
    store.endurance(0..i64::MAX).await.unwrap();
    store.event_summary(0..i64::MAX, 10).await.unwrap();
    store.status().await.unwrap();
}

/// 热路径 1:事件 gating(采集侧每条事件先过它)。
fn bench_gating(c: &mut Criterion) {
    let params = params();
    c.bench_function("gating/collects_event", |b| {
        b.iter(|| params.collects_event(std::hint::black_box("searches")));
    });
}

/// 热路径 2:搜索词稳定散列(散列档 / 去重)。
fn bench_query_hash(c: &mut Criterion) {
    c.bench_function("gating/query_hash", |b| {
        b.iter(|| query_hash(std::hint::black_box("周杰伦 稻香 现场")));
    });
}

/// 落库吞吐:单行 record_play / 单条 record_event(actor 串行落库的每单位开销;
/// spec §9.1「落库总耗时」的每行核心,通道 / actor 包裹层近乎零开销)。
fn bench_writes(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (_dir, store, sid) = rt.block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let store = StatsStore::open(&dir.path().join("bench.db"))
            .await
            .unwrap();
        let sid = store.open_session(0).await.unwrap().unwrap();
        (dir, store, sid)
    });
    let counter = AtomicI64::new(0);
    let mut group = c.benchmark_group("write");
    group.bench_function("record_play", |b| {
        b.to_async(&rt).iter(|| async {
            let i = counter.fetch_add(1, Ordering::Relaxed);
            store
                .record_play(&fixture::play_record(i, sid, i * 1_000))
                .await
                .unwrap();
        });
    });
    group.bench_function("record_event", |b| {
        b.to_async(&rt).iter(|| async {
            let i = counter.fetch_add(1, Ordering::Relaxed);
            store
                .record_event(i * 1_000, Some(sid), &fixture::event(i))
                .await
                .unwrap();
        });
    });
    group.finish();
}

/// 每次迭代灌入的行数(throughput bench 按元素数报 elem/s)。
const THROUGHPUT_BATCH: i64 = 1_000;

/// 落库吞吐(elem/s):单写 actor 串行落库的**持续**吞吐上限。
///
/// 采集侧非阻塞 `try_send` 可瞬时吞入(4096 缓冲,受 gating + 散列 + try_send 的纳秒级
/// 开销约束),但持续吞吐由 actor 的落库速率封顶——即此处所测。`record_event` 是纯事件
/// 流;`mixed_play_event` 掺 1/5 播放行,近真实会话负载。这也是 spec §9.1「actor 吞吐 /
/// 落库总耗时」的直读。
fn bench_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (_dir, store, sid) = rt.block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let store = StatsStore::open(&dir.path().join("bench.db"))
            .await
            .unwrap();
        let sid = store.open_session(0).await.unwrap().unwrap();
        (dir, store, sid)
    });
    let counter = AtomicI64::new(0);
    let mut group = c.benchmark_group("throughput");
    group.throughput(Throughput::Elements(
        u64::try_from(THROUGHPUT_BATCH).unwrap(),
    ));
    group.bench_function("record_event", |b| {
        b.to_async(&rt).iter(|| async {
            let base = counter.fetch_add(THROUGHPUT_BATCH, Ordering::Relaxed);
            for k in 0..THROUGHPUT_BATCH {
                let i = base + k;
                store
                    .record_event(i * 1_000, Some(sid), &fixture::event(i))
                    .await
                    .unwrap();
            }
        });
    });
    group.bench_function("mixed_play_event", |b| {
        b.to_async(&rt).iter(|| async {
            let base = counter.fetch_add(THROUGHPUT_BATCH, Ordering::Relaxed);
            for k in 0..THROUGHPUT_BATCH {
                let i = base + k;
                if i % 5 == 0 {
                    store
                        .record_play(&fixture::play_record(i, sid, i * 1_000))
                        .await
                        .unwrap();
                } else {
                    store
                        .record_event(i * 1_000, Some(sid), &fixture::event(i))
                        .await
                        .unwrap();
                }
            }
        });
    });
    group.finish();
}

/// 查询压测(1k plays 基线):totals / top_songs / distributions / status。
fn bench_queries_1k(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (_dir, store) = seeded(&rt, 1_000, 0);
    let opts = ReportOptions::builder()
        .min_listen_ms(0)
        .top_limit(20)
        .build();
    let mut group = c.benchmark_group("query/1k");
    group.bench_function("totals", |b| {
        b.to_async(&rt)
            .iter(|| async { store.totals(0..i64::MAX).await.unwrap() });
    });
    group.bench_function("top_songs", |b| {
        b.to_async(&rt).iter(|| async {
            store
                .top_songs(0..i64::MAX, TopBy::Plays, &opts)
                .await
                .unwrap()
        });
    });
    group.bench_function("distributions", |b| {
        b.to_async(&rt)
            .iter(|| async { store.distributions(0..i64::MAX).await.unwrap() });
    });
    group.bench_function("status", |b| {
        b.to_async(&rt)
            .iter(|| async { store.status().await.unwrap() });
    });
    group.finish();
}

/// 查询压测(年量级:10⁴ plays + 10⁵ events)+ 库体积(spec §9.1)。
///
/// 一次年量级种子既喂查询压测(各单项 + 全套 `report` 体感),又量 stats.db 磁盘占用
/// (验证「一年 < 几十 MB」假设),打印留档。
fn bench_report_year(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (dir, store) = seeded(&rt, 10_000, 100_000);
    let db_bytes = dir_bytes(dir.path());
    eprintln!(
        "[stats bench] 年量级 stats.db 磁盘占用 = {db_bytes} bytes (~{} KiB / ~{} MiB) \
         @ 10k plays + 100k events",
        db_bytes / 1_024,
        db_bytes / 1_048_576,
    );
    let opts = ReportOptions::builder()
        .min_listen_ms(0)
        .top_limit(20)
        .build();
    let mut group = c.benchmark_group("query/year");
    group.bench_function("totals", |b| {
        b.to_async(&rt)
            .iter(|| async { store.totals(0..i64::MAX).await.unwrap() });
    });
    group.bench_function("top_songs", |b| {
        b.to_async(&rt).iter(|| async {
            store
                .top_songs(0..i64::MAX, TopBy::Plays, &opts)
                .await
                .unwrap()
        });
    });
    group.bench_function("distributions", |b| {
        b.to_async(&rt)
            .iter(|| async { store.distributions(0..i64::MAX).await.unwrap() });
    });
    group.bench_function("listen_buckets_hour", |b| {
        b.to_async(&rt).iter(|| async {
            store
                .listen_buckets(0..i64::MAX, BucketBy::Hour)
                .await
                .unwrap()
        });
    });
    group.bench_function("top_contexts", |b| {
        b.to_async(&rt)
            .iter(|| async { store.top_contexts(0..i64::MAX, None, 0, 20).await.unwrap() });
    });
    group.bench_function("recent_plays", |b| {
        b.to_async(&rt)
            .iter(|| async { store.recent_plays(0..i64::MAX, None, 50).await.unwrap() });
    });
    group.bench_function("endurance", |b| {
        b.to_async(&rt)
            .iter(|| async { store.endurance(0..i64::MAX).await.unwrap() });
    });
    group.bench_function("status", |b| {
        b.to_async(&rt)
            .iter(|| async { store.status().await.unwrap() });
    });
    group.bench_function("report_suite", |b| {
        b.to_async(&rt)
            .iter(|| async { report_suite(&store, &opts).await });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_gating,
    bench_query_hash,
    bench_writes,
    bench_throughput,
    bench_queries_1k,
    bench_report_year,
);
criterion_main!(benches);
