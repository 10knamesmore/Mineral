//! 曲目过滤结果的复用与只读视图。

use std::sync::Arc;

use mineral_model::PlaylistId;

use crate::runtime::view_model::PlaylistEntryView;

use super::search::SearchState;

/// 当前歌单与查询对应的行顺序；只存下标，歌曲与装饰始终从原始列表现读。
#[derive(Default)]
pub(super) struct TrackFilterCache {
    /// 尚未读过曲目时为空；切换歌单、查询或曲目版本时替换。
    cached: Option<CachedTracks>,
}

/// 一份过滤结果及其失效条件。
struct CachedTracks {
    /// 结果所属歌单，区分同一版本下的不同集合。
    playlist: PlaylistId,

    /// 曲目内容版本；收藏、播放次数等装饰变化不影响匹配。
    generation: u64,

    /// 产生当前结果的查询词。
    query: String,

    /// 按得分稳定排序的原列表下标；空词直接使用原顺序。
    order: Option<Arc<[usize]>>,

    /// 当前结果中已知时长的合计，空集合为零。
    total_duration_ms: u64,
}

/// 借用原始曲目的过滤视图；复制视图只共享行顺序，不复制歌曲。
pub(crate) struct FilteredTracks<'a> {
    /// 包含实时装饰值的原始曲目。
    entries: &'a [PlaylistEntryView],

    /// 有查询时的可见行顺序；无查询时直接按原始下标访问。
    order: Option<Arc<[usize]>>,

    /// 已知时长合计。
    total_duration_ms: u64,
}

impl<'a> FilteredTracks<'a> {
    /// 尚无当前歌单时的空视图。
    pub(super) fn empty() -> Self {
        Self {
            entries: &[],
            order: None,
            total_duration_ms: 0,
        }
    }

    /// 可见曲目数。
    pub(crate) fn len(&self) -> usize {
        self.order
            .as_ref()
            .map_or(self.entries.len(), |order| order.len())
    }

    /// 当前查询是否没有可见曲目。
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 按过滤后的下标借用一条 relation，保留其原始 CollectionIndex。
    pub(crate) fn get(&self, index: usize) -> Option<&'a PlaylistEntryView> {
        let raw = match &self.order {
            Some(order) => *order.get(index)?,
            None => index,
        };
        self.entries.get(raw)
    }

    /// 测试读取当前过滤视图的首行。
    #[cfg(test)]
    pub(crate) fn first(&self) -> Option<&'a PlaylistEntryView> {
        self.get(/*index*/ 0)
    }

    /// 按显示顺序借用全部曲目；需要发送歌曲时由调用方显式复制。
    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &'a PlaylistEntryView> + '_ {
        (0..self.len()).filter_map(|index| self.get(index))
    }

    /// 当前结果的已知时长合计，随曲目或查询变化更新。
    pub(crate) fn total_duration_ms(&self) -> u64 {
        self.total_duration_ms
    }
}

impl TrackFilterCache {
    /// 获取当前视图；数据与查询未变时复用排序结果和时长合计。
    ///
    /// # Params:
    ///   - `playlist`: 当前歌单身份
    ///   - `generation`: 曲目内容版本
    ///   - `entries`: 当前原始曲目与装饰
    ///   - `search`: 当前查询及可复用的匹配器
    ///
    /// # Return:
    ///   借用当前曲目的视图，不持有缓存的可变借用。
    pub(super) fn view<'a>(
        &mut self,
        playlist: &PlaylistId,
        generation: u64,
        entries: &'a [PlaylistEntryView],
        search: &SearchState,
    ) -> FilteredTracks<'a> {
        let cached = self
            .cached
            .get_or_insert_with(|| Self::rebuild(playlist, generation, entries, search));
        if cached.playlist != *playlist
            || cached.generation != generation
            || cached.query != search.query()
        {
            *cached = Self::rebuild(playlist, generation, entries, search);
        }
        FilteredTracks {
            entries,
            order: cached.order.clone(),
            total_duration_ms: cached.total_duration_ms,
        }
    }

    /// 查询或曲目变化时重建下标顺序；同分项保持 relation 的原始顺序。
    fn rebuild(
        playlist: &PlaylistId,
        generation: u64,
        entries: &[PlaylistEntryView],
        search: &SearchState,
    ) -> CachedTracks {
        let started = std::time::Instant::now();
        let (order, total_duration_ms) = if search.query().is_empty() {
            let total = entries
                .iter()
                .filter_map(|entry| entry.data.song.duration_ms)
                .sum();
            (None, total)
        } else {
            let mut scored = entries
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    search
                        .song_score(&entry.data.song)
                        .map(|score| (score, index))
                })
                .collect::<Vec<_>>();
            scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
            let total = scored
                .iter()
                .filter_map(|&(_, index)| {
                    entries
                        .get(index)
                        .and_then(|entry| entry.data.song.duration_ms)
                })
                .sum();
            let order = scored
                .into_iter()
                .map(|(_, index)| index)
                .collect::<Arc<[usize]>>();
            (Some(order), total)
        };
        mineral_log::debug!(
            target: "filter",
            playlist = %playlist.qualified(),
            tracks = entries.len(),
            matches = order.as_ref().map_or(entries.len(), |order| order.len()),
            elapsed_us = started.elapsed().as_micros(),
            "rebuilt track filter"
        );
        CachedTracks {
            playlist: playlist.clone(),
            generation,
            query: search.query().to_owned(),
            order,
            total_duration_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::{CollectionIndex, Playlist, PlaylistEntry, PlaylistId, SourceKind};
    use mineral_task::TaskEvent;

    use crate::runtime::state::{AppState, View};
    use crate::runtime::view_model::PlaylistView;

    /// 通过真实详情事件更新曲目，保持调用方已有的 relation 坐标。
    fn replace_tracks(state: &mut AppState, id: &PlaylistId, entries: Vec<PlaylistEntry>) {
        state.apply(&TaskEvent::PlaylistDetailFetched {
            id: id.clone(),
            playlist: Box::new(
                Playlist::builder()
                    .id(id.clone())
                    .name("歌单".to_owned())
                    .entries(entries)
                    .build(),
            ),
        });
    }

    /// 查询复用后仍消费实时装饰，并在同一歌单 metadata 更新后重排与重算时长。
    #[test]
    fn filter_refreshes_metadata_query_and_decorations() -> color_eyre::Result<()> {
        let mut state = AppState::test_default()?;
        let id = PlaylistId::new(SourceKind::NETEASE, "p1");
        state.library.playlists = vec![PlaylistView {
            data: Playlist::builder()
                .id(id.clone())
                .name("歌单".to_owned())
                .build(),
        }];
        state.browse.view.switch_to(View::Library);
        let matching = mineral_test::with_duration(
            mineral_test::with_name(mineral_test::song("same"), "春日影"),
            60_000,
        );
        let other = mineral_test::with_duration(
            mineral_test::with_name(mineral_test::song("other"), "夏天"),
            120_000,
        );
        replace_tracks(
            &mut state,
            &id,
            vec![
                PlaylistEntry::builder()
                    .index(CollectionIndex::new(/*value*/ 4))
                    .song(matching.clone())
                    .build(),
                PlaylistEntry::builder()
                    .index(CollectionIndex::new(/*value*/ 8))
                    .song(other.clone())
                    .build(),
                PlaylistEntry::builder()
                    .index(CollectionIndex::new(/*value*/ 12))
                    .song(matching.clone())
                    .build(),
            ],
        );
        state.browse.search.set_query("cry");
        let indexes = || {
            state
                .filtered_tracks()
                .iter()
                .map(|entry| entry.data.index.get())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            indexes(),
            vec![4, 12],
            "同分重复歌曲保留各自 relation 和原序"
        );
        assert_eq!(state.filtered_tracks().total_duration_ms(), 120_000);
        state.toggle_loved_local(&matching);
        assert!(
            state.filtered_tracks().iter().all(|entry| entry.loved),
            "缓存不能冻结收藏装饰"
        );

        let renamed = mineral_test::with_name(matching, "冬天");
        let newly_matching = mineral_test::with_name(other, "春日影");
        replace_tracks(
            &mut state,
            &id,
            PlaylistEntry::enumerate(vec![renamed, newly_matching]),
        );
        assert_eq!(
            state
                .filtered_tracks()
                .iter()
                .map(|entry| entry.data.song.id.value())
                .collect::<Vec<_>>(),
            vec!["other"]
        );
        assert_eq!(state.filtered_tracks().total_duration_ms(), 120_000);
        state.browse.search.set_query("冬天");
        assert_eq!(
            state
                .filtered_tracks()
                .iter()
                .map(|entry| entry.data.song.id.value())
                .collect::<Vec<_>>(),
            vec!["same"]
        );
        state.browse.search.clear();
        assert_eq!(state.filtered_tracks().len(), 2);
        assert_eq!(state.filtered_tracks().total_duration_ms(), 180_000);
        Ok(())
    }
}
