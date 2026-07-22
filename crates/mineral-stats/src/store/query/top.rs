//! 排行榜:歌曲 / 专辑 / 艺人 / 单曲循环 / 队列语境。

use std::ops::Range;

use color_eyre::eyre::WrapErr as _;
use mineral_model::{AlbumId, ArtistId, SourceKind};

use crate::report::{ContextSlice, ReportOptions, TopAlbum, TopArtist, TopBy, TopSong};
use crate::store::StatsStore;

use super::shared::{song_id, split_qualified};

/// `top_songs` / `top_repeat_songs` 的每行;`song` 由 ns+value 重建故经中转。
struct TopSongRow {
    /// 来源 name。
    ns: String,

    /// 裸歌曲 id。
    song_value: String,

    /// 播放次数。
    plays: i64,

    /// 累计收听 ms。
    listen_ms: i64,

    /// songs 维表回查的歌名;未覆盖为 `None`。
    name: Option<String>,
}

/// `top_albums` 的每行(context_ref 聚合);typed id 由 `context_ref` 重建故经中转。
struct TopContextRefRow {
    /// 语境引用(qualified id 串)。
    reference: String,

    /// 从该语境起播的次数。
    plays: i64,

    /// 累计收听 ms。
    listen_ms: i64,

    /// 组内任意非空的显示名快照(`MAX` 略过 NULL);全组缺名为 `None`。
    name: Option<String>,
}

/// `top_artists`(口味口径)的每行:按 `song_artists` 的 `artist_value` 聚合。
struct TopArtistRow {
    /// 来源 name。
    ns: String,

    /// 艺人裸值。
    artist_value: String,

    /// 艺人名(维表恒非空,`MAX` 只为在多行同 `artist_value` 时给 SQLite 一个确定性取值,
    /// 不是「缺失回填」——与 `top_contexts` 的 `MAX(context_name)` 目的不同)。
    artist_name: String,

    /// 播放次数(合作曲对每位艺人各记一次)。
    plays: i64,

    /// 累计收听 ms(合作曲对每位艺人各记全量,非按人数折算)。
    listen_ms: i64,
}

impl StatsStore {
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
        // ORDER BY 列随口径变(列位不可 bind),故按 TopBy 分两条。songs 维表 LEFT JOIN
        // 在 (ns, song_value) 键上,与 GROUP BY 键一致——s.name 组内恒定,裸取合法。
        let rows = match by {
            TopBy::Plays => sqlx::query_as!(
                TopSongRow,
                r#"SELECT p.ns AS "ns!", p.song_value AS "song_value!",
                    COUNT(*) AS "plays!: i64", COALESCE(SUM(p.listen_ms), 0) AS "listen_ms!: i64",
                    s.name AS "name?"
                   FROM plays p
                   LEFT JOIN songs s ON s.ns = p.ns AND s.song_value = p.song_value
                   WHERE p.started_at >= ? AND p.started_at < ? AND p.listen_ms >= ?
                   GROUP BY p.ns, p.song_value ORDER BY 3 DESC, 4 DESC LIMIT ?"#,
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
                name: r.name,
                plays: r.plays,
                listen_ms: r.listen_ms,
            })
            .collect(),
            TopBy::Time => sqlx::query_as!(
                TopSongRow,
                r#"SELECT p.ns AS "ns!", p.song_value AS "song_value!",
                    COUNT(*) AS "plays!: i64", COALESCE(SUM(p.listen_ms), 0) AS "listen_ms!: i64",
                    s.name AS "name?"
                   FROM plays p
                   LEFT JOIN songs s ON s.ns = p.ns AND s.song_value = p.song_value
                   WHERE p.started_at >= ? AND p.started_at < ? AND p.listen_ms >= ?
                   GROUP BY p.ns, p.song_value ORDER BY 4 DESC, 3 DESC LIMIT ?"#,
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
                name: r.name,
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
                    name: r.name.clone(),
                    plays: r.plays,
                    listen_ms: r.listen_ms,
                })
            })
            .collect())
    }

    /// top 艺人(口味口径:按歌曲的 `song_artists` 归属聚合——「你听了谁的歌」,区别于
    /// `top_contexts(Some("artist"))` 的「你从谁的详情页起播」)。合作曲对每位艺人各记
    /// 一次全量 `listen_ms`,故各艺人时长之和会超过实际收听总时长,这是榜单口径的固有
    /// 性质。维表未覆盖的 `plays` 行(如贫投影从未富化)天然不进任何艺人的榜,不报错。
    ///
    /// # Params:
    ///   - `range`: 时间窗口
    ///   - `by`: 次数 / 时长口径
    ///   - `options`: 有效播放阈值 + 榜长
    ///
    /// # Return:
    ///   top 艺人列表
    pub async fn top_artists(
        &self,
        range: Range<i64>,
        by: TopBy,
        options: &ReportOptions,
    ) -> color_eyre::Result<Vec<TopArtist>> {
        let Some(pool) = self.pool() else {
            return Ok(Vec::new());
        };
        let min = options.min_listen_ms();
        let limit = options.top_limit();
        // ORDER BY 列随口径变(列位不可 bind),故按 TopBy 分两条。INNER JOIN(非 LEFT):
        // 没有 song_artists 行的歌天然不贡献任何艺人,不需要额外过滤。
        let rows = match by {
            TopBy::Plays => sqlx::query_as!(
                TopArtistRow,
                r#"SELECT sa.ns AS "ns!", sa.artist_value AS "artist_value!",
                    MAX(sa.artist_name) AS "artist_name!: String",
                    COUNT(*) AS "plays!: i64", COALESCE(SUM(p.listen_ms), 0) AS "listen_ms!: i64"
                   FROM plays p
                   JOIN song_artists sa ON sa.ns = p.ns AND sa.song_value = p.song_value
                   WHERE p.started_at >= ? AND p.started_at < ? AND p.listen_ms >= ?
                   GROUP BY sa.ns, sa.artist_value ORDER BY 4 DESC, 5 DESC LIMIT ?"#,
                range.start,
                range.end,
                min,
                limit
            )
            .fetch_all(pool)
            .await
            .wrap_err("top_artists(plays) 查询失败")?,
            TopBy::Time => sqlx::query_as!(
                TopArtistRow,
                r#"SELECT sa.ns AS "ns!", sa.artist_value AS "artist_value!",
                    MAX(sa.artist_name) AS "artist_name!: String",
                    COUNT(*) AS "plays!: i64", COALESCE(SUM(p.listen_ms), 0) AS "listen_ms!: i64"
                   FROM plays p
                   JOIN song_artists sa ON sa.ns = p.ns AND sa.song_value = p.song_value
                   WHERE p.started_at >= ? AND p.started_at < ? AND p.listen_ms >= ?
                   GROUP BY sa.ns, sa.artist_value ORDER BY 5 DESC, 4 DESC LIMIT ?"#,
                range.start,
                range.end,
                min,
                limit
            )
            .fetch_all(pool)
            .await
            .wrap_err("top_artists(time) 查询失败")?,
        };
        Ok(rows
            .into_iter()
            .map(|r| TopArtist {
                artist: ArtistId::new(SourceKind::from_name(&r.ns), r.artist_value),
                name: Some(r.artist_name),
                plays: r.plays,
                listen_ms: r.listen_ms,
            })
            .collect())
    }

    /// 按某语境 kind 聚合 `context_ref`(`top_albums` 专用;`kind` 是内部常量,非用户
    /// 输入,但 `query!` 需字面量 SQL,故按 kind 值直接内联比对)。
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
        // 名字取组内 MAX(context_name):快照列 NULL 被略过,带名行照亮全组(含历史行)。
        let rows = match by {
            TopBy::Plays => {
                sqlx::query_as!(
                    TopContextRefRow,
                    r#"SELECT context_ref AS "reference!", COUNT(*) AS "plays!: i64",
                    COALESCE(SUM(listen_ms), 0) AS "listen_ms!: i64",
                    MAX(context_name) AS "name?: String"
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
                    COALESCE(SUM(listen_ms), 0) AS "listen_ms!: i64",
                    MAX(context_name) AS "name?: String"
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
            r#"SELECT p.ns AS "ns!", p.song_value AS "song_value!",
                COUNT(*) AS "plays!: i64", COALESCE(SUM(p.listen_ms), 0) AS "listen_ms!: i64",
                s.name AS "name?"
               FROM plays p
               LEFT JOIN songs s ON s.ns = p.ns AND s.song_value = p.song_value
               WHERE p.started_at >= ? AND p.started_at < ? AND p.play_mode = 'repeat_one'
               GROUP BY p.ns, p.song_value ORDER BY 3 DESC, 4 DESC LIMIT ?"#,
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
                name: r.name,
                plays: r.plays,
                listen_ms: r.listen_ms,
            })
            .collect())
    }

    /// top 队列语境:最常从哪个搜索 / 歌单 / 专辑 / 艺人听(GROUP BY context_kind+ref)。
    /// `min` 有效播放阈值与 `top_songs` / `top_artists` 同语义,统一「有效播放」定义。
    ///
    /// # Params:
    ///   - `range`: 时间窗口
    ///   - `kind`: 只看某类语境(`Some("album")` / `Some("artist")` 即从该专辑 / 艺人
    ///     详情页起播的量;`None` 全类混排)
    ///   - `min`: 有效播放阈值 ms(`listen_ms` 不足此值的行不计入)
    ///   - `limit`: 榜单长度
    ///
    /// # Return:
    ///   各语境的播放次数 + 收听 ms,按次数降序
    pub async fn top_contexts(
        &self,
        range: Range<i64>,
        kind: Option<&str>,
        min: i64,
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
                          COUNT(*) AS "plays!: i64", SUM(listen_ms) AS "listen_ms!: i64",
                          MAX(context_name) AS "name?: String"
                   FROM plays
                   WHERE started_at >= ? AND started_at < ? AND context_kind = ? AND listen_ms >= ?
                   GROUP BY context_ref ORDER BY 3 DESC, context_ref LIMIT ?"#,
                range.start,
                range.end,
                k,
                min,
                limit
            )
            .fetch_all(pool)
            .await
            .wrap_err("top_contexts(kind) 查询失败")?,
            None => sqlx::query_as!(
                ContextSlice,
                r#"SELECT context_kind AS "kind!", context_ref AS "reference",
                          COUNT(*) AS "plays!: i64", SUM(listen_ms) AS "listen_ms!: i64",
                          MAX(context_name) AS "name?: String"
                   FROM plays
                   WHERE started_at >= ? AND started_at < ? AND listen_ms >= ?
                   GROUP BY context_kind, context_ref ORDER BY 3 DESC, context_kind LIMIT ?"#,
                range.start,
                range.end,
                min,
                limit
            )
            .fetch_all(pool)
            .await
            .wrap_err("top_contexts 查询失败")?,
        };
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::QueueContext;
    use crate::vocab::FinishReason;

    use super::super::test_support::{HOUR, T0, full_range, open_temp, options, play, seed};

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

    /// top_songs LEFT JOIN songs 维表出名;维表未覆盖的落 None(展示层回落 id)。
    #[tokio::test]
    async fn top_songs_joins_dim_names() -> color_eyre::Result<()> {
        let (_d, store) = open_temp().await?;
        seed(&store).await?;
        store
            .upsert_song(&mineral_test::with_name(mineral_test::song("1"), "栞"))
            .await?;
        let tops = store
            .top_songs(full_range(), TopBy::Plays, &options(0))
            .await?;
        let names = tops
            .iter()
            .map(|t| (t.song.value(), t.name.as_deref()))
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![("1", Some("栞")), ("3", None), ("2", None)],
            "维表命中出名,未覆盖 None"
        );
        Ok(())
    }

    /// top_albums:按专辑语境的 context_ref 聚合,typed id 重建;名字取组内
    /// `MAX(context_name)`——旧行(快照 NULL)被后来带名的行照亮。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn top_albums_groups_by_context_ref() -> color_eyre::Result<()> {
        use mineral_model::AlbumId;
        let (_dir, store) = open_temp().await?;
        let sid = store
            .open_session(T0)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("session"))?;
        // 同一专辑语境两行:先一行缺名(旧 client / 历史行),再一行带名快照。
        let album_a_nameless = QueueContext::Album {
            id: AlbumId::new(SourceKind::NETEASE, "aaa"),
            name: None,
        };
        let album_a = QueueContext::Album {
            id: AlbumId::new(SourceKind::NETEASE, "aaa"),
            name: Some("Album A".to_owned()),
        };
        let album_b = QueueContext::Album {
            id: AlbumId::new(SourceKind::BILIBILI, "bbb"),
            name: None,
        };
        // album_a ×2、album_b ×1。
        for (value, at, ctx) in [
            ("1", T0, album_a_nameless.clone()),
            ("2", T0 + HOUR, album_a.clone()),
            ("3", T0 + 2 * HOUR, album_b.clone()),
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
        assert_eq!(
            top.name.as_deref(),
            Some("Album A"),
            "MAX 略过 NULL 快照,带名行照亮全组"
        );
        Ok(())
    }

    /// top_artists(口味口径):合作曲每位艺人各记一次全量播放 + 全量 listen_ms;
    /// 没有 song_artists 覆盖的歌(维表未覆盖,如未富化的贫投影)不进榜、不报错。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn top_artists_credits_each_collaborator_in_full() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let sid = store
            .open_session(T0)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("session"))?;
        // 歌 1:单艺人 A,播两次(60s + 70s)。
        store
            .upsert_song(&mineral_test::with_artists(mineral_test::song("1"), &["A"]))
            .await?;
        store
            .record_play(&play(
                "netease",
                "1",
                T0,
                60_000,
                FinishReason::Eof,
                None,
                None,
                sid,
            ))
            .await?;
        store
            .record_play(&play(
                "netease",
                "1",
                T0 + HOUR,
                70_000,
                FinishReason::Eof,
                None,
                None,
                sid,
            ))
            .await?;
        // 歌 2:合作曲 A+B,播一次(100s)——A、B 各记一次全量。
        store
            .upsert_song(&mineral_test::with_artists(
                mineral_test::song("2"),
                &["A", "B"],
            ))
            .await?;
        store
            .record_play(&play(
                "netease",
                "2",
                T0 + 2 * HOUR,
                100_000,
                FinishReason::Eof,
                None,
                None,
                sid,
            ))
            .await?;
        // 歌 3:无艺人维表覆盖(未 upsert_song),不该进任何艺人的榜、不该报错。
        store
            .record_play(&play(
                "netease",
                "3",
                T0 + 3 * HOUR,
                999_000,
                FinishReason::Eof,
                None,
                None,
                sid,
            ))
            .await?;

        let opts = ReportOptions::builder()
            .min_listen_ms(0)
            .top_limit(10)
            .build();
        let artists = store.top_artists(0..i64::MAX, TopBy::Plays, &opts).await?;
        assert_eq!(artists.len(), 2, "只有 A、B,歌 3 不贡献任何艺人");
        let by_value = |v: &str| {
            artists
                .iter()
                .find(|a| a.artist.value() == v)
                .ok_or_else(|| color_eyre::eyre::eyre!("missing artist {v}"))
        };
        let a = by_value("A")?;
        assert_eq!(a.plays, 3, "歌1×2 + 歌2×1");
        assert_eq!(a.listen_ms, 60_000 + 70_000 + 100_000, "各记全量,非折半");
        let b = by_value("B")?;
        assert_eq!(b.plays, 1);
        assert_eq!(b.listen_ms, 100_000, "合作曲对 B 同样记全量,不因合作打折");
        Ok(())
    }

    /// top_artists 双排序 + 有效阈值:低于阈值的行整体不计入任何艺人的次数与时长。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn top_artists_dual_metric_with_min_filter() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let sid = store
            .open_session(T0)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("session"))?;
        store
            .upsert_song(&mineral_test::with_artists(mineral_test::song("1"), &["A"]))
            .await?;
        store
            .upsert_song(&mineral_test::with_artists(mineral_test::song("2"), &["B"]))
            .await?;
        // A:一次 130s(有效)。B:一次 5s(低于 30s 阈值,应被剔除)。
        store
            .record_play(&play(
                "netease",
                "1",
                T0,
                130_000,
                FinishReason::Eof,
                None,
                None,
                sid,
            ))
            .await?;
        store
            .record_play(&play(
                "netease",
                "2",
                T0 + HOUR,
                5_000,
                FinishReason::Eof,
                None,
                None,
                sid,
            ))
            .await?;

        let by_plays = store
            .top_artists(0..i64::MAX, TopBy::Plays, &options(30_000))
            .await?;
        let plays_order = by_plays
            .iter()
            .map(|a| (a.artist.value().to_owned(), a.plays))
            .collect::<Vec<_>>();
        assert_eq!(plays_order, vec![("A".to_owned(), 1)], "B 被有效阈值剔除");

        let by_time = store
            .top_artists(0..i64::MAX, TopBy::Time, &options(30_000))
            .await?;
        let time_order = by_time
            .iter()
            .map(|a| (a.artist.value().to_owned(), a.listen_ms))
            .collect::<Vec<_>>();
        assert_eq!(time_order, vec![("A".to_owned(), 130_000)]);
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
            name: Some("收藏夹".to_owned()),
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
        let contexts = store.top_contexts(0..i64::MAX, None, 0, 10).await?;
        let first = contexts
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("无语境"))?;
        assert_eq!(first.kind, "playlist");
        assert_eq!(first.reference.as_deref(), Some("netease:7"));
        assert_eq!(first.plays, 2);
        assert_eq!(first.listen_ms, 120_000);
        assert_eq!(first.name.as_deref(), Some("收藏夹"), "歌单名快照直出");
        // kind 过滤 = top 专辑 / 艺人的 context 版:只留 search 语境那一条。
        let searches = store
            .top_contexts(0..i64::MAX, Some("search"), 0, 10)
            .await?;
        assert_eq!(searches.len(), 1, "只 search 类");
        let only = searches
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("无 search 语境"))?;
        assert_eq!(only.kind, "search");
        assert_eq!(only.reference.as_deref(), Some("李志"));
        Ok(())
    }

    /// top_contexts 有效阈值:低于 `min` 的行整条不进任何语境的次数与时长(kind 过滤
    /// 与全类混排两条查询路径都要受阈值约束)。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn top_contexts_respects_min_listen_ms() -> color_eyre::Result<()> {
        use mineral_model::PlaylistId;
        let (_dir, store) = open_temp().await?;
        let sid = store
            .open_session(T0)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("session"))?;
        let pl = QueueContext::Playlist {
            id: PlaylistId::new(SourceKind::NETEASE, "7"),
            name: Some("收藏夹".to_owned()),
        };
        // 一次有效播放(130s)+ 一次低于阈值(5s),同一歌单语境。
        for (value, at, listen_ms) in [("1", T0, 130_000), ("2", T0 + HOUR, 5_000)] {
            let mut r = play(
                "netease",
                value,
                at,
                listen_ms,
                FinishReason::Eof,
                None,
                None,
                sid,
            );
            r.context = pl.clone();
            store.record_play(&r).await?;
        }
        let filtered = store
            .top_contexts(0..i64::MAX, Some("playlist"), 30_000, 10)
            .await?;
        let only = filtered
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("无语境"))?;
        assert_eq!(only.plays, 1, "5s 那次被阈值剔除,只剩 130s 那次");
        assert_eq!(only.listen_ms, 130_000);

        let unfiltered = store
            .top_contexts(0..i64::MAX, Some("playlist"), 0, 10)
            .await?;
        let both = unfiltered
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("无语境"))?;
        assert_eq!(both.plays, 2, "阈值为 0 时两次都计入");
        Ok(())
    }
}
