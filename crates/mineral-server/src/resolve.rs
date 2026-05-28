//! 本地优先的播放解析:把待播 [`Song`] 解析到本地已有的音频文件(cache 或 download
//! 导出),命中则跳过整条网络取链路径。
//!
//! 解析按音质从高到低枚举(只看 `>= want`):同一音质先探 cache(热、id-safe)后查
//! download 索引,第一个命中即「本地最高可用音质」(故 lossless 也能喂给较低请求播放,
//! 同音质时优先 cache)。两侧后端都是 [`CacheIndex`](DB 内存镜像 + 写穿透):`get` 内部
//! 已 stat 校验文件存在、漂移时内存删项自愈,故解析层**纯查、不再按路径回退**,不受同源同
//! 专辑同名撞车影响;命中即得绝对路径。

use std::path::PathBuf;

use mineral_model::{BitRate, Song};
use mineral_persist::CacheIndex;

use crate::media_cache::{MediaCache, cache_key};

/// 把 `song` 解析到本地音频文件(音质 `>= want` 的最高可用副本)。
///
/// # Params:
///   - `media_cache`: 音频本体缓存(LRU,id-safe)
///   - `downloads`: 下载导出索引(`download_export` 表,不驱逐)
///   - `song`: 待播歌曲
///   - `want`: 期望的最低音质
///
/// # Return:
///   命中返回 `(本地文件绝对路径, 实际命中音质)`;本地无 `>= want` 副本返回 `None`(走远端)。
pub(crate) fn resolve_local(
    media_cache: &MediaCache,
    downloads: &CacheIndex,
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
        if let Some(path) = downloads.get(&cache_key(&song.id, q)) {
            return Some((path, q));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use mineral_model::{AlbumId, AlbumRef, AudioFormat, BitRate, Song, SongId, SourceKind};
    use mineral_persist::{CacheIndex, ServerStore};

    use super::resolve_local;
    use crate::media_cache::{MediaCache, cache_key, library_relpath};

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
    async fn open_cache(
        persist: &ServerStore,
        dir: std::path::PathBuf,
    ) -> color_eyre::Result<MediaCache> {
        MediaCache::open(persist, dir, 1_000_000).await
    }

    /// 开一个不驱逐的下载导出索引(文件根 `root`)。
    async fn open_downloads(
        persist: &ServerStore,
        root: std::path::PathBuf,
    ) -> color_eyre::Result<CacheIndex> {
        persist.download_export(root).await
    }

    /// 把一首歌按指定音质「下载」到 root 下并登记索引(写真实文件 + record relpath)。
    async fn put_download(
        downloads: &CacheIndex,
        root: &Path,
        s: &Song,
        quality: BitRate,
        format: &AudioFormat,
        bytes: &[u8],
    ) -> color_eyre::Result<()> {
        let (subdir, file_name) = library_relpath(s, quality, format);
        let rel = format!("{subdir}/{file_name}");
        let abs = root.join(&rel);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&abs, bytes)?;
        let len = u64::try_from(bytes.len())?;
        downloads
            .record(&cache_key(&s.id, quality), &rel, len)
            .await?;
        Ok(())
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

    /// 只有 cache 命中(无 download)→ 返回 cache 路径与该音质。
    #[tokio::test]
    async fn cache_only_hit() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let persist = ServerStore::open(&d.path().join("t.db")).await?;
        let cache = open_cache(&persist, d.path().join("cache")).await?;
        let downloads = CacheIndex::disabled();
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

        let Some((path, q)) = resolve_local(&cache, &downloads, &s, BitRate::Exhigh) else {
            return Err(color_eyre::eyre::eyre!("应命中 cache"));
        };
        assert_eq!(q, BitRate::Exhigh);
        assert_eq!(std::fs::read(&path)?, b"AUDIO");
        Ok(())
    }

    /// 只有 download 命中(cache disabled)→ 返回 download 路径与该音质。
    #[tokio::test]
    async fn download_only_hit() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let persist = ServerStore::open(&d.path().join("t.db")).await?;
        let cache = MediaCache::disabled();
        let root = d.path().join("music");
        let downloads = open_downloads(&persist, root.clone()).await?;
        let s = song("1", "晴天", Some("叶惠美"));
        put_download(
            &downloads,
            &root,
            &s,
            BitRate::Lossless,
            &AudioFormat::Flac,
            b"FLAC",
        )
        .await?;

        let Some((path, q)) = resolve_local(&cache, &downloads, &s, BitRate::Exhigh) else {
            return Err(color_eyre::eyre::eyre!("应命中 download"));
        };
        assert_eq!(q, BitRate::Lossless, "下载是 Lossless,>= Exhigh 应命中");
        assert!(path.ends_with("netease/lossless/叶惠美/晴天.flac"));
        Ok(())
    }

    /// cache 与 download 同音质 → 优先 cache。
    #[tokio::test]
    async fn same_quality_prefers_cache() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let persist = ServerStore::open(&d.path().join("t.db")).await?;
        let cache = open_cache(&persist, d.path().join("cache")).await?;
        let root = d.path().join("music");
        let downloads = open_downloads(&persist, root.clone()).await?;
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
        put_download(
            &downloads,
            &root,
            &s,
            BitRate::Exhigh,
            &AudioFormat::Mp3,
            b"FROM_DL",
        )
        .await?;

        let Some((path, _)) = resolve_local(&cache, &downloads, &s, BitRate::Exhigh) else {
            return Err(color_eyre::eyre::eyre!("应命中"));
        };
        assert_eq!(std::fs::read(&path)?, b"FROM_CACHE", "同音质应取 cache");
        Ok(())
    }

    /// download 比 cache 高 → 取 download(永远最高音质)。
    #[tokio::test]
    async fn higher_download_beats_cache() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let persist = ServerStore::open(&d.path().join("t.db")).await?;
        let cache = open_cache(&persist, d.path().join("cache")).await?;
        let root = d.path().join("music");
        let downloads = open_downloads(&persist, root.clone()).await?;
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
        put_download(
            &downloads,
            &root,
            &s,
            BitRate::Lossless,
            &AudioFormat::Flac,
            b"FROM_DL",
        )
        .await?;

        let Some((path, q)) = resolve_local(&cache, &downloads, &s, BitRate::Exhigh) else {
            return Err(color_eyre::eyre::eyre!("应命中"));
        };
        assert_eq!(q, BitRate::Lossless);
        assert_eq!(std::fs::read(&path)?, b"FROM_DL", "更高音质应取 download");
        Ok(())
    }

    /// 本地只有低于 want 的副本 → miss(走远端)。
    #[tokio::test]
    async fn below_threshold_misses() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let persist = ServerStore::open(&d.path().join("t.db")).await?;
        let cache = MediaCache::disabled();
        let root = d.path().join("music");
        let downloads = open_downloads(&persist, root.clone()).await?;
        let s = song("1", "晴天", Some("叶惠美"));
        // 只有 Standard(< 请求的 Exhigh)。
        put_download(
            &downloads,
            &root,
            &s,
            BitRate::Standard,
            &AudioFormat::Mp3,
            b"LOW",
        )
        .await?;

        assert!(
            resolve_local(&cache, &downloads, &s, BitRate::Exhigh).is_none(),
            "低于 want 的本地副本不应命中"
        );
        Ok(())
    }

    /// 索引有记录但文件被删 → 当 miss(CacheIndex.get 内部自愈)。
    #[tokio::test]
    async fn self_heals_when_file_deleted() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let persist = ServerStore::open(&d.path().join("t.db")).await?;
        let cache = MediaCache::disabled();
        let root = d.path().join("music");
        let downloads = open_downloads(&persist, root.clone()).await?;
        let s = song("1", "晴天", Some("叶惠美"));
        put_download(
            &downloads,
            &root,
            &s,
            BitRate::Lossless,
            &AudioFormat::Flac,
            b"FLAC",
        )
        .await?;
        // 用户手动删了文件。
        let (subdir, file_name) = library_relpath(&s, BitRate::Lossless, &AudioFormat::Flac);
        std::fs::remove_file(root.join(&subdir).join(&file_name))?;

        assert!(
            resolve_local(&cache, &downloads, &s, BitRate::Exhigh).is_none(),
            "文件没了应当 miss"
        );
        assert!(
            downloads
                .get(&cache_key(&s.id, BitRate::Lossless))
                .is_none(),
            "死记录应被自愈"
        );
        Ok(())
    }
}
