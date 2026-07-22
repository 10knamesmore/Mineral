//! songs 维表维护:播放路径 write-through 的歌曲展示元数据。

use color_eyre::eyre::WrapErr as _;
use mineral_model::Song;

use crate::store::StatsStore;

impl StatsStore {
    /// 落 / 富化一行歌曲维表(报表 JOIN 出名的唯一数据源)。降级时静默 no-op。
    ///
    /// `name` 恒以新值为准(实体属性,改名全局生效);`alias` / 专辑 / 时长走
    /// 「非空进步、NULL 不回退」——写入方是富化程度不一的投影,贫投影(如列表页缺
    /// 译名)不得抹掉已知值,旧行的 NULL 随数据流经自然回填。`artists` 是多值,无法
    /// COALESCE,走先删后插;`song.artists` 为空视为贫投影,整个跳过删插以保住已知
    /// 艺人(否则一次缺富化的起播会把维表艺人抹光)。
    ///
    /// # Params:
    ///   - `song`: 起播时刻在手的完整歌曲元数据
    pub async fn upsert_song(&self, song: &Song) -> color_eyre::Result<()> {
        let Some(pool) = self.pool() else {
            return Ok(());
        };
        let ns = song.id.namespace().name();
        let song_value = song.id.value();
        let album_id = song.album.as_ref().map(|a| a.id.value().to_owned());
        let album_name = song.album.as_ref().map(|a| a.name.clone());
        let duration_ms = song.duration_ms.map(i64::try_from).transpose()?;
        let mut tx = pool
            .begin()
            .await
            .wrap_err_with(|| format!("upsert_song 开事务失败 song={song_value}"))?;
        sqlx::query!(
            "INSERT INTO songs (ns, song_value, name, alias, album_id, album_name, duration_ms)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(ns, song_value) DO UPDATE SET
             name = excluded.name,
             alias = COALESCE(excluded.alias, songs.alias),
             album_id = COALESCE(excluded.album_id, songs.album_id),
             album_name = COALESCE(excluded.album_name, songs.album_name),
             duration_ms = COALESCE(excluded.duration_ms, songs.duration_ms)",
            ns,
            song_value,
            song.name,
            song.alias,
            album_id,
            album_name,
            duration_ms,
        )
        .execute(&mut *tx)
        .await
        .wrap_err_with(|| format!("upsert_song 落库失败 song={song_value}"))?;
        if !song.artists.is_empty() {
            sqlx::query!(
                "DELETE FROM song_artists WHERE ns = ? AND song_value = ?",
                ns,
                song_value,
            )
            .execute(&mut *tx)
            .await
            .wrap_err_with(|| format!("upsert_song 清艺人行失败 song={song_value}"))?;
            for (index, artist) in song.artists.iter().enumerate() {
                let position = i64::try_from(index)?;
                let artist_value = artist.id.value();
                sqlx::query!(
                    "INSERT INTO song_artists (ns, song_value, position, artist_value, artist_name)
                     VALUES (?, ?, ?, ?, ?)",
                    ns,
                    song_value,
                    position,
                    artist_value,
                    artist.name,
                )
                .execute(&mut *tx)
                .await
                .wrap_err_with(|| format!("upsert_song 写艺人行失败 song={song_value}"))?;
            }
        }
        tx.commit()
            .await
            .wrap_err_with(|| format!("upsert_song 提交事务失败 song={song_value}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::store::StatsStore;
    use mineral_test::{song, with_album, with_alias, with_artists, with_duration, with_name};

    /// 读回断言用的维表行。
    #[derive(sqlx::FromRow)]
    struct SongRow {
        /// 歌名。
        name: String,

        /// 别名。
        alias: Option<String>,

        /// 专辑裸 id。
        album_id: Option<String>,

        /// 专辑名。
        album_name: Option<String>,

        /// 时长 ms。
        duration_ms: Option<i64>,
    }

    /// 读回断言用的艺人维表行。
    #[derive(sqlx::FromRow)]
    struct ArtistRow {
        /// 专辑内曲序意义上的排位(艺人在歌里的顺序,主艺人在前)。
        position: i64,

        /// 艺人裸值。
        artist_value: String,

        /// 艺人名。
        artist_name: String,
    }

    async fn open_temp() -> color_eyre::Result<(tempfile::TempDir, StatsStore)> {
        let dir = tempfile::tempdir()?;
        let store = StatsStore::open(&dir.path().join("stats.db")).await?;
        Ok((dir, store))
    }

    async fn fetch(store: &StatsStore, value: &str) -> color_eyre::Result<SongRow> {
        let pool = store
            .pool()
            .ok_or_else(|| color_eyre::eyre::eyre!("expected live pool"))?;
        let row = sqlx::query_as::<_, SongRow>(
            "SELECT name, alias, album_id, album_name, duration_ms FROM songs \
             WHERE ns = 'netease' AND song_value = ?",
        )
        .bind(value)
        .fetch_one(pool)
        .await?;
        Ok(row)
    }

    async fn fetch_artists(store: &StatsStore, value: &str) -> color_eyre::Result<Vec<ArtistRow>> {
        let pool = store
            .pool()
            .ok_or_else(|| color_eyre::eyre::eyre!("expected live pool"))?;
        let rows = sqlx::query_as::<_, ArtistRow>(
            "SELECT position, artist_value, artist_name FROM song_artists \
             WHERE ns = 'netease' AND song_value = ? ORDER BY position",
        )
        .bind(value)
        .fetch_all(pool)
        .await?;
        Ok(rows)
    }

    /// 富投影落全列;贫投影跟进时 name 恒新值、其余 COALESCE 不回退。
    #[tokio::test]
    async fn upsert_song_enriches_without_regressing() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let rich = with_duration(with_album(with_alias(song("42"), "译名"), "专辑"), 200_000);
        store.upsert_song(&rich).await?;
        let row = fetch(&store, "42").await?;
        assert_eq!(row.name, "42");
        assert_eq!(row.alias, Some("译名".to_owned()));
        assert_eq!(row.album_id, Some("专辑".to_owned()));
        assert_eq!(row.album_name, Some("专辑".to_owned()));
        assert_eq!(row.duration_ms, Some(200_000));

        store.upsert_song(&with_name(song("42"), "正名")).await?;
        let row = fetch(&store, "42").await?;
        assert_eq!(row.name, "正名", "name 恒以新值为准");
        assert_eq!(row.alias, Some("译名".to_owned()), "贫投影不得抹掉已知值");
        assert_eq!(row.album_id, Some("专辑".to_owned()));
        assert_eq!(row.album_name, Some("专辑".to_owned()));
        assert_eq!(row.duration_ms, Some(200_000));
        Ok(())
    }

    /// 降级句柄静默 no-op。
    #[tokio::test]
    async fn upsert_song_disabled_is_noop() -> color_eyre::Result<()> {
        let store = StatsStore::disabled();
        store.upsert_song(&song("x")).await?;
        Ok(())
    }

    /// 多艺人按 `position` 落序,主艺人(0)在前。
    #[tokio::test]
    async fn upsert_song_writes_artists_in_position_order() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        store
            .upsert_song(&with_artists(song("42"), &["主唱", "客串"]))
            .await?;
        let rows = fetch_artists(&store, "42").await?;
        assert_eq!(rows.len(), 2);
        let first = rows
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("missing row 0"))?;
        assert_eq!(first.position, 0);
        assert_eq!(first.artist_value, "主唱");
        assert_eq!(first.artist_name, "主唱");
        let second = rows
            .get(1)
            .ok_or_else(|| color_eyre::eyre::eyre!("missing row 1"))?;
        assert_eq!(second.position, 1);
        assert_eq!(second.artist_value, "客串");
        Ok(())
    }

    /// 艺人集合变更时旧行不残留(先删后插,不是追加)。
    #[tokio::test]
    async fn upsert_song_replaces_artists_on_change() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        store
            .upsert_song(&with_artists(song("42"), &["旧艺人"]))
            .await?;
        store
            .upsert_song(&with_artists(song("42"), &["新艺人"]))
            .await?;
        let rows = fetch_artists(&store, "42").await?;
        assert_eq!(rows.len(), 1, "旧艺人行不得残留");
        let only = rows
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("missing row"))?;
        assert_eq!(only.artist_name, "新艺人");
        Ok(())
    }

    /// 贫投影(`artists` 为空)不得抹掉已落库的艺人行——与单值列的
    /// 「非空进步、NULL 不回退」同语义,只是集合层面无法 COALESCE,靠 guard 跳过。
    #[tokio::test]
    async fn upsert_song_empty_artists_does_not_wipe_existing() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        store
            .upsert_song(&with_artists(song("42"), &["已知艺人"]))
            .await?;
        // 贫投影:同一首歌再次起播,这次拿到手的 Song 没带 artists(如某条路径缺富化)。
        store.upsert_song(&song("42")).await?;
        let rows = fetch_artists(&store, "42").await?;
        assert_eq!(rows.len(), 1, "空投影不得抹掉已知艺人");
        let only = rows
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("missing row"))?;
        assert_eq!(only.artist_name, "已知艺人");
        Ok(())
    }
}
