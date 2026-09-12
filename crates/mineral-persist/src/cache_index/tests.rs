//! 文件缓存入库、重开、漂移、驱逐与统计契约。

use std::path::Path;

use sea_orm::DatabaseConnection;

use super::CacheTable;
use sea_orm::{ConnectOptions, Database};

use super::{CacheIndex, Evicted};

/// 开一个内存 sqlite 池(每个测试独立)。
async fn mem_pool() -> color_eyre::Result<DatabaseConnection> {
    let mut options = ConnectOptions::new("sqlite::memory:");
    options.max_connections(/*value*/ 1);
    Ok(Database::connect(options).await?)
}

/// 在 `dir` 下造一个内容为 `data` 的源文件(模拟 capture 落盘),返回其路径。
fn make_src(dir: &Path, name: &str, data: &[u8]) -> color_eyre::Result<std::path::PathBuf> {
    let p = dir.join(name);
    std::fs::write(&p, data)?;
    Ok(p)
}

/// record_file 入库 → get 命中可读名;源被移走;内容一致。
#[tokio::test]
async fn record_file_then_get() -> color_eyre::Result<()> {
    let d = tempfile::tempdir()?;
    let root = d.path().join("root");
    let idx = CacheIndex::open(mem_pool().await?, CacheTable::Audio, root, Some(1_000_000)).await?;
    let src = make_src(d.path(), "cap.part", b"AUDIO")?;
    idx.record_file("ne:1:exhigh", &src, "netease/exhigh/专辑", "晴天.mp3")
        .await?;
    let Some(path) = idx.get("ne:1:exhigh") else {
        return Err(color_eyre::eyre::eyre!("入库后应命中"));
    };
    assert!(path.ends_with("netease/exhigh/专辑/晴天.mp3"), "可读库路径");
    assert_eq!(tokio::fs::read(&path).await?, b"AUDIO");
    assert!(!src.exists(), "源应被移走");
    Ok(())
}

/// 写穿透不依赖 Drop flush；从同一 DB 与目录重开实例后仍须命中。
#[tokio::test]
async fn survives_reopen_without_drop() -> color_eyre::Result<()> {
    let d = tempfile::tempdir()?;
    let root = d.path().join("root");
    let pool = mem_pool().await?;
    {
        let idx = CacheIndex::open(
            pool.clone(),
            CacheTable::Audio,
            root.clone(),
            Some(1_000_000),
        )
        .await?;
        let src = make_src(d.path(), "cap.part", b"AUDIO")?;
        idx.record_file("ne:1:exhigh", &src, "netease/exhigh/x", "a.mp3")
            .await?;
        // 不 flush、不 drop 即"重开"(同池模拟进程内换实例;真实是新进程读同文件)。
    }
    let reopened = CacheIndex::open(pool, CacheTable::Audio, root, Some(1_000_000)).await?;
    assert!(
        reopened.get("ne:1:exhigh").is_some(),
        "写穿透后重开应仍命中,无需 Drop flush"
    );
    Ok(())
}

/// 漂移:文件被删 → get miss,且内存项被清。
#[tokio::test]
async fn drift_is_miss() -> color_eyre::Result<()> {
    let d = tempfile::tempdir()?;
    let root = d.path().join("root");
    let idx = CacheIndex::open(mem_pool().await?, CacheTable::Audio, root, Some(1_000_000)).await?;
    let src = make_src(d.path(), "cap.part", b"X")?;
    idx.record_file("k", &src, "sub", "a.mp3").await?;
    let Some(path) = idx.get("k") else {
        return Err(color_eyre::eyre::eyre!("应命中"));
    };
    std::fs::remove_file(&path)?;
    assert!(idx.get("k").is_none(), "文件没了应 miss");
    Ok(())
}

/// LRU 驱逐:超容量删最旧(触碰过的留下)。
#[tokio::test]
async fn evicts_lru_over_capacity() -> color_eyre::Result<()> {
    let d = tempfile::tempdir()?;
    let root = d.path().join("root");
    // 容量 10 字节。
    let idx = CacheIndex::open(mem_pool().await?, CacheTable::Audio, root, Some(10)).await?;
    idx.record_file("a", &make_src(d.path(), "a", b"12345")?, "s", "a.bin")
        .await?; // 5
    idx.record_file("b", &make_src(d.path(), "b", b"12345")?, "s", "b.bin")
        .await?; // 共 10
    let _ = idx.get("a"); // 触碰 a → b 变最旧
    idx.record_file("c", &make_src(d.path(), "c", b"123")?, "s", "c.bin")
        .await?; // +3 超 → 驱逐最旧 b
    assert!(idx.get("a").is_some());
    assert!(idx.get("b").is_none(), "最旧 b 应被驱逐");
    assert!(idx.get("c").is_some());
    Ok(())
}

/// record_file 返回本次 LRU 驱逐掉的记录(供 cache_evictions 埋点):驱逐 b(5 字节)。
#[tokio::test]
async fn record_file_returns_evicted_entries() -> color_eyre::Result<()> {
    let d = tempfile::tempdir()?;
    let root = d.path().join("root");
    let idx = CacheIndex::open(mem_pool().await?, CacheTable::Audio, root, Some(10)).await?;
    let first = idx
        .record_file("a", &make_src(d.path(), "a", b"12345")?, "s", "a.bin")
        .await?;
    assert!(first.is_empty(), "首入未超容量,无驱逐");
    idx.record_file("b", &make_src(d.path(), "b", b"12345")?, "s", "b.bin")
        .await?; // 共 10
    let _ = idx.get("a"); // 触碰 a → b 变最旧
    let evicted = idx
        .record_file("c", &make_src(d.path(), "c", b"123")?, "s", "c.bin")
        .await?; // +3 超 → 驱逐最旧 b
    assert_eq!(
        evicted,
        vec![Evicted {
            key: "b".to_owned(),
            bytes: 5
        }],
        "应返回被驱逐的 b(5 字节)"
    );
    Ok(())
}

/// 撞名:同落点被别的 key 占 → ` (N)` 去重,两项独立可取。
#[tokio::test]
async fn dedups_colliding_name() -> color_eyre::Result<()> {
    let d = tempfile::tempdir()?;
    let root = d.path().join("root");
    let idx = CacheIndex::open(mem_pool().await?, CacheTable::Audio, root, Some(1_000_000)).await?;
    idx.record_file("k1", &make_src(d.path(), "a", b"one")?, "s", "T.mp3")
        .await?;
    idx.record_file("k2", &make_src(d.path(), "b", b"two")?, "s", "T.mp3")
        .await?;
    let (Some(p1), Some(p2)) = (idx.get("k1"), idx.get("k2")) else {
        return Err(color_eyre::eyre::eyre!("两项都应命中"));
    };
    assert_ne!(p1, p2, "撞名应落不同文件");
    assert_eq!(p2.file_name().and_then(|s| s.to_str()), Some("T (2).mp3"));
    Ok(())
}

/// record(文件已置)+ 不驱逐(capacity None,永久副本语义)。
#[tokio::test]
async fn record_path_no_evict() -> color_eyre::Result<()> {
    let d = tempfile::tempdir()?;
    let root = d.path().join("root");
    std::fs::create_dir_all(root.join("sub"))?;
    tokio::fs::write(root.join("sub/x.flac"), b"BIGFLAC").await?;
    let idx = CacheIndex::open(mem_pool().await?, CacheTable::Audio, root, None).await?;
    idx.record("ne:1:lossless", "sub/x.flac", 7).await?;
    assert!(idx.get("ne:1:lossless").is_some());
    Ok(())
}

/// 降级实例:get 恒空、写 no-op 且不报错。
#[tokio::test]
async fn disabled_is_null_object() -> color_eyre::Result<()> {
    let idx = CacheIndex::disabled();
    assert!(idx.get("k").is_none());
    idx.forget("k");
    idx.record("k", "x", 0).await?;
    assert!(idx.get("k").is_none());
    Ok(())
}

/// clear:删文件 + 清表 + 清镜像;返回清理前的内容快照(条目 + 释放字节)。
#[tokio::test]
async fn clear_removes_everything() -> color_eyre::Result<()> {
    let d = tempfile::tempdir()?;
    let root = d.path().join("root");
    let idx = CacheIndex::open(mem_pool().await?, CacheTable::Audio, root, Some(1_000_000)).await?;
    idx.record_file("k", &make_src(d.path(), "a", b"hello")?, "s", "a.bin")
        .await?;
    let removed = idx.clear().await?;
    assert_eq!(removed.entries.len(), 1, "应回执 1 条被清条目");
    assert_eq!(removed.total_bytes, 5, "释放字节 = 文件大小");
    assert_eq!(
        removed.entries.first().map(|e| e.relpath.as_str()),
        Some("s/a.bin")
    );
    assert!(idx.get("k").is_none());
    assert_eq!(idx.snapshot().total_bytes, 0, "清后快照应为空");
    Ok(())
}

/// snapshot:只读返回当前条目 / 总字节 / 容量,且与实际写入吻合。
#[tokio::test]
async fn snapshot_reports_entries_and_capacity() -> color_eyre::Result<()> {
    let d = tempfile::tempdir()?;
    let root = d.path().join("root");
    let idx = CacheIndex::open(mem_pool().await?, CacheTable::Audio, root, Some(1_000_000)).await?;
    idx.record_file("k1", &make_src(d.path(), "a", b"123")?, "s", "a.bin")
        .await?;
    idx.record_file("k2", &make_src(d.path(), "b", b"45")?, "s", "b.bin")
        .await?;
    let snap = idx.snapshot();
    assert_eq!(snap.entries.len(), 2);
    assert_eq!(snap.total_bytes, 5, "3 + 2 字节");
    assert_eq!(snap.capacity, Some(1_000_000));
    assert!(snap.root.is_some());
    Ok(())
}

/// 降级态 snapshot / clear 均返回空快照,不触盘不报错。
#[tokio::test]
async fn disabled_snapshot_is_empty() -> color_eyre::Result<()> {
    let idx = CacheIndex::disabled();
    let snap = idx.snapshot();
    assert!(snap.root.is_none());
    assert!(snap.entries.is_empty());
    assert_eq!(snap.total_bytes, 0);
    assert_eq!(snap.capacity, None);
    let removed = idx.clear().await?;
    assert!(removed.entries.is_empty());
    Ok(())
}
