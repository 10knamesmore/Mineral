//! 按歌曲身份批量读取元数据与有序艺人。

use mineral_model::{Song, SongId};
use rustc_hash::FxHashMap;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Select};

use crate::persistence::db::namespace::NamespaceStore;
use crate::persistence::db::rows::SongArtistRow;
use crate::persistence::entity::{song_artists, song_meta};

/// 每批 ID 的数量，限制查询绑定参数与临时结果大小。
const READ_BATCH_IDS: usize = 400;

impl NamespaceStore {
    /// 批量读取本来源的歌曲元数据，每批分别读取歌曲和艺人。
    ///
    /// # Params:
    ///   - `ids`: 本 namespace 内的歌曲身份；重复 ID 只返回一份 metadata
    ///
    /// # Return:
    ///   按 SongId 索引的歌曲。缺失或无法重建的记录跳过，坏记录记日志；
    ///   查询失败返回错误。关系顺序与重复条目由调用方用原始 relation 重建。
    pub async fn get_meta_batch(
        &self,
        ids: &[SongId],
    ) -> crate::persistence::Result<FxHashMap<SongId, Song>> {
        for id in ids {
            self.check_source(id.namespace())?;
        }
        let Some(pool) = self.pool() else {
            return Ok(FxHashMap::default());
        };
        self.get_meta_batch_on(pool, ids).await
    }

    /// 使用调用方的连接读取元数据，使歌单关系与歌曲资料属于同一事务视图。
    pub(crate) async fn get_meta_batch_on(
        &self,
        connection: &impl ConnectionTrait,
        ids: &[SongId],
    ) -> crate::persistence::Result<FxHashMap<SongId, Song>> {
        for id in ids {
            self.check_source(id.namespace())?;
        }
        let mut songs = FxHashMap::default();
        let started = std::time::Instant::now();
        let ns = self.namespace();
        for batch in ids.chunks(READ_BATCH_IDS) {
            let rows = metadata_query(ns)
                .filter(song_meta::Column::SongValue.is_in(batch.iter().map(SongId::value)))
                .all(connection)
                .await
                .map_err(|source| crate::persistence::Error::Record {
                    operation: "批量读取 song_meta",
                    record: ns.to_owned(),
                    source,
                })?;
            let artists = artists_query(ns)
                .filter(song_artists::Column::SongValue.is_in(batch.iter().map(SongId::value)))
                .all(connection)
                .await
                .map_err(|source| crate::persistence::Error::Record {
                    operation: "批量读取 song_artists",
                    record: ns.to_owned(),
                    source,
                })?;
            let mut by_song = FxHashMap::<String, Vec<SongArtistRow>>::default();
            for artist in artists {
                by_song
                    .entry(artist.song_value)
                    .or_default()
                    .push(SongArtistRow {
                        artist_id: artist.artist_id,
                        artist_name: artist.artist_name,
                    });
            }
            for row in rows {
                let id = row.song_value.clone();
                match row.into_song(by_song.remove(&id).unwrap_or_default()) {
                    Ok(song) => {
                        songs.insert(song.id.clone(), song);
                    }
                    Err(error) => {
                        mineral_log::warn!(target: "persist", source = ns, song_id = id,
                            error = mineral_log::chain(&error), "skip unreadable song metadata");
                    }
                }
            }
        }
        mineral_log::debug!(target: "persist", source = ns, requested = ids.len(), found = songs.len(),
            elapsed_ms = started.elapsed().as_millis(), "loaded song metadata batch");
        Ok(songs)
    }

    /// 按 id 读回一首歌的元数据并重建 [`Song`]。
    ///
    /// 降级或未命中返回 `Ok(None)`。艺人按 `position` 升序还原顺序。
    ///
    /// # Params:
    ///   - `id`: 歌曲 id(裸值用于查 song_meta / song_artists)
    ///
    /// # Return:
    ///   命中返回 `Ok(Some(song))`,否则 `Ok(None)`。
    pub async fn get_meta(&self, id: &SongId) -> crate::persistence::Result<Option<Song>> {
        self.check_source(id.namespace())?;
        let Some(pool) = self.pool() else {
            return Ok(None);
        };
        let ns = self.namespace();
        let song_value = id.value();

        let Some(row) = metadata_query(ns)
            .filter(song_meta::Column::SongValue.eq(song_value))
            .one(pool)
            .await
            .map_err(|source| crate::persistence::Error::Record {
                operation: "查 song_meta",
                record: song_value.to_owned(),
                source,
            })?
        else {
            return Ok(None);
        };

        let artist_rows = artists_query(ns)
            .filter(song_artists::Column::SongValue.eq(song_value))
            .into_model::<SongArtistRow>()
            .all(pool)
            .await
            .map_err(|source| crate::persistence::Error::Record {
                operation: "查 song_artists",
                record: song_value.to_owned(),
                source,
            })?;

        Ok(Some(row.into_song(artist_rows)?))
    }

    /// 列出本 namespace 全部歌曲元数据(回填类维护操作的 inventory)。
    ///
    /// # Return:
    ///   全部可重建的 [`Song`]（顺序不定）;降级返回空 vec。
    pub async fn list_meta(&self) -> crate::persistence::Result<Vec<Song>> {
        let Some(pool) = self.pool() else {
            return Ok(Vec::new());
        };
        let ns = self.namespace();
        let metas = metadata_query(ns).all(pool).await.map_err(|source| {
            crate::persistence::Error::Record {
                operation: "列出 song_meta",
                record: ns.to_owned(),
                source,
            }
        })?;
        let artist_rows = artists_query(ns).all(pool).await.map_err(|source| {
            crate::persistence::Error::Record {
                operation: "列出 song_artists",
                record: ns.to_owned(),
                source,
            }
        })?;
        // song_value → 保序艺人组(查询已按 song_value 聚簇、position 升序)。
        let mut by_song = rustc_hash::FxHashMap::<String, Vec<SongArtistRow>>::default();
        for artist in artist_rows {
            by_song
                .entry(artist.song_value)
                .or_default()
                .push(SongArtistRow {
                    artist_id: artist.artist_id,
                    artist_name: artist.artist_name,
                });
        }
        metas
            .into_iter()
            .map(|row| {
                let artists = by_song.remove(&row.song_value).unwrap_or_default();
                row.into_song(artists)
            })
            .collect()
    }
}

/// 读取一个来源内的完整歌曲标量投影。
fn metadata_query(namespace: &str) -> Select<song_meta::Entity> {
    song_meta::Entity::find().filter(song_meta::Column::Namespace.eq(namespace))
}

/// 按歌曲身份聚集艺人，并保留每首歌内部的艺人位置。
fn artists_query(namespace: &str) -> Select<song_artists::Entity> {
    song_artists::Entity::find()
        .filter(song_artists::Column::Namespace.eq(namespace))
        .order_by_asc(song_artists::Column::SongValue)
        .order_by_asc(song_artists::Column::Position)
}
