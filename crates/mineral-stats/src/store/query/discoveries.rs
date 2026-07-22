//! 新发现盘点:窗口内首播的新歌 + 首 / 末播放行。

use std::ops::Range;

use color_eyre::eyre::WrapErr as _;

use crate::report::{Discoveries, PlayTail};
use crate::store::StatsStore;
use crate::vocab::FinishReason;

use super::shared::{PlayTailRow, song_id};

impl StatsStore {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::test_support::{HOUR, T0, open_temp, play};

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
}
