//! 本地优先的播放解析:把待播 [`Song`] 解析到本地已有的音频文件(缓存或下载导出),命中则
//! 跳过整条网络取链路径。
//!
//! 解析按音质从高到低枚举(只看 `>= want`):同一音质先探音频缓存([`MediaCache`],热、id 索引),
//! 再探下载导出库,第一个命中即「本地最高可用音质」(故 lossless 也能喂给较低请求播放,同音质时
//! 优先缓存)。
//!
//! 下载导出**不过索引**:导出目录(`~/Music/mineral`)就是权威——按
//! `<source>/<quality>/<album>/<title>.<ext>` 重算专辑目录并 stat,文件在即可播。历史下载、换机
//! 拷库、手动放进去的文件一律可见,不受任何索引漂移影响。代价是命名即身份:同源同专辑同名的两首歌
//! 落到同一路径(下载侧按文件存在幂等,不产生 ` (N)` 副本),概率极低。

use std::path::{Path, PathBuf};

use mineral_model::{BitRate, Song};

use crate::media_cache::{MediaCache, library_dir_and_stem};

/// 把 `song` 解析到本地音频文件(音质 `>= want` 的最高可用副本)。
///
/// # Params:
///   - `media_cache`: 音频本体缓存(LRU,id 索引)
///   - `download_root`: 下载导出根目录(如 `~/Music/mineral`);`None` = 下载不可用
///   - `song`: 待播歌曲
///   - `want`: 期望的最低音质
///
/// # Return:
///   命中返回 `(本地文件绝对路径, 实际命中音质)`;本地无 `>= want` 副本返回 `None`(走远端)。
pub(crate) fn resolve_local(
    media_cache: &MediaCache,
    download_root: Option<&Path>,
    song: &Song,
    want: BitRate,
) -> Option<(PathBuf, BitRate)> {
    // ALL 升序 → rev() 高到低;低于 want 即可停(后续只会更低)。
    for &q in BitRate::ALL.iter().rev() {
        if q < want {
            break;
        }
        if let Some(path) = media_cache.get(&song.id, q) {
            return Some((path, q));
        }
        if let Some(root) = download_root
            && let Some(path) = probe_export(root, song, q)
        {
            return Some((path, q));
        }
    }
    None
}

/// 在下载导出库里找 `song` 该音质的文件(文件系统即真相)。
///
/// 按 `<source>/<quality>/<album>` 定位专辑目录,在其中找「去扩展名后与本曲标题(已 sanitize)
/// 相等、且扩展名是已知音频格式」的文件,命中返回绝对路径。目录不存在 / 无匹配返回 `None`。
/// 同时供下载侧做「已下载」幂等判断(见 [`crate::download`])。
///
/// # Params:
///   - `root`: 下载导出根目录
///   - `song`: 歌曲
///   - `quality`: 音质
///
/// # Return:
///   命中的绝对路径,否则 `None`。
pub(crate) fn probe_export(root: &Path, song: &Song, quality: BitRate) -> Option<PathBuf> {
    let (subdir, stem) = library_dir_and_stem(song, quality);
    let dir = root.join(subdir);
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some((file_stem, ext)) = name.rsplit_once('.') else {
            continue;
        };
        if file_stem == stem && is_audio_ext(ext) && path.is_file() {
            return Some(path);
        }
    }
    None
}

/// 是否已知音频文件扩展名(大小写不敏感)。借此排除 `.part` / `.part-dl` 等未完成 / 非音频文件。
///
/// # Params:
///   - `ext`: 扩展名(不含点)
///
/// # Return:
///   是音频扩展名返回 `true`。
fn is_audio_ext(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "mp3" | "flac" | "aac" | "m4a" | "ogg" | "opus" | "wav" | "ape" | "alac"
    )
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use mineral_model::{AlbumId, AlbumRef, AudioFormat, BitRate, Song, SongId, SourceKind};
    use mineral_persist::ServerStore;

    use super::{probe_export, resolve_local};
    use crate::media_cache::{MediaCache, library_relpath};

    fn song(id: &str, name: &str, album: Option<&str>) -> Song {
        Song {
            id: SongId::new(SourceKind::NETEASE, id),
            name: name.to_owned(),
            artists: Vec::new(),
            album: album.map(|a| AlbumRef {
                id: AlbumId::new(SourceKind::NETEASE, "0"),
                name: a.to_owned(),
            }),
            duration_ms: 0,
            cover_url: None,
            source_url: None,
        }
    }

    /// 开一个启用态 MediaCache(缓存文件落 `dir`,索引落 `persist`)。
    async fn open_cache(persist: &ServerStore, dir: PathBuf) -> color_eyre::Result<MediaCache> {
        MediaCache::open(persist, dir, 1_000_000).await
    }

    /// 把一首歌按指定音质「下载」到 `root` 下(写真实文件;文件系统即索引,无需登记)。返回绝对路径。
    fn put_download(
        root: &Path,
        s: &Song,
        quality: BitRate,
        format: &AudioFormat,
        bytes: &[u8],
    ) -> color_eyre::Result<PathBuf> {
        let (subdir, file_name) = library_relpath(s, quality, format);
        let abs = root.join(&subdir).join(&file_name);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&abs, bytes)?;
        Ok(abs)
    }

    /// 把一首歌按指定音质塞进 cache(经 put_played move 一个临时文件入库)。
    async fn put_cache(
        cache: &MediaCache,
        tmp_dir: &Path,
        s: &Song,
        quality: BitRate,
        format: &AudioFormat,
        bytes: &[u8],
    ) -> color_eyre::Result<()> {
        let src = tmp_dir.join(format!("cap-{}-{}.part", s.id.value(), quality.as_str()));
        std::fs::write(&src, bytes)?;
        cache.put_played(s, quality, format, &src).await
    }

    /// 只有 cache 命中(无下载导出)→ 返回 cache 路径与该音质。
    #[tokio::test]
    async fn cache_only_hit() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let persist = ServerStore::open(&d.path().join("t.db")).await?;
        let cache = open_cache(&persist, d.path().join("cache")).await?;
        let root = d.path().join("music");
        let s = song("1", "晴天", Some("叶惠美"));
        put_cache(
            &cache,
            d.path(),
            &s,
            BitRate::Exhigh,
            &AudioFormat::Mp3,
            b"AUDIO",
        )
        .await?;

        let Some((path, q)) = resolve_local(&cache, Some(&root), &s, BitRate::Exhigh) else {
            return Err(color_eyre::eyre::eyre!("应命中 cache"));
        };
        assert_eq!(q, BitRate::Exhigh);
        assert_eq!(std::fs::read(&path)?, b"AUDIO");
        Ok(())
    }

    /// 回归(本次 bug):盘上有下载文件、**无任何索引登记**,离线仍应命中并播放(高于 want 的最高音质)。
    #[tokio::test]
    async fn download_only_hit() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let cache = MediaCache::disabled();
        let root = d.path().join("music");
        let s = song("1", "晴天", Some("叶惠美"));
        put_download(&root, &s, BitRate::Lossless, &AudioFormat::Flac, b"FLAC")?;

        let Some((path, q)) = resolve_local(&cache, Some(&root), &s, BitRate::Exhigh) else {
            return Err(color_eyre::eyre::eyre!("应命中下载导出文件"));
        };
        assert_eq!(q, BitRate::Lossless, "下载是 Lossless,>= Exhigh 应命中");
        assert!(path.ends_with("netease/lossless/叶惠美/晴天.flac"));
        assert_eq!(std::fs::read(&path)?, b"FLAC");
        Ok(())
    }

    /// cache 与下载导出同音质 → 优先 cache。
    #[tokio::test]
    async fn same_quality_prefers_cache() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let persist = ServerStore::open(&d.path().join("t.db")).await?;
        let cache = open_cache(&persist, d.path().join("cache")).await?;
        let root = d.path().join("music");
        let s = song("1", "晴天", Some("叶惠美"));
        put_cache(
            &cache,
            d.path(),
            &s,
            BitRate::Exhigh,
            &AudioFormat::Mp3,
            b"FROM_CACHE",
        )
        .await?;
        put_download(&root, &s, BitRate::Exhigh, &AudioFormat::Mp3, b"FROM_DL")?;

        let Some((path, _)) = resolve_local(&cache, Some(&root), &s, BitRate::Exhigh) else {
            return Err(color_eyre::eyre::eyre!("应命中"));
        };
        assert_eq!(std::fs::read(&path)?, b"FROM_CACHE", "同音质应取 cache");
        Ok(())
    }

    /// 下载导出比 cache 高(用户场景:exhigh 缓存 + lossless 下载)→ 取 lossless 下载,永远最高音质。
    #[tokio::test]
    async fn higher_download_beats_cache() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let persist = ServerStore::open(&d.path().join("t.db")).await?;
        let cache = open_cache(&persist, d.path().join("cache")).await?;
        let root = d.path().join("music");
        let s = song("1", "捕风", Some("野泳 (Wild Swim)"));
        put_cache(
            &cache,
            d.path(),
            &s,
            BitRate::Exhigh,
            &AudioFormat::Mp3,
            b"FROM_CACHE",
        )
        .await?;
        put_download(&root, &s, BitRate::Lossless, &AudioFormat::Flac, b"FROM_DL")?;

        let Some((path, q)) = resolve_local(&cache, Some(&root), &s, BitRate::Exhigh) else {
            return Err(color_eyre::eyre::eyre!("应命中"));
        };
        assert_eq!(q, BitRate::Lossless);
        assert_eq!(std::fs::read(&path)?, b"FROM_DL", "更高音质应取下载导出");
        Ok(())
    }

    /// 本地只有低于 want 的副本 → miss(走远端)。
    #[tokio::test]
    async fn below_threshold_misses() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let cache = MediaCache::disabled();
        let root = d.path().join("music");
        let s = song("1", "晴天", Some("叶惠美"));
        // 只有 Standard(< 请求的 Exhigh)。
        put_download(&root, &s, BitRate::Standard, &AudioFormat::Mp3, b"LOW")?;

        assert!(
            resolve_local(&cache, Some(&root), &s, BitRate::Exhigh).is_none(),
            "低于 want 的本地副本不应命中"
        );
        Ok(())
    }

    /// 文件被删 → 当 miss(目录在但无匹配文件)。
    #[tokio::test]
    async fn missing_file_misses() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let cache = MediaCache::disabled();
        let root = d.path().join("music");
        let s = song("1", "晴天", Some("叶惠美"));
        let abs = put_download(&root, &s, BitRate::Lossless, &AudioFormat::Flac, b"FLAC")?;
        std::fs::remove_file(&abs)?;

        assert!(
            resolve_local(&cache, Some(&root), &s, BitRate::Exhigh).is_none(),
            "文件没了应当 miss"
        );
        Ok(())
    }

    /// 未下完的 `.part-dl` 残件不应被当成可播文件命中。
    #[tokio::test]
    async fn ignores_part_dl_leftover() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let cache = MediaCache::disabled();
        let root = d.path().join("music");
        let s = song("1", "晴天", Some("叶惠美"));
        let dir = root.join("netease/lossless/叶惠美");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("晴天.part-dl"), b"HALF")?;

        assert!(
            resolve_local(&cache, Some(&root), &s, BitRate::Exhigh).is_none(),
            ".part-dl 残件不应命中"
        );
        Ok(())
    }

    /// `download_root` 为 `None`(下载不可用)→ 只看 cache。
    #[tokio::test]
    async fn no_download_root_uses_cache_only() -> color_eyre::Result<()> {
        let cache = MediaCache::disabled();
        let s = song("1", "晴天", Some("叶惠美"));
        assert!(resolve_local(&cache, /*download_root*/ None, &s, BitRate::Exhigh).is_none());
        Ok(())
    }

    /// probe_export 直接命中可读库路径(与下载侧幂等共用)。
    #[tokio::test]
    async fn probe_export_finds_readable_path() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let root = d.path().join("music");
        let s = song("1", "晴天", Some("叶惠美"));
        put_download(&root, &s, BitRate::Lossless, &AudioFormat::Flac, b"FLAC")?;
        let Some(path) = probe_export(&root, &s, BitRate::Lossless) else {
            return Err(color_eyre::eyre::eyre!("应反查到文件"));
        };
        assert!(path.ends_with("netease/lossless/叶惠美/晴天.flac"));
        // 不同音质目录无此文件 → miss。
        assert!(probe_export(&root, &s, BitRate::Exhigh).is_none());
        Ok(())
    }
}
