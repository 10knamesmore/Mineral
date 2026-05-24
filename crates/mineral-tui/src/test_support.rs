//! 测试专用构造 helper(快照 / 渲染测试共用)。仅 `#[cfg(test)]` 编译。
//!
//! 跨 crate 复用的零件(`song` / `with_*` / `endserenading` / `chinese_football` /
//! `assert_snap!`)来自 [`mineral_test`];本模块只保留依赖 TUI 私有类型
//! (`AppState` / `SongView` / `PlaylistView`)的 fixture。

use mineral_model::{Playlist, PlaylistId, SourceKind};

use crate::state::{AppState, View};
use crate::view_model::{PlaylistView, SongView};

// 共享零件经 mineral-test 收口;re-export 让调用点继续写 `crate::test_support::xxx`。
pub(crate) use mineral_test::{
    assert_snap, chinese_football, endserenading, song, with_duration, with_name,
};

/// 造一个 `PlaylistView`(空曲目,只元信息)。
pub(crate) fn playlist_view(
    id: &str,
    name: &str,
    source: SourceKind,
    track_count: u64,
) -> PlaylistView {
    PlaylistView {
        data: Playlist {
            id: PlaylistId::new(source, id),
            name: name.to_owned(),
            description: String::new(),
            cover_url: None,
            track_count,
            songs: Vec::new(),
        },
    }
}

/// 造一个填了歌单的 `AppState`(view = Playlists,选中第 0 个):Mineral 两张专辑
/// + 一个本地歌单。
pub(crate) fn state_with_playlists() -> AppState {
    let mut s = AppState::empty();
    s.playlists = vec![
        playlist_view("p1", "EndSerenading", SourceKind::NETEASE, 10),
        playlist_view("p2", "The Power of Failing", SourceKind::NETEASE, 8),
        playlist_view("p3", "本地音乐", SourceKind::LOCAL, 5),
    ];
    s
}

/// 在 [`state_with_playlists`] 基础上进入《EndSerenading》、填前 3 首(含收藏 /
/// 当前在播标记),view = Library,选中第 1 首。
pub(crate) fn state_with_tracks() -> AppState {
    let mut s = state_with_playlists();
    s.view = View::Library;
    let tracks = endserenading(3);
    let plays = [1200_u32, 999, 88];
    let views = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| SongView {
            data: t.clone(),
            loved: i == 1,
            plays: plays.get(i).copied().unwrap_or(0),
        })
        .collect::<Vec<SongView>>();
    s.current = tracks.first().cloned();
    s.tracks_cache
        .insert(PlaylistId::new(SourceKind::NETEASE, "p1"), views);
    s.sel_track = 1;
    s
}

/// 进入「Chinese Football」歌单、填前 4 首(含最长的「不是人人都能穿十号球衣」),
/// 专用于 CJK 宽字符在多列表格里的对齐 / 截断快照。
pub(crate) fn state_with_cjk_tracks() -> AppState {
    let mut s = AppState::empty();
    s.playlists = vec![playlist_view(
        "cf",
        "Chinese Football",
        SourceKind::NETEASE,
        10,
    )];
    s.view = View::Library;
    let tracks = chinese_football(4);
    let views = tracks
        .iter()
        .map(|t| SongView {
            data: t.clone(),
            loved: false,
            plays: 0,
        })
        .collect::<Vec<SongView>>();
    s.current = tracks.first().cloned();
    s.tracks_cache
        .insert(PlaylistId::new(SourceKind::NETEASE, "cf"), views);
    s
}
