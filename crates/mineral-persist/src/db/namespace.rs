//! 某来源命名空间下的存储视图。

use color_eyre::eyre::WrapErr;
use mineral_log::trace;
use mineral_model::{AlbumId, ArtistId, MediaUrl, PlaylistId, Song, SongId, SourceKind};

use crate::ServerStore;
use crate::db::rows::{SongArtistRow, SongMetaRow};

/// 一首歌的聚合统计(出参)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SongStats {
    /// 完整播放次数。
    pub play_count: u32,

    /// 跳过次数。
    pub skip_count: u32,

    /// 累计收听毫秒。
    pub total_listen_ms: u64,

    /// 最近播放 unix ms(无则 None)。
    pub last_played_at: Option<i64>,

    /// 是否 loved。
    pub loved: bool,
}

/// 一条播放历史(出参)。
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    /// 歌曲 id(带本来源 namespace;展示时配 song_meta 重建 Song)。
    pub song_id: SongId,

    /// 播放时刻 unix ms。
    pub played_at: i64,

    /// 是否完整播完。
    pub completed: bool,

    /// 本次收听毫秒。
    pub listen_ms: u64,
}

/// 歌单缓存出参(曲目 id 保序，展示时配 song_meta 重建)。
#[derive(Debug, Clone)]
pub struct PlaylistCacheEntry {
    /// 歌单名(可空)。
    pub name: Option<String>,

    /// 抓取时刻 unix ms。
    pub fetched_at: i64,

    /// 歌单版本戳(网易云 `trackUpdateTime`,unix ms;旧库或未知为 `None`)。
    ///
    /// 曲目增删改/重排会更新它,供调用方做条件刷新的版本比对。
    pub track_update_time: Option<i64>,

    /// 曲目 id(带本来源 namespace),按 position 保序。
    pub track_values: Vec<SongId>,
}

/// 绑定单一来源 namespace 的结构态视图。降级 ServerStore 下所有方法 no-op/空。
pub struct NamespaceStore {
    /// 顶层句柄(经 `persist.pool()` 取连接池)。
    persist: ServerStore,

    /// 本视图绑定的来源(用 `source.name()` 做 namespace 过滤)。
    source: SourceKind,
}

impl NamespaceStore {
    /// 构造。
    ///
    /// # Params:
    ///   - `persist`: 顶层句柄
    ///   - `source`: 绑定的来源
    pub(crate) fn new(persist: ServerStore, source: SourceKind) -> Self {
        Self { persist, source }
    }

    /// 底层连接池(降级 ServerStore 为 `None`;同 crate 扩展方法用,如 song_kv)。
    pub(crate) fn pool(&self) -> Option<&sqlx::SqlitePool> {
        self.persist.pool()
    }

    /// 本视图的 namespace 过滤值(= `source.name()`)。
    pub(crate) fn namespace(&self) -> &str {
        self.source.name()
    }

    /// upsert 一首歌的元数据(song_meta + 按需重写 song_artists 保序)。
    ///
    /// 降级 ServerStore 下静默 no-op。
    ///
    /// song_meta 是**富化程度不一的投影**的落点(同一首歌可能先由列表投影写入、后被 detail
    /// 投影刷新,反之亦然),统一一条合并规则:**「本次输入缺失该字段 = 无新信息 → 保留旧值」**。
    /// 标量富化字段(alias / album / duration / cover)用 SQL NULL 表达「缺失」,走
    /// `COALESCE(excluded.x, song_meta.x)`;艺人列表用**空 Vec** 表达「缺失」,空则跳过重写、
    /// 保留已存行,非空才按 [`Song::artists`] 顺序整体重写(先删后插,`position` 即下标)。
    /// name 是 NOT NULL 列、每个投影必带,恒以新值为准。
    ///
    /// 空艺人列表判「缺失」而非「权威零艺人」是**领域不变量**:本仓任何 channel 产出的 Song
    /// 都必带艺人(netease `ar` / album_artist_refs 有 primary 兜底;bilibili UP 主映射),
    /// 零艺人只会是投影没带、不会是歌真没有。故与 `put_playlist_cache`「空曲目列表清空歌单」
    /// 相反并不矛盾:歌单曲目是权威可编辑的,song 的艺人不是。
    ///
    /// # Params:
    ///   - `song`: 待写入的歌曲元数据
    ///
    /// # Return:
    ///   成功返回 `Ok(())`;降级时同样 `Ok(())`。
    pub async fn upsert_meta(&self, song: &Song) -> color_eyre::Result<()> {
        let Some(pool) = self.persist.pool() else {
            return Ok(());
        };
        trace!(target: "persist", song = %song.id.value(), "upsert_meta");
        let ns = self.source.name();
        let song_value = song.id.value();
        let album_id = song.album.as_ref().map(|a| a.id.value().to_owned());
        let album_name = song.album.as_ref().map(|a| a.name.clone());
        let cover_url = song.cover_url.as_ref().map(MediaUrl::to_string);
        let duration_ms = song.duration_ms.map(i64::try_from).transpose()?;

        // 整体放进一个事务:song_meta upsert + song_artists 先删后插必须原子完成。
        // 否则并发 upsert 同一首歌(多个歌单含同曲、channel_fetch 多 worker 并行刷新)时,
        // DELETE 与 INSERT 之间会被另一路语句插入,撞 song_artists 主键
        // (UNIQUE constraint failed: namespace, song_value, position)。事务在单连接池下
        // 互斥串行,消除交错。
        let mut tx = pool.begin().await.wrap_err("开启 upsert_meta 事务失败")?;

        // 可空富化字段走「非空进步、NULL 不回退」(COALESCE):写入方是富化程度不一的投影
        // (如 netease 列表带译名而 detail 常缺),后到的贫投影不得抹掉已知值。旧行的 NULL
        // 由此在数据流经时自然回填,无需专门修复步。name 非空列,恒以新值为准。
        sqlx::query(
            "INSERT INTO song_meta \
             (namespace, song_value, name, alias, album_id, album_name, duration_ms, cover_url) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(namespace, song_value) DO UPDATE SET \
             name = excluded.name, \
             alias = COALESCE(excluded.alias, song_meta.alias), \
             album_id = COALESCE(excluded.album_id, song_meta.album_id), \
             album_name = COALESCE(excluded.album_name, song_meta.album_name), \
             duration_ms = COALESCE(excluded.duration_ms, song_meta.duration_ms), \
             cover_url = COALESCE(excluded.cover_url, song_meta.cover_url)",
        )
        .bind(ns)
        .bind(song_value)
        .bind(&song.name)
        .bind(&song.alias)
        .bind(album_id)
        .bind(album_name)
        .bind(duration_ms)
        .bind(cover_url)
        .execute(&mut *tx)
        .await
        .wrap_err_with(|| format!("upsert song_meta 失败 song={song_value}"))?;

        // 艺人列表同理:空列表视作「未知」,保留已存行;非空才整体重写(先删后插保序)。
        if !song.artists.is_empty() {
            sqlx::query("DELETE FROM song_artists WHERE namespace = ? AND song_value = ?")
                .bind(ns)
                .bind(song_value)
                .execute(&mut *tx)
                .await
                .wrap_err_with(|| format!("清空 song_artists 失败 song={song_value}"))?;

            for (i, artist) in song.artists.iter().enumerate() {
                let position = i64::try_from(i)?;
                sqlx::query(
                    "INSERT INTO song_artists \
                     (namespace, song_value, position, artist_id, artist_name) \
                     VALUES (?, ?, ?, ?, ?)",
                )
                .bind(ns)
                .bind(song_value)
                .bind(position)
                .bind(artist.id.value())
                .bind(&artist.name)
                .execute(&mut *tx)
                .await
                .wrap_err_with(|| {
                    format!("写入 song_artists 失败 song={song_value} position={position}")
                })?;
            }
        }

        tx.commit()
            .await
            .wrap_err_with(|| format!("提交 upsert_meta 事务失败 song={song_value}"))?;
        Ok(())
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
    pub async fn get_meta(&self, id: &SongId) -> color_eyre::Result<Option<Song>> {
        let Some(pool) = self.persist.pool() else {
            return Ok(None);
        };
        let ns = self.source.name();
        let song_value = id.value();

        let Some(row) = sqlx::query_as::<_, SongMetaRow>(
            "SELECT namespace, song_value, name, alias, album_id, album_name, duration_ms, \
             cover_url FROM song_meta WHERE namespace = ? AND song_value = ?",
        )
        .bind(ns)
        .bind(song_value)
        .fetch_optional(pool)
        .await
        .wrap_err_with(|| format!("查 song_meta 失败 song={song_value}"))?
        else {
            return Ok(None);
        };

        let artist_rows = sqlx::query_as::<_, SongArtistRow>(
            "SELECT artist_id, artist_name FROM song_artists \
             WHERE namespace = ? AND song_value = ? ORDER BY position",
        )
        .bind(ns)
        .bind(song_value)
        .fetch_all(pool)
        .await
        .wrap_err_with(|| format!("查 song_artists 失败 song={song_value}"))?;

        Ok(Some(row.into_song(artist_rows)?))
    }

    /// 按 album id 回查专辑名(取任一成员歌 `song_meta.album_name`;专辑名只作为歌的投影存,
    /// 无独立专辑表)。降级 / 未命中返回 `Ok(None)`。
    ///
    /// # Params:
    ///   - `id`: 专辑 id(裸值查 `song_meta.album_id`)
    ///
    /// # Return:
    ///   命中返回 `Ok(Some(name))`,否则 `Ok(None)`。
    pub async fn album_name(&self, id: &AlbumId) -> color_eyre::Result<Option<String>> {
        let Some(pool) = self.persist.pool() else {
            return Ok(None);
        };
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT album_name FROM song_meta \
             WHERE namespace = ? AND album_id = ? AND album_name IS NOT NULL LIMIT 1",
        )
        .bind(self.source.name())
        .bind(id.value())
        .fetch_optional(pool)
        .await
        .wrap_err_with(|| format!("回查专辑名失败 album={}", id.value()))?;
        Ok(row.map(|(n,)| n))
    }

    /// 按 artist id 回查艺名(取任一署名行 `song_artists.artist_name`;艺名同样只作为歌的
    /// 投影存)。降级 / 未命中返回 `Ok(None)`。
    ///
    /// # Params:
    ///   - `id`: 艺人 id(裸值查 `song_artists.artist_id`)
    ///
    /// # Return:
    ///   命中返回 `Ok(Some(name))`,否则 `Ok(None)`。
    pub async fn artist_name(&self, id: &ArtistId) -> color_eyre::Result<Option<String>> {
        let Some(pool) = self.persist.pool() else {
            return Ok(None);
        };
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT artist_name FROM song_artists \
             WHERE namespace = ? AND artist_id = ? LIMIT 1",
        )
        .bind(self.source.name())
        .bind(id.value())
        .fetch_optional(pool)
        .await
        .wrap_err_with(|| format!("回查艺名失败 artist={}", id.value()))?;
        Ok(row.map(|(n,)| n))
    }

    /// 记一次完整播放：play_count+1、累加时长、刷新 last_played_at。降级 no-op。
    ///
    /// # Params:
    ///   - `id`: 歌曲 id(用其裸值入库)
    ///   - `listen_ms`: 本次收听毫秒
    ///
    /// # Return:
    ///   成功返回 `Ok(())`;降级时同样 `Ok(())`。
    pub async fn record_play(&self, id: &SongId, listen_ms: u64) -> color_eyre::Result<()> {
        let Some(pool) = self.persist.pool() else {
            return Ok(());
        };
        trace!(target: "persist", song = %id.value(), "record_play");
        let listen = i64::try_from(listen_ms)?;
        sqlx::query(
            "INSERT INTO song_stats(namespace,song_value,play_count,total_listen_ms,last_played_at) \
             VALUES(?,?,1,?,?) \
             ON CONFLICT(namespace,song_value) DO UPDATE SET \
               play_count=play_count+1, \
               total_listen_ms=total_listen_ms+excluded.total_listen_ms, \
               last_played_at=excluded.last_played_at",
        )
        .bind(self.source.name())
        .bind(id.value())
        .bind(listen)
        .bind(crate::db::time::now_ms())
        .execute(pool)
        .await
        .wrap_err_with(|| format!("记录播放统计失败 song={}", id.value()))?;
        Ok(())
    }

    /// 记一次跳过：skip_count+1。降级 no-op。
    ///
    /// # Params:
    ///   - `id`: 歌曲 id(用其裸值入库)
    ///
    /// # Return:
    ///   成功返回 `Ok(())`;降级时同样 `Ok(())`。
    pub async fn record_skip(&self, id: &SongId) -> color_eyre::Result<()> {
        let Some(pool) = self.persist.pool() else {
            return Ok(());
        };
        trace!(target: "persist", song = %id.value(), "record_skip");
        sqlx::query(
            "INSERT INTO song_stats(namespace,song_value,skip_count) VALUES(?,?,1) \
             ON CONFLICT(namespace,song_value) DO UPDATE SET skip_count=skip_count+1",
        )
        .bind(self.source.name())
        .bind(id.value())
        .execute(pool)
        .await
        .wrap_err_with(|| format!("记录跳过统计失败 song={}", id.value()))?;
        Ok(())
    }

    /// 查一首歌的聚合统计。降级或无记录返回 `Ok(None)`。
    ///
    /// # Params:
    ///   - `id`: 歌曲 id(裸值用于查 song_stats)
    ///
    /// # Return:
    ///   命中返回 `Ok(Some(stats))`,否则 `Ok(None)`。
    pub async fn query_stats(&self, id: &SongId) -> color_eyre::Result<Option<SongStats>> {
        let Some(pool) = self.persist.pool() else {
            return Ok(None);
        };
        let Some(row) = sqlx::query_as::<_, (i64, i64, i64, Option<i64>, Option<i64>)>(
            "SELECT play_count,skip_count,total_listen_ms,last_played_at,loved_at \
             FROM song_stats WHERE namespace=? AND song_value=?",
        )
        .bind(self.source.name())
        .bind(id.value())
        .fetch_optional(pool)
        .await
        .wrap_err_with(|| format!("查播放统计失败 song={}", id.value()))?
        else {
            return Ok(None);
        };

        let (play_count, skip_count, total_listen_ms, last_played_at, loved_at) = row;
        Ok(Some(SongStats {
            play_count: u32::try_from(play_count)?,
            skip_count: u32::try_from(skip_count)?,
            total_listen_ms: u64::try_from(total_listen_ms)?,
            last_played_at,
            loved: loved_at.is_some(),
        }))
    }

    /// 设/取消一首歌的 love：写 `loved_at`(true=now，false=NULL)。降级 no-op。
    ///
    /// # Params:
    ///   - `id`: 歌曲 id
    ///   - `loved`: true=喜欢，false=取消
    ///
    /// # Return:
    ///   成功返回 `Ok(())`;降级时同样 `Ok(())`。
    pub async fn set_loved(&self, id: &SongId, loved: bool) -> color_eyre::Result<()> {
        let Some(pool) = self.persist.pool() else {
            return Ok(());
        };
        trace!(target: "persist", song = %id.value(), loved, "set_loved");
        let at: Option<i64> = if loved {
            Some(crate::db::time::now_ms())
        } else {
            None
        };
        sqlx::query(
            "INSERT INTO song_stats(namespace,song_value,loved_at) VALUES(?,?,?) \
             ON CONFLICT(namespace,song_value) DO UPDATE SET loved_at=excluded.loved_at",
        )
        .bind(self.source.name())
        .bind(id.value())
        .bind(at)
        .execute(pool)
        .await
        .wrap_err_with(|| format!("写 love 状态失败 song={}", id.value()))?;
        Ok(())
    }

    /// 是否 loved。降级 / 无记录返回 false。
    ///
    /// # Params:
    ///   - `id`: 歌曲 id
    ///
    /// # Return:
    ///   `loved_at` 非 NULL 时 true。
    pub async fn is_loved(&self, id: &SongId) -> color_eyre::Result<bool> {
        let Some(pool) = self.persist.pool() else {
            return Ok(false);
        };
        let row: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT loved_at FROM song_stats WHERE namespace=? AND song_value=?")
                .bind(self.source.name())
                .bind(id.value())
                .fetch_optional(pool)
                .await
                .wrap_err_with(|| format!("查 love 状态失败 song={}", id.value()))?;
        Ok(matches!(row, Some((Some(_),))))
    }

    /// 本来源全部 loved 歌 id 集合。降级返回空集。
    ///
    /// # Return:
    ///   `loved_at` 非 NULL 的歌 id 集合。
    pub async fn loved_ids(&self) -> color_eyre::Result<rustc_hash::FxHashSet<SongId>> {
        let mut out = rustc_hash::FxHashSet::default();
        let Some(pool) = self.persist.pool() else {
            return Ok(out);
        };
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT song_value FROM song_stats WHERE namespace=? AND loved_at IS NOT NULL",
        )
        .bind(self.source.name())
        .fetch_all(pool)
        .await
        .wrap_err("查 loved 列表失败")?;
        for (v,) in rows {
            out.insert(SongId::new(self.source, v));
        }
        Ok(out)
    }

    /// 追加一条播放历史。降级 no-op。
    ///
    /// # Params:
    ///   - `id`: 歌曲 id
    ///   - `completed`: 是否完整播完(false=跳过)
    ///   - `listen_ms`: 本次收听毫秒
    ///
    /// # Return:
    ///   成功返回 `Ok(())`;降级时同样 `Ok(())`。
    pub async fn push_history(
        &self,
        id: &SongId,
        completed: bool,
        listen_ms: u64,
    ) -> color_eyre::Result<()> {
        let Some(pool) = self.persist.pool() else {
            return Ok(());
        };
        trace!(target: "persist", song = %id.value(), completed, "push_history");
        let listen = i64::try_from(listen_ms)?;
        sqlx::query(
            "INSERT INTO play_history(namespace,song_value,played_at,completed,listen_ms) \
             VALUES(?,?,?,?,?)",
        )
        .bind(self.source.name())
        .bind(id.value())
        .bind(crate::db::time::now_ms())
        .bind(i64::from(completed))
        .bind(listen)
        .execute(pool)
        .await
        .wrap_err_with(|| format!("追加播放历史失败 song={}", id.value()))?;
        Ok(())
    }

    /// 最近 `limit` 条历史(本来源，按 played_at 倒序——最新在前)。降级返回空。
    ///
    /// # Params:
    ///   - `limit`: 最多返回条数
    ///
    /// # Return:
    ///   历史条目列表，最新优先。
    pub async fn recent_history(&self, limit: u32) -> color_eyre::Result<Vec<HistoryEntry>> {
        let mut out = Vec::new();
        let Some(pool) = self.persist.pool() else {
            return Ok(out);
        };
        let lim = i64::from(limit);
        let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
            "SELECT song_value,played_at,completed,listen_ms FROM play_history \
             WHERE namespace=? ORDER BY played_at DESC, id DESC LIMIT ?",
        )
        .bind(self.source.name())
        .bind(lim)
        .fetch_all(pool)
        .await
        .wrap_err("查最近播放历史失败")?;
        for (song_value, played_at, completed, listen_ms) in rows {
            out.push(HistoryEntry {
                song_id: SongId::new(self.source, song_value),
                played_at,
                completed: completed != 0,
                listen_ms: u64::try_from(listen_ms)?,
            });
        }
        Ok(out)
    }

    /// 裁剪保留窗口：删 `played_at` 早于 `before_ms` 的历史。降级 no-op。
    ///
    /// # Params:
    ///   - `before_ms`: 阈值 unix ms，早于它的记录删除
    ///
    /// # Return:
    ///   成功返回 `Ok(())`;降级时同样 `Ok(())`。
    pub async fn prune_history(&self, before_ms: i64) -> color_eyre::Result<()> {
        let Some(pool) = self.persist.pool() else {
            return Ok(());
        };
        sqlx::query("DELETE FROM play_history WHERE namespace=? AND played_at < ?")
            .bind(self.source.name())
            .bind(before_ms)
            .execute(pool)
            .await
            .wrap_err_with(|| format!("裁剪播放历史失败 before_ms={before_ms}"))?;
        Ok(())
    }

    /// 写歌单缓存(覆盖：upsert 元信息 + 先删后插 tracks 保序，刷新 fetched_at)。降级 no-op。
    ///
    /// # Params:
    ///   - `id`: 歌单 id
    ///   - `name`: 歌单名(可空)
    ///   - `track_update_time`: 歌单版本戳(网易云 `trackUpdateTime`,可空)
    ///   - `track_values`: 曲目 id,按展示顺序(仅存其裸值,namespace 由本 store 隐含)
    ///
    /// # Return:
    ///   成功返回 `Ok(())`;降级时同样 `Ok(())`。
    pub async fn put_playlist_cache(
        &self,
        id: &PlaylistId,
        name: Option<&str>,
        track_update_time: Option<i64>,
        track_values: &[SongId],
    ) -> color_eyre::Result<()> {
        let Some(pool) = self.persist.pool() else {
            return Ok(());
        };
        trace!(target: "persist", playlist = %id.value(), tracks = track_values.len(), "put_playlist_cache");
        let ns = self.source.name();
        let pid = id.value();

        // 多步写(playlist_cache upsert + playlist_tracks 先删后插)包进事务原子完成,
        // 否则并发刷新同一歌单时 DELETE/INSERT 交错会撞 playlist_tracks 主键。
        let mut tx = pool
            .begin()
            .await
            .wrap_err("开启 put_playlist_cache 事务失败")?;

        sqlx::query(
            "INSERT INTO playlist_cache(namespace,playlist_id,name,fetched_at,track_update_time) \
             VALUES(?,?,?,?,?) \
             ON CONFLICT(namespace,playlist_id) DO UPDATE SET \
               name=excluded.name, fetched_at=excluded.fetched_at, \
               track_update_time=excluded.track_update_time",
        )
        .bind(ns)
        .bind(pid)
        .bind(name)
        .bind(crate::db::time::now_ms())
        .bind(track_update_time)
        .execute(&mut *tx)
        .await
        .wrap_err_with(|| format!("upsert playlist_cache 失败 playlist={pid}"))?;

        sqlx::query("DELETE FROM playlist_tracks WHERE namespace=? AND playlist_id=?")
            .bind(ns)
            .bind(pid)
            .execute(&mut *tx)
            .await
            .wrap_err_with(|| format!("清空 playlist_tracks 失败 playlist={pid}"))?;

        for (i, v) in track_values.iter().enumerate() {
            let pos = i64::try_from(i)?;
            sqlx::query(
                "INSERT INTO playlist_tracks(namespace,playlist_id,position,song_value) \
                 VALUES(?,?,?,?)",
            )
            .bind(ns)
            .bind(pid)
            .bind(pos)
            .bind(v.value())
            .execute(&mut *tx)
            .await
            .wrap_err_with(|| format!("写入 playlist_tracks 失败 playlist={pid} position={pos}"))?;
        }

        tx.commit()
            .await
            .wrap_err_with(|| format!("提交 put_playlist_cache 事务失败 playlist={pid}"))?;
        Ok(())
    }

    /// 读歌单缓存(曲目按 position 升序)。降级 / 未命中返回 `None`。
    ///
    /// # Params:
    ///   - `id`: 歌单 id
    ///
    /// # Return:
    ///   命中返回缓存条目(含 fetched_at / track_update_time 供调用方做版本比对)，否则 None。
    pub async fn get_playlist_cache(
        &self,
        id: &PlaylistId,
    ) -> color_eyre::Result<Option<PlaylistCacheEntry>> {
        let Some(pool) = self.persist.pool() else {
            return Ok(None);
        };
        let ns = self.source.name();
        let pid = id.value();
        let head: Option<(Option<String>, i64, Option<i64>)> = sqlx::query_as(
            "SELECT name,fetched_at,track_update_time FROM playlist_cache \
             WHERE namespace=? AND playlist_id=?",
        )
        .bind(ns)
        .bind(pid)
        .fetch_optional(pool)
        .await
        .wrap_err_with(|| format!("查 playlist_cache 失败 playlist={pid}"))?;
        let Some((name, fetched_at, track_update_time)) = head else {
            return Ok(None);
        };
        let tracks: Vec<(String,)> = sqlx::query_as(
            "SELECT song_value FROM playlist_tracks WHERE namespace=? AND playlist_id=? \
             ORDER BY position",
        )
        .bind(ns)
        .bind(pid)
        .fetch_all(pool)
        .await
        .wrap_err_with(|| format!("查 playlist_tracks 失败 playlist={pid}"))?;
        Ok(Some(PlaylistCacheEntry {
            name,
            fetched_at,
            track_update_time,
            track_values: tracks
                .into_iter()
                .map(|(v,)| SongId::new(self.source, v))
                .collect::<Vec<SongId>>(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::{ArtistId, ArtistRef, Song, SongId, SourceKind};

    #[tokio::test]
    async fn upsert_meta_then_get_roundtrips() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        let song = Song::builder()
            .id(SongId::new(SourceKind::NETEASE, "123"))
            .name("迷跡波".to_owned())
            .artists(vec![ArtistRef {
                id: ArtistId::new(SourceKind::NETEASE, "a1"),
                name: "演者".to_owned(),
            }])
            .duration_ms(Some(200_000))
            .build();
        s.upsert_meta(&song).await?;
        let got = s.get_meta(&song.id).await?;
        assert!(got.is_some());
        if let Some(g) = got {
            assert_eq!(g.name, "迷跡波");
            assert_eq!(g.artists.len(), song.artists.len());
            assert_eq!(g.duration_ms, Some(200_000));
        }
        Ok(())
    }

    /// album_name / artist_name:按 id 回查名(取任一成员歌 / 署名行),未命中 None。
    #[tokio::test]
    async fn album_and_artist_name_lookup() -> color_eyre::Result<()> {
        use mineral_model::{AlbumId, AlbumRef, ArtistId, ArtistRef};

        let dir = tempfile::tempdir()?;
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        let song = Song::builder()
            .id(SongId::new(SourceKind::NETEASE, "1"))
            .name("稻香".to_owned())
            .artists(vec![ArtistRef {
                id: ArtistId::new(SourceKind::NETEASE, "jay"),
                name: "周杰伦".to_owned(),
            }])
            .album(Some(AlbumRef {
                id: AlbumId::new(SourceKind::NETEASE, "mojito"),
                name: "魔杰座".to_owned(),
            }))
            .build();
        s.upsert_meta(&song).await?;

        assert_eq!(
            s.album_name(&AlbumId::new(SourceKind::NETEASE, "mojito"))
                .await?,
            Some("魔杰座".to_owned())
        );
        assert_eq!(
            s.artist_name(&ArtistId::new(SourceKind::NETEASE, "jay"))
                .await?,
            Some("周杰伦".to_owned())
        );
        assert_eq!(
            s.album_name(&AlbumId::new(SourceKind::NETEASE, "absent"))
                .await?,
            None,
            "未命中回落 None"
        );
        Ok(())
    }

    /// 可空富化字段「非空进步、NULL 不回退」:贫投影(alias/duration/cover/album 全缺、
    /// 艺人空)后到,不得抹掉先前富投影写入的值;name 非空列恒以新值为准。
    #[tokio::test]
    async fn upsert_meta_null_fields_do_not_regress() -> color_eyre::Result<()> {
        use mineral_model::{AlbumId, AlbumRef, MediaUrl};

        let dir = tempfile::tempdir()?;
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        let id = SongId::new(SourceKind::NETEASE, "42");

        let rich = Song::builder()
            .id(id.clone())
            .name("ButterFly".to_owned())
            .alias(Some("黄油飞".to_owned()))
            .artists(vec![ArtistRef {
                id: ArtistId::new(SourceKind::NETEASE, "a1"),
                name: "和田光司".to_owned(),
            }])
            .album(Some(AlbumRef {
                id: AlbumId::new(SourceKind::NETEASE, "al1"),
                name: "数码宝贝".to_owned(),
            }))
            .duration_ms(Some(259_000))
            .cover_url(Some(MediaUrl::remote("https://p1.example/c.jpg")?))
            .build();
        s.upsert_meta(&rich).await?;

        // 贫投影:除 name 外全缺。
        let poor = Song::builder()
            .id(id.clone())
            .name("Butter-Fly".to_owned())
            .build();
        s.upsert_meta(&poor).await?;

        let got = s
            .get_meta(&id)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("应命中 meta"))?;
        assert_eq!(got.name, "Butter-Fly", "name 恒以新值为准");
        assert_eq!(
            got.alias.as_deref(),
            Some("黄油飞"),
            "alias 不得被 NULL 回退"
        );
        assert_eq!(got.duration_ms, Some(259_000), "duration 不得被 NULL 回退");
        assert!(got.cover_url.is_some(), "cover 不得被 NULL 回退");
        assert!(got.album.is_some(), "album 不得被 NULL 回退");
        assert_eq!(got.artists.len(), 1, "空艺人列表应保留已存行");

        // 后续富投影仍能正常更新非空值(进步方向不受影响)。
        let newer = Song::builder()
            .id(id.clone())
            .name("Butter-Fly".to_owned())
            .alias(Some("黄油飞(数码宝贝OP)".to_owned()))
            .duration_ms(Some(260_000))
            .build();
        s.upsert_meta(&newer).await?;
        let got2 = s
            .get_meta(&id)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("应命中 meta"))?;
        assert_eq!(got2.alias.as_deref(), Some("黄油飞(数码宝贝OP)"));
        assert_eq!(got2.duration_ms, Some(260_000));
        Ok(())
    }

    #[tokio::test]
    async fn upsert_meta_preserves_artist_order_and_clears_stale() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        let id = SongId::new(SourceKind::NETEASE, "777");

        // 首次 upsert:3 个艺人,验证保序 + 内容
        let song = Song::builder()
            .id(id.clone())
            .name("多人合作".to_owned())
            .artists(vec![
                ArtistRef {
                    id: ArtistId::new(SourceKind::NETEASE, "a1"),
                    name: "甲".to_owned(),
                },
                ArtistRef {
                    id: ArtistId::new(SourceKind::NETEASE, "a2"),
                    name: "乙".to_owned(),
                },
                ArtistRef {
                    id: ArtistId::new(SourceKind::NETEASE, "a3"),
                    name: "丙".to_owned(),
                },
            ])
            .duration_ms(Some(123_000))
            .build();
        s.upsert_meta(&song).await?;

        let got = s.get_meta(&id).await?;
        assert!(got.is_some());
        if let Some(g) = got {
            let names = g
                .artists
                .iter()
                .map(|a| a.name.clone())
                .collect::<Vec<String>>();
            assert_eq!(
                names,
                vec!["甲".to_owned(), "乙".to_owned(), "丙".to_owned()]
            );
        }

        // 再次 upsert 同 id:换成 1 个不同艺人,验证旧 3 行被清
        let updated = Song::builder()
            .id(id.clone())
            .name("改为单人".to_owned())
            .artists(vec![ArtistRef {
                id: ArtistId::new(SourceKind::NETEASE, "b1"),
                name: "丁".to_owned(),
            }])
            .duration_ms(Some(99_000))
            .build();
        s.upsert_meta(&updated).await?;

        let got2 = s.get_meta(&id).await?;
        assert!(got2.is_some());
        if let Some(g) = got2 {
            assert_eq!(g.name, "改为单人");
            let names = g
                .artists
                .iter()
                .map(|a| a.name.clone())
                .collect::<Vec<String>>();
            assert_eq!(names, vec!["丁".to_owned()]);
        }
        Ok(())
    }

    #[tokio::test]
    async fn get_meta_miss_returns_none() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        assert!(
            s.get_meta(&SongId::new(SourceKind::NETEASE, "nope"))
                .await?
                .is_none()
        );
        Ok(())
    }

    #[tokio::test]
    async fn play_then_skip_accumulates() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        let id = SongId::new(SourceKind::NETEASE, "123");
        s.record_play(&id, 200_000).await?;
        s.record_play(&id, 180_000).await?;
        s.record_skip(&id).await?;
        let st = s.query_stats(&id).await?;
        assert!(st.is_some());
        if let Some(st) = st {
            assert_eq!(st.play_count, 2);
            assert_eq!(st.skip_count, 1);
            assert_eq!(st.total_listen_ms, 380_000);
            assert!(st.last_played_at.is_some());
            assert!(!st.loved);
        }
        Ok(())
    }

    #[tokio::test]
    async fn query_stats_miss_returns_none() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        assert!(
            s.query_stats(&SongId::new(SourceKind::NETEASE, "x"))
                .await?
                .is_none()
        );
        Ok(())
    }

    #[tokio::test]
    async fn love_toggle_and_list() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        let id = SongId::new(SourceKind::NETEASE, "123");
        assert!(!s.is_loved(&id).await?);
        s.set_loved(&id, true).await?;
        assert!(s.is_loved(&id).await?);
        assert!(s.loved_ids().await?.contains(&id));
        s.set_loved(&id, false).await?;
        assert!(!s.is_loved(&id).await?);
        assert!(!s.loved_ids().await?.contains(&id));
        Ok(())
    }

    #[tokio::test]
    async fn loved_ids_isolated_by_source() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
        let netease = p.scope(SourceKind::NETEASE);
        let local = p.scope(SourceKind::LOCAL);
        netease
            .set_loved(&SongId::new(SourceKind::NETEASE, "n1"), true)
            .await?;
        assert_eq!(netease.loved_ids().await?.len(), 1);
        assert_eq!(local.loved_ids().await?.len(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn history_push_and_recent() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        let id = SongId::new(SourceKind::NETEASE, "123");
        s.push_history(&id, /*completed*/ true, 200_000).await?;
        s.push_history(&id, /*completed*/ false, 5_000).await?;
        let recent = s.recent_history(10).await?;
        assert_eq!(recent.len(), 2);
        // 最新(第二条 completed=false)在前
        let Some(first) = recent.first() else {
            return Err(color_eyre::eyre::eyre!("empty"));
        };
        assert!(!first.completed);
        assert_eq!(first.listen_ms, 5_000);
        let Some(second) = recent.get(1) else {
            return Err(color_eyre::eyre::eyre!("no second"));
        };
        assert!(second.completed);
        Ok(())
    }

    #[tokio::test]
    async fn prune_history_removes_old() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        let id = SongId::new(SourceKind::NETEASE, "123");
        s.push_history(&id, /*completed*/ true, 100).await?;
        // 用一个未来的阈值删掉所有(played_at < 远未来)
        s.prune_history(i64::MAX).await?;
        assert_eq!(s.recent_history(10).await?.len(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn playlist_cache_roundtrip() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        let pid = mineral_model::PlaylistId::new(SourceKind::NETEASE, "p1");
        let songs = vec![
            SongId::new(SourceKind::NETEASE, "s1"),
            SongId::new(SourceKind::NETEASE, "s2"),
            SongId::new(SourceKind::NETEASE, "s3"),
        ];
        s.put_playlist_cache(&pid, Some("我的歌单"), Some(1_775_781_450_653), &songs)
            .await?;
        let got = s.get_playlist_cache(&pid).await?;
        assert!(got.is_some());
        if let Some(g) = got {
            assert_eq!(g.name, Some("我的歌单".to_owned()));
            assert_eq!(g.track_values, songs); // 保序
            assert!(g.fetched_at > 0);
            assert_eq!(g.track_update_time, Some(1_775_781_450_653)); // 版本戳 roundtrip
        }
        Ok(())
    }

    /// 出参曲目是带本 store namespace 的结构化 `SongId`(非裸 `String`):写入裸值,
    /// 读出时由 store 自己的 `source` 补全 namespace,消除消费端硬编码来源的需要。
    #[tokio::test]
    async fn playlist_cache_returns_namespaced_song_ids() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::LOCAL);
        let pid = mineral_model::PlaylistId::new(SourceKind::LOCAL, "p1");
        let tracks = vec![
            SongId::new(SourceKind::LOCAL, "s1"),
            SongId::new(SourceKind::LOCAL, "s2"),
        ];
        s.put_playlist_cache(&pid, Some("本地歌单"), Some(1), &tracks)
            .await?;
        let Some(g) = s.get_playlist_cache(&pid).await? else {
            return Err(color_eyre::eyre::eyre!("应命中缓存"));
        };
        assert_eq!(g.track_values, tracks, "保序且裸值一致");
        for sid in &g.track_values {
            assert_eq!(
                sid.namespace(),
                SourceKind::LOCAL,
                "namespace 应为本 store 的 source"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn playlist_cache_overwrite_clears_old_tracks() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        let pid = mineral_model::PlaylistId::new(SourceKind::NETEASE, "p1");
        s.put_playlist_cache(
            &pid,
            Some("v1"),
            Some(100),
            &[
                SongId::new(SourceKind::NETEASE, "a"),
                SongId::new(SourceKind::NETEASE, "b"),
            ],
        )
        .await?;
        s.put_playlist_cache(
            &pid,
            Some("v2"),
            Some(200),
            &[SongId::new(SourceKind::NETEASE, "c")],
        )
        .await?; // 覆盖
        let got = s.get_playlist_cache(&pid).await?;
        assert!(got.is_some());
        if let Some(g) = got {
            assert_eq!(g.name, Some("v2".to_owned()));
            assert_eq!(g.track_values, vec![SongId::new(SourceKind::NETEASE, "c")]); // 旧 a,b 不残留
            assert_eq!(g.track_update_time, Some(200)); // 版本戳也被覆盖刷新
        }
        Ok(())
    }

    #[tokio::test]
    async fn playlist_cache_miss_returns_none() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        let pid = mineral_model::PlaylistId::new(SourceKind::NETEASE, "absent");
        assert!(s.get_playlist_cache(&pid).await?.is_none());
        Ok(())
    }
}
