//! 索引行与 model 的互转:`ScannedFile → ShelfFileRow`(入库)、`ShelfFileRow → Song`(出库)。
//!
//! domain 类型(u64/u32/u8/SystemTime)↔ sqlite 的 i64 换算全在这一层做,persist 只见 i64。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use mineral_model::{AlbumId, AlbumRef, ArtistId, ArtistRef, MediaUrl, Song, SongId, SourceKind};
use mineral_persist::ShelfFileRow;

use crate::scan::ScannedFile;

/// 把扫描到的文件事实 + 已定 uuid 组装成一条索引行(domain → i64 换算在此)。
///
/// # Params:
///   - `uuid`: 已分配 / 复用的稳定裸值
///   - `mount`: 所属 mount 根
///   - `path`: 文件绝对路径(UTF-8;非 UTF-8 由调用方剔除)
///   - `file`: 扫描文件事实(含探测结果)
///
/// # Return:
///   可 upsert 的 [`ShelfFileRow`]。
pub(crate) fn scanned_to_row(
    uuid: String,
    mount: &str,
    path: &str,
    file: &ScannedFile,
) -> ShelfFileRow {
    let probed = file.probed();
    let tags = probed.tags();
    // derive-getters 返回 &Option<T>,对 Copy 内层(u64/u32/u8/SystemTime)先 deref 再取 Option 方法。
    ShelfFileRow {
        uuid,
        mount: mount.to_owned(),
        path: path.to_owned(),
        size: (*file.size()).and_then(|s| i64::try_from(s).ok()),
        mtime_ms: (*file.mtime()).and_then(systemtime_to_epoch_ms),
        format: probed.format().as_ref().map(|f| f.as_str().to_owned()),
        bitrate_kbps: (*probed.bitrate_kbps()).map(i64::from),
        bit_depth: (*probed.bit_depth()).map(i64::from),
        duration_ms: (*probed.duration_ms()).and_then(|d| i64::try_from(d).ok()),
        title: tags.title().clone(),
        artist: tags.artist().clone(),
        album: tags.album().clone(),
        album_artist: tags.album_artist().clone(),
        track_no: (*tags.track_no()).map(i64::from),
        genre: tags.genre().clone(),
    }
}

/// 把一条索引行还原成 [`Song`](i64 → domain 换算 + 派生 Album/Artist ID)。
///
/// - `name` 缺 title 兜底文件名去扩展名(必须有);
/// - artist / album 从 tag 建 ref,ID 走确定性派生(同名跨 rescan 稳定、可分组);
/// - `source_url` = `Local(绝对路径)`,播放据此直接开文件。
///
/// # Params:
///   - `row`: 索引行
///
/// # Return:
///   还原的 [`Song`]。
pub(crate) fn row_to_song(row: &ShelfFileRow) -> Song {
    let name = row
        .title
        .clone()
        .unwrap_or_else(|| filename_stem(&row.path));

    let artists = row
        .artist
        .as_ref()
        .map(|a| {
            vec![ArtistRef {
                id: derive_artist_id(a),
                name: a.clone(),
            }]
        })
        .unwrap_or_default();

    let album = row.album.as_ref().map(|al| {
        // 合辑用 album_artist 分组,缺则回落主 artist,再缺只按专辑名。
        let group_artist = row.album_artist.as_deref().or(row.artist.as_deref());
        AlbumRef {
            id: derive_album_id(group_artist, al),
            name: al.clone(),
        }
    });

    Song::builder()
        .id(SongId::new(SourceKind::SHELF, row.uuid.as_str()))
        .name(name)
        .artists(artists)
        .album(album)
        .track_no(row.track_no.and_then(|n| u32::try_from(n).ok()))
        .duration_ms(row.duration_ms.and_then(|d| u64::try_from(d).ok()))
        .tags(row.genre.iter().cloned().collect::<Vec<String>>())
        .source_url(Some(MediaUrl::Local(PathBuf::from(&row.path))))
        .build()
}

/// 派生艺人 ID(规范化艺名作裸值,同名跨 rescan 稳定)。
///
/// # Params:
///   - `name`: 艺名原文
///
/// # Return:
///   确定性 [`ArtistId`]。
fn derive_artist_id(name: &str) -> ArtistId {
    ArtistId::new(SourceKind::SHELF, normalize(name))
}

/// 派生专辑 ID(规范化 `分组艺人\u{1f}专辑名` 作裸值,消歧同名专辑)。
///
/// # Params:
///   - `group_artist`: 分组艺人(album_artist 优先,回落主 artist);无则只按专辑名
///   - `album`: 专辑名原文
///
/// # Return:
///   确定性 [`AlbumId`]。
fn derive_album_id(group_artist: Option<&str>, album: &str) -> AlbumId {
    let key = match group_artist {
        Some(artist) => format!("{}\u{1f}{}", normalize(artist), normalize(album)),
        None => normalize(album),
    };
    AlbumId::new(SourceKind::SHELF, key)
}

/// 规范化分组键:去首尾空白 + 转小写(中文不受 to_lowercase 影响)。
///
/// # Params:
///   - `s`: 原文
///
/// # Return:
///   规范化后的键。
fn normalize(s: &str) -> String {
    s.trim().to_lowercase()
}

/// 文件名去扩展名(title 缺失时的曲名兜底)。
///
/// # Params:
///   - `path`: 文件路径
///
/// # Return:
///   去扩展名的文件名;取不到给 `"unknown"`。
fn filename_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| "unknown".to_owned())
}

/// 一个扫描文件的 `(size, mtime_ms)` 签名(rename 调和按它匹配「消失的行」)。
///
/// 与 [`scanned_to_row`] 用**同一套** i64 换算,才能和索引里存的 i64 对得上。size 或 mtime
/// 任一未知返回 `None`——签名不全无法可靠匹配 rename。
///
/// # Params:
///   - `file`: 扫描文件事实
///
/// # Return:
///   `(size_i64, mtime_ms_i64)`;任一缺失为 `None`。
pub(crate) fn scanned_sig(file: &ScannedFile) -> Option<(i64, i64)> {
    let size = (*file.size()).and_then(|s| i64::try_from(s).ok())?;
    let mtime = (*file.mtime()).and_then(systemtime_to_epoch_ms)?;
    Some((size, mtime))
}

/// [`SystemTime`] → epoch 毫秒(i64);早于 epoch / 溢出为 `None`。
///
/// # Params:
///   - `t`: 时间点
///
/// # Return:
///   epoch 毫秒;不可表示为 `None`。
fn systemtime_to_epoch_ms(t: SystemTime) -> Option<i64> {
    t.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
}

#[cfg(test)]
mod tests {
    use mineral_model::{MediaUrl, SourceKind};
    use mineral_persist::ShelfFileRow;

    use super::{derive_album_id, derive_artist_id, row_to_song};

    /// 造一条带 tag 的行(uuid/path 给定,其余填典型值)。
    fn row(uuid: &str, path: &str, title: Option<&str>) -> ShelfFileRow {
        ShelfFileRow {
            uuid: uuid.to_owned(),
            mount: "/music".to_owned(),
            path: path.to_owned(),
            size: Some(1024),
            mtime_ms: Some(1_700_000_000_000),
            format: Some("flac".to_owned()),
            bitrate_kbps: Some(900),
            bit_depth: Some(16),
            duration_ms: Some(240_000),
            title: title.map(str::to_owned),
            artist: Some("惘闻".to_owned()),
            album: Some("八匹马".to_owned()),
            album_artist: None,
            track_no: Some(3),
            genre: Some("post-rock".to_owned()),
        }
    }

    /// row_to_song:id 带 SHELF namespace、name 用 title、artist/album ref 派生、
    /// source_url 是 Local(绝对路径)、genre 进 tags、track_no/duration 换回 domain。
    #[test]
    fn row_to_song_maps_all_fields() -> color_eyre::Result<()> {
        let song = row_to_song(&row("u1", "/music/惘闻/八匹马/03.flac", Some("八匹马")));
        assert_eq!(song.source(), SourceKind::SHELF);
        assert_eq!(song.name, "八匹马");
        assert_eq!(song.track_no, Some(3));
        assert_eq!(song.duration_ms, Some(240_000));
        assert_eq!(song.tags, vec!["post-rock".to_owned()]);
        assert_eq!(
            song.artists.first().map(|a| a.name.as_str()),
            Some("惘闻")
        );
        assert_eq!(song.album.as_ref().map(|a| a.name.as_str()), Some("八匹马"));
        assert!(matches!(song.source_url, Some(MediaUrl::Local(_))));
        Ok(())
    }

    /// 缺 title:曲名兜底文件名去扩展名。
    #[test]
    fn row_to_song_title_falls_back_to_filename() -> color_eyre::Result<()> {
        let song = row_to_song(&row("u1", "/music/album/track01.flac", /*title*/ None));
        assert_eq!(song.name, "track01");
        Ok(())
    }

    /// 派生 ID 确定性:同名归一(大小写 / 空白无关)映射到同一 ID,可分组。
    #[test]
    fn derived_ids_are_deterministic_and_normalized() {
        assert_eq!(derive_artist_id("惘闻"), derive_artist_id(" 惘闻 "));
        assert_eq!(derive_artist_id("Radiohead"), derive_artist_id("RADIOHEAD"));
        // 同专辑名不同分组艺人 → 不同 AlbumId(消歧)。
        assert_ne!(
            derive_album_id(Some("A"), "Greatest Hits"),
            derive_album_id(Some("B"), "Greatest Hits")
        );
        // 同分组艺人 + 同专辑 → 同 AlbumId。
        assert_eq!(
            derive_album_id(Some("惘闻"), "八匹马"),
            derive_album_id(Some("惘闻"), "八匹马")
        );
    }
}
