//! 歌单曲目的已加载数据与请求状态；首批结果不满足整张操作。

use crate::runtime::view_model::PlaylistEntryView;
use mineral_channel_core::PlaylistLoad;

/// 一张歌单已到达的曲目；像曲目列表一样读取，同时保留完整性。
#[derive(Clone, Debug)]
pub struct PlaylistTracks {
    /// 按歌单原始位置排列的已加载条目。
    pub entries: Vec<PlaylistEntryView>,

    /// channel 是否已检查整个歌单。
    pub complete: bool,

    /// 来源确认的下一批起点；和条目数量独立，保留缺失 metadata 的位置缺口。
    pub next_offset: Option<u64>,
}

impl std::ops::Deref for PlaylistTracks {
    type Target = Vec<PlaylistEntryView>;
    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl std::ops::DerefMut for PlaylistTracks {
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

/// 首批、续页和完整请求独立登记，完整意图不会被预览吞掉。
#[derive(Default)]
pub struct PlaylistRequests {
    /// 首批预览的请求状态。
    preview: RequestState,

    /// 完整加载的请求状态。
    complete: RequestState,

    /// 正在请求或失败待重试的续页；一次只推进一批。
    more: Option<(u64, RequestState)>,

    /// 上次检查续页时的导航动作；失败后再次移动光标才重试。
    page_navigation: Option<std::time::Instant>,
}

impl PlaylistRequests {
    /// 每次导航动作只允许一次失败重试，持续停在底部不会逐帧重发。
    pub(super) fn observe_page_navigation(&mut self, changed_at: std::time::Instant) -> bool {
        self.page_navigation.replace(changed_at) != Some(changed_at)
    }

    /// 完整加载仍在进行，供 deep search 展示按歌单计数的进度。
    pub(super) fn completing(&self) -> bool {
        self.complete == RequestState::Pending
    }

    /// 取得与请求意图匹配的状态。
    fn state(&self, load: PlaylistLoad) -> RequestState {
        match load {
            PlaylistLoad::Preview => self.preview,
            PlaylistLoad::Complete => self.complete,
            PlaylistLoad::More { offset } => self
                .more
                .filter(|(requested, _)| *requested == offset)
                .map_or(RequestState::Idle, |(_, state)| state),
        }
    }

    /// 更新一个意图的状态，不影响另一个请求。
    fn set(&mut self, load: PlaylistLoad, state: RequestState) {
        match load {
            PlaylistLoad::Preview => self.preview = state,
            PlaylistLoad::Complete => self.complete = state,
            PlaylistLoad::More { offset } => self.more = Some((offset, state)),
        }
    }

    /// 判断是否可以提交；正在进行的完整请求也覆盖预览需求。
    pub(super) fn can_request(&self, load: PlaylistLoad, retry_failed: bool) -> bool {
        if load != PlaylistLoad::Complete && self.complete == RequestState::Pending {
            return false;
        }
        match self.state(load) {
            RequestState::Idle => true,
            RequestState::Pending => false,
            RequestState::Failed => retry_failed,
        }
    }

    /// 记录交给提交层的意图。
    pub(super) fn requested(&mut self, load: PlaylistLoad) {
        self.set(load, RequestState::Pending);
    }

    /// 收束请求；失败保留已加载曲目，后续显式操作可重试。
    pub(super) fn finished(&mut self, load: PlaylistLoad, success: bool) {
        if let PlaylistLoad::More { offset } = load
            && self.more.is_some_and(|(requested, _)| requested != offset)
        {
            return;
        }
        self.set(
            load,
            if success {
                RequestState::Idle
            } else {
                RequestState::Failed
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::state::AppState;
    use mineral_channel_core::{PlaylistDetail, PlaylistLoad};
    use mineral_model::{Playlist, PlaylistEntry, PlaylistId, SourceKind};
    use mineral_task::TaskEvent;

    /// 构造首批或完整结果，曲目数量故意小于标称总数，避免依赖计数推断完整性。
    fn fetched(
        id: &PlaylistId,
        load: PlaylistLoad,
        complete: bool,
        songs: &[&str],
    ) -> color_eyre::Result<TaskEvent> {
        Ok(TaskEvent::PlaylistDetailFetched {
            id: id.clone(),
            load,
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
                next_offset: (!complete).then_some(u64::try_from(songs.len())?),
            }),
        })
    }

    #[test]
    fn preview_can_upgrade_and_late_preview_cannot_replace_complete_data() -> color_eyre::Result<()>
    {
        let mut state = AppState::test_default()?;
        let id = PlaylistId::new(SourceKind::NETEASE, "p");
        state
            .library
            .request_playlist(id.clone(), PlaylistLoad::Preview);
        assert!(
            state
                .library
                .needs_playlist(&id, PlaylistLoad::Complete, false)
        );
        state
            .library
            .request_playlist(id.clone(), PlaylistLoad::Complete);
        state.apply(&fetched(&id, PlaylistLoad::Complete, true, &["1", "2"])?);
        state.apply(&fetched(&id, PlaylistLoad::Preview, false, &["1"])?);
        let tracks = state
            .library
            .tracks
            .get(&id)
            .ok_or_else(|| color_eyre::eyre::eyre!("missing playlist"))?;
        assert!(tracks.complete);
        assert_eq!(tracks.len(), 2, "晚到首批不能截短完整结果");
        assert!(
            !state
                .library
                .needs_playlist(&id, PlaylistLoad::Complete, true)
        );
        Ok(())
    }

    #[test]
    fn late_page_does_not_shrink_tracks_or_clear_next_request() -> color_eyre::Result<()> {
        let mut state = AppState::test_default()?;
        let id = PlaylistId::new(SourceKind::NETEASE, "p");
        state.apply(&fetched(&id, PlaylistLoad::Preview, false, &["1"])?);
        let second = PlaylistLoad::More { offset: 1 };
        state.library.request_playlist(id.clone(), second);
        state.apply(&fetched(&id, second, false, &["1", "2"])?);
        let third = PlaylistLoad::More { offset: 2 };
        state.library.request_playlist(id.clone(), third);
        state.apply(&fetched(&id, second, false, &["1", "2"])?);
        assert!(
            !state.library.needs_playlist(&id, third, true),
            "晚到的上一页不能清掉下一页的在途状态"
        );
        state.apply(&fetched(&id, third, false, &["1", "2", "3"])?);
        state.apply(&fetched(&id, second, false, &["1", "2"])?);
        state.apply(&fetched(&id, PlaylistLoad::Preview, false, &["1"])?);
        assert_eq!(
            state
                .library
                .tracks
                .get(&id)
                .map(|tracks| (tracks.len(), tracks.next_offset)),
            Some((3, Some(3)))
        );
        Ok(())
    }

    #[test]
    fn failed_completion_keeps_preview_and_waits_for_explicit_retry() -> color_eyre::Result<()> {
        let mut state = AppState::test_default()?;
        let id = PlaylistId::new(SourceKind::NETEASE, "p");
        state.apply(&fetched(&id, PlaylistLoad::Preview, false, &["1"])?);
        assert!(
            state
                .library
                .needs_playlist(&id, PlaylistLoad::Complete, false)
        );
        state
            .library
            .request_playlist(id.clone(), PlaylistLoad::Complete);
        state.apply(&TaskEvent::PlaylistDetailFailed {
            id: id.clone(),
            load: PlaylistLoad::Complete,
        });
        assert_eq!(
            state.library.tracks.get(&id).map(|tracks| tracks.len()),
            Some(1)
        );
        assert!(!state.library.playlist_complete(&id));
        assert!(
            !state
                .library
                .needs_playlist(&id, PlaylistLoad::Complete, false),
            "逐帧预取不能自动重试失败"
        );
        assert!(
            state
                .library
                .needs_playlist(&id, PlaylistLoad::Complete, true),
            "显式操作可以重试"
        );
        state.apply(&fetched(&id, PlaylistLoad::Complete, true, &[])?);
        assert!(
            state.library.playlist_complete(&id),
            "空歌单也能确认加载完整"
        );
        Ok(())
    }
}
