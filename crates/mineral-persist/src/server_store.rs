//! ServerStore 句柄与后端。

use std::sync::Arc;

use color_eyre::eyre::WrapErr;
use mineral_log::{info, warn};
use mineral_model::SourceKind;
use sqlx::SqlitePool;

use crate::CacheIndex;
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
