//! shelf 本地库扫描任务:daemon 启动时扫配置的 roots → 调和进索引 → 刷 shelf 歌单库。
//!
//! 扫描是 daemon 级职责(spec §8):它握有 persist 索引与活配置。参数在 [`Player`] 构造时
//! 从配置切片取一次快照(同 favorites backfill);roots 空 = shelf 未激活,不扫。

use mineral_channel_shelf::FsStorage;
use mineral_model::SourceKind;

use crate::player::PlayerCore;

/// shelf 扫描参数(daemon 启动时从配置切片取)。
pub(crate) struct ShelfScanParams {
    /// 扫描根列表(每个即一个 mount);空 = 不激活。
    pub(crate) roots: Vec<String>,

    /// 遍历深度上限。
    pub(crate) max_depth: usize,

    /// 名称排除 regex 原文。
    pub(crate) exclude: Vec<String>,
}

impl ShelfScanParams {
    /// 从三段配置值构造。
    ///
    /// # Params:
    ///   - `roots`: 扫描根列表
    ///   - `max_depth`: 遍历深度上限
    ///   - `exclude`: 排除 regex 原文
    ///
    /// # Return:
    ///   扫描参数。
    pub(crate) fn new(roots: Vec<String>, max_depth: usize, exclude: Vec<String>) -> Self {
        Self {
            roots,
            max_depth,
            exclude,
        }
    }
}

impl PlayerCore {
    /// 起一次 shelf 扫描:遍历配置 roots 扫盘 → 调和进索引 → 完成后刷 shelf 歌单库(填侧栏)。
    ///
    /// roots 空(shelf 未激活)直接返回。best-effort:扫描失败只 warn,不影响 daemon。
    /// 完成后 `submit_my_playlists(SHELF)`——首次扫完索引才有内容,侧栏由空转有;无 client
    /// 连接时结论落库缓存,client 连上即被推送。
    pub(crate) fn spawn_shelf_scan(&self) {
        let params = &self.inner.shelf_scan;
        if params.roots.is_empty() {
            return;
        }
        let roots = params.roots.clone();
        let max_depth = params.max_depth;
        let exclude = params.exclude.clone();
        let player = self.clone();
        tokio::spawn(async move {
            let store = player.persist().shelf();
            if let Err(e) =
                mineral_channel_shelf::scan_and_index(&FsStorage, &store, &roots, max_depth, &exclude)
                    .await
            {
                mineral_log::warn!(error = mineral_log::chain(&e), "shelf 扫描失败(不影响 daemon)");
            }
            player.submit_my_playlists(SourceKind::SHELF);
        });
    }
}
