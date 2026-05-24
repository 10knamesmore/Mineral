//! 歌词的结构化类型:行级 LRC 与逐字(word-level)。

use std::ops::Deref;

use serde::{Deserialize, Serialize};

/// 一行行级 LRC 歌词:时间戳 + 文本。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LrcLine {
    /// 行起始时间(绝对毫秒)。
    pub time_ms: u64,

    /// 行文本。
    pub text: String,
}

/// 一个逐字渲染单元:可能是一个汉字、一个音节,或一个英文单词。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Word {
    /// 起始时间(绝对毫秒)。
    pub start_ms: u64,

    /// 时长(毫秒)。
    pub dur_ms: u64,

    /// 字面文本(原样保留前后空格,渲染时直接拼)。
    pub text: String,
}

/// 一行逐字歌词:行级时间戳 + 字单元序列。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WordLine {
    /// 行起始时间(绝对毫秒)。
    pub start_ms: u64,

    /// 行时长(毫秒)。
    pub dur_ms: u64,

    /// 该行的字单元序列(按时间升序)。
    pub words: Vec<Word>,
}

/// 行级歌词:按时间升序的 [`LrcLine`] 序列。
///
/// channel 层用 [`LrcLyric::parse`](crate::lyrics::LrcLyric::parse) 完成「反序列化 +
/// 清洗」;[`to_lrc_string`](crate::lyrics::LrcLyric::to_lrc_string) 再序列化成标准
/// LRC(给外部系统)。`#[serde(transparent)]` 让 wire 上就是一个普通数组。
/// 解引用为 `[LrcLine]`,可直接 `iter` / `is_empty` / `get`。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LrcLyric(pub(crate) Vec<LrcLine>);

impl Deref for LrcLyric {
    type Target = [LrcLine];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// 逐字歌词:按时间升序的 [`WordLine`] 序列。
///
/// 各 channel 把自己的私有逐字格式解析成 `Vec<WordLine>` 后 `into()` 即可(要求**已按
/// `start_ms` 升序**)。解引用为 `[WordLine]`,可直接 `iter` / `is_empty` / `get`。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WordLyric(pub(crate) Vec<WordLine>);

impl Deref for WordLyric {
    type Target = [WordLine];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Vec<WordLine>> for WordLyric {
    /// 包装一组**已按 `start_ms` 升序**的逐字行。
    fn from(lines: Vec<WordLine>) -> Self {
        Self(lines)
    }
}

/// 一首歌的歌词集合。
///
/// 所有字段在 channel 层已完成「反序列化 + 清洗」:内部完全结构化、协议无关,
/// 消费方直接渲染;要喂外部系统(如 MPRIS)时再序列化成对应协议。拿不到的格式为空,
/// 渲染时按 `words → lrc → 空` 的优先级降级。翻译 / 罗马音本就是行级,`lrc` 与 `words`
/// 共用同一份,不再按逐字单列。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lyrics {
    /// 行级 LRC 原文。
    pub lrc: LrcLyric,

    /// 逐字原文(非空时渲染优先于 `lrc`)。
    pub words: WordLyric,

    /// 行级翻译(`lrc` 与 `words` 共用)。
    pub translation: LrcLyric,

    /// 行级罗马音(`lrc` 与 `words` 共用)。
    pub romanization: LrcLyric,
}
