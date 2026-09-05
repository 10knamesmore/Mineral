//! 新发现盘点:窗口内首播的新歌 + 首 / 末播放行。

use std::ops::Range;

use crate::entity::plays;
use color_eyre::eyre::WrapErr as _;
use sea_orm::sea_query::{ExprTrait, Order};
use sea_orm::{ColumnTrait, EntityTrait, QueryOrder, QuerySelect};

use crate::report::{Discoveries, PlayTail};
use crate::store::StatsStore;
#[cfg(test)]
use crate::vocab::FinishReason;

use super::shared::{PlayTailRow, plays_in, song_id};

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
        let Some(db) = self.pool() else {
            return Ok(Discoveries::default());
        };
        let rows = plays::Entity::find()
            .select_only()
            .columns([plays::Column::Ns, plays::Column::SongValue])
            .group_by(plays::Column::Ns)
            .group_by(plays::Column::SongValue)
            .having(plays::Column::StartedAt.min().gte(range.start))
            .having(plays::Column::StartedAt.min().lt(range.end))
            .order_by_asc(plays::Column::StartedAt.min())
            .limit(u64::try_from(limit).ok())
            .into_tuple::<(String, String)>()
            .all(db)
            .await
            .wrap_err("discoveries new_songs 查询失败")?;
        Ok(Discoveries {
            new_songs: rows
                .into_iter()
                .map(|(ns, value)| song_id(&ns, &value))
                .collect(),
            first_play: self.edge_play(range.clone(), /*earliest*/ true).await?,
            last_play: self.edge_play(range, /*earliest*/ false).await?,
        })
    }

    /// 取窗口内最早或最晚的播放行；无播放时返回 `None`。
    async fn edge_play(
        &self,
        range: Range<i64>,
        earliest: bool,
    ) -> color_eyre::Result<Option<PlayTail>> {
        let Some(db) = self.pool() else {
            return Ok(None);
        };
        let order = if earliest { Order::Asc } else { Order::Desc };
        let row = plays_in(range)
            .select_only()
            .columns([
                plays::Column::Ns,
                plays::Column::SongValue,
                plays::Column::StartedAt,
                plays::Column::ListenMs,
                plays::Column::FinishReason,
            ])
            .order_by(plays::Column::StartedAt, order)
            .limit(/*limit*/ 1)
            .into_model::<PlayTailRow>()
            .one(db)
            .await
            .wrap_err("discoveries edge_play 查询失败")?;
        Ok(row.map(|row| PlayTail {
            song: song_id(&row.ns, &row.song_value),
            started_at: row.started_at,
            listen_ms: row.listen_ms,
            finish_reason: row.finish_reason,
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
