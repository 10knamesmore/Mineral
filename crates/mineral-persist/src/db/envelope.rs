//! 每曲振幅包络缓存(`song_envelope` 表)。挂在 [`NamespaceStore`] 上的扩展方法。
//!
//! 包络与音质无关(振幅形状跨码率基本一致),按 `(namespace, song_value)` 每曲一行;
//! 读取按算法版本过滤,版本不符视同缺失,由产出方重算覆盖,不让旧算法数据毒化渲染。

use color_eyre::eyre::WrapErr;
use mineral_log::trace;
use mineral_model::{Envelope, SongId};

use crate::db::namespace::NamespaceStore;

impl NamespaceStore {
    /// 写(覆盖)一首歌的振幅包络。降级 no-op。
    ///
    /// # Params:
    ///   - `id`: 歌曲 id(裸值入库,namespace 由本 store 隐含)
    ///   - `envelope`: 包络数据(points + 算法版本)
    ///
    /// # Return:
    ///   成功返回 `Ok(())`;降级时同样 `Ok(())`。
    pub async fn put_envelope(&self, id: &SongId, envelope: &Envelope) -> color_eyre::Result<()> {
        let Some(pool) = self.pool() else {
            return Ok(());
        };
        trace!(target: "persist", song = %id.value(), version = envelope.version, "put_envelope");
        sqlx::query(
            "INSERT INTO song_envelope(namespace,song_value,version,points,updated_at) \
             VALUES(?,?,?,?,?) \
             ON CONFLICT(namespace,song_value) DO UPDATE SET \
               version=excluded.version, points=excluded.points, updated_at=excluded.updated_at",
        )
        .bind(self.namespace())
        .bind(id.value())
        .bind(i64::from(envelope.version))
        .bind(envelope.points.as_slice())
        .bind(crate::db::time::now_ms())
        .execute(pool)
        .await
        .wrap_err_with(|| format!("写包络失败 song={}", id.value()))?;
        Ok(())
    }

    /// 读一首歌的振幅包络,**按算法版本过滤**:行存在但版本不符视同缺失
    /// (由产出方重算覆盖)。降级 / 未命中返回 `Ok(None)`。
    ///
    /// # Params:
    ///   - `id`: 歌曲 id
    ///   - `version`: 期望的算法版本
    ///
    /// # Return:
    ///   命中且版本相符返回 `Ok(Some(envelope))`,否则 `Ok(None)`。
    pub async fn get_envelope(
        &self,
        id: &SongId,
        version: u16,
    ) -> color_eyre::Result<Option<Envelope>> {
        let Some(pool) = self.pool() else {
            return Ok(None);
        };
        let row: Option<(Vec<u8>,)> = sqlx::query_as(
            "SELECT points FROM song_envelope WHERE namespace=? AND song_value=? AND version=?",
        )
        .bind(self.namespace())
        .bind(id.value())
        .bind(i64::from(version))
        .fetch_optional(pool)
        .await
        .wrap_err_with(|| format!("读包络失败 song={}", id.value()))?;
        Ok(row.map(|(points,)| Envelope { points, version }))
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::{Envelope, SongId, SourceKind};

    use crate::ServerStore;

    /// 写读往返:put 后按同版本 get 原样回来(points 字节与 version 都不变形)。
    #[tokio::test]
    async fn envelope_roundtrips() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ServerStore::open(&dir.path().join("t.db")).await?;
        let scope = store.scope(SourceKind::NETEASE);
        let id = SongId::new(SourceKind::NETEASE, "s1");
        let envelope = Envelope {
            points: vec![0, 7, 255, 128],
            version: 1,
        };
        scope.put_envelope(&id, &envelope).await?;
        assert_eq!(
            scope.get_envelope(&id, /*version*/ 1).await?,
            Some(envelope)
        );
        Ok(())
    }

    /// 版本过滤:行存在但版本不符 → 视同缺失返回 `None`(旧算法产物不外泄)。
    #[tokio::test]
    async fn stale_version_reads_as_missing() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ServerStore::open(&dir.path().join("t.db")).await?;
        let scope = store.scope(SourceKind::NETEASE);
        let id = SongId::new(SourceKind::NETEASE, "s1");
        scope
            .put_envelope(
                &id,
                &Envelope {
                    points: vec![1, 2, 3],
                    version: 1,
                },
            )
            .await?;
        assert_eq!(scope.get_envelope(&id, /*version*/ 2).await?, None);
        Ok(())
    }

    /// 覆盖写:同曲再 put(如版本升级重算)后 get 拿到新数据,不残留旧行。
    #[tokio::test]
    async fn put_overwrites_previous_row() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ServerStore::open(&dir.path().join("t.db")).await?;
        let scope = store.scope(SourceKind::NETEASE);
        let id = SongId::new(SourceKind::NETEASE, "s1");
        scope
            .put_envelope(
                &id,
                &Envelope {
                    points: vec![1, 1, 1],
                    version: 1,
                },
            )
            .await?;
        let renewed = Envelope {
            points: vec![9, 9, 9],
            version: 2,
        };
        scope.put_envelope(&id, &renewed).await?;
        assert_eq!(scope.get_envelope(&id, /*version*/ 2).await?, Some(renewed));
        assert_eq!(
            scope.get_envelope(&id, /*version*/ 1).await?,
            None,
            "旧版本行已被覆盖,不再可读"
        );
        Ok(())
    }

    /// namespace 隔离:A 源写入的包络,B 源同裸值读不到。
    #[tokio::test]
    async fn envelope_scoped_by_namespace() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ServerStore::open(&dir.path().join("t.db")).await?;
        let netease = store.scope(SourceKind::NETEASE);
        let bilibili = store.scope(SourceKind::BILIBILI);
        let id = SongId::new(SourceKind::NETEASE, "s1");
        netease
            .put_envelope(
                &id,
                &Envelope {
                    points: vec![5],
                    version: 1,
                },
            )
            .await?;
        let other = SongId::new(SourceKind::BILIBILI, "s1");
        assert_eq!(bilibili.get_envelope(&other, /*version*/ 1).await?, None);
        Ok(())
    }

    /// 降级句柄:put 静默成功、get 恒 `None`,播放路径无需特判。
    #[tokio::test]
    async fn disabled_store_is_noop() -> color_eyre::Result<()> {
        let scope = ServerStore::disabled().scope(SourceKind::NETEASE);
        let id = SongId::new(SourceKind::NETEASE, "s1");
        scope
            .put_envelope(
                &id,
                &Envelope {
                    points: vec![1],
                    version: 1,
                },
            )
            .await?;
        assert_eq!(scope.get_envelope(&id, /*version*/ 1).await?, None);
        Ok(())
    }
}
