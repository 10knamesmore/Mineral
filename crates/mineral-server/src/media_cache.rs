//! 音频本体的本地缓存:在通用 [`CacheIndex`] 之上,补"按歌组织可读库路径"的领域逻辑。
//!
//! 落盘布局 `<source>/<quality>/<album>/<title>.<ext>`(可读、无 hash,可被播放器直接打开);
//! index 键 `cache_key` = `{song_id.qualified()}:{quality}`,每音质一份,落 `audio_cache` 表。
//! 缓存不可用(未开/open 失败)时退化成 null-object:`get` 恒 miss、`put_played` 静默成功,
//! 不影响流式播放。

use std::path::PathBuf;

use mineral_model::{AudioFormat, BitRate, Song, SongId};
use mineral_persist::{CacheIndex, ServerStore};

/// 单段文件名 / 目录名的最大字节数(留余量,远低于 255 上限)。
const SEGMENT_MAX_BYTES: usize = 200;

/// 音频本体缓存。线程安全(内部 [`CacheIndex`] 自带锁)。`get` sync、`put_played` async(写穿透)。
pub(crate) struct MediaCache {
    /// 后端缓存索引(DB-backed);禁用时为 null-object。
    index: CacheIndex,

    /// 缓存根目录;`None` = 禁用。capture 临时文件落到其下 `tmp/`(与最终库路径同分区 → harvest 走 rename)。
    root: Option<PathBuf>,
}

impl MediaCache {
    /// 打开(或创建)音频缓存:在 `persist` 上建 `audio_cache` 索引表 + 载入镜像。
    ///
    /// # Params:
    ///   - `persist`: daemon 持久化句柄(索引落其 `mineral.db`)
    ///   - `dir`: 缓存文件根目录(`relpath` 相对它)
    ///   - `capacity`: 容量上限字节
    ///
    /// # Return:
    ///   就绪缓存;底层 open 失败返回 `Err`(调用方应降级到 [`Self::disabled`])。
    pub(crate) async fn open(
        persist: &ServerStore,
        dir: PathBuf,
        capacity: u64,
    ) -> color_eyre::Result<Self> {
        // 清掉上次进程遗留的半截 capture(崩溃 / 被 kill 时没下完、也没 harvest 的 .part)。
        let tmp = dir.join("tmp");
        if tmp.is_dir() {
            drop(std::fs::remove_dir_all(&tmp));
        }
        let index = persist.audio_cache(dir.clone(), capacity).await?;
        Ok(Self {
            index,
            root: Some(dir),
        })
    }

    /// 禁用态缓存:`get` 恒 miss、`put_played` 静默成功、`capture_path` 恒 `None`。
    pub(crate) fn disabled() -> Self {
        Self {
            index: CacheIndex::disabled(),
            root: None,
        }
    }

    /// capture 临时落点 `<root>/tmp/<sanitized-key>.part`,与最终库路径同分区(harvest 走 rename)。
    ///
    /// # Params:
    ///   - `song_id`: 歌曲 ID
    ///   - `quality`: 入库音质
    ///
    /// # Return:
    ///   临时路径;缓存禁用返回 `None`。
    pub(crate) fn capture_path(&self, song_id: &SongId, quality: BitRate) -> Option<PathBuf> {
        let root = self.root.as_ref()?;
        let safe = sanitize_segment(&cache_key(song_id, quality), "capture");
        Some(root.join("tmp").join(format!("{safe}.part")))
    }

    /// 命中返回缓存音频文件的绝对路径(可直接 `MediaUrl::Local` 播放),否则 `None`。
    ///
    /// # Params:
    ///   - `song_id`: 歌曲 ID
    ///   - `quality`: 请求音质(与入库时一致才命中)
    ///
    /// # Return:
    ///   命中且文件存在返回路径,否则 None。
    pub(crate) fn get(&self, song_id: &SongId, quality: BitRate) -> Option<PathBuf> {
        self.index.get(&cache_key(song_id, quality))
    }

    /// 把一首**已自然播完**的歌的音频文件收编进缓存(move 入库,可读库路径)。
    ///
    /// # Params:
    ///   - `song`: 刚播完的歌(取来源 / 专辑 / 标题组库路径)
    ///   - `quality`: 入库音质(决定 index 键与目录;应与播放请求一致)
    ///   - `format`: 实际音频格式(决定扩展名;未知按音质兜底)
    ///   - `src`: capture 落盘文件路径(成功后被移走)
    ///
    /// # Return:
    ///   入库时 LRU 驱逐掉的记录(可空;供上层 cache_evictions 埋点);缓存禁用返回空 vec。
    pub(crate) async fn put_played(
        &self,
        song: &Song,
        quality: BitRate,
        format: Option<&AudioFormat>,
        src: &std::path::Path,
    ) -> color_eyre::Result<Vec<mineral_persist::Evicted>> {
        let key = cache_key(&song.id, quality);
        let (subdir, file_name) = library_relpath(song, quality, format);
        self.index.record_file(&key, src, &subdir, &file_name).await
    }
}

/// 一首歌在库里的相对落点 `(subdir, file_name)` = `<source>/<quality>/<album>` + `<title>.<ext>`。
/// 缓存入库与永久导出共用同一套命名(下载导出 [`crate::download`] 复用)。
///
/// # Params:
///   - `song`: 歌曲(取来源 / 专辑 / 标题)
///   - `quality`: 音质(目录层)
///   - `format`: 实际格式(定扩展名;未知按音质兜底)
///
/// # Return:
///   `(subdir, file_name)`。
pub(crate) fn library_relpath(
    song: &Song,
    quality: BitRate,
    format: Option<&AudioFormat>,
) -> (String, String) {
    let (subdir, stem) = library_dir_and_stem(song, quality);
    let ext_seg = sanitize_segment(&ext_for(format, quality), "bin");
    (subdir, format!("{stem}.{ext_seg}"))
}

/// 一首歌在库里的「专辑目录 + 文件名主干(不含扩展名)」= `<source>/<quality>/<album>` + `<title>`。
/// 与 [`library_relpath`] 同一套命名,只是不带扩展名——供下载导出**按文件系统反查**:磁盘上
/// 扩展名未知时,定位到专辑目录后按主干匹配文件(见 [`crate::resolve`])。
///
/// # Params:
///   - `song`: 歌曲(取来源 / 专辑 / 标题)
///   - `quality`: 音质(目录层)
///
/// # Return:
///   `(subdir, title_stem)`,各段已 sanitize。
pub(crate) fn library_dir_and_stem(song: &Song, quality: BitRate) -> (String, String) {
    let album = song.album.as_ref().map(|a| a.name.as_str());
    let title = if song.name.is_empty() {
        song.id.as_str()
    } else {
        song.name.as_str()
    };
    (
        library_subdir(song.source().name(), album, quality),
        sanitize_segment(title, "_untitled"),
    )
}

/// 音频缓存索引键:`{song_id.qualified()}:{quality}`,全局唯一、每音质独立(`audio_cache` 表用)。
/// 下载导出不走索引(按文件系统路径反查,见 [`crate::resolve`]),故无需此键。
///
/// # Params:
///   - `song_id`: 歌曲 ID
///   - `quality`: 音质
///
/// # Return:
///   形如 `netease:186016:lossless`。
pub(crate) fn cache_key(song_id: &SongId, quality: BitRate) -> String {
    format!("{}:{}", song_id.qualified(), quality.as_str())
}

/// 库内专辑目录 `<source>/<quality>/<album>`(各段已 sanitize;`album` 为 `None` / 空 → `_unknown`)。
///
/// # Params:
///   - `source`: 来源名(如 `netease`)
///   - `album`: 专辑名
///   - `quality`: 音质(目录层)
///
/// # Return:
///   形如 `netease/lossless/叶惠美`。
fn library_subdir(source: &str, album: Option<&str>, quality: BitRate) -> String {
    let source_seg = sanitize_segment(source, "_unknown");
    let album_seg = sanitize_segment(album.unwrap_or(""), "_unknown");
    format!("{source_seg}/{}/{album_seg}", quality.as_str())
}

/// 扩展名:优先用实际格式,未知(拿不到格式)按音质兜底无损 `flac` / 有损 `mp3`。
///
/// # Params:
///   - `format`: 实际音频格式;`None` = 未知
///   - `quality`: 音质(format 未知时据此兜底)
///
/// # Return:
///   扩展名(不含点)。
fn ext_for(format: Option<&AudioFormat>, quality: BitRate) -> String {
    if let Some(f) = format {
        return f.as_str().to_owned();
    }
    match quality {
        BitRate::Lossless | BitRate::Hires => "flac",
        BitRate::Standard | BitRate::Higher | BitRate::Exhigh => "mp3",
    }
    .to_owned()
}

/// 把一段文本规整成合法的文件 / 目录名:非法字符→`_`、去首尾空白与尾点、按字节截断、空段兜底。
///
/// # Params:
///   - `raw`: 原始文本(歌名 / 专辑名等)
///   - `fallback`: 规整后为空时的兜底名
///
/// # Return:
///   合法的单段名(非空)。
fn sanitize_segment(raw: &str, fallback: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || ch.is_control() {
            out.push('_');
        } else {
            out.push(ch);
        }
    }
    let trimmed = out.trim().trim_end_matches('.').trim();
    let truncated = truncate_bytes(trimmed, SEGMENT_MAX_BYTES);
    if truncated.is_empty() {
        fallback.to_owned()
    } else {
        truncated.to_owned()
    }
}

/// 按 UTF-8 字符边界把 `s` 截断到不超过 `max` 字节。
///
/// # Params:
///   - `s`: 输入
///   - `max`: 最大字节数
///
/// # Return:
///   `s` 的前缀子串(`<= max` 字节,落在字符边界)。
fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.get(..end).unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use mineral_model::{AlbumId, AlbumRef, AudioFormat, BitRate, Song, SongId, SourceKind};

    use mineral_persist::ServerStore;

    use super::{
        MediaCache, cache_key, ext_for, library_relpath, sanitize_segment, truncate_bytes,
    };

    /// 起一个临时 DB 上的启用态 MediaCache(缓存文件落 `dir`)。
    async fn open_cache(
        db: &std::path::Path,
        dir: std::path::PathBuf,
    ) -> color_eyre::Result<MediaCache> {
        let persist = ServerStore::open(db).await?;
        let capacity = *mineral_config::Config::defaults()?.cache().audio_capacity();
        MediaCache::open(&persist, dir, capacity).await
    }

    fn song(id: &str, name: &str, album: Option<&str>) -> Song {
        Song::builder()
            .id(SongId::new(SourceKind::NETEASE, id))
            .name(name.to_owned())
            .album(album.map(|a| AlbumRef {
                id: AlbumId::new(SourceKind::NETEASE, "0"),
                name: a.to_owned(),
            }))
            .build()
    }

    #[test]
    fn key_includes_qualified_and_quality() {
        let s = song("186016", "晴天", Some("叶惠美"));
        assert_eq!(
            cache_key(&s.id, BitRate::Lossless),
            "netease:186016:lossless"
        );
        assert_eq!(cache_key(&s.id, BitRate::Exhigh), "netease:186016:exhigh");
        assert_ne!(
            cache_key(&s.id, BitRate::Lossless),
            cache_key(&s.id, BitRate::Exhigh),
            "不同音质应是不同键"
        );
    }

    #[test]
    fn relpath_is_readable_library_layout() {
        let s = song("186016", "晴天", Some("叶惠美"));
        let (subdir, file) = library_relpath(&s, BitRate::Lossless, Some(&AudioFormat::Flac));
        assert_eq!(subdir, "netease/lossless/叶惠美");
        assert_eq!(file, "晴天.flac");
    }

    #[test]
    fn relpath_falls_back_on_empty_album_and_title() {
        // 专辑缺失 → _unknown;标题 sanitize 后为空(全空白)→ _untitled。
        let s = song("186016", "   ", None);
        let (subdir, file) = library_relpath(&s, BitRate::Higher, Some(&AudioFormat::Mp3));
        assert_eq!(subdir, "netease/higher/_unknown");
        assert_eq!(file, "_untitled.mp3");
    }

    /// put_played 把 capture 文件 move 入可读库路径,get 用同 (song_id, quality) 命中;源被移走。
    #[tokio::test]
    async fn put_played_then_get_roundtrips() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let cache = open_cache(&d.path().join("t.db"), d.path().join("cache")).await?;
        let s = song("186016", "晴天", Some("叶惠美"));
        // 模拟 capture 落盘文件。
        let src = d.path().join("capture.part");
        std::fs::write(&src, b"FLACdata")?;
        cache
            .put_played(&s, BitRate::Lossless, Some(&AudioFormat::Flac), &src)
            .await?;

        let Some(path) = cache.get(&s.id, BitRate::Lossless) else {
            return Err(color_eyre::eyre::eyre!("入库后应命中"));
        };
        assert!(
            path.ends_with("netease/lossless/叶惠美/晴天.flac"),
            "应落到可读库路径: {}",
            path.display()
        );
        assert_eq!(std::fs::read(&path)?, b"FLACdata");
        assert!(!src.exists(), "源 capture 文件应被移走");
        // 不同音质不命中(键含音质)。
        assert!(cache.get(&s.id, BitRate::Exhigh).is_none());
        Ok(())
    }

    /// 回归(本次 bug):harvest 入库后**整个 MediaCache + DB 连接池销毁**(模拟 daemon 重启,
    /// 不做任何显式 flush —— 正是旧 `BlobCache` 只靠 Drop flush 的崩点),用同一 db 文件 +
    /// 同一缓存目录重开 → 仍命中同一文件;再 harvest 同曲应**原地覆盖、绝不产生 ` (2)`**。
    #[tokio::test]
    async fn survives_daemon_restart_no_redownload_dup() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let db = d.path().join("mineral.db");
        let dir = d.path().join("cache");
        let s = song("186016", "昨天的去向", Some("夜晚做决定"));

        let first_path = {
            let cache = open_cache(&db, dir.clone()).await?;
            let src = d.path().join("cap.part");
            std::fs::write(&src, b"AUDIO")?;
            cache
                .put_played(&s, BitRate::Exhigh, Some(&AudioFormat::Mp3), &src)
                .await?;
            let Some(p) = cache.get(&s.id, BitRate::Exhigh) else {
                return Err(color_eyre::eyre::eyre!("入库后应命中"));
            };
            p
            // cache(连同它持有的连接池)在此 drop —— 无显式 flush。
        };

        // 模拟重启:同 db 文件 + 同缓存目录重开一个全新 MediaCache(全新连接池)。
        let reopened = open_cache(&db, dir.clone()).await?;
        let Some(p2) = reopened.get(&s.id, BitRate::Exhigh) else {
            return Err(color_eyre::eyre::eyre!("重启后应仍命中,不该回退重下"));
        };
        assert_eq!(p2, first_path, "重启后应命中同一文件");

        // 再 harvest 一次同曲(若 get 没命中、走了重下 capture 才会发生)→ 同 key 原地覆盖。
        let src2 = d.path().join("cap2.part");
        std::fs::write(&src2, b"AUDIO2")?;
        reopened
            .put_played(&s, BitRate::Exhigh, Some(&AudioFormat::Mp3), &src2)
            .await?;
        let album_dir = dir.join("netease/exhigh/夜晚做决定");
        let mut names = Vec::<String>::new();
        for entry in std::fs::read_dir(&album_dir)? {
            if let Some(name) = entry?.file_name().to_str() {
                names.push(name.to_owned());
            }
        }
        assert_eq!(
            names,
            vec!["昨天的去向.mp3".to_owned()],
            "同 key 应原地覆盖,绝不产生 ` (2)` 副本"
        );
        Ok(())
    }

    /// capture_path 落在缓存根的 tmp/ 下、带 .part 后缀;键里的冒号被 sanitize 成 `_`。
    #[tokio::test]
    async fn capture_path_under_tmp() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let dir = d.path().join("cache");
        let cache = open_cache(&d.path().join("t.db"), dir.clone()).await?;
        let s = song("186016", "晴天", None);
        let Some(p) = cache.capture_path(&s.id, BitRate::Lossless) else {
            return Err(color_eyre::eyre::eyre!("启用态应给 capture 路径"));
        };
        assert!(
            p.starts_with(dir.join("tmp")),
            "应在 tmp/ 下: {}",
            p.display()
        );
        assert_eq!(
            p.file_name().and_then(|s| s.to_str()),
            Some("netease_186016_lossless.part")
        );
        Ok(())
    }

    /// 禁用态:get 恒 miss、put_played 静默成功、capture_path 恒 None。
    #[tokio::test]
    async fn disabled_is_null_object() -> color_eyre::Result<()> {
        let cache = MediaCache::disabled();
        let s = song("1", "x", None);
        assert!(cache.get(&s.id, BitRate::Lossless).is_none());
        assert!(cache.capture_path(&s.id, BitRate::Lossless).is_none());
        // put_played 不报错(src 不存在也无所谓,直接返回 Ok)。
        cache
            .put_played(
                &s,
                BitRate::Lossless,
                Some(&AudioFormat::Flac),
                std::path::Path::new("/nope"),
            )
            .await?;
        Ok(())
    }

    #[test]
    fn sanitize_replaces_illegal_chars() {
        assert_eq!(sanitize_segment("AC/DC: Back?", "_x"), "AC_DC_ Back_");
        assert_eq!(sanitize_segment("   ", "_unknown"), "_unknown");
        assert_eq!(sanitize_segment("a/b", "_x"), "a_b");
    }

    #[test]
    fn sanitize_truncates_on_char_boundary() {
        let long = "晴".repeat(100); // 每个 3 字节 → 300 字节,超 200
        let out = sanitize_segment(&long, "_x");
        assert!(out.len() <= 200, "应截到 <=200 字节, got {}", out.len());
        assert!(out.chars().all(|c| c == '晴'), "不应在字符中间截断出乱码");
    }

    #[test]
    fn truncate_keeps_char_boundary() {
        assert_eq!(truncate_bytes("abcdef", 3), "abc");
        assert_eq!(truncate_bytes("晴天", 4), "晴"); // 晴=3字节,4 退到 3
        assert_eq!(truncate_bytes("abc", 10), "abc");
    }

    #[test]
    fn ext_prefers_format_then_falls_back_by_quality() {
        assert_eq!(ext_for(Some(&AudioFormat::Flac), BitRate::Lossless), "flac");
        assert_eq!(ext_for(Some(&AudioFormat::Mp3), BitRate::Higher), "mp3");
        // 格式未知(None)→ 按音质兜底
        assert_eq!(ext_for(None, BitRate::Lossless), "flac");
        assert_eq!(ext_for(None, BitRate::Higher), "mp3");
    }
}
