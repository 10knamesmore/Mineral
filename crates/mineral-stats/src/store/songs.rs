//! songs 维表维护:播放路径 write-through 的歌曲展示元数据。

use color_eyre::eyre::WrapErr as _;
use mineral_model::Song;

use crate::store::StatsStore;

impl StatsStore {
    /// 落 / 富化一行歌曲维表(报表 JOIN 出名的唯一数据源)。降级时静默 no-op。
    ///
    /// `name` 恒以新值为准(实体属性,改名全局生效);`alias` / 专辑 / 时长走
    /// 「非空进步、NULL 不回退」——写入方是富化程度不一的投影,贫投影(如列表页缺
    /// 译名)不得抹掉已知值,旧行的 NULL 随数据流经自然回填。
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
        .execute(pool)
        .await
        .wrap_err_with(|| format!("upsert_song 落库失败 song={song_value}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::store::StatsStore;
    use mineral_test::{song, with_album, with_alias, with_duration, with_name};

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
}
