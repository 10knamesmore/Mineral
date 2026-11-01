//! 同一艺人的专辑数据与读取状态共享，详情帧只保留展示快照。

use mineral_model::Album;
use mineral_protocol::SubscriptionId;

/// LibraryData 持有订阅；所有保留的艺人详情帧共享其读取进度。
#[derive(Default)]
pub(crate) struct ArtistAlbumListing {
    /// 成功建立的订阅，退订入队后才清除。
    pub subscription: Option<SubscriptionId>,

    /// 已加载数据和正在等待的批次。
    pub albums: ArtistAlbums,
}

/// 艺人专辑的累计内容与批次进度；克隆到详情帧时不转移订阅所有权。
#[derive(Clone, PartialEq)]
pub(crate) struct ArtistAlbums {
    /// 已接受的专辑，保持来源顺序。
    albums: Vec<Album>,

    /// 下一批序号；None 表示来源已读完。
    next_batch: Option<u64>,

    /// 已成功提交、尚未收取的批次。
    pending_batch: Option<u64>,
}

impl Default for ArtistAlbums {
    fn default() -> Self {
        Self {
            albums: Vec::new(),
            next_batch: Some(0),
            pending_batch: None,
        }
    }
}

impl ArtistAlbums {
    pub(crate) fn items(&self) -> &[Album] {
        &self.albums
    }

    pub(crate) fn next_batch(&self) -> Option<u64> {
        self.next_batch.filter(|_| self.pending_batch.is_none())
    }

    pub(crate) fn batch_requested(&mut self, batch: u64) {
        self.pending_batch = Some(batch);
    }

    /// 首批建立内容；续批只接受已请求且与当前进度一致的回包。
    pub(crate) fn accept_batch(&mut self, albums: Vec<Album>, batch: u64, complete: bool) -> bool {
        if self.next_batch != Some(batch) || (batch != 0 && self.pending_batch != Some(batch)) {
            return false;
        }
        self.albums.extend(albums);
        self.pending_batch = None;
        self.next_batch = (!complete).then_some(batch + 1);
        true
    }

    pub(crate) fn fail_batch(&mut self, batch: u64) {
        if self.pending_batch == Some(batch) {
            self.pending_batch = None;
        }
    }

    pub(crate) fn has_more(&self) -> bool {
        self.next_batch.is_some()
    }

    pub(crate) fn is_loading(&self) -> bool {
        self.pending_batch.is_some()
    }
}
