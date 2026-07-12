//! 默认目录分组:含音频文件的目录 = 一张歌单(spec §4 开箱约定)。
//!
//! 分组是索引行的纯函数(不落表可重算);PlaylistId 用目录绝对路径作裸值(确定性、跨 rescan 稳定)。
//! organize 的 Lua 覆盖尚未接入,这里是它的回落基线。

use std::path::Path;

use mineral_model::{Playlist, PlaylistId, Song, SourceKind};
use mineral_persist::ShelfFileRow;
use rustc_hash::FxHashMap;

use super::row::row_to_song;

/// 目录路径 → 确定性 [`PlaylistId`]。
///
/// # Params:
///   - `dir`: 目录绝对路径
///
/// # Return:
///   该目录的歌单 ID。
pub(crate) fn dir_playlist_id(dir: &str) -> PlaylistId {
    PlaylistId::new(SourceKind::SHELF, dir)
}

/// 按父目录分组,产出「目录 = 歌单」列表(元信息版,songs 空,按目录路径升序稳定)。
///
/// # Params:
///   - `rows`: 全部索引行
///
/// # Return:
///   歌单列表(每个含音频的目录一张)。
pub(crate) fn playlists_from_rows(rows: &[ShelfFileRow]) -> Vec<Playlist> {
    let mut counts: FxHashMap<&str, u64> = FxHashMap::default();
    for row in rows {
        if let Some(dir) = parent_dir(&row.path) {
            *counts.entry(dir).or_insert(0) += 1;
        }
    }
    let mut dirs: Vec<(&str, u64)> = counts.into_iter().collect();
    dirs.sort_by(|a, b| a.0.cmp(b.0));
    dirs.into_iter()
        .map(|(dir, count)| {
            Playlist::builder()
                .id(dir_playlist_id(dir))
                .name(dir_name(dir))
                .track_count(count)
                .build()
        })
        .collect()
}

/// 某目录(歌单)的全曲目版:筛出该目录直接含的文件,还原成 Song。
///
/// # Params:
///   - `rows`: 全部索引行
///   - `dir`: 目标目录绝对路径(歌单 ID 的裸值)
///
/// # Return:
///   带 songs 的歌单(目录无文件则 songs 空)。
pub(crate) fn playlist_detail_from_rows(rows: &[ShelfFileRow], dir: &str) -> Playlist {
    let songs: Vec<Song> = rows
        .iter()
        .filter(|row| parent_dir(&row.path) == Some(dir))
        .map(row_to_song)
        .collect();
    let track_count = u64::try_from(songs.len()).unwrap_or(0);
    Playlist::builder()
        .id(dir_playlist_id(dir))
        .name(dir_name(dir))
        .track_count(track_count)
        .songs(songs)
        .build()
}

/// 取文件路径的父目录(UTF-8;取不到为 `None`)。
///
/// # Params:
///   - `path`: 文件路径
///
/// # Return:
///   父目录路径。
fn parent_dir(path: &str) -> Option<&str> {
    Path::new(path).parent().and_then(Path::to_str)
}

/// 目录名(basename;取不到回落整段路径)。
///
/// # Params:
///   - `dir`: 目录路径
///
/// # Return:
///   目录显示名。
fn dir_name(dir: &str) -> String {
    Path::new(dir)
        .file_name()
        .and_then(|s| s.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| dir.to_owned())
}

#[cfg(test)]
mod tests {
    use mineral_persist::ShelfFileRow;

    use super::{dir_playlist_id, playlist_detail_from_rows, playlists_from_rows};

    /// 造一条只关心 path 的行。
    fn row(uuid: &str, path: &str) -> ShelfFileRow {
        ShelfFileRow {
            uuid: uuid.to_owned(),
            mount: "/music".to_owned(),
            path: path.to_owned(),
            size: Some(1),
            mtime_ms: Some(1),
            format: Some("flac".to_owned()),
            bitrate_kbps: None,
            bit_depth: None,
            duration_ms: None,
            title: Some(uuid.to_owned()),
            artist: None,
            album: None,
            album_artist: None,
            track_no: None,
            genre: None,
        }
    }

    /// 分组:两个目录各成一张歌单,名 = 目录名,track_count = 目录内文件数,按路径升序。
    #[test]
    fn groups_by_directory() {
        let rows = vec![
            row("a", "/music/albumB/2.flac"),
            row("b", "/music/albumA/1.flac"),
            row("c", "/music/albumA/2.flac"),
        ];
        let lists = playlists_from_rows(&rows);
        assert_eq!(lists.len(), 2);
        // 升序:albumA 在前。
        assert_eq!(lists.first().map(|p| p.name.as_str()), Some("albumA"));
        assert_eq!(lists.first().map(|p| p.track_count), Some(2));
        assert_eq!(lists.get(1).map(|p| p.track_count), Some(1));
    }

    /// playlist_detail:按目录筛出曲目;歌单列表版无 songs、详情版有。
    #[test]
    fn detail_filters_by_directory() {
        let rows = vec![
            row("a", "/music/albumA/1.flac"),
            row("b", "/music/albumA/2.flac"),
            row("c", "/music/albumB/1.flac"),
        ];
        let detail = playlist_detail_from_rows(&rows, "/music/albumA");
        assert_eq!(detail.track_count, 2);
        assert_eq!(detail.songs.len(), 2, "详情版带全曲目");
        assert_eq!(detail.id, dir_playlist_id("/music/albumA"));

        // 列表版不带曲目载荷。
        let lists = playlists_from_rows(&rows);
        assert!(lists.iter().all(|p| p.songs.is_empty()), "列表版 songs 空");
    }
}
