//! 回填类维护查询(非报表):给存量维护操作提供行级 inventory。

use std::path::PathBuf;

use color_eyre::eyre::WrapErr;
use mineral_model::{BitRate, SongId, SourceKind};

use crate::store::StatsStore;

impl StatsStore {
    /// 列出全部成功下载记录 `(SongId, quality, path)`(回填内嵌 tag 的导出侧 inventory)。
    ///
    /// 同一歌可能有多行(不同音质 / 重下),调用方自行去重。降级返回空 vec。
    ///
    /// # Return:
    ///   结构化三元组;quality / path 列异常的行跳过(数据是我方自己写的,不该有)。
    pub async fn successful_downloads(
        &self,
    ) -> color_eyre::Result<Vec<(SongId, BitRate, PathBuf)>> {
        let Some(pool) = self.pool() else {
            return Ok(Vec::new());
        };
        let rows = sqlx::query!(
            "SELECT ns, song_value, quality, path FROM downloads \
             WHERE outcome = 'downloaded' AND path IS NOT NULL"
        )
        .fetch_all(pool)
        .await
        .wrap_err("successful_downloads 查询失败")?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let quality = BitRate::from_token(&r.quality)?;
                let path = r.path?;
                Some((
                    SongId::new(SourceKind::from_name(&r.ns), r.song_value),
                    quality,
                    PathBuf::from(path),
                ))
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::{BitRate, SongId, SourceKind};

    use crate::event::{BehaviorEvent, DownloadHook, DownloadOutcome, StatsEvent};
    use crate::store::StatsStore;
    use crate::vocab::Actor;

    /// 落一条下载记录(经公开事件入口,与 server 埋点同路径)。
    async fn seed_download(
        store: &StatsStore,
        song_value: &str,
        outcome: DownloadOutcome,
        path: Option<&str>,
    ) -> color_eyre::Result<()> {
        store
            .record_event(
                1_000,
                /*session_id*/ None,
                &StatsEvent::Behavior {
                    actor: Actor::System,
                    event: BehaviorEvent::Download {
                        song: SongId::new(SourceKind::NETEASE, song_value),
                        quality: BitRate::Lossless.as_str().to_owned(),
                        format: Some("flac".to_owned()),
                        outcome,
                        hooked: DownloadHook::None,
                        path: path.map(str::to_owned),
                    },
                },
            )
            .await
    }

    /// 只列成功行:skipped / failed 与 path 为 NULL 的都被过滤;降级返回空。
    #[tokio::test]
    async fn lists_only_successful_rows() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let store = StatsStore::open(&d.path().join("stats.db")).await?;
        seed_download(
            &store,
            "1",
            DownloadOutcome::Downloaded,
            Some("/music/a.flac"),
        )
        .await?;
        seed_download(&store, "2", DownloadOutcome::Skipped, /*path*/ None).await?;
        seed_download(&store, "3", DownloadOutcome::Failed, /*path*/ None).await?;
        seed_download(
            &store,
            "4",
            DownloadOutcome::Downloaded,
            Some("/music/b.flac"),
        )
        .await?;

        let rows = store.successful_downloads().await?;
        assert_eq!(rows.len(), 2, "只应有两条成功行: {rows:?}");
        assert!(
            rows.iter().all(|(id, q, _)| {
                id.namespace() == SourceKind::NETEASE && *q == BitRate::Lossless
            }),
            "ns / quality 应结构化还原"
        );
        assert!(
            rows.iter().any(|(_, _, p)| p.ends_with("a.flac"))
                && rows.iter().any(|(_, _, p)| p.ends_with("b.flac")),
            "路径应带出: {rows:?}"
        );
        assert!(
            StatsStore::disabled()
                .successful_downloads()
                .await?
                .is_empty(),
            "降级应为空"
        );
        Ok(())
    }
}
