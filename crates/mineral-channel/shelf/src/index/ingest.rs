//! 顶层入库:遍历配置的 roots,扫描 + 调和进索引(daemon 扫描任务的入口)。

use std::path::Path;

use mineral_persist::ShelfStore;
use regex::Regex;

use super::reconcile::reconcile;
use crate::scan::{ScanOptions, ScannedFile, scan};
use crate::storage::ShelfStorage;

/// 扫描 `roots` 并调和进索引。
///
/// 每个 root 是一个 mount(独立调和域)。**root 不可达则跳过、不调和**——避免空扫把该 mount
/// 的库全删(NAS 约束:不可达 ≠ 删除)。可达则 scan → reconcile。`exclude` 是 regex 原文,
/// 编译失败的 pattern warn + 跳过(不拖垮扫描)。
///
/// # Params:
///   - `storage`: 存储后端
///   - `store`: shelf 索引存储
///   - `roots`: 扫描根列表(每个即一个 mount)
///   - `max_depth`: 遍历深度上限
///   - `exclude`: 名称排除 regex 原文
///
/// # Return:
///   全部可达根调和成功返回 `Ok(())`;单根调和失败即冒泡。
pub async fn scan_and_index(
    storage: &dyn ShelfStorage,
    store: &ShelfStore,
    roots: &[String],
    max_depth: usize,
    exclude: &[String],
) -> color_eyre::Result<()> {
    let patterns = compile_excludes(exclude);
    for root in roots {
        let root_path = Path::new(root);
        // 可达性探针:root 列不出来(不可达 / 无权限)→ 跳过,保库不动。
        if let Err(e) = storage.list_dir(root_path).await {
            mineral_log::warn!(root = %root, error = mineral_log::chain(&e), "root 不可达,跳过(不清库)");
            continue;
        }
        let opts = ScanOptions::new(max_depth, patterns.clone());
        let dirs = scan(storage, root_path, &opts).await;
        let files: Vec<ScannedFile> = dirs.into_iter().flat_map(|d| d.files().clone()).collect();
        reconcile(store, root, &files).await?;
    }
    Ok(())
}

/// 编译 exclude regex 原文;编译失败的 pattern warn + 跳过。
///
/// # Params:
///   - `exclude`: regex 原文列表
///
/// # Return:
///   编译成功的 regex(坏 pattern 已剔除)。
fn compile_excludes(exclude: &[String]) -> Vec<Regex> {
    exclude
        .iter()
        .filter_map(|pattern| match Regex::new(pattern) {
            Ok(re) => Some(re),
            Err(e) => {
                mineral_log::warn!(pattern = %pattern, error = mineral_log::chain(&e), "exclude regex 编译失败,跳过该 pattern");
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use mineral_persist::ServerStore;

    use super::scan_and_index;
    use crate::storage::FsStorage;

    /// 合法最小 WAV。
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

    /// 扫描多个 root 入库;每个 root 独立成 mount。
    #[tokio::test]
    async fn indexes_multiple_roots() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let root_a = dir.path().join("libA");
        let root_b = dir.path().join("libB");
        std::fs::create_dir_all(root_a.join("album"))?;
        std::fs::create_dir_all(&root_b)?;
        std::fs::write(root_a.join("album").join("1.wav"), wav_bytes(2000))?;
        std::fs::write(root_b.join("2.wav"), wav_bytes(2000))?;

        let persist = ServerStore::open(&dir.path().join("t.db")).await?;
        let shelf = persist.shelf();
        let roots = vec![
            root_a.to_string_lossy().into_owned(),
            root_b.to_string_lossy().into_owned(),
        ];
        scan_and_index(&FsStorage, &shelf, &roots, 8, &["^\\.".to_owned()]).await?;

        assert_eq!(shelf.list_all().await?.len(), 2, "两个 root 各一首入库");
        Ok(())
    }

    /// root 不可达:跳过、不清该 mount 的库(NAS 约束:不可达 ≠ 删除)。
    #[tokio::test]
    async fn unreachable_root_keeps_index() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("lib");
        std::fs::create_dir_all(&root)?;
        std::fs::write(root.join("1.wav"), wav_bytes(2000))?;
        let persist = ServerStore::open(&dir.path().join("t.db")).await?;
        let shelf = persist.shelf();
        let root_str = root.to_string_lossy().into_owned();

        // 首扫入库。
        scan_and_index(&FsStorage, &shelf, std::slice::from_ref(&root_str), 8, &[]).await?;
        assert_eq!(shelf.list_all().await?.len(), 1);

        // 删掉 root 使其不可达,再扫:库应保留(不因扫不到而清空)。
        std::fs::remove_dir_all(&root)?;
        scan_and_index(&FsStorage, &shelf, std::slice::from_ref(&root_str), 8, &[]).await?;
        assert_eq!(
            shelf.list_all().await?.len(),
            1,
            "root 不可达时库不动,不清空"
        );
        Ok(())
    }

    /// 坏 exclude regex:warn + 跳过该 pattern,扫描照常(不炸)。
    #[tokio::test]
    async fn bad_exclude_pattern_is_skipped() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("lib");
        std::fs::create_dir_all(&root)?;
        std::fs::write(root.join("1.wav"), wav_bytes(2000))?;
        let persist = ServerStore::open(&dir.path().join("t.db")).await?;
        let shelf = persist.shelf();
        let root_str = root.to_string_lossy().into_owned();

        // "[" 是非法 regex;应被跳过,扫描仍入库。
        scan_and_index(&FsStorage, &shelf, &[root_str], 8, &["[".to_owned()]).await?;
        assert_eq!(shelf.list_all().await?.len(), 1);
        Ok(())
    }
}
