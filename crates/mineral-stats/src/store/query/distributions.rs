//! 时段分桶 + 各维度分布。

use std::ops::Range;

use super::shared::{ReportColumn, plays_in};
use crate::entity::plays;
use color_eyre::eyre::WrapErr as _;
use sea_orm::sea_query::{self, Expr, ExprTrait, Func, Iden};
use sea_orm::{ColumnTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect};

/// SQLite 的时间格式化函数。
#[derive(Iden)]
struct Strftime;

/// SQLite 的整数转换类型。
#[derive(Iden)]
struct Integer;

/// 时间分桶的输出键。
#[derive(Clone, Copy, Debug, sea_orm::DeriveColumn)]
enum BucketColumn {
    /// 分桶的数值键。
    Key,
}

use crate::report::{Bucket, BucketBy, Distributions, Slice};
use crate::store::StatsStore;

impl StatsStore {
    /// 时段分桶(UTC):Hour(0-23)/ Weekday(0-6,周日=0)/ Month(1-12)。
    pub async fn listen_buckets(
        &self,
        range: Range<i64>,
        by: BucketBy,
    ) -> color_eyre::Result<Vec<Bucket>> {
        let Some(db) = self.pool() else {
            return Ok(Vec::new());
        };
        let format = match by {
            BucketBy::Hour => "%H",
            BucketBy::Weekday => "%w",
            BucketBy::Month => "%m",
        };
        let key = Expr::from(Func::cust(Strftime).args([
            Expr::value(format),
            Expr::col((plays::Entity, plays::Column::StartedAt)).div(1000),
            Expr::value("unixepoch"),
        ]))
        .cast_as(Integer);
        plays_in(range)
            .select_only()
            .column_as(key.clone(), BucketColumn::Key)
            .column_as(plays::Column::Id.count(), ReportColumn::Plays)
            .column_as(
                plays::Column::ListenMs.sum().if_null(0),
                ReportColumn::ListenMs,
            )
            .group_by(key.clone())
            .order_by_asc(key)
            .into_model::<Bucket>()
            .all(db)
            .await
            .wrap_err("listen_buckets 查询失败")
    }

    /// 各维度分布(来源 / 发起方式 / 模式 / 格式 / 音质 / 来源位置)+ 无损播放数。
    pub async fn distributions(&self, range: Range<i64>) -> color_eyre::Result<Distributions> {
        let Some(pool) = self.pool() else {
            return Ok(Distributions::default());
        };
        let lossless_plays = i64::try_from(
            plays_in(range.clone())
                .filter(plays::Column::IsLossless.eq(1))
                .count(pool)
                .await
                .wrap_err("distributions(lossless) 查询失败")?,
        )?;
        Ok(Distributions {
            by_source: self
                .distribution_by(range.clone(), plays::Column::Ns)
                .await?,
            by_origin: self
                .distribution_by(range.clone(), plays::Column::OriginKind)
                .await?,
            by_play_mode: self
                .distribution_by(range.clone(), plays::Column::PlayMode)
                .await?,
            by_format: self
                .distribution_by(range.clone(), plays::Column::AudioFormat)
                .await?,
            by_quality: self
                .distribution_by(range.clone(), plays::Column::Quality)
                .await?,
            by_playback_origin: self
                .distribution_by(range, plays::Column::PlaybackOrigin)
                .await?,
            lossless_plays,
        })
    }

    /// 按某列分桶计数(列名是内部常量;NULL 归入空串桶)。各分布形状同一,只列名不同,
    /// 列表化比 6 条字面量查询更 DRY。
    async fn distribution_by(
        &self,
        range: Range<i64>,
        column: plays::Column,
    ) -> color_eyre::Result<Vec<Slice>> {
        let Some(db) = self.pool() else {
            return Ok(Vec::new());
        };
        let label = Expr::col((plays::Entity, column)).if_null("");
        let rows = plays_in(range)
            .select_only()
            .expr(label.clone())
            .column_as(plays::Column::Id.count(), ReportColumn::Plays)
            .group_by(label.clone())
            .order_by_desc(Expr::col(ReportColumn::Plays))
            .order_by_asc(label)
            .into_tuple::<(String, i64)>()
            .all(db)
            .await
            .wrap_err_with(|| format!("distribution_by {column:?} 查询失败"))?;
        Ok(rows
            .into_iter()
            .map(|(value, plays)| Slice { value, plays })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::test_support::{full_range, open_temp, seed};

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
}
