//! 调和:把一次扫描结果并入索引——path 命中更新、rename 复用 uuid、消失删行。

use mineral_model::{SongId, SourceKind};
use mineral_persist::{ShelfFileRow, ShelfStore};
use rustc_hash::{FxHashMap, FxHashSet};

use super::row::{scanned_sig, scanned_to_row};
use crate::scan::ScannedFile;

/// 把 `mount` 下的一次扫描结果 `scanned` 调和进索引。
///
/// 规则(spec §2):
/// - path 仍在:同 uuid upsert,刷新探测快照 + size/mtime。
/// - path 新增:先按 `(size, mtime)` 匹配「消失的 path」→ 命中即 rename、复用其 uuid;
///   匹配不到才发新 uuid。size/mtime 任一未知不参与匹配(直接发新 uuid)。
/// - path 消失且未被 rename 认领:删行。
///
/// 非 UTF-8 路径无法作 TEXT 存储,warn + 跳过。
///
/// # Params:
///   - `store`: shelf 索引存储
///   - `mount`: 本次扫描的 mount 根
///   - `scanned`: 该 mount 下扫描出的音频文件(已探测)
///
/// # Return:
///   调和成功 `Ok(())`;降级 store 下各写 no-op。
pub async fn reconcile(
    store: &ShelfStore,
    mount: &str,
    scanned: &[ScannedFile],
) -> color_eyre::Result<()> {
    let existing = store.list_mount(mount).await?;
    let existing_by_path: FxHashMap<&str, &str> = existing
        .iter()
        .map(|r| (r.path.as_str(), r.uuid.as_str()))
        .collect();
    let present_paths: FxHashSet<&str> = scanned.iter().filter_map(|f| f.path().to_str()).collect();

    // 消失的行(existing - present):待删集初始纳入全部,按 (size,mtime) 建索引供 rename 认领。
    let mut gone_by_sig: FxHashMap<(i64, i64), Vec<&ShelfFileRow>> = FxHashMap::default();
    let mut to_delete: FxHashSet<String> = FxHashSet::default();
    for row in &existing {
        if present_paths.contains(row.path.as_str()) {
            continue;
        }
        to_delete.insert(row.uuid.clone());
        if let (Some(size), Some(mtime)) = (row.size, row.mtime_ms) {
            gone_by_sig.entry((size, mtime)).or_default().push(row);
        }
    }

    for file in scanned {
        let Some(path) = file.path().to_str() else {
            mineral_log::warn!(path = %file.path().display(), "非 UTF-8 路径,跳过入库");
            continue;
        };

        let uuid = match existing_by_path.get(path) {
            // 已知路径:同 uuid 原地更新。
            Some(known) => (*known).to_owned(),
            // 新路径:先试 rename 认领(复用消失行的 uuid),否则发新 uuid。
            None => match rename_reuse(&mut gone_by_sig, file) {
                Some(reused) => {
                    to_delete.remove(&reused);
                    reused
                }
                None => new_uuid(),
            },
        };

        store
            .upsert(&scanned_to_row(uuid, mount, path, file))
            .await?;
    }

    store
        .delete(&to_delete.into_iter().collect::<Vec<String>>())
        .await?;
    Ok(())
}

/// 按 `(size, mtime)` 从「消失的行」里认领一个 uuid 复用(rename 调和)。
///
/// 命中即从桶里取走一个(同签名多文件时逐个配对,不重复认领)。
///
/// # Params:
///   - `gone_by_sig`: 消失行按签名的索引(会被取走命中项)
///   - `file`: 新出现的扫描文件
///
/// # Return:
///   可复用的 uuid;签名不全 / 无匹配为 `None`。
fn rename_reuse(
    gone_by_sig: &mut FxHashMap<(i64, i64), Vec<&ShelfFileRow>>,
    file: &ScannedFile,
) -> Option<String> {
    let sig = scanned_sig(file)?;
    let bucket = gone_by_sig.get_mut(&sig)?;
    let matched = bucket.pop()?;
    Some(matched.uuid.clone())
}

/// 生成一个新的随机 uuid 裸值(shelf SongId)。
///
/// # Return:
///   新 uuid 字符串。
fn new_uuid() -> String {
    SongId::new_uuid(SourceKind::SHELF).value().to_owned()
}

#[cfg(test)]
mod tests {
    use mineral_persist::ServerStore;

    use super::reconcile;
    use crate::scan::{ScanOptions, scan};
    use crate::storage::FsStorage;

    /// 合法最小 WAV(44B 头 + `data_len` 个 0 PCM)。
    fn wav_bytes(data_len: usize) -> Vec<u8> {
        let data = u32::try_from(data_len).unwrap_or(0);
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&36u32.saturating_add(data).to_le_bytes());
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&8u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data.to_le_bytes());
        v.resize(v.len() + data_len, 0u8);
        v
    }

    /// 扫描一个目录并调和进索引,返回该 mount 的行数。
    async fn scan_and_reconcile(
        store: &mineral_persist::ShelfStore,
        root: &std::path::Path,
    ) -> color_eyre::Result<usize> {
        let opts = ScanOptions::new(8, vec![regex::Regex::new(r"^\.")?]);
        let dirs = scan(&FsStorage, root, &opts).await;
        let files = dirs
            .into_iter()
            .flat_map(|d| d.files().clone())
            .collect::<Vec<_>>();
        let mount = root.to_str().unwrap_or_default();
        reconcile(store, mount, &files).await?;
        Ok(store.list_mount(mount).await?.len())
    }

    /// 首扫:目录里的音频全部入库,uuid 各不相同。
    #[tokio::test]
    async fn first_scan_indexes_all() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("a.wav"), wav_bytes(2000))?;
        std::fs::write(dir.path().join("b.wav"), wav_bytes(3000))?;
        let persist = ServerStore::open(&dir.path().join("t.db")).await?;
        let shelf = persist.shelf();
        assert_eq!(scan_and_reconcile(&shelf, dir.path()).await?, 2);
        Ok(())
    }

    /// 重扫无变化:行数不变,uuid 稳定(同 path 复用)。
    #[tokio::test]
    async fn rescan_unchanged_keeps_uuids() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("a.wav"), wav_bytes(2000))?;
        let persist = ServerStore::open(&dir.path().join("t.db")).await?;
        let shelf = persist.shelf();
        let mount = dir.path().to_str().unwrap_or_default();

        scan_and_reconcile(&shelf, dir.path()).await?;
        let uuid_before = shelf.list_mount(mount).await?.first().map(|r| r.uuid.clone());
        scan_and_reconcile(&shelf, dir.path()).await?;
        let rows = shelf.list_mount(mount).await?;
        assert_eq!(rows.len(), 1, "重扫不产生重复行");
        assert_eq!(rows.first().map(|r| r.uuid.clone()), uuid_before, "同 path uuid 稳定");
        Ok(())
    }

    /// rename(改名但内容/大小/mtime 不变):按 (size,mtime) 认领,uuid 复用、新 path 生效、不留旧行。
    #[tokio::test]
    async fn rename_reuses_uuid() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let old = dir.path().join("old.wav");
        std::fs::write(&old, wav_bytes(2000))?;
        let persist = ServerStore::open(&dir.path().join("t.db")).await?;
        let shelf = persist.shelf();
        let mount = dir.path().to_str().unwrap_or_default();

        scan_and_reconcile(&shelf, dir.path()).await?;
        let uuid_before = shelf
            .list_mount(mount)
            .await?
            .first()
            .map(|r| r.uuid.clone());

        // 保留 mtime 地重命名(fs rename 不改 mtime/size)。
        std::fs::rename(&old, dir.path().join("new.wav"))?;
        scan_and_reconcile(&shelf, dir.path()).await?;

        let rows = shelf.list_mount(mount).await?;
        assert_eq!(rows.len(), 1, "rename 不产生第二行(旧行被认领删除)");
        let row = rows.first().ok_or_else(|| color_eyre::eyre::eyre!("应有一行"))?;
        assert!(row.path.ends_with("new.wav"), "path 更新为新名");
        assert_eq!(Some(row.uuid.clone()), uuid_before, "uuid 复用(收藏/统计不断链)");
        Ok(())
    }

    /// 文件删除(无 rename 匹配):对应行出库。
    #[tokio::test]
    async fn deleted_file_removed() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("a.wav"), wav_bytes(2000))?;
        std::fs::write(dir.path().join("b.wav"), wav_bytes(3000))?;
        let persist = ServerStore::open(&dir.path().join("t.db")).await?;
        let shelf = persist.shelf();

        assert_eq!(scan_and_reconcile(&shelf, dir.path()).await?, 2);
        std::fs::remove_file(dir.path().join("b.wav"))?;
        assert_eq!(scan_and_reconcile(&shelf, dir.path()).await?, 1, "删掉的文件出库");
        Ok(())
    }
}
