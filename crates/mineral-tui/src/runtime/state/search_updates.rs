//! 将搜索分页与实体详情回包应用到匹配的会话和详情帧。

use mineral_channel_core::Page;
use mineral_model::{Album, AlbumId, Artist, ArtistId, SearchKind, SourceKind};
use mineral_task::SearchPayload;

use super::AppState;

impl AppState {
    /// 把一页搜索结果落进配对的会话：按事件自带 `source` 找会话、`query` 配对（源级，
    /// 改 query 作废全部 kind 桶），事件自带 `kind` 决定存哪个桶——这样切 kind 时旧 kind
    /// 的飞行响应也能落对桶，per-kind 缓存才完整。首页（`offset == 0`）建桶、翻页 append。
    ///
    /// # Params:
    ///   - `source` / `kind` / `query`: 回带的请求三元组（source 找会话、query 配对、kind 选桶）
    ///   - `page`: 分页参数（`offset == 0` 首页建桶，否则 append）
    ///   - `payload`: 结果载荷
    ///   - `has_more`: 源的显式翻页信号
    pub(super) fn apply_search_results(
        &mut self,
        source: SourceKind,
        kind: SearchKind,
        query: &str,
        page: Page,
        payload: &SearchPayload,
        has_more: Option<bool>,
    ) {
        // caps 先读:随后 session 借走 self.channel_search。artist 源的可用分区落定桶级判定,让 artist
        // root 帧把分区收到首个可用区(无热门曲的源如 B站即只有 Albums;含后续 set_sel 复位)。
        let sections = self
            .caps
            .get(&source)
            .map(|channel_caps| channel_caps.artist_sections().clone());
        let Some(session) = self.channel_search.session_for_mut(source) else {
            return;
        };
        if session.query() != query {
            return;
        }
        if !session.apply_page(kind, payload.clone(), page, has_more) {
            return;
        }
        if let Some(sections) = sections {
            session.apply_sections(kind, sections);
        }
    }

    /// 按请求的 source、kind、query 与分页参数释放失败续页；切 source / kind 后仍更新原桶。
    pub(super) fn apply_search_page_failed(
        &mut self,
        source: SourceKind,
        kind: SearchKind,
        query: &str,
        page: Page,
    ) {
        if let Some(session) = self.channel_search.session_for_mut(source)
            && session.query() == query
        {
            session.fail_page(kind, page);
        }
    }

    /// 将艺人详情应用到所有会话中匹配的保留帧。
    pub(super) fn apply_artist_detail(&mut self, id: &ArtistId, artist: &Artist) {
        for results in self.channel_search.retained_results_mut() {
            results.fill_artist_detail(id, artist);
        }
    }

    /// 将艺人专辑列表应用到所有会话中匹配的保留帧。
    pub(super) fn apply_artist_albums(&mut self, id: &ArtistId, albums: &[Album]) {
        for results in self.channel_search.retained_results_mut() {
            results.fill_artist_albums(id, albums);
        }
    }

    /// 将完整专辑应用到所有会话中匹配的保留帧和结果行。
    pub(super) fn apply_album_detail(&mut self, id: &AlbumId, album: &Album) {
        for results in self.channel_search.retained_results_mut() {
            results.fill_album_detail(id, album);
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
        s.caps = caps;
        s.channel_search.enter(&s.caps);
        if let Some(session) = s.channel_search.current_mut() {
            session.set_query(query.to_owned());
        }
        Ok(s)
    }

    /// 读当前会话的结果条数(无结果 / 非 Songs 载荷计 0)。
    fn session_song_count(s: &AppState) -> usize {
        use mineral_task::SearchPayload;
        match s.channel_search.active_results().map(|kr| &kr.results) {
            Some(SearchPayload::Songs(songs)) => songs.len(),
            _ => 0,
        }
    }

    /// query 配对的 SearchResults 落进当前会话。
    #[test]
    fn search_results_populate_matching_session() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::{SearchKind, SourceKind};
        use mineral_task::{SearchPayload, TaskEvent};

        use crate::test_support::endserenading;

        let mut s = state_in_search("hello")?;
        s.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Song,
            query: "hello".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Songs(endserenading(2)),
            has_more: None,
        });
        assert_eq!(session_song_count(&s), 2, "配对结果入会");
        Ok(())
    }

    /// query 已变的过期 SearchResults 直接丢弃,不污染当前会话。
    #[test]
    fn stale_search_results_dropped() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::{SearchKind, SourceKind};
        use mineral_task::{SearchPayload, TaskEvent};

        use crate::test_support::endserenading;

        let mut s = state_in_search("hello")?;
        s.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Song,
            query: "stale".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Songs(endserenading(5)),
            has_more: None,
        });
        assert_eq!(session_song_count(&s), 0, "过期响应不入会");
        Ok(())
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
        s.caps = caps;
        s.channel_search.enter(&s.caps);
        if let Some(session) = s.channel_search.current_mut() {
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
        s.channel_search
            .active_results()
            .and_then(|results| results.detail.current())
            .ok_or_else(|| color_eyre::eyre::eyre!("应有详情帧"))
    }

    /// 标记当前详情帧已发起预取，模拟等待回包时离开该帧。
    fn mark_detail_requested(s: &mut AppState) -> color_eyre::Result<()> {
        let frame = s
            .channel_search
            .active_results_mut()
            .and_then(|results| results.detail.current_mut())
            .ok_or_else(|| color_eyre::eyre::eyre!("应有详情帧"))?;
        frame.mark_requested();
        Ok(())
    }

    /// 成功回包更新切走的来源或类型；其他来源的同名原始 ID 不接收。
    #[test]
    fn album_response_reaches_retained_source_and_kind() -> color_eyre::Result<()> {
        use crate::runtime::state::DetailData;
        use mineral_channel_core::Page;
        use mineral_task::{SearchPayload, TaskEvent};

        for switch_source in [false, true] {
            let mut s = state_searching("q", SearchKind::Album)?;
            let album = album_fixture("al1");
            s.apply(&TaskEvent::SearchResults {
                source: SourceKind::NETEASE,
                kind: SearchKind::Album,
                query: "q".to_owned(),
                page: Page::default(),
                payload: SearchPayload::Albums(vec![album.clone()]),
                has_more: None,
            });
            mark_detail_requested(&mut s)?;
            if switch_source {
                let caps = s
                    .caps
                    .get(&SourceKind::NETEASE)
                    .cloned()
                    .ok_or_else(|| color_eyre::eyre::eyre!("应有来源能力"))?;
                s.caps.insert(SourceKind::BILIBILI, caps);
                s.channel_search
                    .switch_source(SourceKind::BILIBILI, &s.caps);
                let session = s
                    .channel_search
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
                    Page::default(),
                    None,
                );
            } else {
                s.channel_search.select_kind(SearchKind::Artist);
            }
            let mut detailed = album;
            detailed.tracks =
                mineral_model::AlbumTrack::enumerate(crate::test_support::endserenading(3));
            s.apply(&TaskEvent::AlbumDetailFetched {
                id: detailed.id.clone(),
                album: Box::new(detailed.clone()),
            });
            if switch_source {
                assert!(
                    current_frame(&s)?.data.is_none(),
                    "其他来源的同名 ID 不受影响"
                );
                s.channel_search.switch_source(SourceKind::NETEASE, &s.caps);
            } else {
                assert!(
                    s.channel_search.active_results().is_none(),
                    "回包不创建当前 kind 的结果"
                );
                s.channel_search.select_kind(SearchKind::Album);
            }
            match &current_frame(&s)?.data {
                Some(DetailData::Album(received)) => assert_eq!(**received, detailed),
                _ => color_eyre::eyre::bail!("切回后应有完整专辑详情"),
            }
        }
        Ok(())
    }

    /// 艺人两路数据先后到达，切类型或下钻后收到的那一路仍合并到原帧。
    #[test]
    fn artist_responses_merge_while_frame_is_inactive() -> color_eyre::Result<()> {
        use crate::runtime::state::{DetailData, EntityRef};
        use mineral_channel_core::Page;
        use mineral_task::{SearchPayload, TaskEvent};

        for albums_first in [false, true] {
            let mut s = state_searching("q", SearchKind::Artist)?;
            let mut artist = artist_fixture("ar1");
            let album = album_fixture("al1");
            s.apply(&TaskEvent::SearchResults {
                source: SourceKind::NETEASE,
                kind: SearchKind::Artist,
                query: "q".to_owned(),
                page: Page::default(),
                payload: SearchPayload::Artists(vec![artist.clone()]),
                has_more: None,
            });
            mark_detail_requested(&mut s)?;
            artist.songs = crate::test_support::endserenading(2);
            let detail_event = TaskEvent::ArtistDetailFetched {
                id: artist.id.clone(),
                artist: Box::new(artist.clone()),
            };
            let albums_event = TaskEvent::ArtistAlbumsFetched {
                id: artist.id.clone(),
                page: Page::default(),
                albums: vec![album.clone()],
            };
            let (first, second) = if albums_first {
                (&albums_event, &detail_event)
            } else {
                (&detail_event, &albums_event)
            };
            s.apply(first);
            if albums_first {
                s.channel_search
                    .active_results_mut()
                    .ok_or_else(|| color_eyre::eyre::eyre!("应有结果"))?
                    .detail
                    .push(EntityRef::Album(Box::new(album.clone())), 1);
            } else {
                s.channel_search.select_kind(SearchKind::Album);
                s.channel_search
                    .current_mut()
                    .ok_or_else(|| color_eyre::eyre::eyre!("应有搜索会话"))?
                    .apply_page(
                        SearchKind::Album,
                        SearchPayload::Albums(vec![album.clone()]),
                        Page::default(),
                        None,
                    );
            }
            s.apply(second);
            assert!(
                current_frame(&s)?.data.is_none(),
                "艺人回包不填入当前专辑帧"
            );
            if albums_first {
                assert!(
                    s.channel_search
                        .active_results_mut()
                        .ok_or_else(|| color_eyre::eyre::eyre!("应有结果"))?
                        .detail
                        .pop(1)
                );
            } else {
                s.channel_search.select_kind(SearchKind::Artist);
            }
            match &current_frame(&s)?.data {
                Some(DetailData::Artist {
                    detail: Some(received),
                    albums: Some(albums),
                }) => {
                    assert_eq!(**received, artist);
                    assert_eq!(*albums, vec![album]);
                }
                _ => color_eyre::eyre::bail!("返回父帧后应有两路艺人数据"),
            }
        }
        Ok(())
    }

    /// 同专辑在另一类型中再次加载，只更新数据，不重置歌曲详情内已移动的光标和视口。
    #[test]
    fn repeated_album_response_preserves_retained_song_navigation() -> color_eyre::Result<()> {
        use crate::runtime::state::DetailData;
        use mineral_channel_core::Page;
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
        s.channel_search
            .current_mut()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有搜索会话"))?
            .apply_page(
                SearchKind::Song,
                SearchPayload::Songs(vec![song]),
                Page::default(),
                None,
            );
        mark_detail_requested(&mut s)?;
        album.tracks = AlbumTrack::enumerate(songs);
        s.apply(&TaskEvent::AlbumDetailFetched {
            id: album.id.clone(),
            album: Box::new(album.clone()),
        });
        assert_eq!(
            current_frame(&s)?.list().sel(),
            0,
            "首次到货定位到结果选中的歌曲"
        );
        s.channel_search
            .active_results_mut()
            .and_then(|results| results.detail.current_mut())
            .ok_or_else(|| color_eyre::eyre::eyre!("应有详情帧"))?
            .list_mut()
            .place(2, 1);
        s.channel_search.select_kind(SearchKind::Album);
        s.channel_search
            .current_mut()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有搜索会话"))?
            .apply_page(
                SearchKind::Album,
                SearchPayload::Albums(vec![album_fixture("al1")]),
                Page::default(),
                None,
            );
        mark_detail_requested(&mut s)?;
        album.description = "updated description".to_owned();
        s.apply(&TaskEvent::AlbumDetailFetched {
            id: album.id.clone(),
            album: Box::new(album.clone()),
        });
        s.channel_search.select_kind(SearchKind::Song);
        let frame = current_frame(&s)?;
        assert_eq!(frame.list().sel(), 2, "保留用户选中的另一首歌曲");
        assert_eq!(frame.list().scroll_target(), 1, "保留原视口");
        match &frame.data {
            Some(DetailData::Album(received)) => assert_eq!(**received, album),
            _ => color_eyre::eyre::bail!("应有更新后的专辑详情"),
        }
        Ok(())
    }

    /// 艺人帧重建后收到旧请求的单路数据，返回时仍能补拉缺失的那一路。
    #[test]
    fn rebuilt_artist_frame_with_partial_response_still_prefetches() -> color_eyre::Result<()> {
        use crate::runtime::state::DetailData;
        use mineral_channel_core::Page;
        use mineral_task::{SearchPayload, TaskEvent};

        let mut s = state_searching("q", SearchKind::Artist)?;
        let artist = artist_fixture("ar1");
        s.channel_search
            .current_mut()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有搜索会话"))?
            .apply_page(
                SearchKind::Artist,
                SearchPayload::Artists(vec![artist.clone(), artist_fixture("ar2")]),
                Page::default(),
                None,
            );
        mark_detail_requested(&mut s)?;
        let detail_event = TaskEvent::ArtistDetailFetched {
            id: artist.id.clone(),
            artist: Box::new(artist.clone()),
        };
        s.apply(&detail_event);
        let results = s
            .channel_search
            .active_results_mut()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有结果桶"))?;
        results.set_sel(1);
        results.set_sel(0);
        s.channel_search.select_kind(SearchKind::Album);
        s.apply(&TaskEvent::ArtistAlbumsFetched {
            id: artist.id.clone(),
            page: Page::default(),
            albums: vec![album_fixture("al1")],
        });
        s.channel_search.select_kind(SearchKind::Artist);
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
        use mineral_channel_core::Page;
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
            page: Page::default(),
            payload: SearchPayload::Playlists(vec![playlist.clone()]),
            has_more: None,
        });
        mark_detail_requested(&mut s)?;
        s.channel_search.select_kind(SearchKind::Song);
        playlist.entries = PlaylistEntry::enumerate(crate::test_support::endserenading(2));
        s.apply(&TaskEvent::PlaylistDetailFetched {
            id: playlist.id.clone(),
            playlist: Box::new(playlist.clone()),
        });
        assert_eq!(s.library.tracks.get(&playlist.id).map(Vec::len), Some(2));
        s.channel_search.select_kind(SearchKind::Playlist);
        match &current_frame(&s)?.data {
            Some(DetailData::PlaylistEntries(entries)) => assert_eq!(*entries, playlist.entries),
            _ => color_eyre::eyre::bail!("切回歌单后应有曲目"),
        }
        Ok(())
    }

    /// 编辑中的新词不继承旧请求 loading，过期回包也不能恢复旧结果。
    #[test]
    fn editing_pending_query_stays_idle_after_stale_response() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_task::{SearchPayload, TaskEvent};

        for insert in [false, true] {
            let mut s = state_in_search("hello")?;
            s.channel_search.mark_loading(SearchKind::Song);
            s.channel_search.mark_loading(SearchKind::Album);
            assert!(s.channel_search.current_loading());
            let session = s
                .channel_search
                .current_mut()
                .ok_or_else(|| color_eyre::eyre::eyre!("应有搜索会话"))?;
            if insert {
                session.push_query_char('x');
            } else {
                assert!(session.pop_query_char());
            }
            assert!(
                !s.channel_search.current_loading(),
                "编辑未提交的词不显示 searching"
            );
            s.apply(&TaskEvent::SearchResults {
                source: SourceKind::NETEASE,
                kind: SearchKind::Song,
                query: "hello".to_owned(),
                page: Page::default(),
                payload: SearchPayload::Songs(crate::test_support::endserenading(2)),
                has_more: None,
            });
            for kind in [SearchKind::Song, SearchKind::Album] {
                s.channel_search.select_kind(kind);
                assert!(
                    !s.channel_search.current_loading(),
                    "所有 kind 的旧 loading 都失效"
                );
                assert!(
                    s.channel_search.active_results().is_none(),
                    "未提交的词没有结果桶"
                );
            }
            s.channel_search.select_kind(SearchKind::Song);
            s.channel_search.mark_loading(SearchKind::Song);
            assert!(s.channel_search.current_loading(), "重新提交后恢复加载状态");
        }
        Ok(())
    }

    /// AlbumSongs 回包落到「选中专辑」的 detail 栈顶帧(配对成功)。
    #[test]
    fn album_songs_fill_selected_detail_frame() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::AlbumId;
        use mineral_task::{SearchPayload, TaskEvent};

        use crate::runtime::state::DetailData;

        let mut s = state_searching("q", SearchKind::Album)?;
        s.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Album,
            query: "q".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Albums(vec![album_fixture("al1")]),
            has_more: None,
        });
        // detail root = al1，fetch = AlbumDetail(al1)。喂该专辑完整详情(含曲目)。
        s.apply(&TaskEvent::AlbumDetailFetched {
            id: AlbumId::new(SourceKind::NETEASE, "al1"),
            album: Box::new(
                mineral_model::Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, "al1"))
                    .name("al1".to_owned())
                    .tracks(mineral_model::AlbumTrack::enumerate(
                        crate::test_support::endserenading(3),
                    ))
                    .build(),
            ),
        });
        let kr = s
            .channel_search
            .active_results()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有结果桶"))?;
        let frame = kr
            .detail
            .current()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有 detail root"))?;
        match &frame.data {
            Some(DetailData::Album(a)) => assert_eq!(a.tracks.len(), 3, "专辑详情落帧"),
            _ => color_eyre::eyre::bail!("detail 帧应填 Album"),
        }
        Ok(())
    }

    /// 不匹配的 AlbumSongs(别的专辑 id)不污染当前帧。
    #[test]
    fn mismatched_album_songs_dropped() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::AlbumId;
        use mineral_task::{SearchPayload, TaskEvent};

        let mut s = state_searching("q", SearchKind::Album)?;
        s.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Album,
            query: "q".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Albums(vec![album_fixture("al1")]),
            has_more: None,
        });
        s.apply(&TaskEvent::AlbumDetailFetched {
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
        let kr = s
            .channel_search
            .active_results()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有结果桶"))?;
        let frame = kr
            .detail
            .current()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有 detail root"))?;
        assert!(frame.data.is_none(), "别的专辑回包不落当前帧");
        Ok(())
    }

    /// artist 详情两路(热门曲 + 专辑列表)分别到货、合并进同一帧。
    #[test]
    fn artist_detail_and_albums_merge() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::ArtistId;
        use mineral_task::{SearchPayload, TaskEvent};

        use crate::runtime::state::DetailData;

        let mut s = state_searching("q", SearchKind::Artist)?;
        s.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Artist,
            query: "q".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Artists(vec![artist_fixture("ar1")]),
            has_more: None,
        });
        let id = ArtistId::new(SourceKind::NETEASE, "ar1");
        s.apply(&TaskEvent::ArtistDetailFetched {
            id: id.clone(),
            artist: Box::new(artist_fixture("ar1")),
        });
        s.apply(&TaskEvent::ArtistAlbumsFetched {
            id,
            page: Page::default(),
            albums: vec![album_fixture("al1"), album_fixture("al2")],
        });
        let kr = s
            .channel_search
            .active_results()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有结果桶"))?;
        let frame = kr
            .detail
            .current()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有 detail root"))?;
        match &frame.data {
            Some(DetailData::Artist { detail, albums }) => {
                assert!(detail.is_some(), "热门曲那一路到货");
                assert_eq!(albums.as_ref().map(Vec::len), Some(2), "专辑那一路到货");
            }
            _ => color_eyre::eyre::bail!("detail 帧应是 Artist"),
        }
        Ok(())
    }
}
