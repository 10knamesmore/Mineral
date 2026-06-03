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

use lofty::file::AudioFile;
use lofty::probe::Probe;
use mineral_model::{AudioFormat, BitRate, MediaUrl, PlayUrl, Song};
use mineral_protocol::PlaybackOrigin;

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
///   命中返回 `(本地文件绝对路径, 实际命中音质, 来源)`,来源只会是
///   [`PlaybackOrigin::Cache`] 或 [`PlaybackOrigin::Download`];本地无 `>= want` 副本返回
///   `None`(走远端)。
pub(crate) fn resolve_local(
    media_cache: &MediaCache,
    download_root: Option<&Path>,
    song: &Song,
    want: BitRate,
) -> Option<(PathBuf, BitRate, PlaybackOrigin)> {
    // ALL 升序 → rev() 高到低;低于 want 即可停(后续只会更低)。
    for &q in BitRate::ALL.iter().rev() {
        if q < want {
            break;
        }
        if let Some(path) = media_cache.get(&song.id, q) {
            return Some((path, q, PlaybackOrigin::Cache));
        }
        if let Some(root) = download_root
            && let Some(path) = probe_export(root, song, q)
        {
            return Some((path, q, PlaybackOrigin::Download));
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

/// 为本地命中的文件构造 [`PlayUrl`],供 transport 显 format / bitrate / 位深。
///
/// format、bitrate、位深都按**文件内容**经 lofty 读出(见 [`probe_format_props`],不信文件名
/// 扩展名)。lofty 取不到 bitrate 时回退「文件大小 / 时长」的均值估算;时长未知(0)或 stat 失败
/// 时 bitrate 记 0。位深仅无损容器有值,有损 / 取不到为 `None`。
///
/// # Params:
///   - `song`: 命中的歌曲(取 id 与时长)
///   - `path`: 本地文件绝对路径
///   - `quality`: 命中的音质档
///
/// # Return:
///   填好 format / bitrate / 位深 / size / quality 的 `PlayUrl`(`url` = `Local(path)`)。
pub(crate) fn local_play_url(song: &Song, path: &Path, quality: BitRate) -> PlayUrl {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let (format, probed_kbps, bit_depth) = probe_format_props(path);
    let bitrate_bps = probed_kbps
        .and_then(|kbps| kbps.checked_mul(1000))
        .or_else(|| est_bitrate_bps(size, song.duration_ms))
        .unwrap_or(0);
    PlayUrl {
        song_id: song.id.clone(),
        url: MediaUrl::Local(path.to_path_buf()),
        bitrate_bps,
        quality,
        size,
        format,
        bit_depth,
    }
}

/// 按**文件内容**(经 lofty)读出 `(格式, 码率kbps)`——全程不碰扩展名。
///
/// 先用 [`detect_file_type`] 按 magic bytes 判容器类型,再以该类型解析属性取 bitrate 与位深。
/// 任一步失败回退 `(空格式, None, None)`。
///
/// # Params:
///   - `path`: 本地文件绝对路径
///
/// # Return:
///   `(格式, 码率kbps, 位深bit)`;识别失败为 `(AudioFormat::default(), None, None)`。
fn probe_format_props(path: &Path) -> (AudioFormat, Option<u32>, Option<u8>) {
    let Some(ft) = detect_file_type(path) else {
        return (AudioFormat::default(), None, None);
    };
    let (kbps, bit_depth) = read_audio_props(path, ft);
    (file_type_to_format(ft), kbps, bit_depth)
}

/// 按**文件内容**判容器类型,走 lofty 的 [`Probe`](先读 ID3 标签再认底层帧)。
///
/// 两个关键选择:
///
/// - **不用** [`lofty::file::FileType::from_buffer`]:它一旦见到 `ID3` 前缀就返回 `None`
///   (无论给多少字节),而 NetEase exhigh 等 FFmpeg 转码的 mp3 恰是「ID3 标签 + MPEG 帧」结构,
///   会被它整片漏判。[`Probe::guess_file_type`] 会跳过标签再认帧,带标签的文件也判得对。
/// - 走 [`Probe::new`](reader)而非 [`Probe::open`](path):后者在内容认不出时**回退文件扩展名**,
///   违背「只认内容、不信扩展名」(缓存文件名本就按 format 推,信扩展名会循环依赖);前者无路径、
///   纯按内容判,认不出即 `None`。
///
/// # Params:
///   - `path`: 本地文件绝对路径
///
/// # Return:
///   识别出的类型,打开 / 读取 / 识别失败为 `None`。
fn detect_file_type(path: &Path) -> Option<lofty::file::FileType> {
    let file = std::fs::File::open(path).ok()?;
    Probe::new(std::io::BufReader::new(file))
        .guess_file_type()
        .ok()?
        .file_type()
}

/// 以**已知类型**(不经扩展名猜测)一次解析出音频 `(码率kbps, 位深bit)`。
///
/// 单次 lofty `read()` 同时取两者:位深仅无损容器(FLAC / WAV / APE 等)有值,有损为 `None`。
///
/// # Params:
///   - `path`: 本地文件绝对路径
///   - `ft`: 已由内容判出的容器类型
///
/// # Return:
///   `(码率kbps, 位深bit)`;各自解析失败 / lofty 未提供为 `None`。
fn read_audio_props(path: &Path, ft: lofty::file::FileType) -> (Option<u32>, Option<u8>) {
    let Ok(file) = std::fs::File::open(path) else {
        return (None, None);
    };
    let Ok(tagged) = Probe::new(std::io::BufReader::new(file))
        .set_file_type(ft)
        .read()
    else {
        return (None, None);
    };
    let props = tagged.properties();
    (props.audio_bitrate(), props.bit_depth())
}

/// 「文件大小 / 时长」估算码率(bps),作 lofty 取不到 bitrate 时的兜底。
///
/// # Params:
///   - `size`: 文件字节数
///   - `duration_ms`: 时长(ms),0 表示未知
///
/// # Return:
///   估算 bps;时长未知(checked_div → None)或溢出为 `None`。
fn est_bitrate_bps(size: u64, duration_ms: u64) -> Option<u32> {
    // size(B) * 8(bit/B) * 1000(ms/s) / duration(ms) = bit/s。
    size.saturating_mul(8000)
        .checked_div(duration_ms)
        .and_then(|bps| u32::try_from(bps).ok())
}

/// lofty 容器类型 → model 的 [`AudioFormat`]。未覆盖类型回退空格式。
///
/// # Params:
///   - `ft`: lofty 探测出的文件类型
///
/// # Return:
///   对应的 [`AudioFormat`]。
fn file_type_to_format(ft: lofty::file::FileType) -> AudioFormat {
    use lofty::file::FileType;
    match ft {
        FileType::Mpeg => AudioFormat::Mp3,
        FileType::Flac => AudioFormat::Flac,
        FileType::Mp4 => AudioFormat::Aac,
        FileType::Vorbis => AudioFormat::Ogg,
        FileType::Wav => AudioFormat::Wav,
        FileType::Ape => AudioFormat::Ape,
        FileType::Aac => AudioFormat::Aac,
        FileType::Opus => AudioFormat::Other("opus".to_owned()),
        _ => AudioFormat::default(),
    }
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

    use mineral_model::{
        AlbumId, AlbumRef, AudioFormat, BitRate, MediaUrl, Song, SongId, SourceKind,
    };
    use mineral_persist::ServerStore;
    use mineral_protocol::PlaybackOrigin;

    use super::{local_play_url, probe_export, resolve_local};
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

        let Some((path, q, origin)) = resolve_local(&cache, Some(&root), &s, BitRate::Exhigh)
        else {
            return Err(color_eyre::eyre::eyre!("应命中 cache"));
        };
        assert_eq!(q, BitRate::Exhigh);
        assert_eq!(origin, PlaybackOrigin::Cache);
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

        let Some((path, q, origin)) = resolve_local(&cache, Some(&root), &s, BitRate::Exhigh)
        else {
            return Err(color_eyre::eyre::eyre!("应命中下载导出文件"));
        };
        assert_eq!(q, BitRate::Lossless, "下载是 Lossless,>= Exhigh 应命中");
        assert_eq!(origin, PlaybackOrigin::Download);
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

        let Some((path, _, origin)) = resolve_local(&cache, Some(&root), &s, BitRate::Exhigh)
        else {
            return Err(color_eyre::eyre::eyre!("应命中"));
        };
        assert_eq!(origin, PlaybackOrigin::Cache, "同音质应取 cache");
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

        let Some((path, q, origin)) = resolve_local(&cache, Some(&root), &s, BitRate::Exhigh)
        else {
            return Err(color_eyre::eyre::eyre!("应命中"));
        };
        assert_eq!(q, BitRate::Lossless);
        assert_eq!(origin, PlaybackOrigin::Download, "更高音质应取下载导出");
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

    /// 合法最小 WAV(44B 头 + `data_len` 个 0 PCM 数据):8000Hz / 8bit / 单声道 →
    /// lofty 算出 64kbps。chunk size 写正确,lofty 才能完整解析属性。
    fn wav_bytes(data_len: usize) -> Vec<u8> {
        let data = u32::try_from(data_len).unwrap_or(0);
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&36u32.saturating_add(data).to_le_bytes()); // RIFF chunk size
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
        v.extend_from_slice(&1u16.to_le_bytes()); // PCM
        v.extend_from_slice(&1u16.to_le_bytes()); // 单声道
        v.extend_from_slice(&8000u32.to_le_bytes()); // sample rate
        v.extend_from_slice(&8000u32.to_le_bytes()); // byte rate
        v.extend_from_slice(&1u16.to_le_bytes()); // block align
        v.extend_from_slice(&8u16.to_le_bytes()); // bits per sample
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data.to_le_bytes()); // data chunk size
        v.resize(v.len() + data_len, 0u8);
        v
    }

    /// 构造「ID3v2.4 标签 + 一个 MPEG-1 Layer III 帧」的最小 mp3 字节,复刻 NetEase exhigh
    /// (FFmpeg `Lavf` 转码)产物:**首字节是 `ID3`,真正的 MPEG 同步头在标签之后**。
    ///
    /// 这正是 lofty `FileType::from_buffer` 认不出(见 ID3 前缀即返回 None,与缓冲区大小无关)、
    /// 必须改走 `Probe` 的场景。帧头 `FF FB 90 00` = MPEG1 / Layer III / 128kbps / 44100Hz / stereo。
    fn id3_prefixed_mp3() -> Vec<u8> {
        let mut v = Vec::new();
        // ID3v2.4 头(10B):"ID3" + 版本 04 00 + flags 00 + synchsafe size = 35。
        v.extend_from_slice(b"ID3");
        v.extend_from_slice(&[0x04, 0x00, 0x00]);
        v.extend_from_slice(&[0x00, 0x00, 0x00, 0x23]); // synchsafe 35
        v.resize(v.len() + 35, 0u8); // 35B 标签体(内容无关紧要)
        // MPEG-1 Layer III 帧头 + 帧体补零至帧长(144*128000/44100 ≈ 417B)。
        v.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        v.resize(v.len() + 417 - 4, 0u8);
        v
    }

    /// 回归(本次 bug):ID3 前缀的 mp3(缓存 / 下载库里 NetEase exhigh 的样子)经 `local_play_url`
    /// 应判出 `Mp3`——旧实现用 `from_buffer` 见 ID3 即 None、format 落空,本测试守住改走 `Probe` 后的行为。
    #[tokio::test]
    async fn local_play_url_detects_id3_prefixed_mp3() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let root = d.path().join("music");
        let s = song("1", "环岛", Some("夜晚做决定"));
        let bytes = id3_prefixed_mp3();
        // 坐实旧路径为何失效:from_buffer 见 ID3 前缀直接认不出(与字节多少无关)。
        assert!(
            lofty::file::FileType::from_buffer(&bytes).is_none(),
            "from_buffer 对 ID3 前缀应返回 None(正是本 bug 根因)"
        );

        let abs = put_download(&root, &s, BitRate::Exhigh, &AudioFormat::Mp3, &bytes)?;
        let pu = local_play_url(&s, &abs, BitRate::Exhigh);
        assert_eq!(pu.format, AudioFormat::Mp3, "ID3 前缀的 mp3 应判 Mp3");
        Ok(())
    }

    /// 杀手锏:format 与 bitrate 都按**内容**(lofty)、不信扩展名——WAV 内容写进 `.flac`
    /// 文件 → 仍判 WAV,且 bitrate 来自 lofty 解析(8000Hz/8bit/单声道 = 64kbps),
    /// **不等于** size/时长 估算(8044B/1s ≈ 64352bps),证明走的是真实解析。
    #[tokio::test]
    async fn local_play_url_format_and_bitrate_from_content() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let root = d.path().join("music");
        let mut s = song("1", "晴天", Some("叶惠美"));
        s.duration_ms = 1000; // 1s
        // 内容是 WAV,但 put_download 用 Flac 决定盘上扩展名 → 文件名是 .flac。
        let abs = put_download(
            &root,
            &s,
            BitRate::Lossless,
            &AudioFormat::Flac,
            &wav_bytes(8000),
        )?;
        assert!(
            abs.ends_with("netease/lossless/叶惠美/晴天.flac"),
            "盘上是 .flac 名"
        );

        let pu = local_play_url(&s, &abs, BitRate::Lossless);
        assert_eq!(
            pu.format,
            AudioFormat::Wav,
            "应按内容判 WAV,而非文件名的 .flac"
        );
        assert_eq!(pu.size, 8044);
        assert_eq!(
            pu.bitrate_bps, 64_000,
            "lofty 解析 64kbps(非 size/时长 估算)"
        );
        assert_eq!(pu.quality, BitRate::Lossless);
        assert!(matches!(pu.url, MediaUrl::Local(_)));
        Ok(())
    }

    /// 无法识别的内容 → format 空、lofty 无 bitrate → 回退 size/时长 估算。
    #[tokio::test]
    async fn local_play_url_unknown_content_estimates_bitrate() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let root = d.path().join("music");
        let mut s = song("1", "晴天", Some("叶惠美"));
        s.duration_ms = 1000;
        // 16000B / 1s → 128_000 bps(走估算兜底)。
        let abs = put_download(
            &root,
            &s,
            BitRate::Lossless,
            &AudioFormat::Flac,
            &[0u8; 16000],
        )?;

        let pu = local_play_url(&s, &abs, BitRate::Lossless);
        assert!(pu.format.is_empty(), "乱字节识别不出格式 → 空");
        assert_eq!(pu.bitrate_bps, 128_000, "回退估算:16000B/1s → 128kbps");
        Ok(())
    }

    /// 既识别不出格式、时长又未知(0)→ bitrate 记 0(估算 checked_div 不除零)。
    #[tokio::test]
    async fn local_play_url_unknown_content_zero_duration() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let root = d.path().join("music");
        let s = song("1", "晴天", Some("叶惠美")); // duration_ms = 0
        let abs = put_download(
            &root,
            &s,
            BitRate::Lossless,
            &AudioFormat::Flac,
            &[0u8; 8000],
        )?;

        let pu = local_play_url(&s, &abs, BitRate::Lossless);
        assert!(pu.format.is_empty());
        assert_eq!(pu.bitrate_bps, 0, "时长未知且无解析 → 0,不除零");
        Ok(())
    }

    /// file_type_to_format:lofty 容器类型 → AudioFormat 映射。
    #[test]
    fn file_type_to_format_maps_known() {
        use lofty::file::FileType;
        assert_eq!(super::file_type_to_format(FileType::Mpeg), AudioFormat::Mp3);
        assert_eq!(
            super::file_type_to_format(FileType::Flac),
            AudioFormat::Flac
        );
        assert_eq!(super::file_type_to_format(FileType::Wav), AudioFormat::Wav);
        assert_eq!(
            super::file_type_to_format(FileType::Vorbis),
            AudioFormat::Ogg
        );
        assert_eq!(super::file_type_to_format(FileType::Mp4), AudioFormat::Aac);
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
