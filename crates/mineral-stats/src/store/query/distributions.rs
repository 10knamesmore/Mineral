//! 时段分桶 + 各维度分布。

use std::ops::Range;

use color_eyre::eyre::WrapErr as _;

use crate::report::{Bucket, BucketBy, Distributions, Slice};
use crate::store::StatsStore;

impl StatsStore {
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
