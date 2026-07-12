//! ClientStore 句柄:客户端(TUI)自己的 sqlite 库(`tui.db`),与 server 的
//! [`ServerStore`](crate::ServerStore) 并列、各占一个库文件。
//!
//! 住封面缓存索引(`cover_cache` 表)、UI 偏好(`ui_prefs` 表,通用 KV)与
//! 歌单内光标位置记忆(`track_pos` 表)。文件本体落
//! [`mineral_paths::cover_cache_dir`] 等可清理目录,索引落 `tui.db`。
//!
//! `ui_prefs.value` 只放**标量字符串**(枚举名等),禁 JSON blob——结构化数据
//! 开专表(如 `track_pos`),与 server 库「规范化、无 JSON 列」同一纪律。

use std::path::{Path, PathBuf};

use color_eyre::eyre::WrapErr;
use mineral_model::{PlaylistId, SongId, SourceKind};
use sqlx::SqlitePool;

use crate::CacheIndex;

/// 歌单内光标位置记忆一行(`track_pos` 表的投影):双锚 + 屏上相对行。
/// 双锚语义(song 优先、index 兜底)由客户端解释,本层只负责结构化存取。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackPosRow {
    /// 歌单 id。
    pub playlist: PlaylistId,

    /// 记录时光标所在歌曲(优先锚)。
    pub song: SongId,

    /// 记录时的行下标(兜底锚)。
    pub index: u64,

    /// 记录时光标在视口内的相对行。
    pub screen_row: u64,
}

/// 客户端持久化句柄。持有 `tui.db` 连接池,按需打开其中的缓存索引表 / 读写 UI 偏好。
pub struct ClientStore {
    /// `tui.db` 连接池。
    pool: SqlitePool,
}

impl ClientStore {
    /// 打开(或创建)客户端库文件,并把 schema 迁移到最新版本。
    ///
    /// # Params:
    ///   - `db_path`: `tui.db` 路径(父目录需已存在;不存在则建文件)
    ///
    /// # Return:
    ///   就绪句柄;连接 / 迁移失败返回 `Err`(调用方应降级,如封面不缓存、偏好不存)。
    pub async fn open(db_path: &Path) -> color_eyre::Result<Self> {
        let pool = crate::pool::connect(db_path).await?;
        Self::with_pool(pool).await
    }

    /// 用现成连接池组装句柄并跑 client 库迁移(测试用内存池入口)。
    ///
    /// 与 server 库同一纪律(每次结构变更新增 `migrations_client/NNNN_*.sql`,
    /// 永不改已发布迁移);唯 `cover_cache` 因表名运行时参数化留在
    /// [`CacheIndex`] 的建表代码里。
    async fn with_pool(pool: SqlitePool) -> color_eyre::Result<Self> {
        /// client 库(tui.db)的版本化迁移,编译期嵌入二进制。
        static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations_client");
        MIGRATOR.run(&pool).await.wrap_err(
            "client 库 schema 迁移失败;若此库建于迁移机制引入之前,请停掉 daemon 后运行 \
             `mineral cache reset --yes` 删库重建",
        )?;
        Ok(Self { pool })
    }

    /// 封面缓存索引(`cover_cache` 表,LRU 驱逐)。键 = 封面 URL。
    ///
    /// # Params:
    ///   - `root`: 封面文件根目录(`relpath` 相对它)
    ///   - `capacity`: 容量上限字节(LRU 满了驱逐最旧)
    ///
    /// # Return:
    ///   就绪索引;建表 / 载入失败返回 `Err`。
    pub async fn cover_cache(
        &self,
        root: PathBuf,
        capacity: u64,
    ) -> color_eyre::Result<CacheIndex> {
        CacheIndex::open(self.pool.clone(), "cover_cache", root, Some(capacity)).await
    }

    /// 读一条 UI 偏好(`ui_prefs` 表)。
    ///
    /// # Params:
    ///   - `key`: 偏好键(如 `"lyric_extra"`)
    ///
    /// # Return:
    ///   键存在为 `Some(值)`,不存在为 `None`。
    pub async fn get_pref(&self, key: &str) -> color_eyre::Result<Option<String>> {
        sqlx::query_scalar::<_, String>("SELECT value FROM ui_prefs WHERE key = ?1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .wrap_err_with(|| format!("读 ui_prefs 失败 key={key}"))
    }

    /// 写一条 UI 偏好(单条 upsert,同键覆盖)。
    ///
    /// # Params:
    ///   - `key`: 偏好键
    ///   - `value`: 偏好值(调用方自行定义稳定字符串编码)
    pub async fn set_pref(&self, key: &str, value: &str) -> color_eyre::Result<()> {
        sqlx::query(
            "INSERT INTO ui_prefs (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await
        .wrap_err_with(|| format!("写 ui_prefs 失败 key={key}"))?;
        Ok(())
    }

    /// 读回全部歌单内光标位置记忆(`track_pos` 表)。
    ///
    /// # Return:
    ///   全部行(顺序不保证;客户端按歌单 id 入 map);行下标为负(库损坏)时报错。
    pub async fn load_track_positions(&self) -> color_eyre::Result<Vec<TrackPosRow>> {
        let rows: Vec<(String, String, String, String, i64, i64)> = sqlx::query_as(
            "SELECT playlist_namespace, playlist_value, song_namespace, song_value, \
             sel_index, screen_row FROM track_pos",
        )
        .fetch_all(&self.pool)
        .await
        .wrap_err("读 track_pos 失败")?;
        rows.into_iter()
            .map(
                |(playlist_ns, playlist_value, song_ns, song_value, sel_index, screen_row)| {
                    Ok(TrackPosRow {
                        playlist: PlaylistId::new(
                            SourceKind::from_name(&playlist_ns),
                            playlist_value,
                        ),
                        song: SongId::new(SourceKind::from_name(&song_ns), song_value),
                        index: u64::try_from(sel_index)?,
                        screen_row: u64::try_from(screen_row)?,
                    })
                },
            )
            .collect()
    }

    /// 整表替换歌单内光标位置记忆(事务内先清后插,与客户端「整表落盘」策略配套)。
    ///
    /// # Params:
    ///   - `rows`: 当前全量记忆(表规模 ~ 歌单数)
    pub async fn replace_track_positions(&self, rows: &[TrackPosRow]) -> color_eyre::Result<()> {
        let mut tx = self
            .pool
            .begin()
            .await
            .wrap_err("开启 track_pos 事务失败")?;
        sqlx::query("DELETE FROM track_pos")
            .execute(&mut *tx)
            .await
            .wrap_err("清 track_pos 失败")?;
        for row in rows {
            sqlx::query(
                "INSERT INTO track_pos (playlist_namespace, playlist_value, \
                 song_namespace, song_value, sel_index, screen_row) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(row.playlist.namespace().name())
            .bind(row.playlist.value())
            .bind(row.song.namespace().name())
            .bind(row.song.value())
            .bind(i64::try_from(row.index)?)
            .bind(i64::try_from(row.screen_row)?)
            .execute(&mut *tx)
            .await
            .wrap_err_with(|| format!("写 track_pos 失败 playlist={}", row.playlist.value()))?;
        }
        tx.commit().await.wrap_err("提交 track_pos 事务失败")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::ClientStore;

    /// 开一个内存 sqlite 的 [`ClientStore`](每个测试独立)。
    async fn mem_store() -> color_eyre::Result<ClientStore> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?;
        ClientStore::with_pool(pool).await
    }

    /// ui_prefs round-trip:set 后 get 读回原值;同键再 set 覆盖;未写键为 `None`。
    #[tokio::test]
    async fn ui_prefs_round_trip_and_overwrite() -> color_eyre::Result<()> {
        let store = mem_store().await?;
        assert_eq!(store.get_pref("lyric_extra").await?, None, "未写键应 None");
        store.set_pref("lyric_extra", "translation").await?;
        assert_eq!(
            store.get_pref("lyric_extra").await?.as_deref(),
            Some("translation")
        );
        store.set_pref("lyric_extra", "none").await?;
        assert_eq!(
            store.get_pref("lyric_extra").await?.as_deref(),
            Some("none"),
            "同键 upsert 应覆盖"
        );
        Ok(())
    }

    /// 不同键互不串扰。
    #[tokio::test]
    async fn ui_prefs_keys_are_independent() -> color_eyre::Result<()> {
        let store = mem_store().await?;
        store.set_pref("a", "1").await?;
        store.set_pref("b", "2").await?;
        assert_eq!(store.get_pref("a").await?.as_deref(), Some("1"));
        assert_eq!(store.get_pref("b").await?.as_deref(), Some("2"));
        Ok(())
    }

    /// track_pos 整表替换 round-trip:replace 后 load 读回原行(含跨 source 歌单);
    /// 再次 replace 是全量覆盖,旧行不残留。
    #[tokio::test]
    async fn track_pos_replace_and_load_round_trip() -> color_eyre::Result<()> {
        use mineral_model::{PlaylistId, SongId, SourceKind};

        use super::TrackPosRow;

        let store = mem_store().await?;
        assert!(store.load_track_positions().await?.is_empty(), "初始空表");

        let rows = vec![
            TrackPosRow {
                playlist: PlaylistId::new(SourceKind::NETEASE, "p1"),
                song: SongId::new(SourceKind::NETEASE, "s1"),
                index: 3,
                screen_row: 7,
            },
            TrackPosRow {
                playlist: PlaylistId::new(SourceKind::SHELF, "p1"),
                song: SongId::new(SourceKind::SHELF, "s2"),
                index: 0,
                screen_row: 0,
            },
        ];
        store.replace_track_positions(&rows).await?;
        let mut got = store.load_track_positions().await?;
        got.sort_by_key(|r| r.playlist.qualified());
        let mut want = rows.clone();
        want.sort_by_key(|r| r.playlist.qualified());
        assert_eq!(got, want);

        // 全量覆盖:换成单行后旧行消失。
        let only = vec![TrackPosRow {
            playlist: PlaylistId::new(SourceKind::NETEASE, "p9"),
            song: SongId::new(SourceKind::NETEASE, "s9"),
            index: 1,
            screen_row: 2,
        }];
        store.replace_track_positions(&only).await?;
        assert_eq!(store.load_track_positions().await?, only);
        Ok(())
    }
}
