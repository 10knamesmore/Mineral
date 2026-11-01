//! 歌单曲目的已加载数据与请求状态；首批结果不满足整张操作。

use mineral_common::revision::Revision;
use mineral_protocol::{SubscriptionDemand, SubscriptionId};

use crate::runtime::view_model::CollectionTrackView;

/// 歌单或专辑已到达的曲目，同时保留内容完整性与来源确认状态。
#[derive(Clone, Debug)]
pub struct CollectionTracks {
    /// 曲目内容最后一次更新的曲库版本；
    pub revision: Revision,

    /// 按歌单原始位置排列的已加载条目。
    pub entries: Vec<CollectionTrackView>,

    /// channel 是否已检查整个集合的内容。
    pub complete: bool,

    /// channel 已确认当前快照有效；旧快照仅供展示，整张操作等待确认。
    pub fresh: bool,
}

impl std::ops::Deref for CollectionTracks {
    type Target = Vec<CollectionTrackView>;
    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl std::ops::DerefMut for CollectionTracks {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.entries
    }
}

/// 一个加载意图的生命周期；失败等待显式操作重试。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RequestState {
    /// 尚未提交，或先前请求已成功收束。
    #[default]
    Idle,

    /// 已提交，等待结果。
    Pending,

    /// 上一次请求失败。
    Failed,
}

/// 同一个订阅保留读取进度；完整需求会持续等待到 complete 的新鲜快照。
#[derive(Default)]
pub struct PlaylistRequests {
    /// 已建立的共享订阅；失败后显式重试会换一个订阅。
    pub(crate) subscription: Option<SubscriptionId>,

    /// Release is still needed until unsubscribe has entered the client queue.
    pub(crate) release_requested: bool,

    /// Whether a requested batch is pending or failed.
    state: RequestState,

    /// An intermediate Fresh snapshot must not finish a whole-playlist demand.
    reading_to_end: bool,

    /// 上次检查续页时的导航动作；失败后再次移动光标才重试。
    page_navigation: Option<std::time::Instant>,
}

impl PlaylistRequests {
    /// Subscription succeeded; no batch is pending until Advance also succeeds.
    pub(crate) fn subscribed(&mut self, id: SubscriptionId) {
        self.subscription = Some(id);
        self.release_requested = false;
        self.state = RequestState::Idle;
        self.reading_to_end = false;
    }

    /// Refresh or removal no longer needs this stream; keep its identity until release succeeds.
    pub(crate) fn release(&mut self) {
        self.release_requested = true;
        self.state = RequestState::Idle;
        self.reading_to_end = false;
        self.page_navigation = None;
    }

    /// 每次导航动作只允许一次失败重试，持续停在底部不会逐帧重发。
    pub(super) fn observe_page_navigation(&mut self, changed_at: std::time::Instant) -> bool {
        self.page_navigation.replace(changed_at) != Some(changed_at)
    }

    /// 完整加载仍在进行，供 deep search 展示按歌单计数的进度。
    pub(super) fn completing(&self) -> bool {
        self.reading_to_end && self.state == RequestState::Pending
    }

    /// 判断是否可以提交；正在进行的完整请求也覆盖预览需求。
    pub(super) fn can_request(&self, demand: SubscriptionDemand, retry_failed: bool) -> bool {
        match self.state {
            RequestState::Idle => true,
            RequestState::Pending => demand == SubscriptionDemand::Complete && !self.reading_to_end,
            RequestState::Failed => retry_failed,
        }
    }

    /// 记录交给提交层的意图。
    pub(crate) fn requested(&mut self, demand: SubscriptionDemand) {
        self.state = RequestState::Pending;
        self.reading_to_end |= demand == SubscriptionDemand::Complete;
    }

    /// 失败的流已经结束，显式重试需要重新订阅。
    pub(crate) fn failed(&self) -> bool {
        self.state == RequestState::Failed
    }

    /// 完整需求不能被中途的新鲜前缀收束。
    pub(super) fn finished(&mut self, complete: bool, success: bool) {
        if complete || !success {
            self.release_requested = true;
        }
        if !success {
            self.state = RequestState::Failed;
            self.reading_to_end = false;
        } else if complete || !self.reading_to_end {
            self.state = RequestState::Idle;
            self.reading_to_end = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::state::AppState;
    use mineral_channel_core::PlaylistDetail;
    use mineral_model::{Playlist, PlaylistEntry, PlaylistId, SourceKind};
    use mineral_protocol::SubscriptionDemand;
    use mineral_task::TaskEvent;

    /// 构造首批或完整结果，曲目数量故意小于标称总数，避免依赖计数推断完整性。
    fn fetched(id: &PlaylistId, complete: bool, songs: &[&str]) -> TaskEvent {
        TaskEvent::PlaylistDetailFetched {
            fresh: true,
            id: id.clone(),
            detail: Box::new(PlaylistDetail {
                playlist: Playlist::builder()
                    .id(id.clone())
                    .name("playlist".to_owned())
                    .track_count(20)
                    .entries(PlaylistEntry::enumerate(
                        songs.iter().map(|id| mineral_test::song(id)).collect(),
                    ))
                    .build(),
                complete,
            }),
        }
    }

    #[test]
    fn preview_can_upgrade_and_late_preview_cannot_replace_complete_data() -> color_eyre::Result<()>
    {
        let mut state = AppState::test_default()?;
        let id = PlaylistId::new(SourceKind::NETEASE, "p");
        state
            .models
            .library
            .request_playlist(id.clone(), SubscriptionDemand::Next);
        assert!(
            state
                .models
                .library
                .needs_playlist(&id, SubscriptionDemand::Complete, false)
        );
        state
            .models
            .library
            .request_playlist(id.clone(), SubscriptionDemand::Complete);
        state.apply(&fetched(&id, true, &["1", "2"]));
        state.apply(&fetched(&id, false, &["1"]));
        let tracks = state
            .models
            .library
            .tracks
            .get(&id)
            .ok_or_else(|| color_eyre::eyre::eyre!("missing playlist"))?;
        assert!(tracks.complete);
        assert_eq!(tracks.len(), 2, "晚到首批不能截短完整结果");
        assert!(
            !state
                .models
                .library
                .needs_playlist(&id, SubscriptionDemand::Complete, true)
        );
        Ok(())
    }

    #[test]
    fn cached_snapshot_does_not_shrink_tracks_or_complete_pending_read() -> color_eyre::Result<()> {
        let mut state = AppState::test_default()?;
        let id = PlaylistId::new(SourceKind::NETEASE, "p");
        state.apply(&fetched(&id, false, &["1", "2"]));
        state
            .models
            .library
            .request_playlist(id.clone(), SubscriptionDemand::Complete);
        let mut cached = fetched(&id, false, &["1"]);
        if let TaskEvent::PlaylistDetailFetched { fresh, .. } = &mut cached {
            *fresh = false;
        }
        state.apply(&cached);
        assert!(
            !state
                .models
                .library
                .needs_playlist(&id, SubscriptionDemand::Complete, true),
            "缓存结果不能收束完整读取"
        );
        state.apply(&fetched(&id, false, &["1", "2", "3"]));
        state.apply(&cached);
        assert_eq!(
            state
                .models
                .library
                .tracks
                .get(&id)
                .map(|tracks| tracks.len()),
            Some(3)
        );
        assert_eq!(state.models.library.completing_collections(), 1);
        Ok(())
    }

    #[test]
    fn failed_completion_keeps_preview_and_waits_for_explicit_retry() -> color_eyre::Result<()> {
        let mut state = AppState::test_default()?;
        let id = PlaylistId::new(SourceKind::NETEASE, "p");
        state.apply(&fetched(&id, false, &["1"]));
        assert!(
            state
                .models
                .library
                .needs_playlist(&id, SubscriptionDemand::Complete, false)
        );
        state
            .models
            .library
            .request_playlist(id.clone(), SubscriptionDemand::Complete);
        state.apply(&TaskEvent::PlaylistDetailFailed { id: id.clone() });
        assert_eq!(
            state
                .models
                .library
                .tracks
                .get(&id)
                .map(|tracks| tracks.len()),
            Some(1)
        );
        assert!(!state.models.library.playlist_complete(&id));
        assert!(
            !state
                .models
                .library
                .needs_playlist(&id, SubscriptionDemand::Complete, false),
            "逐帧预取不能自动重试失败"
        );
        assert!(
            state
                .models
                .library
                .needs_playlist(&id, SubscriptionDemand::Complete, true),
            "显式操作可以重试"
        );
        state.apply(&fetched(&id, true, &[]));
        assert!(
            state.models.library.playlist_complete(&id),
            "空歌单也能确认加载完整"
        );
        Ok(())
    }
}
