//! 文件级 metadata tag 写引擎:一份结构化 [`SongTags`] → 按容器写入音频文件。
//!
//! 只依赖 lofty,不发起网络请求、不关心字段来源。写盘协议是「同目录临时副本 +
//! rename 原子替换」:lofty 各格式的 save 全是原地 splice + `truncate(0)` 重写,
//! 中途崩溃会损坏文件;且缓存文件落盘时可能正被解码器持有 fd 读取,前置 ID3v2
//! 的全文件移位会让读者拿到错位字节。副本 + rename 后旧读者继续持有旧 inode
//! 读到 EOF,新路径指向打好 tag 的文件。

use std::io::Seek as _;
use std::path::Path;

use color_eyre::eyre::WrapErr;
use lofty::config::WriteOptions;
use lofty::error::ErrorKind;
use lofty::file::{FileType, TaggedFileExt};
use lofty::id3::v2::Id3v2Tag;
use lofty::mp4::{Atom, AtomData, AtomIdent, Ilst};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey, ItemValue, Tag, TagExt, TagItem};

/// 打标水印(写入 `EncodedBy`):标记「本文件已被当前版本打标流程完整处理过」,回填据此
/// 跳过增量。**tag 结构变更(增删字段)时必须 bump 版本号**,老文件由此自然重打。
const TAG_WATERMARK: &str = "mineral-tag/1";

/// 一首歌的内嵌 metadata 集合。字段为 `None` / 空 vec = 不写对应 tag(采集方
/// 单项失败即降级为缺字段,见 `super::assemble`)。
#[derive(Clone, Debug, Default)]
pub(crate) struct SongTags {
    /// 标题(TIT2 / TITLE / ©nam)。
    pub(crate) title: Option<String>,

    /// 艺人,多值、主在前(TPE1 / ARTIST / ©ART)。
    pub(crate) artists: Vec<String>,

    /// 专辑(TALB / ALBUM / ©alb)。
    pub(crate) album: Option<String>,

    /// 专辑艺人,多值(TPE2 / ALBUMARTIST / aART)。
    pub(crate) album_artists: Vec<String>,

    /// 发行年(TDRC / DATE / ©day,只写年份)。
    pub(crate) year: Option<u32>,

    /// 厂牌(TPUB / LABEL / ----:com.apple.iTunes:LABEL)。
    pub(crate) label: Option<String>,

    /// 歌词,lrc 文本(USLT / LYRICS / ©lyr)。
    pub(crate) lyrics_lrc: Option<String>,

    /// 封面图字节(APIC / METADATA_BLOCK_PICTURE / covr);mime 按字节 sniff,不信 URL 后缀。
    pub(crate) cover: Option<Vec<u8>>,
}

/// 写 tag 的结局(`Err` 另表失败)。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WriteOutcome {
    /// 已写入并原子替换目标路径。
    Tagged,

    /// 内容无法探测或容器不支持写 tag,原文件未动。
    SkippedUnsupported,
}

/// 把 `tags` 写进 `path` 指向的音频文件(同目录副本 + rename 原子替换)。
///
/// 任何结局都保证:失败 / 跳过时原文件字节不变、无残留副本。本函数是阻塞 I/O
/// (整文件 copy + lofty 全量重写),async 调用方应下沉 `spawn_blocking`。
///
/// # Params:
///   - `path`: 目标音频文件
///   - `tags`: 要写的 tag 集合
///   - `watermark`: 是否写打标水印(`EncodedBy`);采集有「可重试失败」时传 `false`,
///     让下次回填重试本文件(见 [`TAG_WATERMARK`])
///
/// # Return:
///   写入成功 / 容器不支持;copy、探测 IO、写盘、rename 失败返回 `Err`。
pub(crate) fn write_tags(
    path: &Path,
    tags: &SongTags,
    watermark: bool,
) -> color_eyre::Result<WriteOutcome> {
    let tmp = path.with_extension("part-tag");
    std::fs::copy(path, &tmp).wrap_err_with(|| format!("复制到临时副本失败 {}", tmp.display()))?;
    match tag_in_place(&tmp, tags, watermark) {
        Ok(WriteOutcome::Tagged) => {
            std::fs::rename(&tmp, path)
                .wrap_err_with(|| format!("rename 回目标失败 {}", path.display()))?;
            Ok(WriteOutcome::Tagged)
        }
        Ok(outcome) => {
            cleanup(&tmp);
            Ok(outcome)
        }
        Err(e) => {
            cleanup(&tmp);
            Err(e)
        }
    }
}

/// 在副本上探测容器并写入 tag(lofty 原地 save,只作用于副本)。容器不支持时
/// 返回 `Ok(SkippedUnsupported)` 而非报错——那是常态(B 站偶发裸流),不是故障。
///
/// # Params:
///   - `path`: 临时副本路径(可原地改写)
///   - `tags`: 要写的 tag 集合
///   - `watermark`: 是否写打标水印
///
/// # Return:
///   写入成功 / 容器不支持;打开、探测、写盘失败返回 `Err`。
fn tag_in_place(path: &Path, tags: &SongTags, watermark: bool) -> color_eyre::Result<WriteOutcome> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .wrap_err_with(|| format!("打开副本失败 {}", path.display()))?;
    let probed = Probe::new(&mut file)
        .guess_file_type()
        .wrap_err_with(|| format!("读取副本失败 {}", path.display()))?;
    let tagged = match probed.read() {
        Ok(t) => t,
        Err(e) if matches!(e.kind(), ErrorKind::UnknownFormat) => {
            return Ok(WriteOutcome::SkippedUnsupported);
        }
        Err(e) => return Err(e).wrap_err_with(|| format!("探测音频容器失败 {}", path.display())),
    };
    let file_type = tagged.file_type();
    let mut tag = tagged
        .primary_tag()
        .cloned()
        .unwrap_or_else(|| Tag::new(tagged.primary_tag_type()));
    fill_tag(&mut tag, tags, file_type);
    if watermark {
        tag.insert_text(ItemKey::EncodedBy, TAG_WATERMARK.to_owned());
    }
    // save_to 内部从**当前偏移**重新探测容器(guess_inner 以 stream_position 为起点),
    // 上面的 read() 已把偏移推到文件中部——不 rewind,FLAC magic / MP4 atom 全读岔。
    // 别把这句挪进 save_to 之后,也别指望 lofty 替你复位。
    file.rewind()
        .wrap_err_with(|| format!("复位副本偏移失败 {}", path.display()))?;
    match file_type {
        // owned 转换路径:多值文本按 v2.4 规范 `\0` 拼进单 frame(generic 直存会出多个
        // 同名 frame,不规范)。裸 ADTS(Aac)靠前置 ID3v2 获得 metadata 能力。
        FileType::Mpeg | FileType::Aac | FileType::Aiff | FileType::Wav => {
            Id3v2Tag::from(tag)
                .save_to(&mut file, WriteOptions::default())
                .wrap_err_with(|| format!("写 ID3v2 tag 失败 {}", path.display()))?;
        }
        // Vorbis 系天然多值(每个 item 一行 comment),generic 直存即可。
        FileType::Flac | FileType::Vorbis | FileType::Opus | FileType::Speex => {
            tag.save_to(&mut file, WriteOptions::default())
                .wrap_err_with(|| format!("写 Vorbis tag 失败 {}", path.display()))?;
        }
        FileType::Mp4 => {
            let mut ilst = Ilst::from(tag);
            collapse_multi_value(&mut ilst, *b"\xA9ART", &tags.artists);
            collapse_multi_value(&mut ilst, *b"aART", &tags.album_artists);
            ilst.save_to(&mut file, WriteOptions::default())
                .wrap_err_with(|| format!("写 MP4 ilst 失败 {}", path.display()))?;
        }
        _ => return Ok(WriteOutcome::SkippedUnsupported),
    }
    file.sync_data()
        .wrap_err_with(|| format!("fsync 副本失败 {}", path.display()))?;
    Ok(WriteOutcome::Tagged)
}

/// 把 [`SongTags`] 填进 generic [`Tag`](多值字段先清后整组 push,重打 tag 不重复累积)。
///
/// # Params:
///   - `tag`: 目标 tag(可能是文件里读出的既有 tag)
///   - `tags`: 要写的字段;`None` / 空 vec 的字段保持既有值不动
///   - `file_type`: 已探测的容器(决定封面 mime 限制)
fn fill_tag(tag: &mut Tag, tags: &SongTags, file_type: FileType) {
    if let Some(title) = &tags.title {
        tag.set_title(title.clone());
    }
    if let Some(album) = &tags.album {
        tag.set_album(album.clone());
    }
    if !tags.artists.is_empty() {
        tag.remove_key(&ItemKey::TrackArtist);
        for name in &tags.artists {
            tag.push(TagItem::new(
                ItemKey::TrackArtist,
                ItemValue::Text(name.clone()),
            ));
        }
    }
    if !tags.album_artists.is_empty() {
        tag.remove_key(&ItemKey::AlbumArtist);
        for name in &tags.album_artists {
            tag.push(TagItem::new(
                ItemKey::AlbumArtist,
                ItemValue::Text(name.clone()),
            ));
        }
    }
    if let Some(year) = tags.year {
        tag.set_year(year);
    }
    if let Some(label) = &tags.label {
        tag.insert_text(ItemKey::Label, label.clone());
    }
    if let Some(lrc) = &tags.lyrics_lrc {
        tag.insert_text(ItemKey::Lyrics, lrc.clone());
    }
    if let Some(bytes) = &tags.cover {
        let mime = sniff_mime(bytes);
        // MP4 covr 写侧只接受 Gif/Jpeg/Png/Bmp(lofty 硬校验,webp 等直接 FileEncoding
        // 失败掀掉整个写入);其余容器没这限制。宁缺封面,不丢整单 tag。
        let cover_ok = file_type != FileType::Mp4
            || matches!(
                mime,
                MimeType::Jpeg | MimeType::Png | MimeType::Gif | MimeType::Bmp
            );
        if cover_ok {
            tag.remove_picture_type(PictureType::CoverFront);
            tag.push_picture(Picture::new_unchecked(
                PictureType::CoverFront,
                Some(mime),
                None,
                bytes.clone(),
            ));
        } else {
            mineral_log::warn!(target: "tagging", "MP4 不支持的封面格式,跳过封面(其余 tag 照写)");
        }
    }
}

/// MP4 多值收敛:generic → ilst 的 merge 会把每个文本 item 落成**独立**同名 atom
/// (不规范,多数播放器只读首个);收敛成单 atom + 多 data。
///
/// # Params:
///   - `ilst`: 转换产物
///   - `ident`: atom 四字节标识(如 `©ART` / `aART`)
///   - `values`: 该字段的全部值(0 = 没写、1 = merge 产物已规范,都不动)
fn collapse_multi_value(ilst: &mut Ilst, ident: [u8; 4], values: &[String]) {
    if values.len() <= 1 {
        return;
    }
    let data = values
        .iter()
        .map(|v| AtomData::UTF8(v.clone()))
        .collect::<Vec<_>>();
    if let Some(atom) = Atom::from_collection(AtomIdent::Fourcc(ident), data) {
        ilst.replace_atom(atom);
    }
}

/// 探测文件是否已带当前版本打标水印(回填增量跳过的判据)。
///
/// # Params:
///   - `path`: 音频文件路径
///
/// # Return:
///   `true` = 已带当前版本水印(跳过);打不开 / 探测失败 / 无水印 / 旧版本均 `false`。
pub(crate) fn has_watermark(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(probed) = Probe::new(&mut file).guess_file_type() else {
        return false;
    };
    let Ok(tagged) = probed.read() else {
        return false;
    };
    tagged
        .primary_tag()
        .and_then(|t| t.get_string(&ItemKey::EncodedBy).map(str::to_owned))
        .as_deref()
        == Some(TAG_WATERMARK)
}

/// 按 magic bytes sniff 图片 mime(不信任 URL 后缀)。
///
/// # Params:
///   - `bytes`: 图片字节
///
/// # Return:
///   识别出的 [`MimeType`];认不出的给 `Unknown`。
fn sniff_mime(bytes: &[u8]) -> MimeType {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        MimeType::Jpeg
    } else if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        MimeType::Png
    } else if bytes.starts_with(b"GIF8") {
        MimeType::Gif
    } else if bytes.starts_with(b"BM") {
        MimeType::Bmp
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        MimeType::Unknown("image/webp".to_owned())
    } else {
        MimeType::Unknown("application/octet-stream".to_owned())
    }
}

/// 删除临时副本(尽力而为;删除失败只留孤儿 `.part-tag`,不影响正确性)。
///
/// # Params:
///   - `tmp`: 临时副本路径
fn cleanup(tmp: &Path) {
    if let Err(e) = std::fs::remove_file(tmp) {
        mineral_log::warn!(target: "tagging", error = mineral_log::chain(&e), path = %tmp.display(), "清理临时副本失败");
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use super::*;

    /// 1s 440Hz 正弦 fixture(ffmpeg 生成,无既有 tag;生成命令见 fixtures/README.md)。
    const MP3: &[u8] = include_bytes!("fixtures/tone.mp3");
    /// flac 容器 fixture。
    const FLAC: &[u8] = include_bytes!("fixtures/tone.flac");
    /// m4a(MP4 ilst)容器 fixture。
    const M4A: &[u8] = include_bytes!("fixtures/tone.m4a");
    /// 裸 ADTS fixture(前置 ID3v2 获得 metadata 能力)。
    const ADTS: &[u8] = include_bytes!("fixtures/tone.aac");

    /// 1x1 红 PNG。
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0xDA, 0x63, 0xF8,
        0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x05, 0x01, 0xFF, 0xA9, 0x99, 0x81, 0x69, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    /// 全字段 tag 集合(双艺人 + 双专辑艺人,覆盖多值路径)。
    fn full_tags() -> SongTags {
        SongTags {
            title: Some("晴天".to_owned()),
            artists: vec!["周杰伦".to_owned(), "第二艺人".to_owned()],
            album: Some("叶惠美".to_owned()),
            album_artists: vec!["周杰伦".to_owned(), "专辑合艺人".to_owned()],
            year: Some(2003),
            label: Some("杰威尔".to_owned()),
            lyrics_lrc: Some("[00:01.00]第一句\n[00:05.00]第二句\n".to_owned()),
            cover: Some(PNG.to_vec()),
        }
    }

    /// 把 fixture 字节落进临时目录,返回 (tempdir, 文件路径)。
    fn stage(
        bytes: &[u8],
        name: &str,
    ) -> color_eyre::Result<(tempfile::TempDir, std::path::PathBuf)> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(name);
        std::fs::write(&path, bytes)?;
        Ok((dir, path))
    }

    /// 读回指定文件的 primary tag,断言除多值艺人外的全字段与 [`full_tags`] 一致
    /// (多值艺人的表示按容器不同,由各容器测试单独断言)。
    ///
    /// # Params:
    ///   - `path`: 打过 tag 的文件
    ///   - `context`: 断言失败时的场景说明(容器名)
    fn assert_full_tags(path: &Path, context: &str) -> color_eyre::Result<()> {
        let mut file = std::fs::File::open(path)?;
        let tagged = Probe::new(&mut file).guess_file_type()?.read()?;
        let tag = tagged
            .primary_tag()
            .ok_or_else(|| color_eyre::eyre::eyre!("{context}: 应有 primary tag"))?;
        assert_eq!(tag.title().as_deref(), Some("晴天"), "{context} title");
        assert_eq!(
            tag.get_string(&ItemKey::AlbumTitle).map(str::to_owned),
            Some("叶惠美".to_owned()),
            "{context} album"
        );
        assert_eq!(tag.year(), Some(2003), "{context} year");
        assert_eq!(
            tag.get_string(&ItemKey::Label).map(str::to_owned),
            Some("杰威尔".to_owned()),
            "{context} label"
        );
        let lyrics = tag.get_string(&ItemKey::Lyrics).map(str::to_owned);
        assert!(
            lyrics.as_deref().is_some_and(|l| l.contains("第二句")),
            "{context} lyrics 应含 lrc 文本,实际: {lyrics:?}"
        );
        // MP4 covr 没有 pic_type 概念(lofty 写侧强制改成 Other),按字节找而非按类型找。
        let cover_ok = tag.pictures().iter().any(|p| p.data() == PNG);
        assert!(cover_ok, "{context} 封面字节应一致");
        Ok(())
    }

    /// 多值艺人的通用读回断言(ID3v2 `\0` 拼帧 / Vorbis 多行,lofty 读侧都能全部翻出)。
    ///
    /// # Params:
    ///   - `path`: 打过 tag 的文件
    ///   - `context`: 断言失败时的场景说明(容器名)
    fn assert_multi_value_generic(path: &Path, context: &str) -> color_eyre::Result<()> {
        let mut file = std::fs::File::open(path)?;
        let tagged = Probe::new(&mut file).guess_file_type()?.read()?;
        let tag = tagged
            .primary_tag()
            .ok_or_else(|| color_eyre::eyre::eyre!("{context}: 应有 primary tag"))?;
        let artists = tag.get_strings(&ItemKey::TrackArtist).collect::<Vec<_>>();
        assert!(
            artists.contains(&"周杰伦") && artists.contains(&"第二艺人"),
            "{context} 多值艺人应都能读回,实际: {artists:?}"
        );
        let album_artists = tag.get_strings(&ItemKey::AlbumArtist).collect::<Vec<_>>();
        assert!(
            album_artists.contains(&"专辑合艺人"),
            "{context} 多值专辑艺人应都能读回,实际: {album_artists:?}"
        );
        Ok(())
    }

    /// MP4 多值艺人的 atom 级断言:单 atom + 多 data(from_collection 收敛产物)。
    /// lofty 读侧把 ilst 翻成 generic Tag 时多 data atom 只取首值,必须落到 atom 层看。
    ///
    /// # Params:
    ///   - `path`: 打过 tag 的 m4a 文件
    fn assert_multi_value_mp4(path: &Path) -> color_eyre::Result<()> {
        use lofty::file::AudioFile as _;
        use lofty::mp4::{AtomData, AtomIdent, Mp4File};

        let mut file = std::fs::File::open(path)?;
        let mp4 = Mp4File::read_from(&mut file, lofty::config::ParseOptions::new())?;
        let ilst = mp4
            .ilst()
            .ok_or_else(|| color_eyre::eyre::eyre!("m4a: 应有 ilst"))?;
        for (ident, expected) in [
            (*b"\xA9ART", vec!["周杰伦", "第二艺人"]),
            (*b"aART", vec!["周杰伦", "专辑合艺人"]),
        ] {
            let atom = ilst
                .get(&AtomIdent::Fourcc(ident))
                .ok_or_else(|| color_eyre::eyre::eyre!("m4a: 缺 atom {ident:?}"))?;
            let values = atom
                .data()
                .filter_map(|d| match d {
                    AtomData::UTF8(s) => Some(s.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(values, expected, "m4a: atom {ident:?} 应单 atom 多 data");
        }
        Ok(())
    }

    #[test]
    fn roundtrip_mp3() -> color_eyre::Result<()> {
        let (_dir, path) = stage(MP3, "tone.mp3")?;
        assert_eq!(
            write_tags(&path, &full_tags(), /*watermark*/ true)?,
            WriteOutcome::Tagged
        );
        assert_full_tags(&path, "mp3")?;
        assert_multi_value_generic(&path, "mp3")
    }

    #[test]
    fn roundtrip_flac() -> color_eyre::Result<()> {
        let (_dir, path) = stage(FLAC, "tone.flac")?;
        assert_eq!(
            write_tags(&path, &full_tags(), /*watermark*/ true)?,
            WriteOutcome::Tagged
        );
        assert_full_tags(&path, "flac")?;
        assert_multi_value_generic(&path, "flac")
    }

    #[test]
    fn roundtrip_m4a() -> color_eyre::Result<()> {
        let (_dir, path) = stage(M4A, "tone.m4a")?;
        assert_eq!(
            write_tags(&path, &full_tags(), /*watermark*/ true)?,
            WriteOutcome::Tagged
        );
        assert_full_tags(&path, "m4a")?;
        assert_multi_value_mp4(&path)
    }

    /// 裸 ADTS 不是「无 metadata 容器」:lofty 前置 ID3v2 即可写,读回全字段。
    #[test]
    fn roundtrip_adts_via_prepended_id3v2() -> color_eyre::Result<()> {
        let (_dir, path) = stage(ADTS, "tone.aac")?;
        assert_eq!(
            write_tags(&path, &full_tags(), /*watermark*/ true)?,
            WriteOutcome::Tagged
        );
        assert_full_tags(&path, "adts")?;
        assert_multi_value_generic(&path, "adts")
    }

    /// 原子替换语义:读者在打 tag 前持有 fd,替换后旧 fd 读到的是**原始内容**,
    /// 新路径读到打好 tag 的文件(播放中的缓存文件打 tag 不破坏播放)。
    #[test]
    fn replace_is_atomic_for_existing_readers() -> color_eyre::Result<()> {
        let (_dir, path) = stage(MP3, "tone.mp3")?;
        let mut old_reader = std::fs::File::open(&path)?;
        assert_eq!(
            write_tags(&path, &full_tags(), /*watermark*/ true)?,
            WriteOutcome::Tagged
        );
        let mut old_bytes = Vec::new();
        old_reader.read_to_end(&mut old_bytes)?;
        assert_eq!(old_bytes, MP3, "旧 fd 应读到原始未打 tag 内容");
        assert_full_tags(&path, "atomic-mp3")
    }

    /// 不可探测内容(垃圾字节)→ SkippedUnsupported,原文件不动、无残留副本。
    #[test]
    fn garbage_is_skipped_untouched() -> color_eyre::Result<()> {
        let (_dir, path) = stage(b"NOT-AUDIO-GARBAGE", "garbage.bin")?;
        assert_eq!(
            write_tags(&path, &full_tags(), /*watermark*/ true)?,
            WriteOutcome::SkippedUnsupported
        );
        assert_eq!(std::fs::read(&path)?, b"NOT-AUDIO-GARBAGE", "原文件应不变");
        assert!(
            !path.with_extension("part-tag").exists(),
            "跳过后应无残留副本"
        );
        Ok(())
    }

    /// 水印:`watermark: true` 写入后 `has_watermark` 命中(回填将跳过);`false` 不写。
    #[test]
    fn watermark_marks_tagged_files() -> color_eyre::Result<()> {
        let (_dir, path) = stage(MP3, "tone.mp3")?;
        assert!(!has_watermark(&path), "新文件应无水印");
        assert_eq!(
            write_tags(&path, &full_tags(), /*watermark*/ false)?,
            WriteOutcome::Tagged
        );
        assert!(!has_watermark(&path), "watermark: false 不应留水印");
        assert_eq!(
            write_tags(&path, &full_tags(), /*watermark*/ true)?,
            WriteOutcome::Tagged
        );
        assert!(has_watermark(&path), "写入后应带当前版本水印");
        Ok(())
    }

    /// 目录只读 → copy 即失败,原文件字节不变、无残留副本(失败不留半成品)。
    #[cfg(unix)]
    #[test]
    fn readonly_dir_fails_clean() -> color_eyre::Result<()> {
        use std::os::unix::fs::PermissionsExt as _;

        let (_dir, path) = stage(MP3, "tone.mp3")?;
        let parent = path
            .parent()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有父目录"))?
            .to_path_buf();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555))?;
        let result = write_tags(&path, &full_tags(), /*watermark*/ true);
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755))?;
        assert!(result.is_err(), "只读目录应报错");
        assert_eq!(std::fs::read(&path)?, MP3, "原文件应不变");
        assert!(
            !path.with_extension("part-tag").exists(),
            "失败后应无残留副本"
        );
        Ok(())
    }

    /// webp 封面写 MP4:封面被跳过但其余 tag 照写(lofty covr 只收 Gif/Jpeg/Png/Bmp)。
    #[test]
    fn mp4_skips_webp_cover_but_writes_rest() -> color_eyre::Result<()> {
        let mut tags = full_tags();
        // 合法 webp 头(RIFF....WEBP;内容无所谓,写引擎不解析图片)。
        tags.cover = Some(b"RIFF\x1A\x00\x00\x00WEBPVP8 ".to_vec());
        let (_dir, path) = stage(M4A, "tone.m4a")?;
        assert_eq!(
            write_tags(&path, &tags, /*watermark*/ true)?,
            WriteOutcome::Tagged
        );
        let mut file = std::fs::File::open(&path)?;
        let tagged = Probe::new(&mut file).guess_file_type()?.read()?;
        let tag = tagged
            .primary_tag()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有 primary tag"))?;
        assert_eq!(tag.title().as_deref(), Some("晴天"), "其余字段应照写");
        assert!(
            tag.get_picture_type(PictureType::CoverFront).is_none(),
            "webp 封面应被跳过"
        );
        Ok(())
    }

    #[test]
    fn sniff_mime_magic_bytes() {
        assert!(matches!(
            sniff_mime(&[0xFF, 0xD8, 0xFF, 0xE0]),
            MimeType::Jpeg
        ));
        assert!(matches!(sniff_mime(PNG), MimeType::Png));
        assert!(matches!(sniff_mime(b"GIF89a..."), MimeType::Gif));
        assert!(matches!(sniff_mime(b"BM...."), MimeType::Bmp));
        assert!(
            matches!(
                sniff_mime(b"RIFF\x00\x00\x00\x00WEBP...."),
                MimeType::Unknown(_)
            ),
            "webp 应识别为 Unknown(用于 MP4 封面门)"
        );
    }
}
