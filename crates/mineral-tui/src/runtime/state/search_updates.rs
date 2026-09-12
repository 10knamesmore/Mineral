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
        session.apply_page(kind, payload.clone(), page, has_more);
        if let Some(sections) = sections {
            session.apply_sections(kind, sections);
        }
    }

    /// ArtistDetail 回包：落到当前 detail 栈顶帧（若正等这个 artist；否则丢弃）。
    pub(super) fn apply_artist_detail(&mut self, id: &ArtistId, artist: &Artist) {
        if let Some(kr) = self.channel_search.active_results_mut() {
            kr.fill_artist_detail(id, Box::new(artist.clone()));
        }
    }

    /// ArtistAlbums 回包：落到当前 detail 栈顶帧（若正等这个 artist）。
    pub(super) fn apply_artist_albums(&mut self, id: &ArtistId, albums: &[Album]) {
        if let Some(kr) = self.channel_search.active_results_mut() {
            kr.fill_artist_albums(id, albums.to_vec());
        }
    }

    /// AlbumDetail 回包：完整专辑落到当前 detail 栈顶帧（若正等这张专辑）。
    pub(super) fn apply_album_detail(&mut self, id: &AlbumId, album: &Album) {
        if let Some(kr) = self.channel_search.active_results_mut() {
            kr.fill_album_detail(id, Box::new(album.clone()));
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
                .searchable(vec![SearchKind::Song])
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
                .searchable(vec![kind])
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
