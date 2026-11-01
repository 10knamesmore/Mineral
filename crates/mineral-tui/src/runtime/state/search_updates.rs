//! 将搜索分页与实体详情回包应用到匹配的会话和详情帧。

use mineral_model::{Album, AlbumId, Artist, ArtistId, SearchKind, SourceKind};
use mineral_task::SearchPayload;

use super::AppState;

impl AppState {
    /// 把一页搜索结果落进配对的会话：按事件自带 `source` 找会话、`query` 配对（源级，
    /// 改 query 作废全部 kind 桶），事件自带 `kind` 决定存哪个桶——这样切 kind 时旧 kind
    /// 的飞行响应也能落对桶。首批建桶，后续批次追加。
    ///
    /// # Params:
    ///   - `source` / `kind` / `query`: 回带的请求三元组（source 找会话、query 配对、kind 选桶）
    ///   - `batch`: 从零开始的批次序号
    ///   - `payload`: 结果载荷
    ///   - `complete`: 来源是否已经读完
    pub(super) fn apply_search_results(
        &mut self,
        source: SourceKind,
        kind: SearchKind,
        query: &str,
        batch: u64,
        payload: &SearchPayload,
        complete: bool,
    ) {
        // caps 先读，随后 session 借走 channel_search。仅首页建桶时设置默认分区；
        // 续页保留已操作的详情导航，新选中实体的默认分区由结果桶保存的能力初始化。
        let sections = self
            .models
            .caps
            .get(&source)
            .map(|channel_caps| channel_caps.artist_sections().clone());
        let Some(session) = self.ui.channel_search.session_for_mut(source) else {
            return;
        };
        if session.query() != query {
            return;
        }
        if !session.apply_page(kind, payload.clone(), batch, complete) {
            return;
        }
        if batch == 0
            && let Some(sections) = sections
        {
            mineral_log::debug!(target: "tui", ?source, ?kind, "initialize search detail sections");
            session.apply_sections(kind, sections);
        }
    }

    /// 按 source、kind、query 与批次序号释放失败请求；切 source / kind 后仍更新原桶。
    pub(super) fn apply_search_page_failed(
        &mut self,
        source: SourceKind,
        kind: SearchKind,
        query: &str,
        batch: u64,
    ) {
        if let Some(session) = self.ui.channel_search.session_for_mut(source)
            && session.query() == query
        {
            session.fail_page(kind, batch);
        }
    }

    /// 将艺人详情应用到所有会话中匹配的保留帧。
    pub(super) fn apply_artist_detail(&mut self, id: &ArtistId, artist: &Artist) {
        for results in self.ui.channel_search.retained_results_mut() {
            results.fill_artist_detail(id, artist);
        }
    }

    /// 共享列表接收批次，再同步到各个保留帧。
    pub(super) fn apply_artist_albums(
        &mut self,
        id: &ArtistId,
        albums: &[Album],
        batch: u64,
        complete: bool,
    ) {
        let listing = self
            .models
            .library
            .artist_albums
            .entry(id.clone())
            .or_default();
        if listing
            .albums
            .accept_batch(albums.to_vec(), batch, complete)
        {
            self.sync_artist_albums(id);
        }
    }

    /// 失败只释放对应批次，后续导航继续原订阅。
    pub(super) fn apply_artist_albums_page_failed(&mut self, id: &ArtistId, batch: u64) {
        if let Some(listing) = self.models.library.artist_albums.get_mut(id) {
            listing.albums.fail_batch(batch);
            self.sync_artist_albums(id);
        }
    }

    pub(crate) fn sync_artist_albums(&mut self, id: &ArtistId) {
        if let Some(listing) = self.models.library.artist_albums.get(id) {
            for results in self.ui.channel_search.retained_results_mut() {
                results.fill_artist_albums(id, &listing.albums);
            }
        }
    }

    /// 将完整专辑应用到所有会话中匹配的保留帧和结果行。
    pub(super) fn apply_album_detail(&mut self, id: &AlbumId, album: &Album, fresh: bool) {
        for results in self.ui.channel_search.retained_results_mut() {
            results.fill_album_detail(id, album, fresh);
        }
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::{SearchKind, SourceKind};

    use super::AppState;

    /// 造一个已入会(源 NETEASE、kind Song、query 给定)的 AppState,供 SearchResults 配对测试用。
    fn state_in_search(query: &str) -> color_eyre::Result<AppState> {
        use mineral_channel_core::ChannelCaps;
        use mineral_model::{SearchKind, SourceKind};
        use rustc_hash::FxHashMap;

        let mut s = AppState::test_default()?;
        let mut caps = FxHashMap::default();
        caps.insert(
            SourceKind::NETEASE,
            ChannelCaps::builder()
                .searchable(vec![SearchKind::Song, SearchKind::Album])
                .playlist_edit(false)
                .artist_sections(mineral_channel_core::ArtistSections::new(vec![
                    mineral_channel_core::ArtistSectionKind::TopSongs,
                    mineral_channel_core::ArtistSectionKind::Albums,
                ]))
                .build(),
        );
        s.models.caps = caps;
        s.ui.channel_search.enter(&s.models.caps);
        if let Some(session) = s.ui.channel_search.current_mut() {
            session.set_query(query.to_owned());
        }
        Ok(s)
    }

    /// 造一个入会(源 NETEASE、给定 kind、query)的 AppState。
    fn state_searching(query: &str, kind: SearchKind) -> color_eyre::Result<AppState> {
        use mineral_channel_core::ChannelCaps;
        use rustc_hash::FxHashMap;

        let mut s = AppState::test_default()?;
        let mut caps = FxHashMap::default();
        caps.insert(
            SourceKind::NETEASE,
            ChannelCaps::builder()
                .searchable(vec![
                    SearchKind::Song,
                    SearchKind::Album,
                    SearchKind::Artist,
                    SearchKind::Playlist,
                ])
                .playlist_edit(false)
                .artist_sections(mineral_channel_core::ArtistSections::new(vec![
                    mineral_channel_core::ArtistSectionKind::TopSongs,
                    mineral_channel_core::ArtistSectionKind::Albums,
                ]))
                .build(),
        );
        s.models.caps = caps;
        s.ui.channel_search.enter(&s.models.caps);
        if let Some(session) = s.ui.channel_search.current_mut() {
            session.set_query(query.to_owned());
            session.set_kind(kind);
        }
        Ok(s)
    }

    /// 造一张专辑(测试 helper)。
    fn album_fixture(raw: &str) -> mineral_model::Album {
        mineral_model::Album::builder()
            .id(mineral_model::AlbumId::new(SourceKind::NETEASE, raw))
            .name(format!("album {raw}"))
            .build()
    }

    /// 造一个 artist(测试 helper)。
    fn artist_fixture(raw: &str) -> mineral_model::Artist {
        mineral_model::Artist::builder()
            .id(mineral_model::ArtistId::new(SourceKind::NETEASE, raw))
            .name(format!("artist {raw}"))
            .build()
    }

    /// 读取当前详情帧，缺帧时让测试明确失败。
    fn current_frame(s: &AppState) -> color_eyre::Result<&crate::runtime::state::DetailFrame> {
        s.ui.channel_search
            .active_results()
            .and_then(|results| results.detail.current())
            .ok_or_else(|| color_eyre::eyre::eyre!("应有详情帧"))
    }

    /// 标记当前详情帧已发起预取，模拟等待回包时离开该帧。
    fn mark_detail_requested(s: &mut AppState) -> color_eyre::Result<()> {
        let frame =
            s.ui.channel_search
                .active_results_mut()
                .and_then(|results| results.detail.current_mut())
                .ok_or_else(|| color_eyre::eyre::eyre!("应有详情帧"))?;
        frame.mark_requested();
        Ok(())
    }

    /// 搜索续页追加艺人时保留已选分区、光标和滚动位置，离屏回包也不重置。
    #[test]
    fn artist_search_page_preserves_detail_navigation() -> color_eyre::Result<()> {
        use crate::test_support::artist_albums;
        use mineral_task::{SearchPayload, TaskEvent};

        use crate::runtime::state::{ArtistSection, EntityRef};

        for switch_kind in [false, true] {
            let mut state = state_searching("q", SearchKind::Artist)?;
            state.apply(&TaskEvent::SearchResults {
                source: SourceKind::NETEASE,
                kind: SearchKind::Artist,
                query: "q".to_owned(),
                batch: 0,
                payload: SearchPayload::Artists(vec![artist_fixture("first")]),
                complete: false,
            });
            assert_eq!(current_frame(&state)?.section, ArtistSection::Hot);
            let results = state
                .ui
                .channel_search
                .active_results_mut()
                .ok_or_else(|| color_eyre::eyre::eyre!("缺少艺人结果"))?;
            assert_eq!(results.next_batch(), Some(1));
            results.batch_requested(1);
            let frame = results
                .detail
                .current_mut()
                .ok_or_else(|| color_eyre::eyre::eyre!("缺少艺人详情"))?;
            let mut artist = artist_fixture("first");
            artist.songs = crate::test_support::endserenading(2);
            frame.set_artist_detail(Box::new(artist));
            frame.set_artist_albums(artist_albums(
                (0..8)
                    .map(|index| album_fixture(&format!("album-{index}")))
                    .collect(),
                true,
            ));
            frame.cycle_section(8);
            frame.list_mut().place(6, 2);
            frame.nudge_description(3);
            state.ui.channel_search.tick();
            state.ui.channel_search.tick();
            assert_eq!(current_frame(&state)?.list().offset(8, 3), 4);

            if switch_kind {
                state.ui.channel_search.select_kind(SearchKind::Album);
            }
            state.apply(&TaskEvent::SearchResults {
                source: SourceKind::NETEASE,
                kind: SearchKind::Artist,
                query: "q".to_owned(),
                batch: 1,
                payload: SearchPayload::Artists(vec![artist_fixture("second")]),
                complete: true,
            });
            if switch_kind {
                state.ui.channel_search.select_kind(SearchKind::Artist);
            }
            let results = state
                .ui
                .channel_search
                .active_results()
                .ok_or_else(|| color_eyre::eyre::eyre!("缺少艺人结果"))?;
            assert_eq!(results.len(), 2, "续页必须实际追加，不能丢弃回包来保留导航");
            assert_eq!(results.sel(), 0);
            let frame = current_frame(&state)?;
            assert_eq!(frame.section, ArtistSection::Albums);
            assert_eq!(frame.list().sel(), 6);
            assert_eq!(frame.list().offset(8, 3), 4);
            assert_eq!(frame.description_scroll(), 3);
            assert!(matches!(frame.row_entity(), Some(EntityRef::Album(album))
                if album.id == album_fixture("album-6").id));

            state
                .ui
                .channel_search
                .active_results_mut()
                .ok_or_else(|| color_eyre::eyre::eyre!("缺少艺人结果"))?
                .set_sel(1);
            let frame = current_frame(&state)?;
            assert!(matches!(&frame.entity, EntityRef::Artist(artist)
                if artist.id == artist_fixture("second").id));
            assert_eq!(
                frame.section,
                ArtistSection::Hot,
                "真正选择新艺人时初始化默认分区"
            );
            assert_eq!(frame.list().sel(), 0);
        }
        Ok(())
    }

    /// 只提供 Albums 的来源在首页及续页艺人的新详情帧中都默认展示 Albums。
    #[test]
    fn album_only_source_initializes_first_and_later_artist_frames() -> color_eyre::Result<()> {
        use mineral_channel_core::{ArtistSectionKind, ArtistSections, ChannelCaps};
        use mineral_model::{Artist, ArtistId};
        use mineral_task::{SearchPayload, TaskEvent};

        use crate::runtime::state::{ArtistSection, EntityRef};

        let mut state = state_searching("q", SearchKind::Artist)?;
        state.models.caps.insert(
            SourceKind::BILIBILI,
            ChannelCaps::builder()
                .searchable(vec![SearchKind::Artist])
                .playlist_edit(false)
                .artist_sections(ArtistSections::new(vec![ArtistSectionKind::Albums]))
                .build(),
        );
        state
            .ui
            .channel_search
            .switch_source(SourceKind::BILIBILI, &state.models.caps);
        state
            .ui
            .channel_search
            .current_mut()
            .ok_or_else(|| color_eyre::eyre::eyre!("缺少 Bilibili 搜索会话"))?
            .set_query("q");
        for offset in 0..2 {
            if offset > 0 {
                assert_eq!(
                    state
                        .ui
                        .channel_search
                        .active_results_mut()
                        .ok_or_else(|| color_eyre::eyre::eyre!("缺少艺人结果"))?
                        .next_batch(),
                    Some(1)
                );
                state
                    .ui
                    .channel_search
                    .current_mut()
                    .ok_or_else(|| color_eyre::eyre::eyre!("缺少搜索会话"))?
                    .batch_requested(SearchKind::Artist, 1);
            }
            let id = ArtistId::new(SourceKind::BILIBILI, format!("uploader-{offset}"));
            state.apply(&TaskEvent::SearchResults {
                source: SourceKind::BILIBILI,
                kind: SearchKind::Artist,
                query: "q".to_owned(),
                batch: offset,
                payload: SearchPayload::Artists(vec![
                    Artist::builder()
                        .id(id.clone())
                        .name(format!("UP {offset}"))
                        .build(),
                ]),
                complete: offset != 0,
            });
            let results = state
                .ui
                .channel_search
                .active_results_mut()
                .ok_or_else(|| color_eyre::eyre::eyre!("缺少艺人结果"))?;
            assert_eq!(results.len(), usize::try_from(offset + 1)?);
            results.set_sel(usize::try_from(offset)?);
            let frame = current_frame(&state)?;
            assert!(matches!(&frame.entity, EntityRef::Artist(artist) if artist.id == id));
            assert_eq!(frame.section, ArtistSection::Albums);
            assert_eq!(
                frame.artist_sections().map(ArtistSections::kinds),
                Some([ArtistSectionKind::Albums].as_slice())
            );
        }
        Ok(())
    }

    /// 成功回包更新切走的来源或类型；其他来源的同名原始 ID 不接收。
    #[test]
    fn album_response_reaches_retained_source_and_kind() -> color_eyre::Result<()> {
        use crate::runtime::state::DetailData;
        use mineral_task::{SearchPayload, TaskEvent};

        for switch_source in [false, true] {
            let mut s = state_searching("q", SearchKind::Album)?;
            let album = album_fixture("al1");
            s.apply(&TaskEvent::SearchResults {
                source: SourceKind::NETEASE,
                kind: SearchKind::Album,
                query: "q".to_owned(),
                batch: 0,
                payload: SearchPayload::Albums(vec![album.clone()]),
                complete: false,
            });
            mark_detail_requested(&mut s)?;
            if switch_source {
                let caps = s
                    .models
                    .caps
                    .get(&SourceKind::NETEASE)
                    .cloned()
                    .ok_or_else(|| color_eyre::eyre::eyre!("应有来源能力"))?;
                s.models.caps.insert(SourceKind::BILIBILI, caps);
                s.ui.channel_search
                    .switch_source(SourceKind::BILIBILI, &s.models.caps);
                let session =
                    s.ui.channel_search
                        .current_mut()
                        .ok_or_else(|| color_eyre::eyre::eyre!("应有搜索会话"))?;
                session.set_query("q");
                session.set_kind(SearchKind::Album);
                // 原始 ID 相同但 namespace 不同，当前帧不能误收另一个来源的回包。
                let mut other_album = album.clone();
                other_album.id = mineral_model::AlbumId::new(SourceKind::BILIBILI, "al1");
                session.apply_page(
                    SearchKind::Album,
                    SearchPayload::Albums(vec![other_album]),
                    0,
                    false,
                );
            } else {
                s.ui.channel_search.select_kind(SearchKind::Artist);
            }
            let mut detailed = album;
            detailed.tracks =
                mineral_model::AlbumTrack::enumerate(crate::test_support::endserenading(3));
            s.apply(&TaskEvent::AlbumDetailFetched {
                fresh: true,
                id: detailed.id.clone(),
                album: Box::new(detailed.clone()),
            });
            if switch_source {
                assert!(
                    current_frame(&s)?.data.is_none(),
                    "其他来源的同名 ID 不受影响"
                );
                s.ui.channel_search
                    .switch_source(SourceKind::NETEASE, &s.models.caps);
            } else {
                assert!(
                    s.ui.channel_search.active_results().is_none(),
                    "回包不创建当前 kind 的结果"
                );
                s.ui.channel_search.select_kind(SearchKind::Album);
            }
            match &current_frame(&s)?.data {
                Some(DetailData::Album(received)) => assert_eq!(**received, detailed),
                _ => {
                    color_eyre::eyre::bail!("切回后应有完整专辑详情");
                }
            }
        }
        Ok(())
    }

    /// 艺人两路数据先后到达，切类型或下钻后收到的那一路仍合并到原帧。
    #[test]
    fn artist_responses_merge_while_frame_is_inactive() -> color_eyre::Result<()> {
        use crate::runtime::state::{DetailData, EntityRef};
        use mineral_task::{SearchPayload, TaskEvent};

        for albums_first in [false, true] {
            let mut s = state_searching("q", SearchKind::Artist)?;
            let mut artist = artist_fixture("ar1");
            let album = album_fixture("al1");
            s.apply(&TaskEvent::SearchResults {
                source: SourceKind::NETEASE,
                kind: SearchKind::Artist,
                query: "q".to_owned(),
                batch: 0,
                payload: SearchPayload::Artists(vec![artist.clone()]),
                complete: false,
            });
            mark_detail_requested(&mut s)?;
            artist.songs = crate::test_support::endserenading(2);
            let detail_event = TaskEvent::ArtistDetailFetched {
                id: artist.id.clone(),
                artist: Box::new(artist.clone()),
            };
            let albums_event = TaskEvent::ArtistAlbumsFetched {
                id: artist.id.clone(),
                batch: 0,
                albums: vec![album.clone()],
                complete: true,
            };
            let (first, second) = if albums_first {
                (&albums_event, &detail_event)
            } else {
                (&detail_event, &albums_event)
            };
            s.apply(first);
            if albums_first {
                s.ui.channel_search
                    .active_results_mut()
                    .ok_or_else(|| color_eyre::eyre::eyre!("应有结果"))?
                    .detail
                    .push(EntityRef::Album(Box::new(album.clone())), 1);
            } else {
                s.ui.channel_search.select_kind(SearchKind::Album);
                s.ui.channel_search
                    .current_mut()
                    .ok_or_else(|| color_eyre::eyre::eyre!("应有搜索会话"))?
                    .apply_page(
                        SearchKind::Album,
                        SearchPayload::Albums(vec![album.clone()]),
                        0,
                        false,
                    );
            }
            s.apply(second);
            assert!(
                current_frame(&s)?.data.is_none(),
                "艺人回包不填入当前专辑帧"
            );
            if albums_first {
                assert!(
                    s.ui.channel_search
                        .active_results_mut()
                        .ok_or_else(|| color_eyre::eyre::eyre!("应有结果"))?
                        .detail
                        .pop(1)
                );
            } else {
                s.ui.channel_search.select_kind(SearchKind::Artist);
            }
            match &current_frame(&s)?.data {
                Some(DetailData::Artist {
                    detail: Some(received),
                    albums: Some(albums),
                }) => {
                    assert_eq!(**received, artist);
                    assert_eq!(albums.items(), &[album]);
                }
                _ => {
                    color_eyre::eyre::bail!("返回父帧后应有两路艺人数据");
                }
            }
        }
        Ok(())
    }

    /// 同专辑在另一类型中再次加载，只更新数据，不重置歌曲详情内已移动的光标和视口。
    #[test]
    fn repeated_album_response_preserves_retained_song_navigation() -> color_eyre::Result<()> {
        use crate::runtime::state::DetailData;
        use mineral_model::{AlbumRef, AlbumTrack};
        use mineral_task::{SearchPayload, TaskEvent};

        let mut s = state_searching("q", SearchKind::Song)?;
        let mut album = album_fixture("al1");
        let songs = crate::test_support::endserenading(3);
        let mut song = songs
            .first()
            .cloned()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有歌曲"))?;
        song.album = Some(AlbumRef {
            id: album.id.clone(),
            name: album.name.clone(),
        });
        s.ui.channel_search
            .current_mut()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有搜索会话"))?
            .apply_page(SearchKind::Song, SearchPayload::Songs(vec![song]), 0, false);
        mark_detail_requested(&mut s)?;
        album.tracks = AlbumTrack::enumerate(songs);
        s.apply(&TaskEvent::AlbumDetailFetched {
            fresh: true,
            id: album.id.clone(),
            album: Box::new(album.clone()),
        });
        assert_eq!(
            current_frame(&s)?.list().sel(),
            0,
            "首次到货定位到结果选中的歌曲"
        );
        s.ui.channel_search
            .active_results_mut()
            .and_then(|results| results.detail.current_mut())
            .ok_or_else(|| color_eyre::eyre::eyre!("应有详情帧"))?
            .list_mut()
            .place(2, 1);
        s.ui.channel_search.select_kind(SearchKind::Album);
        s.ui.channel_search
            .current_mut()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有搜索会话"))?
            .apply_page(
                SearchKind::Album,
                SearchPayload::Albums(vec![album_fixture("al1")]),
                0,
                false,
            );
        mark_detail_requested(&mut s)?;
        album.description = "updated description".to_owned();
        s.apply(&TaskEvent::AlbumDetailFetched {
            fresh: true,
            id: album.id.clone(),
            album: Box::new(album.clone()),
        });
        s.ui.channel_search.select_kind(SearchKind::Song);
        let frame = current_frame(&s)?;
        assert_eq!(frame.list().sel(), 2, "保留用户选中的另一首歌曲");
        assert_eq!(frame.list().scroll_target(), 1, "保留原视口");
        match &frame.data {
            Some(DetailData::Album(received)) => assert_eq!(**received, album),
            _ => {
                color_eyre::eyre::bail!("应有更新后的专辑详情");
            }
        }
        Ok(())
    }

    /// 艺人帧重建后收到旧请求的单路数据，返回时仍能补拉缺失的那一路。
    #[test]
    fn rebuilt_artist_frame_with_partial_response_still_prefetches() -> color_eyre::Result<()> {
        use crate::runtime::state::DetailData;
        use mineral_task::{SearchPayload, TaskEvent};

        let mut s = state_searching("q", SearchKind::Artist)?;
        let artist = artist_fixture("ar1");
        s.ui.channel_search
            .current_mut()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有搜索会话"))?
            .apply_page(
                SearchKind::Artist,
                SearchPayload::Artists(vec![artist.clone(), artist_fixture("ar2")]),
                0,
                false,
            );
        mark_detail_requested(&mut s)?;
        let detail_event = TaskEvent::ArtistDetailFetched {
            id: artist.id.clone(),
            artist: Box::new(artist.clone()),
        };
        s.apply(&detail_event);
        let results =
            s.ui.channel_search
                .active_results_mut()
                .ok_or_else(|| color_eyre::eyre::eyre!("应有结果桶"))?;
        results.set_sel(1);
        results.set_sel(0);
        s.ui.channel_search.select_kind(SearchKind::Album);
        s.apply(&TaskEvent::ArtistAlbumsFetched {
            id: artist.id.clone(),
            batch: 0,
            albums: vec![album_fixture("al1")],
            complete: true,
        });
        s.ui.channel_search.select_kind(SearchKind::Artist);
        assert!(
            matches!(
                current_frame(&s)?.data,
                Some(DetailData::Artist {
                    detail: None,
                    albums: Some(_)
                })
            ),
            "新帧收到旧请求的专辑列表"
        );
        assert!(
            current_frame(&s)?.needs_fetch(),
            "热门曲未到且新帧未请求，应允许预取"
        );
        mark_detail_requested(&mut s)?;
        assert!(
            !current_frame(&s)?.needs_fetch(),
            "等待新回包期间不重复预取"
        );
        s.apply(&detail_event);
        assert!(
            matches!(
                current_frame(&s)?.data,
                Some(DetailData::Artist {
                    detail: Some(_),
                    albums: Some(_)
                })
            ),
            "补拉后两路完整"
        );
        Ok(())
    }

    /// 歌单回包同时更新 Library 缓存和已切走的搜索详情。
    #[test]
    fn playlist_response_reaches_inactive_kind_and_library() -> color_eyre::Result<()> {
        use crate::runtime::state::DetailData;
        use mineral_model::{Playlist, PlaylistEntry, PlaylistId};
        use mineral_task::{SearchPayload, TaskEvent};

        let mut s = state_searching("q", SearchKind::Playlist)?;
        let mut playlist = Playlist::builder()
            .id(PlaylistId::new(SourceKind::NETEASE, "pl1"))
            .name("playlist".to_owned())
            .build();
        s.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Playlist,
            query: "q".to_owned(),
            batch: 0,
            payload: SearchPayload::Playlists(vec![playlist.clone()]),
            complete: false,
        });
        mark_detail_requested(&mut s)?;
        s.ui.channel_search.select_kind(SearchKind::Song);
        playlist.entries = PlaylistEntry::enumerate(crate::test_support::endserenading(2));
        s.apply(&TaskEvent::PlaylistDetailFetched {
            fresh: true,
            id: playlist.id.clone(),
            detail: Box::new(mineral_channel_core::PlaylistDetail::complete(
                playlist.clone(),
            )),
        });
        assert_eq!(
            s.models
                .library
                .tracks
                .get(&playlist.id)
                .map(|tracks| tracks.len()),
            Some(2)
        );
        s.ui.channel_search.select_kind(SearchKind::Playlist);
        match &current_frame(&s)?.data {
            Some(DetailData::PlaylistEntries(entries)) => assert_eq!(*entries, playlist.entries),
            _ => {
                color_eyre::eyre::bail!("切回歌单后应有曲目");
            }
        }
        Ok(())
    }

    /// 编辑中的新词不继承旧请求 loading，过期回包也不能恢复旧结果。
    #[test]
    fn editing_pending_query_stays_idle_after_stale_response() -> color_eyre::Result<()> {
        use mineral_task::{SearchPayload, TaskEvent};

        for insert in [false, true] {
            let mut s = state_in_search("hello")?;
            s.ui.channel_search.mark_loading(SearchKind::Song);
            s.ui.channel_search.mark_loading(SearchKind::Album);
            assert!(s.ui.channel_search.current_loading());
            let session =
                s.ui.channel_search
                    .current_mut()
                    .ok_or_else(|| color_eyre::eyre::eyre!("应有搜索会话"))?;
            if insert {
                session.push_query_char('x');
            } else {
                assert!(session.pop_query_char());
            }
            assert!(
                !s.ui.channel_search.current_loading(),
                "编辑未提交的词不显示 searching"
            );
            s.apply(&TaskEvent::SearchResults {
                source: SourceKind::NETEASE,
                kind: SearchKind::Song,
                query: "hello".to_owned(),
                batch: 0,
                payload: SearchPayload::Songs(crate::test_support::endserenading(2)),
                complete: false,
            });
            for kind in [SearchKind::Song, SearchKind::Album] {
                s.ui.channel_search.select_kind(kind);
                assert!(
                    !s.ui.channel_search.current_loading(),
                    "所有 kind 的旧 loading 都失效"
                );
                assert!(
                    s.ui.channel_search.active_results().is_none(),
                    "未提交的词没有结果桶"
                );
            }
            s.ui.channel_search.select_kind(SearchKind::Song);
            s.ui.channel_search.mark_loading(SearchKind::Song);
            assert!(
                s.ui.channel_search.current_loading(),
                "重新提交后恢复加载状态"
            );
        }
        Ok(())
    }

    /// 不匹配的 AlbumSongs(别的专辑 id)不污染当前帧。
    #[test]
    fn mismatched_album_songs_dropped() -> color_eyre::Result<()> {
        use mineral_model::AlbumId;
        use mineral_task::{SearchPayload, TaskEvent};

        let mut s = state_searching("q", SearchKind::Album)?;
        s.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Album,
            query: "q".to_owned(),
            batch: 0,
            payload: SearchPayload::Albums(vec![album_fixture("al1")]),
            complete: false,
        });
        s.apply(&TaskEvent::AlbumDetailFetched {
            fresh: true,
            id: AlbumId::new(SourceKind::NETEASE, "OTHER"),
            album: Box::new(
                mineral_model::Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, "OTHER"))
                    .name("other".to_owned())
                    .tracks(mineral_model::AlbumTrack::enumerate(
                        crate::test_support::endserenading(3),
                    ))
                    .build(),
            ),
        });
        let kr =
            s.ui.channel_search
                .active_results()
                .ok_or_else(|| color_eyre::eyre::eyre!("应有结果桶"))?;
        let frame = kr
            .detail
            .current()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有 detail root"))?;
        assert!(frame.data.is_none(), "别的专辑回包不落当前帧");
        Ok(())
    }
}
