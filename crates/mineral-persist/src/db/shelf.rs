//! shelf source 文件索引存储(`shelf_file` 表)。
//!
//! 只管文件事实的 CRUD 原语;rename 调和(gone/new 按 size+mtime 匹配)与 `Song` 映射是
//! 上层(shelf channel)的事——它才握有扫描结果与 uuid 生成。

use color_eyre::eyre::WrapErr;
use sqlx::FromRow;

use crate::ServerStore;

/// `shelf_file` 一行:文件位置 + 增量信号 + 探测快照。
///
/// 字段类型贴 sqlite(整数一律 `i64`),domain 类型(`u64`/`u32` 等)的换算在 shelf 侧边界做,
/// 不在此处猜 sqlx 的无符号解码支持。各 `Option` 列 `None` = 未知 / 未探出。
#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct ShelfFileRow {
    /// 稳定 SongId 裸值(define_uuid 生成)。
    pub uuid: String,

    /// 所属 mount 根(跨 backend 防路径碰撞)。
    pub mount: String,

    /// 当前路径(mount 命名空间下)。
    pub path: String,

    /// 字节大小(backend 未给为 `None`)。
    pub size: Option<i64>,

    /// 最后修改毫秒(epoch ms;backend 未给为 `None`)。
    pub mtime_ms: Option<i64>,

    /// 容器格式名(如 `"flac"`;未探出为 `None`)。
    pub format: Option<String>,

    /// 码率(kbps)。
    pub bitrate_kbps: Option<i64>,

    /// 位深(bit)。
    pub bit_depth: Option<i64>,

    /// 时长(ms)。
    pub duration_ms: Option<i64>,

    /// 曲名标签。
    pub title: Option<String>,

    /// 艺人标签(原始字符串)。
    pub artist: Option<String>,

    /// 专辑标签。
    pub album: Option<String>,

    /// 专辑艺人标签。
    pub album_artist: Option<String>,

    /// 专辑内曲序。
    pub track_no: Option<i64>,

    /// 流派标签。
    pub genre: Option<String>,
}

/// shelf 文件索引视图。降级 [`ServerStore`] 下所有方法 no-op / 空。
pub struct ShelfStore {
    /// 顶层句柄(经 `persist.pool()` 取连接池)。
    persist: ServerStore,
}

impl ShelfStore {
    /// 绑定一个 [`ServerStore`]。
    ///
    /// # Params:
    ///   - `persist`: 顶层句柄
    ///
    /// # Return:
    ///   shelf 索引视图。
    pub(crate) fn new(persist: ServerStore) -> Self {
        Self { persist }
    }

    /// upsert 一条文件事实(按 `uuid` 主键冲突更新全部列)。
    ///
    /// # Params:
    ///   - `row`: 待写入的文件行
    ///
    /// # Return:
    ///   写入成功 `Ok(())`;降级句柄 no-op。
    pub async fn upsert(&self, row: &ShelfFileRow) -> color_eyre::Result<()> {
        let Some(pool) = self.persist.pool() else {
            return Ok(());
        };
        sqlx::query(
            "INSERT INTO shelf_file \
             (uuid, mount, path, size, mtime_ms, format, bitrate_kbps, bit_depth, duration_ms, \
              title, artist, album, album_artist, track_no, genre) \
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) \
             ON CONFLICT(uuid) DO UPDATE SET \
              mount=excluded.mount, path=excluded.path, size=excluded.size, mtime_ms=excluded.mtime_ms, \
              format=excluded.format, bitrate_kbps=excluded.bitrate_kbps, bit_depth=excluded.bit_depth, \
              duration_ms=excluded.duration_ms, title=excluded.title, artist=excluded.artist, \
              album=excluded.album, album_artist=excluded.album_artist, track_no=excluded.track_no, \
              genre=excluded.genre",
        )
        .bind(&row.uuid)
        .bind(&row.mount)
        .bind(&row.path)
        .bind(row.size)
        .bind(row.mtime_ms)
        .bind(&row.format)
        .bind(row.bitrate_kbps)
        .bind(row.bit_depth)
        .bind(row.duration_ms)
        .bind(&row.title)
        .bind(&row.artist)
        .bind(&row.album)
        .bind(&row.album_artist)
        .bind(row.track_no)
        .bind(&row.genre)
        .execute(pool)
        .await
        .wrap_err("upsert shelf_file 失败")?;
        Ok(())
    }

    /// 按 `(mount, path)` 反查 uuid(增量扫描 / 调和用)。
    ///
    /// # Params:
    ///   - `mount`: mount 根
    ///   - `path`: 路径
    ///
    /// # Return:
    ///   命中的 uuid;无则 `None`;降级句柄 `None`。
    pub async fn find_uuid_by_path(
        &self,
        mount: &str,
        path: &str,
    ) -> color_eyre::Result<Option<String>> {
        let Some(pool) = self.persist.pool() else {
            return Ok(None);
        };
        let found: Option<(String,)> =
            sqlx::query_as("SELECT uuid FROM shelf_file WHERE mount = ? AND path = ?")
                .bind(mount)
                .bind(path)
                .fetch_optional(pool)
                .await
                .wrap_err("按路径查 shelf_file uuid 失败")?;
        Ok(found.map(|(uuid,)| uuid))
    }

    /// 列一个 mount 下的全部文件行(调和用:与本次扫描结果比对)。
    ///
    /// # Params:
    ///   - `mount`: mount 根
    ///
    /// # Return:
    ///   该 mount 的全部文件行(按 path 升序);降级句柄空。
    pub async fn list_mount(&self, mount: &str) -> color_eyre::Result<Vec<ShelfFileRow>> {
        let Some(pool) = self.persist.pool() else {
            return Ok(Vec::new());
        };
        sqlx::query_as::<_, ShelfFileRow>(
            "SELECT uuid, mount, path, size, mtime_ms, format, bitrate_kbps, bit_depth, \
             duration_ms, title, artist, album, album_artist, track_no, genre \
             FROM shelf_file WHERE mount = ? ORDER BY path",
        )
        .bind(mount)
        .fetch_all(pool)
        .await
        .wrap_err("列 mount 下 shelf_file 失败")
    }

    /// 列全部文件行(不分 mount;channel 加载进内存做 fuzzy 搜索 / 库视图用)。
    ///
    /// # Return:
    ///   全部文件行(按 mount、path 升序);降级句柄空。
    pub async fn list_all(&self) -> color_eyre::Result<Vec<ShelfFileRow>> {
        let Some(pool) = self.persist.pool() else {
            return Ok(Vec::new());
        };
        sqlx::query_as::<_, ShelfFileRow>(
            "SELECT uuid, mount, path, size, mtime_ms, format, bitrate_kbps, bit_depth, \
             duration_ms, title, artist, album, album_artist, track_no, genre \
             FROM shelf_file ORDER BY mount, path",
        )
        .fetch_all(pool)
        .await
        .wrap_err("列全部 shelf_file 失败")
    }

    /// 按 uuid 取一行(channel 详情点查用)。
    ///
    /// # Params:
    ///   - `uuid`: 文件 uuid
    ///
    /// # Return:
    ///   命中的文件行;无则 `None`;降级句柄 `None`。
    pub async fn get(&self, uuid: &str) -> color_eyre::Result<Option<ShelfFileRow>> {
        let Some(pool) = self.persist.pool() else {
            return Ok(None);
        };
        sqlx::query_as::<_, ShelfFileRow>(
            "SELECT uuid, mount, path, size, mtime_ms, format, bitrate_kbps, bit_depth, \
             duration_ms, title, artist, album, album_artist, track_no, genre \
             FROM shelf_file WHERE uuid = ?",
        )
        .bind(uuid)
        .fetch_optional(pool)
        .await
        .wrap_err("按 uuid 取 shelf_file 失败")
    }

    /// 删除若干 uuid 的文件行(调和:确认消失的文件出库)。
    ///
    /// # Params:
    ///   - `uuids`: 待删 uuid
    ///
    /// # Return:
    ///   删除成功 `Ok(())`;降级句柄 no-op;空输入直接返回。
    pub async fn delete(&self, uuids: &[String]) -> color_eyre::Result<()> {
        if uuids.is_empty() {
            return Ok(());
        }
        let Some(pool) = self.persist.pool() else {
            return Ok(());
        };
        let mut tx = pool.begin().await.wrap_err("开启 shelf_file 删除事务失败")?;
        for uuid in uuids {
            sqlx::query("DELETE FROM shelf_file WHERE uuid = ?")
                .bind(uuid)
                .execute(&mut *tx)
                .await
                .wrap_err("删除 shelf_file 行失败")?;
        }
        tx.commit().await.wrap_err("提交 shelf_file 删除事务失败")?;
        Ok(())
    }

    /// 调和 rename:把 `uuid` 的行迁到新位置(复用 uuid,收藏 / 统计不断链)。
    ///
    /// # Params:
    ///   - `uuid`: 复用的文件 uuid
    ///   - `path`: 新路径
    ///   - `size`: 新大小
    ///   - `mtime_ms`: 新修改时间
    ///
    /// # Return:
    ///   更新成功 `Ok(())`;降级句柄 no-op。
    pub async fn update_location(
        &self,
        uuid: &str,
        path: &str,
        size: Option<i64>,
        mtime_ms: Option<i64>,
    ) -> color_eyre::Result<()> {
        let Some(pool) = self.persist.pool() else {
            return Ok(());
        };
        sqlx::query("UPDATE shelf_file SET path = ?, size = ?, mtime_ms = ? WHERE uuid = ?")
            .bind(path)
            .bind(size)
            .bind(mtime_ms)
            .bind(uuid)
            .execute(pool)
            .await
            .wrap_err("更新 shelf_file 位置失败")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ShelfFileRow, ShelfStore};
    use crate::ServerStore;

    /// 造一条最小文件行(只填必填 + 给定 uuid/path)。
    fn row(uuid: &str, mount: &str, path: &str) -> ShelfFileRow {
        ShelfFileRow {
            uuid: uuid.to_owned(),
            mount: mount.to_owned(),
            path: path.to_owned(),
            size: Some(1024),
            mtime_ms: Some(1_700_000_000_000),
            format: Some("flac".to_owned()),
            bitrate_kbps: Some(900),
            bit_depth: Some(16),
            duration_ms: Some(240_000),
            title: Some("八匹马".to_owned()),
            artist: Some("惘闻".to_owned()),
            album: Some("八匹马".to_owned()),
            album_artist: None,
            track_no: Some(1),
            genre: Some("post-rock".to_owned()),
        }
    }

    /// upsert 后 get 取回完整行(读写往返,探测快照不丢)。
    #[tokio::test]
    async fn upsert_then_get_roundtrip() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ServerStore::open(&dir.path().join("t.db")).await?;
        let shelf = ShelfStore::new(store);
        let r = row("u1", "/music", "惘闻/八匹马/01.flac");
        shelf.upsert(&r).await?;
        assert_eq!(shelf.get("u1").await?.as_ref(), Some(&r));
        Ok(())
    }

    /// upsert 同 uuid 二次:按主键冲突更新(探测快照刷新,不产生第二行)。
    #[tokio::test]
    async fn upsert_same_uuid_updates_in_place() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ServerStore::open(&dir.path().join("t.db")).await?;
        let shelf = ShelfStore::new(store);
        shelf.upsert(&row("u1", "/music", "a.flac")).await?;
        let mut updated = row("u1", "/music", "a.flac");
        updated.title = Some("改了名".to_owned());
        shelf.upsert(&updated).await?;
        assert_eq!(shelf.list_mount("/music").await?.len(), 1, "同 uuid 不产生第二行");
        assert_eq!(
            shelf.get("u1").await?.and_then(|r| r.title),
            Some("改了名".to_owned())
        );
        Ok(())
    }

    /// find_uuid_by_path 命中 / 不命中。
    #[tokio::test]
    async fn find_uuid_by_path_hit_and_miss() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ServerStore::open(&dir.path().join("t.db")).await?;
        let shelf = ShelfStore::new(store);
        shelf.upsert(&row("u1", "/music", "a.flac")).await?;
        assert_eq!(
            shelf.find_uuid_by_path("/music", "a.flac").await?,
            Some("u1".to_owned())
        );
        assert_eq!(shelf.find_uuid_by_path("/music", "nope.flac").await?, None);
        // 同 path 不同 mount 不串:另一 mount 查不到。
        assert_eq!(shelf.find_uuid_by_path("/other", "a.flac").await?, None);
        Ok(())
    }

    /// list_mount 只列该 mount 的行,按 path 升序;跨 mount 不混。
    #[tokio::test]
    async fn list_mount_scopes_and_orders() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ServerStore::open(&dir.path().join("t.db")).await?;
        let shelf = ShelfStore::new(store);
        shelf.upsert(&row("u1", "/music", "b.flac")).await?;
        shelf.upsert(&row("u2", "/music", "a.flac")).await?;
        shelf.upsert(&row("u3", "/other", "z.flac")).await?;
        let paths = shelf
            .list_mount("/music")
            .await?
            .into_iter()
            .map(|r| r.path)
            .collect::<Vec<String>>();
        assert_eq!(paths, vec!["a.flac".to_owned(), "b.flac".to_owned()], "按 path 升序,/other 不混入");
        Ok(())
    }

    /// update_location:rename 复用 uuid,路径 / size / mtime 更新,探测快照保留。
    #[tokio::test]
    async fn update_location_reuses_uuid() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ServerStore::open(&dir.path().join("t.db")).await?;
        let shelf = ShelfStore::new(store);
        shelf.upsert(&row("u1", "/music", "old.flac")).await?;
        shelf
            .update_location("u1", "new.flac", Some(2048), Some(1_800_000_000_000))
            .await?;
        let got = shelf
            .get("u1")
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("uuid 应仍在"))?;
        assert_eq!(got.path, "new.flac");
        assert_eq!(got.size, Some(2048));
        assert_eq!(got.title, Some("八匹马".to_owned()), "探测快照保留");
        Ok(())
    }

    /// delete 移除指定 uuid;空输入 no-op。
    #[tokio::test]
    async fn delete_removes_rows() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = ServerStore::open(&dir.path().join("t.db")).await?;
        let shelf = ShelfStore::new(store);
        shelf.upsert(&row("u1", "/music", "a.flac")).await?;
        shelf.upsert(&row("u2", "/music", "b.flac")).await?;
        shelf.delete(&[]).await?; // 空输入不报错
        shelf.delete(&["u1".to_owned()]).await?;
        assert_eq!(shelf.get("u1").await?, None);
        assert!(shelf.get("u2").await?.is_some(), "只删指定 uuid");
        Ok(())
    }

    /// 降级句柄:写 no-op、读空,不报错。
    #[tokio::test]
    async fn disabled_store_is_noop() -> color_eyre::Result<()> {
        let shelf = ShelfStore::new(ServerStore::disabled());
        shelf.upsert(&row("u1", "/music", "a.flac")).await?;
        assert_eq!(shelf.get("u1").await?, None);
        assert!(shelf.list_mount("/music").await?.is_empty());
        assert_eq!(shelf.find_uuid_by_path("/music", "a.flac").await?, None);
        Ok(())
    }
}
