//! 真实歌词 fixture:MyGO!!!!!《潜在表明》(专辑《迷跡波》),覆盖原文 / 逐字(YRC)/
//! 翻译 / 罗马音四种数据齐全的复杂场景,数据取自实际播放时的 MPRIS metadata。
//!
//! 原文走逐字 MPRIS 导出(`{start,duration,text}` JSON),反序列化后映射成逐字
//! [`LyricLine`](mineral_model::LyricLine)(`kind = Words`);翻译 / 罗马音是行级 LRC,
//! 走 [`parse_lrc`](mineral_model::parse_lrc)。

use mineral_model::{LineKind, LyricLine, Lyrics, Song, Word};
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

/// 把 MPRIS 导出的逐字 JSON 解析成逐字 [`LyricLine`] 序列;行时长 = 末字结束 − 行起始。
/// 解析失败(数据损坏)时返回空,交由消费方的快照测试暴露。
fn parse_mpris_words(json: &str) -> Vec<LyricLine> {
    serde_json::from_str::<Vec<LineDto>>(json)
        .unwrap_or_default()
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
                .collect::<Vec<Word>>();
            LyricLine {
                time_ms: Some(line.start),
                kind: LineKind::Words { dur_ms, words },
                translation: None,
                romanization: None,
            }
        })
        .collect()
}

/// 《潜在表明》的完整歌词:逐字原文 + 行级翻译 + 行级罗马音,三者齐全。
///
/// 副轨时间戳与原文有 0.1s 量级偏差(源自真实数据),正好覆盖装配期「按时间互最近邻
/// 配对而非索引硬配对」的路径。
///
/// # Return:
///   数据齐全的 [`Lyrics`]。
pub fn qianzai_lyrics() -> Lyrics {
    Lyrics::assemble(
        parse_mpris_words(include_str!("../data/qianzai/words.json")),
        &mineral_model::parse_lrc(include_str!("../data/qianzai/translation.lrc")),
        &mineral_model::parse_lrc(include_str!("../data/qianzai/romanization.lrc")),
    )
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

/// Chinese Football《飞鱼转身》的歌词:**只有逐字原文,无翻译 / 无罗马音**。
/// 覆盖「无副歌词可切换」的场景(歌词面板据此不显示 `t` 提示)。
///
/// # Return:
///   全行 `translation` / `romanization` 均为 `None` 的 [`Lyrics`]。
pub fn feiyu_lyrics() -> Lyrics {
    Lyrics::assemble(
        parse_mpris_words(include_str!("../data/feiyu/words.json")),
        /*translation*/ &[],
        /*romanization*/ &[],
    )
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
