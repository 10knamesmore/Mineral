//! 测试专用构造 helper(快照 / 渲染测试共用)。仅 `#[cfg(test)]` 编译。
//!
//! 展示性歌曲数据用 Mineral 乐队专辑《EndSerenading》(1998)的真实曲目 ——
//! 本播放器项目名正取自这支 emo 乐队。

use mineral_model::{ArtistId, ArtistRef, Playlist, PlaylistId, Song, SongId, SourceKind};

use crate::state::{AppState, View};
use crate::view_model::{PlaylistView, SongView};

/// 带中文描述的快照断言:`description` 写进 `.snap` 头,`cargo insta review` 时显示,
/// 便于逐张辨认这张快照测的是什么。用法:`assert_snap!("描述", terminal.backend());`。
macro_rules! assert_snap {
    ($desc:expr, $value:expr $(,)?) => {{
        insta::with_settings!({ description => $desc }, {
            insta::assert_snapshot!($value);
        });
    }};
}

pub(crate) use assert_snap;

/// 造一首最小 `Song`(无艺人/专辑/封面)。
pub(crate) fn song(id: &str, name: &str, duration_ms: u64) -> Song {
    Song {
        source: SourceKind::Netease,
        id: SongId::from(id),
        name: name.to_owned(),
        artists: Vec::new(),
        album: None,
        duration_ms,
        cover_url: None,
        source_url: None,
    }
}

/// 给一首 `Song` 挂上指定艺人(展示性数据更真实)。
fn with_artist(mut s: Song, artist: &str) -> Song {
    s.artists = vec![ArtistRef {
        id: ArtistId::from(artist),
        name: artist.to_owned(),
    }];
    s
}

/// Mineral 乐队专辑《EndSerenading》(1998)前 `n` 首(带艺人 + 真实时长),
/// 用作展示性测试数据(项目名取自这支 emo 乐队)。
pub(crate) fn endserenading(n: usize) -> Vec<Song> {
    const TRACKS: [(&str, &str, u64); 10] = [
        ("1", "LoveLetterTypewriter", 225_000),
        ("2", "Palisade", 271_000),
        ("3", "Gjs", 286_000),
        ("4", "Unfinished", 367_000),
        ("5", "ForIvadell", 216_000),
        ("6", "WakingToWinter", 242_000),
        ("7", "ALetter", 293_000),
        ("8", "SoundsLikeSunday", 320_000),
        ("9", "&serenading", 324_000),
        ("10", "TheLastWordIsRejoice", 309_000),
    ];
    TRACKS
        .iter()
        .take(n)
        .map(|&(id, name, dur)| with_artist(song(id, name, dur), "Mineral"))
        .collect()
}

/// Chinese Football 乐队同名专辑(2015)前 `n` 首,用作 **CJK 宽字符**测试数据
/// —— 含「不是人人都能穿十号球衣」「地球上最后一个EMO男孩」这类长 CJK / 中英混排。
pub(crate) fn chinese_football(n: usize) -> Vec<Song> {
    const TRACKS: [(&str, &str); 10] = [
        ("c1", "守门员"),
        ("c2", "飞鱼转身"),
        ("c3", "400米"),
        ("c4", "不是人人都能穿十号球衣"),
        ("c5", "世界悲"),
        ("c6", "地球上最后一个EMO男孩"),
        ("c7", "红牌罚下"),
        ("c8", "帽子戏法"),
        ("c9", "再见米卢"),
        ("c10", "盲人摸象"),
    ];
    TRACKS
        .iter()
        .take(n)
        .map(|&(id, name)| with_artist(song(id, name, 233_000), "Chinese Football"))
        .collect()
}

/// 造一个 `PlaylistView`(空曲目,只元信息)。
pub(crate) fn playlist_view(
    id: &str,
    name: &str,
    source: SourceKind,
    track_count: u64,
) -> PlaylistView {
    PlaylistView {
        data: Playlist {
            source,
            id: PlaylistId::from(id),
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
        playlist_view("p1", "EndSerenading", SourceKind::Netease, 10),
        playlist_view("p2", "The Power of Failing", SourceKind::Netease, 8),
        playlist_view("p3", "本地音乐", SourceKind::Local, 5),
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
    s.tracks_cache.insert(PlaylistId::from("p1"), views);
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
        SourceKind::Netease,
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
    s.tracks_cache.insert(PlaylistId::from("cf"), views);
    s
}
