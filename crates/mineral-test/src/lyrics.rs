//! 真实歌词 fixture:MyGO!!!!!《潜在表明》(专辑《迷跡波》),覆盖原文 / 逐字(YRC)/
//! 翻译 / 罗马音四种数据齐全的复杂场景,数据取自实际播放时的 MPRIS metadata。
//!
//! 四份原始数据各存一个 data 文件(`include_str!` 编译期嵌入):行级走
//! [`LrcLyric::parse`](mineral_model::LrcLyric::parse);逐字是 MPRIS 导出的
//! `{start,duration,text}` JSON,本模块用 [`WordDto`] / [`LineDto`] 反序列化后映射成
//! [`WordLine`](mineral_model::WordLine)。

use mineral_model::{LrcLyric, Lyrics, Song, Word, WordLine, WordLyric};
use serde::Deserialize;

use crate::builders::{song, with_artist, with_duration};

/// MPRIS 逐字 JSON 里的一个字单元(键名 `start` / `duration` / `text`,毫秒)。
#[derive(Deserialize)]
struct WordDto {
    /// 字起始(绝对毫秒)。
    start: u64,

    /// 字时长(毫秒)。
    duration: u64,

    /// 字面文本(原样保留前后空格)。
    text: String,
}

/// MPRIS 逐字 JSON 里的一行(行起始 + 字单元序列;行时长由字单元推出)。
#[derive(Deserialize)]
struct LineDto {
    /// 行起始(绝对毫秒)。
    start: u64,

    /// 该行的字单元序列。
    words: Vec<WordDto>,
}

/// 把 MPRIS 导出的逐字 JSON 解析成 [`WordLyric`];行时长 = 末字结束 − 行起始。
/// 解析失败(数据损坏)时返回空,交由消费方的快照测试暴露。
fn parse_mpris_words(json: &str) -> WordLyric {
    let lines = serde_json::from_str::<Vec<LineDto>>(json).unwrap_or_default();
    let word_lines = lines
        .into_iter()
        .map(|line| {
            let dur_ms = line
                .words
                .last()
                .map_or(line.start, |w| w.start.saturating_add(w.duration))
                .saturating_sub(line.start);
            let words = line
                .words
                .into_iter()
                .map(|w| Word {
                    start_ms: w.start,
                    dur_ms: w.duration,
                    text: w.text,
                })
                .collect();
            WordLine {
                start_ms: line.start,
                dur_ms,
                words,
            }
        })
        .collect::<Vec<WordLine>>();
    WordLyric::from(word_lines)
}

/// 《潜在表明》的完整歌词:原文 LRC + 逐字 + 行级翻译 + 行级罗马音,四者齐全。
///
/// 各行时间戳精确对齐(原文 0.1s 量级偏差源自真实数据,与翻译 / 罗马音不完全相等,
/// 正好覆盖「按 `current_index` 时间对齐而非索引硬配对」的渲染路径)。
///
/// # Return:
///   数据齐全的 [`Lyrics`]。
pub fn qianzai_lyrics() -> Lyrics {
    Lyrics {
        lrc: LrcLyric::parse(include_str!("../data/qianzai/original.lrc")),
        words: parse_mpris_words(include_str!("../data/qianzai/words.json")),
        translation: LrcLyric::parse(include_str!("../data/qianzai/translation.lrc")),
        romanization: LrcLyric::parse(include_str!("../data/qianzai/romanization.lrc")),
    }
}

/// 《潜在表明》对应的 [`Song`](MyGO!!!!! / 专辑《迷跡波》/ 262s),与
/// [`qianzai_lyrics`] 配对用(歌词缓存以 `SongId` 为键)。
///
/// # Return:
///   来源 Netease、id `qianzai` 的 `Song`。
pub fn qianzai_song() -> Song {
    let mut s = with_artist(with_duration(song("qianzai"), 262_120), "MyGO!!!!!");
    s.name = "潜在表明".to_owned();
    s
}

/// Chinese Football《飞鱼转身》的歌词:**只有原文 LRC + 逐字,无翻译 / 无罗马音**。
/// 覆盖「无副歌词可切换」的场景(歌词面板据此不显示 `t` 提示)。
///
/// # Return:
///   `translation` / `romanization` 均为空的 [`Lyrics`]。
pub fn feiyu_lyrics() -> Lyrics {
    Lyrics {
        lrc: LrcLyric::parse(include_str!("../data/feiyu/original.lrc")),
        words: parse_mpris_words(include_str!("../data/feiyu/words.json")),
        translation: LrcLyric::default(),
        romanization: LrcLyric::default(),
    }
}

/// 《飞鱼转身》对应的 [`Song`](Chinese Football / 384s),与 [`feiyu_lyrics`] 配对用。
///
/// # Return:
///   来源 Netease、id `feiyu` 的 `Song`。
pub fn feiyu_song() -> Song {
    let mut s = with_artist(with_duration(song("feiyu"), 384_435), "Chinese Football");
    s.name = "飞鱼转身".to_owned();
    s
}
