//! stats.db 聚合查询。
//!
//! 全部带时间窗 `range: Range<i64>`([start_ms, end_ms))、跨源。时段 / 日期分桶按 UTC
//! (deterministic;本地时区分桶是后续 refinement,可由 server 传 tz 偏移)。榜单 / 比率类
//! 接 [`crate::ReportOptions`] 的有效播放阈值——落库不过滤,口径在 SQL WHERE 生效。

use std::ops::Range;

use color_eyre::eyre::WrapErr as _;
use mineral_model::{AlbumId, ArtistId, SongId, SourceKind};

use crate::report::{
    Bucket, BucketBy, ContextSlice, Discoveries, Distributions, Endurance, PlayTail, ReportOptions,
    Slice, SongSummary, StatusReport, TopAlbum, TopArtist, TopBy, TopSong, Totals,
};
use crate::store::StatsStore;
use crate::vocab::FinishReason;

/// 由裸 ns + song_value 重建 `SongId`。
fn song_id(ns: &str, value: &str) -> SongId {
    SongId::new(SourceKind::from_name(ns), value)
}

/// 从 `context_ref` 的 qualified 串(`name:value`)拆出 `(namespace, value)`;串无 `:` 则
/// `None`(坏数据跳过,不进榜)。namespace 名不含 `:`,故按首个 `:` 切安全。
fn split_qualified(reference: &str) -> Option<(SourceKind, &str)> {
    let (ns, value) = reference.split_once(':')?;
    Some((SourceKind::from_name(ns), value))
}

/// `recent_plays` 的每行(history tail);`song` 由 ns+value 重建故经中转。
struct PlayTailRow {
    /// 来源 name。
    ns: String,

    /// 裸歌曲 id。
    song_value: String,

    /// 起播时刻 epoch ms。
    started_at: i64,

    /// 实际收听 ms。
    listen_ms: i64,

    /// 结束原因(TEXT → 枚举)。
    finish_reason: FinishReason,
}

/// `top_songs` 的每行;`song` 由 ns+value 重建故经中转。
struct TopSongRow {
    /// 来源 name。
    ns: String,

    /// 裸歌曲 id。
    song_value: String,

    /// 播放次数。
    plays: i64,

    /// 累计收听 ms。
    listen_ms: i64,
}

/// `top_albums` / `top_artists` 的每行;typed id 由 `context_ref` 重建故经中转。
struct TopContextRefRow {
    /// 语境引用(qualified id 串)。
    reference: String,

    /// 从该语境起播的次数。
    plays: i64,

    /// 累计收听 ms。
    listen_ms: i64,
}

/// `status` 主查询单行(events 由各事件表另算,故不直落 StatusReport)。
struct StatusRow {
    /// plays 行数。
    plays: i64,

    /// sessions 行数。
    sessions: i64,

    /// 最早播放起点;无播放为 NULL。
    first_play_at: Option<i64>,

    /// 最晚播放起点;无播放为 NULL。
    last_play_at: Option<i64>,
}

/// `endurance` 单行;avg/longest 未结束会话为 NULL,故经中转 unwrap_or。
struct EnduranceRow {
    /// 会话数。
    sessions: i64,

    /// 平均时长 ms;无会话为 NULL。
    avg_ms: Option<i64>,

    /// 最长时长 ms;无会话为 NULL。
    longest_ms: Option<i64>,
}

impl StatsStore {
    /// 最近播放流水(窗口内、可按来源过滤,按起播时刻倒序取前 `limit` 条)。
    ///
    /// # Params:
    ///   - `range`: 时间窗口 `[start_ms, end_ms)`(全量传 `0..i64::MAX`)
    ///   - `source`: 只看某来源 name(`None` = 全来源);`ns` 列直接比对裸 name 串
    ///   - `limit`: 取前几条
    ///
    /// # Return:
    ///   最近播放流水,最新在前
    pub async fn recent_plays(
        &self,
        range: Range<i64>,
        source: Option<&str>,
        limit: i64,
    ) -> color_eyre::Result<Vec<PlayTail>> {
        let Some(pool) = self.pool() else {
            return Ok(Vec::new());
        };
        // source 过滤与否 bind 集不同,天然拆两条编译期查询(同 top_contexts 的 kind 分支)。
        let rows = match source {
            Some(ns) => sqlx::query_as!(
                PlayTailRow,
                r#"SELECT ns, song_value, started_at, listen_ms,
                          finish_reason AS "finish_reason: FinishReason"
                   FROM plays WHERE started_at >= ? AND started_at < ? AND ns = ?
                   ORDER BY started_at DESC LIMIT ?"#,
                range.start,
                range.end,
                ns,
                limit
            )
            .fetch_all(pool)
            .await
            .wrap_err("recent_plays(source) 查询失败")?,
            None => sqlx::query_as!(
                PlayTailRow,
                r#"SELECT ns, song_value, started_at, listen_ms,
                          finish_reason AS "finish_reason: FinishReason"
                   FROM plays WHERE started_at >= ? AND started_at < ?
                   ORDER BY started_at DESC LIMIT ?"#,
                range.start,
                range.end,
                limit
            )
            .fetch_all(pool)
            .await
            .wrap_err("recent_plays 查询失败")?,
        };
        Ok(rows
            .into_iter()
            .map(|r| PlayTail {
                song: song_id(&r.ns, &r.song_value),
                started_at: r.started_at,
                listen_ms: r.listen_ms,
                finish_reason: r.finish_reason,
            })
            .collect())
    }

    /// 埋点系统自身状态:plays / sessions / 全部事件表行数 + 播放时间覆盖。
    pub async fn status(&self) -> color_eyre::Result<StatusReport> {
        let Some(pool) = self.pool() else {
            return Ok(StatusReport {
                plays: 0,
                sessions: 0,
                events: 0,
                first_play_at: None,
                last_play_at: None,
            });
        };
        let row = sqlx::query_as!(
            StatusRow,
            r#"SELECT (SELECT COUNT(*) FROM plays) AS "plays!",
                      (SELECT COUNT(*) FROM sessions) AS "sessions!",
                      (SELECT MIN(started_at) FROM plays) AS "first_play_at?",
                      (SELECT MAX(started_at) FROM plays) AS "last_play_at?""#,
        )
        .fetch_one(pool)
        .await
        .wrap_err("status 查询失败")?;
        let mut events = 0_i64;
        for table in super::prune::EVENT_TABLES {
            let n = sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(pool)
                .await
                .wrap_err_with(|| format!("status count {table} 失败"))?;
            events += n;
        }
        Ok(StatusReport {
            plays: row.plays,
            sessions: row.sessions,
            events,
            first_play_at: row.first_play_at,
            last_play_at: row.last_play_at,
        })
    }

    /// 总量:收听 ms / 播放次数 / 完播数 / 跳歌数 / 涉及歌曲数 / 活跃天数(UTC 日)。
    pub async fn totals(&self, range: Range<i64>) -> color_eyre::Result<Totals> {
        let Some(pool) = self.pool() else {
            return Ok(Totals {
                listen_ms: 0,
                plays: 0,
                completed: 0,
                skipped: 0,
                distinct_songs: 0,
                active_days: 0,
            });
        };
        sqlx::query_as!(
            Totals,
            r#"SELECT
                COALESCE(SUM(listen_ms), 0) AS "listen_ms!: i64",
                COUNT(*) AS "plays!: i64",
                COALESCE(SUM(CASE WHEN finish_reason = 'eof' THEN 1 ELSE 0 END), 0) AS "completed!: i64",
                COALESCE(SUM(CASE WHEN finish_reason = 'skip' THEN 1 ELSE 0 END), 0) AS "skipped!: i64",
                COUNT(DISTINCT ns || ':' || song_value) AS "distinct_songs!: i64",
                COUNT(DISTINCT date(started_at / 1000, 'unixepoch')) AS "active_days!: i64"
               FROM plays WHERE started_at >= ? AND started_at < ?"#,
            range.start,
            range.end
        )
        .fetch_one(pool)
        .await
        .wrap_err("totals 查询失败")
    }

    /// top 歌曲(次数 / 时长双口径)。`options.min_listen_ms` 过滤有效播放行。
    pub async fn top_songs(
        &self,
        range: Range<i64>,
        by: TopBy,
        options: &ReportOptions,
    ) -> color_eyre::Result<Vec<TopSong>> {
        let Some(pool) = self.pool() else {
            return Ok(Vec::new());
        };
        let min = options.min_listen_ms();
        let limit = options.top_limit();
        // ORDER BY 列随口径变(列位不可 bind),故按 TopBy 分两条。
        let rows = match by {
            TopBy::Plays => sqlx::query_as!(
                TopSongRow,
                r#"SELECT ns AS "ns!", song_value AS "song_value!",
                    COUNT(*) AS "plays!: i64", COALESCE(SUM(listen_ms), 0) AS "listen_ms!: i64"
                   FROM plays
                   WHERE started_at >= ? AND started_at < ? AND listen_ms >= ?
                   GROUP BY ns, song_value ORDER BY 3 DESC, 4 DESC LIMIT ?"#,
                range.start,
                range.end,
                min,
                limit
            )
            .fetch_all(pool)
            .await
            .wrap_err("top_songs(plays) 查询失败")?
            .into_iter()
            .map(|r| TopSong {
                song: song_id(&r.ns, &r.song_value),
                plays: r.plays,
                listen_ms: r.listen_ms,
            })
            .collect(),
            TopBy::Time => sqlx::query_as!(
                TopSongRow,
                r#"SELECT ns AS "ns!", song_value AS "song_value!",
                    COUNT(*) AS "plays!: i64", COALESCE(SUM(listen_ms), 0) AS "listen_ms!: i64"
                   FROM plays
                   WHERE started_at >= ? AND started_at < ? AND listen_ms >= ?
                   GROUP BY ns, song_value ORDER BY 4 DESC, 3 DESC LIMIT ?"#,
                range.start,
                range.end,
                min,
                limit
            )
            .fetch_all(pool)
            .await
            .wrap_err("top_songs(time) 查询失败")?
            .into_iter()
            .map(|r| TopSong {
                song: song_id(&r.ns, &r.song_value),
                plays: r.plays,
                listen_ms: r.listen_ms,
            })
            .collect(),
        };
        Ok(rows)
    }

    /// top 专辑(按专辑语境 `context_ref` 聚合:从该专辑详情页起播的量;名字由上层回查)。
    ///
    /// # Params:
    ///   - `range`: 时间窗口
    ///   - `by`: 次数 / 时长口径
    ///   - `options`: 有效播放阈值 + 榜长
    ///
    /// # Return:
    ///   top 专辑列表(坏格式的 context_ref 跳过)
    pub async fn top_albums(
        &self,
        range: Range<i64>,
        by: TopBy,
        options: &ReportOptions,
    ) -> color_eyre::Result<Vec<TopAlbum>> {
        let rows = self
            .top_context_refs(
                range,
                by,
                "album",
                options.min_listen_ms(),
                options.top_limit(),
            )
            .await
            .wrap_err("top_albums 查询失败")?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                split_qualified(&r.reference).map(|(ns, value)| TopAlbum {
                    album: AlbumId::new(ns, value),
                    plays: r.plays,
                    listen_ms: r.listen_ms,
                })
            })
            .collect())
    }

    /// top 艺人(按艺人语境 `context_ref` 聚合:从该艺人详情页起播的量;名字由上层回查)。
    ///
    /// # Params:
    ///   - `range`: 时间窗口
    ///   - `by`: 次数 / 时长口径
    ///   - `options`: 有效播放阈值 + 榜长
    ///
    /// # Return:
    ///   top 艺人列表(坏格式的 context_ref 跳过)
    pub async fn top_artists(
        &self,
        range: Range<i64>,
        by: TopBy,
        options: &ReportOptions,
    ) -> color_eyre::Result<Vec<TopArtist>> {
        let rows = self
            .top_context_refs(
                range,
                by,
                "artist",
                options.min_listen_ms(),
                options.top_limit(),
            )
            .await
            .wrap_err("top_artists 查询失败")?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                split_qualified(&r.reference).map(|(ns, value)| TopArtist {
                    artist: ArtistId::new(ns, value),
                    plays: r.plays,
                    listen_ms: r.listen_ms,
                })
            })
            .collect())
    }

    /// 按某语境 kind 聚合 `context_ref`(top_albums / top_artists 共用;`kind` 是内部常量,
    /// 非用户输入,但 `query!` 需字面量 SQL,故按 kind 值直接内联比对)。
    async fn top_context_refs(
        &self,
        range: Range<i64>,
        by: TopBy,
        kind: &str,
        min: i64,
        limit: i64,
    ) -> color_eyre::Result<Vec<TopContextRefRow>> {
        let Some(pool) = self.pool() else {
            return Ok(Vec::new());
        };
        // context_kind 由 kind 参数 bind(值,非列名);ORDER BY 列位随口径变故分两条。
        let rows = match by {
            TopBy::Plays => {
                sqlx::query_as!(
                    TopContextRefRow,
                    r#"SELECT context_ref AS "reference!", COUNT(*) AS "plays!: i64",
                    COALESCE(SUM(listen_ms), 0) AS "listen_ms!: i64"
                   FROM plays
                   WHERE started_at >= ? AND started_at < ? AND context_kind = ?
                     AND context_ref IS NOT NULL AND listen_ms >= ?
                   GROUP BY context_ref ORDER BY 2 DESC, 3 DESC LIMIT ?"#,
                    range.start,
                    range.end,
                    kind,
                    min,
                    limit
                )
                .fetch_all(pool)
                .await?
            }
            TopBy::Time => {
                sqlx::query_as!(
                    TopContextRefRow,
                    r#"SELECT context_ref AS "reference!", COUNT(*) AS "plays!: i64",
                    COALESCE(SUM(listen_ms), 0) AS "listen_ms!: i64"
                   FROM plays
                   WHERE started_at >= ? AND started_at < ? AND context_kind = ?
                     AND context_ref IS NOT NULL AND listen_ms >= ?
                   GROUP BY context_ref ORDER BY 3 DESC, 2 DESC LIMIT ?"#,
                    range.start,
                    range.end,
                    kind,
                    min,
                    limit
                )
                .fetch_all(pool)
                .await?
            }
        };
        Ok(rows)
    }

    /// 单曲循环榜:`play_mode = 'repeat_one'` 的行按次数 top(反复循环的歌)。
    ///
    /// # Params:
    ///   - `range`: 时间窗口
    ///   - `limit`: 榜长
    ///
    /// # Return:
    ///   单曲循环最多的歌(复用 [`TopSong`] 形状)
    pub async fn top_repeat_songs(
        &self,
        range: Range<i64>,
        limit: i64,
    ) -> color_eyre::Result<Vec<TopSong>> {
        let Some(pool) = self.pool() else {
            return Ok(Vec::new());
        };
        let rows = sqlx::query_as!(
            TopSongRow,
            r#"SELECT ns AS "ns!", song_value AS "song_value!",
                COUNT(*) AS "plays!: i64", COALESCE(SUM(listen_ms), 0) AS "listen_ms!: i64"
               FROM plays
               WHERE started_at >= ? AND started_at < ? AND play_mode = 'repeat_one'
               GROUP BY ns, song_value ORDER BY 3 DESC, 4 DESC LIMIT ?"#,
            range.start,
            range.end,
            limit
        )
        .fetch_all(pool)
        .await
        .wrap_err("top_repeat_songs 查询失败")?;
        Ok(rows
            .into_iter()
            .map(|r| TopSong {
                song: song_id(&r.ns, &r.song_value),
                plays: r.plays,
                listen_ms: r.listen_ms,
            })
            .collect())
    }

    /// 单曲全量汇总(QuerySongStats 改口用);从未播放返回 `None`。
    pub async fn song_summary(&self, id: &SongId) -> color_eyre::Result<Option<SongSummary>> {
        let Some(pool) = self.pool() else {
            return Ok(None);
        };
        let ns = id.namespace().name();
        let value = id.value();
        let row = sqlx::query_as!(
            SongSummary,
            r#"SELECT
                COUNT(*) AS "plays!: i64",
                COALESCE(SUM(CASE WHEN finish_reason = 'skip' THEN 1 ELSE 0 END), 0) AS "skips!: i64",
                COALESCE(SUM(listen_ms), 0) AS "listen_ms!: i64",
                MAX(started_at) AS "last_played_at?: i64"
               FROM plays WHERE ns = ? AND song_value = ?"#,
            ns,
            value
        )
        .fetch_one(pool)
        .await
        .wrap_err("song_summary 查询失败")?;
        if row.plays == 0 {
            return Ok(None);
        }
        Ok(Some(row))
    }

    /// 时段分桶(UTC):Hour(0-23)/ Weekday(0-6,周日=0)/ Month(1-12)。
    pub async fn listen_buckets(
        &self,
        range: Range<i64>,
        by: BucketBy,
    ) -> color_eyre::Result<Vec<Bucket>> {
        let Some(pool) = self.pool() else {
            return Ok(Vec::new());
        };
        // strftime 格式随维度变(嵌在 SQL 结构里),故按 BucketBy 分三条。
        let rows = match by {
            BucketBy::Hour => sqlx::query_as!(
                Bucket,
                r#"SELECT key AS "key!: i64", COUNT(*) AS "plays!: i64",
                    COALESCE(SUM(listen_ms), 0) AS "listen_ms!: i64"
                   FROM (SELECT CAST(strftime('%H', started_at / 1000, 'unixepoch') AS INTEGER) AS key, listen_ms
                         FROM plays WHERE started_at >= ? AND started_at < ?)
                   GROUP BY key ORDER BY key"#,
                range.start,
                range.end
            )
            .fetch_all(pool)
            .await
            .wrap_err("listen_buckets(hour) 查询失败")?,
            BucketBy::Weekday => sqlx::query_as!(
                Bucket,
                r#"SELECT key AS "key!: i64", COUNT(*) AS "plays!: i64",
                    COALESCE(SUM(listen_ms), 0) AS "listen_ms!: i64"
                   FROM (SELECT CAST(strftime('%w', started_at / 1000, 'unixepoch') AS INTEGER) AS key, listen_ms
                         FROM plays WHERE started_at >= ? AND started_at < ?)
                   GROUP BY key ORDER BY key"#,
                range.start,
                range.end
            )
            .fetch_all(pool)
            .await
            .wrap_err("listen_buckets(weekday) 查询失败")?,
            BucketBy::Month => sqlx::query_as!(
                Bucket,
                r#"SELECT key AS "key!: i64", COUNT(*) AS "plays!: i64",
                    COALESCE(SUM(listen_ms), 0) AS "listen_ms!: i64"
                   FROM (SELECT CAST(strftime('%m', started_at / 1000, 'unixepoch') AS INTEGER) AS key, listen_ms
                         FROM plays WHERE started_at >= ? AND started_at < ?)
                   GROUP BY key ORDER BY key"#,
                range.start,
                range.end
            )
            .fetch_all(pool)
            .await
            .wrap_err("listen_buckets(month) 查询失败")?,
        };
        Ok(rows)
    }

    /// 各维度分布(来源 / 发起方式 / 模式 / 格式 / 音质 / 来源位置)+ 无损播放数。
    pub async fn distributions(&self, range: Range<i64>) -> color_eyre::Result<Distributions> {
        let Some(pool) = self.pool() else {
            return Ok(Distributions::default());
        };
        let lossless_plays = sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "count!: i64" FROM plays
               WHERE started_at >= ? AND started_at < ? AND is_lossless = 1"#,
            range.start,
            range.end
        )
        .fetch_one(pool)
        .await
        .wrap_err("distributions(lossless) 查询失败")?;
        Ok(Distributions {
            by_source: self.distribution_by(range.clone(), "ns").await?,
            by_origin: self.distribution_by(range.clone(), "origin_kind").await?,
            by_play_mode: self.distribution_by(range.clone(), "play_mode").await?,
            by_format: self.distribution_by(range.clone(), "audio_format").await?,
            by_quality: self.distribution_by(range.clone(), "quality").await?,
            by_playback_origin: self.distribution_by(range, "playback_origin").await?,
            lossless_plays,
        })
    }

    /// 按某列分桶计数(列名是内部常量;NULL 归入空串桶)。各分布形状同一,只列名不同,
    /// 列表化比 6 条字面量查询更 DRY。
    async fn distribution_by(
        &self,
        range: Range<i64>,
        column: &str,
    ) -> color_eyre::Result<Vec<Slice>> {
        let Some(pool) = self.pool() else {
            return Ok(Vec::new());
        };
        let rows = sqlx::query_as::<_, (String, i64)>(&format!(
            "SELECT COALESCE({column}, '') AS value, COUNT(*) AS plays FROM plays \
             WHERE started_at >= ? AND started_at < ? GROUP BY value ORDER BY plays DESC, value"
        ))
        .bind(range.start)
        .bind(range.end)
        .fetch_all(pool)
        .await
        .wrap_err_with(|| format!("distribution_by {column} 查询失败"))?;
        Ok(rows
            .into_iter()
            .map(|(value, plays)| Slice { value, plays })
            .collect())
    }

    /// top 队列语境:最常从哪个搜索 / 歌单 / 专辑 / 艺人听(GROUP BY context_kind+ref)。
    ///
    /// # Params:
    ///   - `range`: 时间窗口
    ///   - `kind`: 只看某类语境(`Some("album")` / `Some("artist")` 即 top 专辑 / 艺人;
    ///     `None` 全类混排)
    ///   - `limit`: 榜单长度
    ///
    /// # Return:
    ///   各语境的播放次数 + 收听 ms,按次数降序
    pub async fn top_contexts(
        &self,
        range: Range<i64>,
        kind: Option<&str>,
        limit: i64,
    ) -> color_eyre::Result<Vec<ContextSlice>> {
        let Some(pool) = self.pool() else {
            return Ok(Vec::new());
        };
        // 按 kind 过滤时只 GROUP BY ref(kind 恒定);全类混排则 kind+ref 双分组。SUM(listen_ms)
        // 分组内恒有行故强制非空;两支 bind 集不同,天然拆两条编译期查询。
        let rows = match kind {
            Some(k) => sqlx::query_as!(
                ContextSlice,
                r#"SELECT context_kind AS "kind!", context_ref AS "reference",
                          COUNT(*) AS "plays!: i64", SUM(listen_ms) AS "listen_ms!: i64"
                   FROM plays
                   WHERE started_at >= ? AND started_at < ? AND context_kind = ?
                   GROUP BY context_ref ORDER BY 3 DESC, context_ref LIMIT ?"#,
                range.start,
                range.end,
                k,
                limit
            )
            .fetch_all(pool)
            .await
            .wrap_err("top_contexts(kind) 查询失败")?,
            None => sqlx::query_as!(
                ContextSlice,
                r#"SELECT context_kind AS "kind!", context_ref AS "reference",
                          COUNT(*) AS "plays!: i64", SUM(listen_ms) AS "listen_ms!: i64"
                   FROM plays
                   WHERE started_at >= ? AND started_at < ?
                   GROUP BY context_kind, context_ref ORDER BY 3 DESC, context_kind LIMIT ?"#,
                range.start,
                range.end,
                limit
            )
            .fetch_all(pool)
            .await
            .wrap_err("top_contexts 查询失败")?,
        };
        Ok(rows)
    }

    /// 新发现的歌数:首播落在窗口内的不同歌(全量视角下 = 该窗口内第一次听到的歌)。
    ///
    /// # Params:
    ///   - `range`: 时间窗口
    ///
    /// # Return:
    ///   首播在窗口内的歌数
    pub async fn discoveries(
        &self,
        range: Range<i64>,
        limit: i64,
    ) -> color_eyre::Result<Discoveries> {
        let Some(pool) = self.pool() else {
            return Ok(Discoveries::default());
        };
        // 首播时刻落窗的新歌(全量视角下 = 该窗口内第一次听到的歌),按首播升序取前 limit。
        let new_rows = sqlx::query!(
            r#"SELECT ns AS "ns!", song_value AS "song_value!" FROM
                 (SELECT ns, song_value, MIN(started_at) AS first FROM plays GROUP BY ns, song_value)
               WHERE first >= ? AND first < ? ORDER BY first LIMIT ?"#,
            range.start,
            range.end,
            limit
        )
        .fetch_all(pool)
        .await
        .wrap_err("discoveries new_songs 查询失败")?;
        let new_songs = new_rows
            .into_iter()
            .map(|r| song_id(&r.ns, &r.song_value))
            .collect();
        Ok(Discoveries {
            new_songs,
            first_play: self.edge_play(range.clone(), /*earliest*/ true).await?,
            last_play: self.edge_play(range, /*earliest*/ false).await?,
        })
    }

    /// 窗口内首(`earliest=true`)/ 末播放行;无播放为 `None`。ORDER BY 方向随取端变(不可
    /// bind),故分两条编译期查询。
    async fn edge_play(
        &self,
        range: Range<i64>,
        earliest: bool,
    ) -> color_eyre::Result<Option<PlayTail>> {
        let Some(pool) = self.pool() else {
            return Ok(None);
        };
        let row = if earliest {
            sqlx::query_as!(
                PlayTailRow,
                r#"SELECT ns, song_value, started_at, listen_ms,
                          finish_reason AS "finish_reason: FinishReason"
                   FROM plays WHERE started_at >= ? AND started_at < ?
                   ORDER BY started_at ASC LIMIT 1"#,
                range.start,
                range.end
            )
            .fetch_optional(pool)
            .await
            .wrap_err("discoveries first_play 查询失败")?
        } else {
            sqlx::query_as!(
                PlayTailRow,
                r#"SELECT ns, song_value, started_at, listen_ms,
                          finish_reason AS "finish_reason: FinishReason"
                   FROM plays WHERE started_at >= ? AND started_at < ?
                   ORDER BY started_at DESC LIMIT 1"#,
                range.start,
                range.end
            )
            .fetch_optional(pool)
            .await
            .wrap_err("discoveries last_play 查询失败")?
        };
        Ok(row.map(|r| PlayTail {
            song: song_id(&r.ns, &r.song_value),
            started_at: r.started_at,
            listen_ms: r.listen_ms,
            finish_reason: r.finish_reason,
        }))
    }

    /// 会话续航:窗口内会话数 + 平均 / 最长时长 + 最长连续听歌天数 streak。
    ///
    /// 会话按 `started_at` 落窗;时长 = `ended_at − started_at`(未结束会话按已有 ended 算)。
    /// streak 由 plays 的 UTC 日游程算(gaps-and-islands),与会话窗口同 range。
    ///
    /// # Params:
    ///   - `range`: 时间窗口
    ///
    /// # Return:
    ///   续航聚合;无数据各项为 0
    pub async fn endurance(&self, range: Range<i64>) -> color_eyre::Result<Endurance> {
        let Some(pool) = self.pool() else {
            return Ok(Endurance {
                sessions: 0,
                avg_ms: 0,
                longest_ms: 0,
                streak_days: 0,
            });
        };
        let row = sqlx::query_as!(
            EnduranceRow,
            r#"SELECT COUNT(*) AS "sessions!: i64",
                      CAST(AVG(ended_at - started_at) AS INTEGER) AS "avg_ms?: i64",
                      MAX(ended_at - started_at) AS "longest_ms?: i64"
               FROM sessions WHERE started_at >= ? AND started_at < ?"#,
            range.start,
            range.end
        )
        .fetch_one(pool)
        .await
        .wrap_err("endurance 查询失败")?;
        // 最长连续听歌天数:UTC 日去重后,`day − ROW_NUMBER()` 同值即连续段,取最大段长。
        let streak_days = sqlx::query_scalar!(
            r#"SELECT COALESCE(MAX(run_len), 0) AS "streak!: i64" FROM (
                 SELECT COUNT(*) AS run_len FROM (
                   SELECT day - ROW_NUMBER() OVER (ORDER BY day) AS grp FROM (
                     SELECT DISTINCT started_at / 1000 / 86400 AS day
                     FROM plays WHERE started_at >= ? AND started_at < ?
                   )
                 ) GROUP BY grp
               )"#,
            range.start,
            range.end
        )
        .fetch_one(pool)
        .await
        .wrap_err("endurance streak 查询失败")?;
        Ok(Endurance {
            sessions: row.sessions,
            avg_ms: row.avg_ms.unwrap_or(0),
            longest_ms: row.longest_ms.unwrap_or(0),
            streak_days,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::QueueContext;
    use crate::play::PlayRecord;
    use crate::vocab::{Actor, FinishReason, PlayOrigin, PlaybackOrigin};
    use mineral_model::{AudioFormat, BitRate};

    /// 2026-07-14 00:00:00 UTC(周二),种子时间锚点。
    const T0: i64 = 1_783_987_200_000;
    const HOUR: i64 = 3_600_000;
    const DAY: i64 = 86_400_000;

    async fn open_temp() -> color_eyre::Result<(tempfile::TempDir, StatsStore)> {
        let dir = tempfile::tempdir()?;
        let store = StatsStore::open(&dir.path().join("stats.db")).await?;
        Ok((dir, store))
    }

    /// 造一行播放事实(只填查询关心的列,其余取中性值)。
    // 测试构造器:8 个入参各是独立的种子维度、无自然分组,豁免多参数 lint。
    #[allow(clippy::too_many_arguments)]
    fn play(
        ns: &str,
        value: &str,
        started_at: i64,
        listen_ms: i64,
        finish: FinishReason,
        format: Option<AudioFormat>,
        quality: Option<BitRate>,
        session_id: i64,
    ) -> PlayRecord {
        PlayRecord {
            song_id: song_id(ns, value),
            started_at,
            ended_at: started_at + listen_ms,
            listen_ms,
            duration_ms_snapshot: None,
            finish_reason: finish,
            skip_at_ms: matches!(finish, FinishReason::Skip).then_some(listen_ms),
            play_mode: crate::PlayMode::Sequential,
            session_id,
            origin: PlayOrigin::Explicit,
            actor: Actor::User,
            context: QueueContext::Unknown,
            audio: crate::PlayAudioSnapshot {
                audio_format: format,
                bitrate_bps: None,
                quality,
                bit_depth: None,
                substituted: false,
            },
            playback_origin: PlaybackOrigin::Remote,
        }
    }

    /// 确定性种子:A×2(netease,flac 无损,day1 14 时)、B×1(netease,mp3,skip,
    /// 5s 不足有效阈值,day1 15 时)、C×1(bilibili,无格式,day2 09 时)。
    async fn seed(store: &StatsStore) -> color_eyre::Result<i64> {
        let sid = store
            .open_session(T0)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("session"))?;
        let f = |fmt: &str| Some(AudioFormat::from(fmt.to_owned()));
        store
            .record_play(&play(
                "netease",
                "1",
                T0 + 14 * HOUR,
                60_000,
                FinishReason::Eof,
                f("flac"),
                Some(BitRate::Lossless),
                sid,
            ))
            .await?;
        store
            .record_play(&play(
                "netease",
                "1",
                T0 + 14 * HOUR + 60_000,
                70_000,
                FinishReason::Eof,
                f("flac"),
                Some(BitRate::Lossless),
                sid,
            ))
            .await?;
        store
            .record_play(&play(
                "netease",
                "2",
                T0 + 15 * HOUR,
                5_000,
                FinishReason::Skip,
                f("mp3"),
                Some(BitRate::Exhigh),
                sid,
            ))
            .await?;
        store
            .record_play(&play(
                "bilibili",
                "3",
                T0 + DAY + 9 * HOUR,
                120_000,
                FinishReason::Eof,
                None,
                None,
                sid,
            ))
            .await?;
        Ok(sid)
    }

    /// 覆盖全部种子的时间窗。
    fn full_range() -> Range<i64> {
        T0..(T0 + 2 * DAY)
    }

    fn options(min_listen_ms: i64) -> ReportOptions {
        ReportOptions::builder()
            .min_listen_ms(min_listen_ms)
            .top_limit(10)
            .build()
    }

    #[tokio::test]
    async fn totals_aggregates_all_fields() -> color_eyre::Result<()> {
        let (_d, store) = open_temp().await?;
        seed(&store).await?;
        let t = store.totals(full_range()).await?;
        assert_eq!(t.listen_ms, 60_000 + 70_000 + 5_000 + 120_000);
        assert_eq!(t.plays, 4);
        assert_eq!(t.completed, 3, "eof:A1 A2 C");
        assert_eq!(t.skipped, 1, "skip:B");
        assert_eq!(t.distinct_songs, 3);
        assert_eq!(t.active_days, 2, "day1 + day2");
        Ok(())
    }

    #[tokio::test]
    async fn top_songs_dual_metric_with_min_filter() -> color_eyre::Result<()> {
        let (_d, store) = open_temp().await?;
        seed(&store).await?;
        // 有效阈值 30s:B(5s)被剔除;A(2 次/130s)、C(1 次/120s)。
        let by_plays = store
            .top_songs(full_range(), TopBy::Plays, &options(30_000))
            .await?;
        let plays_order = by_plays
            .iter()
            .map(|t| (t.song.value(), t.plays))
            .collect::<Vec<_>>();
        assert_eq!(
            plays_order,
            vec![("1", 2), ("3", 1)],
            "B 被有效阈值剔除;A(2)>C(1)"
        );

        let by_time = store
            .top_songs(full_range(), TopBy::Time, &options(30_000))
            .await?;
        let time_order = by_time
            .iter()
            .map(|t| (t.song.value(), t.listen_ms))
            .collect::<Vec<_>>();
        assert_eq!(
            time_order,
            vec![("1", 130_000), ("3", 120_000)],
            "A 时长 130s 居首"
        );
        Ok(())
    }

    #[tokio::test]
    async fn song_summary_and_none_for_unknown() -> color_eyre::Result<()> {
        let (_d, store) = open_temp().await?;
        seed(&store).await?;
        let a = store
            .song_summary(&song_id("netease", "1"))
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("A 应有汇总"))?;
        assert_eq!(a.plays, 2);
        assert_eq!(a.skips, 0);
        assert_eq!(a.listen_ms, 130_000);
        assert_eq!(a.last_played_at, Some(T0 + 14 * HOUR + 60_000));

        let b = store
            .song_summary(&song_id("netease", "2"))
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("B 应有汇总"))?;
        assert_eq!(b.skips, 1);

        assert!(
            store
                .song_summary(&song_id("netease", "999"))
                .await?
                .is_none()
        );
        Ok(())
    }

    #[tokio::test]
    async fn hour_buckets_utc() -> color_eyre::Result<()> {
        let (_d, store) = open_temp().await?;
        seed(&store).await?;
        let buckets = store.listen_buckets(full_range(), BucketBy::Hour).await?;
        // UTC:9 时 C(1)、14 时 A1+A2(2)、15 时 B(1)。
        let got = buckets.iter().map(|b| (b.key, b.plays)).collect::<Vec<_>>();
        assert_eq!(got, vec![(9, 1), (14, 2), (15, 1)]);
        let total = buckets.iter().map(|b| b.plays).sum::<i64>();
        assert_eq!(total, 4, "分桶次数和 = 总播放");
        Ok(())
    }

    #[tokio::test]
    async fn distributions_by_dimension() -> color_eyre::Result<()> {
        let (_d, store) = open_temp().await?;
        seed(&store).await?;
        let d = store.distributions(full_range()).await?;
        // 来源:netease 3(A1 A2 B)、bilibili 1(C),按次数降序。
        assert_eq!(
            d.by_source,
            vec![
                Slice {
                    value: "netease".to_owned(),
                    plays: 3
                },
                Slice {
                    value: "bilibili".to_owned(),
                    plays: 1
                },
            ]
        );
        // 格式:flac 2;'' 与 mp3 各 1(次数相同按值升序 '' < mp3)。
        let formats = d
            .by_format
            .iter()
            .map(|s| (s.value.as_str(), s.plays))
            .collect::<Vec<_>>();
        assert_eq!(formats, vec![("flac", 2), ("", 1), ("mp3", 1)]);
        // 无损:A1 A2(flac)=2。
        assert_eq!(d.lossless_plays, 2);
        Ok(())
    }

    #[tokio::test]
    async fn disabled_queries_are_empty() -> color_eyre::Result<()> {
        let store = StatsStore::disabled();
        assert_eq!(store.totals(full_range()).await?.plays, 0);
        assert!(
            store
                .top_songs(full_range(), TopBy::Plays, &options(0))
                .await?
                .is_empty()
        );
        assert!(
            store
                .song_summary(&song_id("netease", "1"))
                .await?
                .is_none()
        );
        Ok(())
    }

    /// top_contexts:按语境分组计次(同歌单两次 > 搜索一次),context_ref 落 qualified id。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn top_contexts_groups_by_context() -> color_eyre::Result<()> {
        use mineral_model::PlaylistId;
        let (_dir, store) = open_temp().await?;
        let sid = store
            .open_session(T0)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("session"))?;
        let pl = QueueContext::Playlist {
            id: PlaylistId::new(SourceKind::NETEASE, "7"),
        };
        for (value, at, ctx) in [
            ("1", T0, pl.clone()),
            ("2", T0 + HOUR, pl.clone()),
            (
                "3",
                T0 + 2 * HOUR,
                QueueContext::Search {
                    query: Some("李志".to_owned()),
                },
            ),
        ] {
            let mut r = play(
                "netease",
                value,
                at,
                60_000,
                FinishReason::Eof,
                None,
                None,
                sid,
            );
            r.context = ctx;
            store.record_play(&r).await?;
        }
        let contexts = store.top_contexts(0..i64::MAX, None, 10).await?;
        let first = contexts
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("无语境"))?;
        assert_eq!(first.kind, "playlist");
        assert_eq!(first.reference.as_deref(), Some("netease:7"));
        assert_eq!(first.plays, 2);
        assert_eq!(first.listen_ms, 120_000);
        // kind 过滤 = top 专辑 / 艺人的 context 版:只留 search 语境那一条。
        let searches = store.top_contexts(0..i64::MAX, Some("search"), 10).await?;
        assert_eq!(searches.len(), 1, "只 search 类");
        let only = searches
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("无 search 语境"))?;
        assert_eq!(only.kind, "search");
        assert_eq!(only.reference.as_deref(), Some("李志"));
        Ok(())
    }

    /// top_albums / top_artists:按专辑 / 艺人语境的 context_ref 聚合,typed id 重建。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn top_albums_and_artists_group_by_context_ref() -> color_eyre::Result<()> {
        use mineral_model::{AlbumId, ArtistId};
        let (_dir, store) = open_temp().await?;
        let sid = store
            .open_session(T0)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("session"))?;
        let album_a = QueueContext::Album {
            id: AlbumId::new(SourceKind::NETEASE, "aaa"),
        };
        let album_b = QueueContext::Album {
            id: AlbumId::new(SourceKind::BILIBILI, "bbb"),
        };
        let artist = QueueContext::Artist {
            id: ArtistId::new(SourceKind::NETEASE, "art"),
        };
        // album_a ×2、album_b ×1、artist ×1。
        for (value, at, ctx) in [
            ("1", T0, album_a.clone()),
            ("2", T0 + HOUR, album_a.clone()),
            ("3", T0 + 2 * HOUR, album_b.clone()),
            ("4", T0 + 3 * HOUR, artist.clone()),
        ] {
            let mut r = play(
                "netease",
                value,
                at,
                60_000,
                FinishReason::Eof,
                None,
                None,
                sid,
            );
            r.context = ctx;
            store.record_play(&r).await?;
        }
        let opts = ReportOptions::builder()
            .min_listen_ms(0)
            .top_limit(10)
            .build();
        let albums = store.top_albums(0..i64::MAX, TopBy::Plays, &opts).await?;
        assert_eq!(albums.len(), 2, "两个专辑语境");
        let top = albums
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("无专辑"))?;
        assert_eq!(
            top.album,
            AlbumId::new(SourceKind::NETEASE, "aaa"),
            "最多的专辑"
        );
        assert_eq!(top.plays, 2);
        let artists = store.top_artists(0..i64::MAX, TopBy::Plays, &opts).await?;
        assert_eq!(artists.len(), 1, "一个艺人语境");
        let a = artists
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("无艺人"))?;
        assert_eq!(a.artist, ArtistId::new(SourceKind::NETEASE, "art"));
        assert_eq!(a.plays, 1);
        Ok(())
    }

    /// top_repeat_songs:只数 `play_mode='repeat_one'` 的行,按次数 top(sequential 不计)。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn top_repeat_songs_filters_repeat_one() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let sid = store
            .open_session(T0)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("session"))?;
        for (value, at, mode) in [
            ("loop", T0, crate::PlayMode::RepeatOne),
            ("loop", T0 + HOUR, crate::PlayMode::RepeatOne),
            ("seq", T0 + 2 * HOUR, crate::PlayMode::RepeatOne),
            ("seq", T0 + 3 * HOUR, crate::PlayMode::Sequential),
        ] {
            let mut r = play(
                "netease",
                value,
                at,
                60_000,
                FinishReason::Eof,
                None,
                None,
                sid,
            );
            r.play_mode = mode;
            store.record_play(&r).await?;
        }
        let repeats = store.top_repeat_songs(0..i64::MAX, 10).await?;
        assert_eq!(repeats.len(), 2, "两首曾单曲循环");
        let top = repeats
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("无循环歌"))?;
        assert_eq!(top.song, song_id("netease", "loop"));
        assert_eq!(top.plays, 2, "loop 循环 2 次");
        let seq = repeats
            .iter()
            .find(|t| t.song == song_id("netease", "seq"))
            .ok_or_else(|| color_eyre::eyre::eyre!("无 seq"))?;
        assert_eq!(seq.plays, 1, "seq 仅 1 次单曲循环(sequential 那次不计)");
        Ok(())
    }

    /// discoveries:按首播时刻计新歌(窗口内首播才算),窗口外无新发现。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn discoveries_counts_first_plays() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let sid = store
            .open_session(T0)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("session"))?;
        for (value, at) in [("1", T0), ("2", T0 + HOUR)] {
            store
                .record_play(&play(
                    "netease",
                    value,
                    at,
                    60_000,
                    FinishReason::Eof,
                    None,
                    None,
                    sid,
                ))
                .await?;
        }
        let disc = store.discoveries(T0..(T0 + 2 * HOUR), 10).await?;
        assert_eq!(disc.new_songs.len(), 2, "窗口内两首新歌");
        let first = disc
            .first_play
            .ok_or_else(|| color_eyre::eyre::eyre!("期望首播行"))?;
        assert_eq!(first.started_at, T0, "首播行是窗口内最早那次");
        assert!(disc.last_play.is_some(), "有末播放行");
        assert_eq!(
            store
                .discoveries((T0 + 3 * HOUR)..i64::MAX, 10)
                .await?
                .new_songs
                .len(),
            0,
            "窗口外无新首播"
        );
        Ok(())
    }

    /// endurance:会话时长(ended−started)取平均 / 最长。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn endurance_from_sessions() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let sid = store
            .open_session(T0)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("session"))?;
        store.touch_session(sid, T0 + 100_000).await?;
        let e = store.endurance(0..i64::MAX).await?;
        assert_eq!(e.sessions, 1);
        assert_eq!(e.longest_ms, 100_000);
        assert_eq!(e.avg_ms, 100_000);
        assert_eq!(e.streak_days, 0, "无 plays → 无连续听歌天数");
        Ok(())
    }

    /// endurance.streak_days:UTC 连续听歌日的最长游程(隔断的日不连成段)。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn endurance_streak_counts_longest_consecutive_days() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let sid = store
            .open_session(T0)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("session"))?;
        // 连续 3 天(D0/D1/D2)各一播,跳过 D3,再 D4 一播 → 最长 streak = 3。
        for day in [0, 1, 2, 4] {
            store
                .record_play(&play(
                    "netease",
                    &day.to_string(),
                    T0 + day * DAY,
                    60_000,
                    FinishReason::Eof,
                    None,
                    None,
                    sid,
                ))
                .await?;
        }
        let e = store.endurance(0..i64::MAX).await?;
        assert_eq!(e.streak_days, 3, "D0-D2 连续三天,D3 断开");
        Ok(())
    }
}
