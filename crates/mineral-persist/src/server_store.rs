//! ServerStore 句柄与后端。

use std::sync::Arc;

use color_eyre::eyre::WrapErr;
use mineral_log::{info, warn};
use mineral_model::{Song, SongId, SourceKind};
use rustc_hash::FxHashMap;
use sqlx::SqlitePool;

use crate::CacheIndex;
use crate::db::rows::{SongArtistRow, SongMetaRow};
use crate::db::schema::ensure_schema;
use crate::db::{NamespaceStore, SessionStore};

/// 持久化服务句柄。廉价 clone(内部 `Arc`)。
///
/// 打开失败时降级为 [`ServerStore::disabled`]:所有写静默成功、所有读返回空,
/// 调用方(channel / server)无需特判,播放照常。
#[derive(Clone)]
pub struct ServerStore {
    /// 内部后端(真实 sqlite 或降级 null)。
    backend: Arc<Backend>,
}

/// 歌单缓存计数(只读统计 / 清理回执)。只读返回 DTO,字段全 `pub`。
pub struct PlaylistCacheStats {
    /// `playlist_cache` 行数(缓存的歌单数)。
    pub playlists: u64,

    /// `playlist_tracks` 行数(总曲目行数)。
    pub tracks: u64,
}

/// 内部后端:真实 sqlite 或降级 null。
enum Backend {
    /// 真实 sqlite 连接池。
    Sqlite(SqlitePool),

    /// 降级:写丢弃、读空。
    Disabled,
}

impl ServerStore {
    /// 打开(或创建)数据库文件并建表。
    ///
    /// # Params:
    ///   - `db_path`: sqlite 文件路径(不存在则创建)
    ///
    /// # Return:
    ///   成功返回启用的句柄;失败返回 `Err`(调用方可改用 [`Self::disabled`] 降级)。
    pub async fn open(db_path: &std::path::Path) -> color_eyre::Result<Self> {
        info!(target: "persist", path = %db_path.display(), "打开 server 数据库");
        let pool = crate::pool::connect(db_path).await?;
        ensure_schema(&pool).await?;
        Ok(Self {
            backend: Arc::new(Backend::Sqlite(pool)),
        })
    }

    /// 降级句柄:不落盘、读空、写丢弃。
    ///
    /// # Return:
    ///   一个永远成功但无副作用的 [`ServerStore`]。
    pub fn disabled() -> Self {
        warn!(target: "persist", "持久化降级为 no-op(disabled)");
        Self {
            backend: Arc::new(Backend::Disabled),
        }
    }

    /// 取连接池(降级时为 `None`)。
    ///
    /// # Return:
    ///   启用时为底层连接池,降级时为 `None`。
    pub(crate) fn pool(&self) -> Option<&SqlitePool> {
        match self.backend.as_ref() {
            Backend::Sqlite(p) => Some(p),
            Backend::Disabled => None,
        }
    }

    /// 取某来源命名空间下的存储视图。
    ///
    /// # Params:
    ///   - `source`: 来源标识(决定 namespace 过滤)
    ///
    /// # Return:
    ///   绑定该 namespace 的 [`NamespaceStore`]。
    pub fn scope(&self, source: SourceKind) -> NamespaceStore {
        NamespaceStore::new(self.clone(), source)
    }

    /// 取全局会话存储。
    ///
    /// # Return:
    ///   [`SessionStore`]。
    pub fn session(&self) -> SessionStore {
        SessionStore::new(self.clone())
    }

    /// 音频本体缓存索引(`audio_cache` 表,LRU 驱逐)。播放命中本地副本走它。
    ///
    /// # Params:
    ///   - `root`: 缓存文件根目录(`relpath` 相对它)
    ///   - `capacity`: 容量上限字节(LRU 满了驱逐最旧)
    ///
    /// # Return:
    ///   就绪索引;降级句柄返回 [`CacheIndex::disabled`];建表 / 载入失败返回 `Err`。
    pub async fn audio_cache(
        &self,
        root: std::path::PathBuf,
        capacity: u64,
    ) -> color_eyre::Result<CacheIndex> {
        match self.pool() {
            Some(pool) => CacheIndex::open(pool.clone(), "audio_cache", root, Some(capacity)).await,
            None => Ok(CacheIndex::disabled()),
        }
    }

    /// 全部源的 loved 歌曲(join meta 重建),按 `loved_at` 降序(最新收藏在顶),
    /// 同毫秒收藏以 `(namespace, song_value)` 破平局,顺序稳定不随库文件重排。
    ///
    /// loved 但缺 meta 的行**跳过**——聚合视图与其曲目计数保持同口径,不出现「有行但
    /// 没名字」的占位。缺 meta 是常态:sync 导入的远端红心先只有 id,meta 随浏览补全。
    /// 降级句柄返回空集。
    ///
    /// # Return:
    ///   跨 namespace 的收藏 `Vec<Song>`。
    pub async fn loved_songs(&self) -> color_eyre::Result<Vec<Song>> {
        let Some(pool) = self.pool() else {
            return Ok(Vec::new());
        };
        let meta_rows = sqlx::query_as::<_, SongMetaRow>(
            "SELECT m.namespace, m.song_value, m.name, m.alias, m.album_id, m.album_name, \
             m.duration_ms, m.cover_url \
             FROM song_stats st \
             JOIN song_meta m ON m.namespace = st.namespace AND m.song_value = st.song_value \
             WHERE st.loved_at IS NOT NULL \
             ORDER BY st.loved_at DESC, st.namespace, st.song_value",
        )
        .fetch_all(pool)
        .await
        .wrap_err("查跨源 loved 元数据失败")?;

        let artist_rows: Vec<(String, String, String, String)> = sqlx::query_as(
            "SELECT a.namespace, a.song_value, a.artist_id, a.artist_name \
             FROM song_stats st \
             JOIN song_artists a ON a.namespace = st.namespace AND a.song_value = st.song_value \
             WHERE st.loved_at IS NOT NULL \
             ORDER BY a.position",
        )
        .fetch_all(pool)
        .await
        .wrap_err("查跨源 loved 艺人失败")?;
        let mut artists_by_song = FxHashMap::<(String, String), Vec<SongArtistRow>>::default();
        for (namespace, song_value, artist_id, artist_name) in artist_rows {
            artists_by_song
                .entry((namespace, song_value))
                .or_default()
                .push(SongArtistRow {
                    artist_id,
                    artist_name,
                });
        }

        meta_rows
            .into_iter()
            .map(|row| {
                let key = (row.namespace.clone(), row.song_value.clone());
                row.into_song(artists_by_song.remove(&key).unwrap_or_default())
            })
            .collect()
    }

    /// 跨源 loved 歌曲计数,与 [`Self::loved_songs`] **严格同口径**(只计 join 到 meta 的
    /// 收藏,缺 meta 的行不计)——聚合歌单列表面只要计数时走它,免为拿个数字重建整个
    /// `Vec<Song>`。降级句柄返回 0。
    ///
    /// # Return:
    ///   有 meta 的跨源收藏数。
    pub async fn loved_count(&self) -> color_eyre::Result<u64> {
        let Some(pool) = self.pool() else {
            return Ok(0);
        };
        let (count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM song_stats st \
             JOIN song_meta m ON m.namespace = st.namespace AND m.song_value = st.song_value \
             WHERE st.loved_at IS NOT NULL",
        )
        .fetch_one(pool)
        .await
        .wrap_err("统计跨源 loved 计数失败")?;
        u64::try_from(count).wrap_err("loved 计数转 u64 失败")
    }

    /// 全部源里 loved 但**缺 meta** 的歌 id(sync 导入的远端红心先只有 id、无 meta)。
    /// 供后台补 meta 任务拉详情回填;补齐后它们就进 [`Self::loved_songs`] 的聚合视图。
    /// 降级句柄返回空。
    ///
    /// # Return:
    ///   跨 namespace 的缺 meta 收藏 id(namespace 从行内 `SourceKind::from_name` 还原)。
    pub async fn missing_meta_loved_ids(&self) -> color_eyre::Result<Vec<SongId>> {
        let Some(pool) = self.pool() else {
            return Ok(Vec::new());
        };
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT st.namespace, st.song_value FROM song_stats st \
             LEFT JOIN song_meta m ON m.namespace = st.namespace AND m.song_value = st.song_value \
             WHERE st.loved_at IS NOT NULL AND m.song_value IS NULL",
        )
        .fetch_all(pool)
        .await
        .wrap_err("查缺 meta 的 loved 行失败")?;
        Ok(rows
            .into_iter()
            .map(|(namespace, value)| SongId::new(SourceKind::from_name(&namespace), value))
            .collect())
    }

    /// 歌单缓存计数(只读)。供 CLI `cache status` 展示用。
    ///
    /// # Return:
    ///   启用态返回 `playlist_cache` / `playlist_tracks` 行数;降级句柄返回全 0。
    pub async fn playlist_cache_stats(&self) -> color_eyre::Result<PlaylistCacheStats> {
        let Some(pool) = self.pool() else {
            return Ok(PlaylistCacheStats {
                playlists: 0,
                tracks: 0,
            });
        };
        let (playlists,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM playlist_cache")
            .fetch_one(pool)
            .await
            .wrap_err("统计 playlist_cache 行数失败")?;
        let (tracks,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM playlist_tracks")
            .fetch_one(pool)
            .await
            .wrap_err("统计 playlist_tracks 行数失败")?;
        Ok(PlaylistCacheStats {
            playlists: u64::try_from(playlists).unwrap_or(0),
            tracks: u64::try_from(tracks).unwrap_or(0),
        })
    }

    /// 清空歌单缓存(`playlist_cache` + `playlist_tracks` 全部来源)。
    ///
    /// 只清可重建的歌单缓存，**不动**播放统计 / love / 历史 / 会话 / song_meta。
    /// 降级句柄下 no-op。
    ///
    /// # Return:
    ///   清理成功返回被清掉的计数(清理前 `playlist_cache` / `playlist_tracks` 行数);降级返回全 0。
    pub async fn clear_playlist_caches(&self) -> color_eyre::Result<PlaylistCacheStats> {
        let Some(pool) = self.pool() else {
            return Ok(PlaylistCacheStats {
                playlists: 0,
                tracks: 0,
            });
        };
        info!(target: "persist", "清理歌单缓存");
        // 两表清理包进事务,避免中途失败留下半清状态(tracks 清了 cache 没清)。
        // 先在同一事务里 COUNT 出清理前计数作为回执,再 DELETE,保证回执与实际删除一致。
        let mut tx = pool
            .begin()
            .await
            .wrap_err("开启 clear_playlist_caches 事务失败")?;
        let (playlists,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM playlist_cache")
            .fetch_one(&mut *tx)
            .await
            .wrap_err("统计 playlist_cache 行数失败")?;
        let (tracks,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM playlist_tracks")
            .fetch_one(&mut *tx)
            .await
            .wrap_err("统计 playlist_tracks 行数失败")?;
        sqlx::query("DELETE FROM playlist_tracks")
            .execute(&mut *tx)
            .await
            .wrap_err("清空 playlist_tracks 失败")?;
        sqlx::query("DELETE FROM playlist_cache")
            .execute(&mut *tx)
            .await
            .wrap_err("清空 playlist_cache 失败")?;
        tx.commit()
            .await
            .wrap_err("提交 clear_playlist_caches 事务失败")?;
        Ok(PlaylistCacheStats {
            playlists: u64::try_from(playlists).unwrap_or(0),
            tracks: u64::try_from(tracks).unwrap_or(0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ServerStore;

    #[tokio::test]
    async fn open_creates_db_and_schema() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = ServerStore::open(&dir.path().join("t.db")).await?;
        assert!(p.pool().is_some());
        Ok(())
    }

    #[test]
    fn disabled_has_no_pool() {
        assert!(ServerStore::disabled().pool().is_none());
    }

    /// 跨源聚合:两源各一首 loved(带 meta)按 loved_at 降序返回;
    /// 无 meta 的 loved 跳过;未 loved 的 meta 不出现。
    #[tokio::test]
    async fn loved_songs_aggregates_across_namespaces() -> color_eyre::Result<()> {
        use mineral_model::{SongId, SourceKind};
        use mineral_test::{song, with_artist, with_name};

        let dir = tempfile::tempdir()?;
        let p = ServerStore::open(&dir.path().join("t.db")).await?;
        let netease = p.scope(SourceKind::NETEASE);
        let bilibili = p.scope(SourceKind::BILIBILI);

        // netease:较早收藏;bilibili:较晚收藏(手动定 loved_at,免同毫秒排序不稳)。
        let n1 = with_artist(with_name(song("n1"), "Palisade"), "Mineral");
        netease.upsert_meta(&n1).await?;
        netease.set_loved(&n1.id, true).await?;
        let b1 = {
            let mut s = with_name(song("b1"), "夜間飛行");
            s.id = SongId::new(SourceKind::BILIBILI, "b1");
            s
        };
        bilibili.upsert_meta(&b1).await?;
        bilibili.set_loved(&b1.id, true).await?;
        // 有 meta 但未 loved:不该出现。
        netease
            .upsert_meta(&with_name(song("n2"), "unloved"))
            .await?;
        // loved 但无 meta:跳过。
        netease
            .set_loved(&SongId::new(SourceKind::NETEASE, "ghost"), true)
            .await?;

        let pool = p
            .pool()
            .ok_or_else(|| color_eyre::eyre::eyre!("测试库应有 pool"))?;
        sqlx::query("UPDATE song_stats SET loved_at = 100 WHERE song_value = 'n1'")
            .execute(pool)
            .await?;
        sqlx::query("UPDATE song_stats SET loved_at = 200 WHERE song_value = 'b1'")
            .execute(pool)
            .await?;

        let songs = p.loved_songs().await?;
        let names = songs.iter().map(|s| s.name.as_str()).collect::<Vec<&str>>();
        assert_eq!(
            names,
            vec!["夜間飛行", "Palisade"],
            "按 loved_at 降序,缺 meta 的 ghost 跳过,未 loved 的不出现"
        );
        let first = songs
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有两首"))?;
        assert_eq!(first.source(), SourceKind::BILIBILI, "namespace 还原为原源");
        let second = songs
            .get(1)
            .ok_or_else(|| color_eyre::eyre::eyre!("应有两首"))?;
        assert_eq!(
            second.artists.first().map(|a| a.name.as_str()),
            Some("Mineral"),
            "艺人列表随 meta 还原"
        );
        Ok(())
    }

    /// loved_count 与 loved_songs 同口径:只计 join 到 meta 的收藏(ghost 无 meta 不计,
    /// unloved 的 meta 不计)。
    #[tokio::test]
    async fn loved_count_matches_loved_songs() -> color_eyre::Result<()> {
        use mineral_model::{SongId, SourceKind};
        use mineral_test::{song, with_name};

        let dir = tempfile::tempdir()?;
        let p = ServerStore::open(&dir.path().join("t.db")).await?;
        let netease = p.scope(SourceKind::NETEASE);
        let a = with_name(song("a"), "Alpha");
        netease.upsert_meta(&a).await?;
        netease.set_loved(&a.id, true).await?;
        // 有 meta 未 loved:不计。
        netease
            .upsert_meta(&with_name(song("b"), "unloved"))
            .await?;
        // loved 无 meta(ghost):不计。
        netease
            .set_loved(&SongId::new(SourceKind::NETEASE, "ghost"), true)
            .await?;

        assert_eq!(p.loved_count().await?, 1, "只计有 meta 的收藏");
        assert_eq!(
            p.loved_count().await?,
            u64::try_from(p.loved_songs().await?.len())?,
            "count 与 songs 长度同口径"
        );
        Ok(())
    }

    /// 降级句柄:loved_count 返回 0 不报错。
    #[tokio::test]
    async fn loved_count_disabled_is_zero() -> color_eyre::Result<()> {
        assert_eq!(ServerStore::disabled().loved_count().await?, 0);
        Ok(())
    }

    /// 同毫秒收藏:tiebreaker `(namespace, song_value)` 给出确定顺序,不靠 SQLite 任意 tie 序;
    /// 逆序插入也按 song_value 升序返回。
    #[tokio::test]
    async fn loved_songs_stable_order_on_same_millisecond() -> color_eyre::Result<()> {
        use mineral_model::SourceKind;
        use mineral_test::{song, with_name};

        let dir = tempfile::tempdir()?;
        let p = ServerStore::open(&dir.path().join("t.db")).await?;
        let netease = p.scope(SourceKind::NETEASE);
        // 逆序插入(先 bbb 后 aaa),验证返回顺序由 tiebreaker 而非插入序决定。
        let b = with_name(song("bbb"), "Beta");
        netease.upsert_meta(&b).await?;
        netease.set_loved(&b.id, true).await?;
        let a = with_name(song("aaa"), "Alpha");
        netease.upsert_meta(&a).await?;
        netease.set_loved(&a.id, true).await?;

        let pool = p
            .pool()
            .ok_or_else(|| color_eyre::eyre::eyre!("测试库应有 pool"))?;
        sqlx::query("UPDATE song_stats SET loved_at = 500")
            .execute(pool)
            .await?;

        let songs = p.loved_songs().await?;
        let names = songs.iter().map(|s| s.name.as_str()).collect::<Vec<&str>>();
        assert_eq!(
            names,
            vec!["Alpha", "Beta"],
            "同 loved_at 下按 song_value 升序(aaa 在 bbb 前),与插入序无关"
        );
        Ok(())
    }

    /// missing_meta_loved_ids:只列 loved 且缺 meta 的行(有 meta 的 loved 不列,unloved 不列)。
    #[tokio::test]
    async fn missing_meta_loved_ids_lists_only_meta_less_loved() -> color_eyre::Result<()> {
        use mineral_model::{SongId, SourceKind};
        use mineral_test::{song, with_name};

        let dir = tempfile::tempdir()?;
        let p = ServerStore::open(&dir.path().join("t.db")).await?;
        let netease = p.scope(SourceKind::NETEASE);
        // 有 meta + loved:不列。
        let a = with_name(song("a"), "Alpha");
        netease.upsert_meta(&a).await?;
        netease.set_loved(&a.id, true).await?;
        // loved 无 meta:列。
        let ghost = SongId::new(SourceKind::NETEASE, "ghost");
        netease.set_loved(&ghost, true).await?;
        // 有 meta 未 loved:不列。
        netease
            .upsert_meta(&with_name(song("b"), "unloved"))
            .await?;

        let ids = p.missing_meta_loved_ids().await?;
        assert_eq!(ids, vec![ghost], "只列 loved 且缺 meta 的");
        Ok(())
    }

    /// 降级句柄:loved_songs 返回空集不报错。
    #[tokio::test]
    async fn loved_songs_disabled_is_empty() -> color_eyre::Result<()> {
        assert!(ServerStore::disabled().loved_songs().await?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn clear_playlist_caches_keeps_user_data() -> color_eyre::Result<()> {
        use mineral_model::{PlaylistId, SongId, SourceKind};
        let dir = tempfile::tempdir()?;
        let p = ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        // 写入：歌单缓存(应被清) + 播放统计/love(应保留)
        let pid = PlaylistId::new(SourceKind::NETEASE, "p1");
        let song = SongId::new(SourceKind::NETEASE, "s1");
        s.put_playlist_cache(&pid, Some("歌单"), Some(1), std::slice::from_ref(&song))
            .await?;
        s.record_play(&song, 1000).await?;
        s.set_loved(&song, true).await?;

        // 清理前计数:1 个歌单、1 条曲目。
        let before = p.playlist_cache_stats().await?;
        assert_eq!(before.playlists, 1);
        assert_eq!(before.tracks, 1);

        // 清理回执 = 清理前计数。
        let removed = p.clear_playlist_caches().await?;
        assert_eq!(removed.playlists, 1);
        assert_eq!(removed.tracks, 1);

        // 歌单缓存没了
        assert!(s.get_playlist_cache(&pid).await?.is_none());
        // 清后计数归零
        let after = p.playlist_cache_stats().await?;
        assert_eq!(after.playlists, 0);
        assert_eq!(after.tracks, 0);
        // 但统计 + love 还在
        assert!(s.query_stats(&song).await?.is_some());
        assert!(s.is_loved(&song).await?);
        Ok(())
    }
}
