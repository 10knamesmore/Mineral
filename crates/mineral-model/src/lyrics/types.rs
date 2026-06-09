//! 歌词的结构化类型:单一有序行序列,每行可选时间戳 + 整行文本或逐字轴。

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

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

/// 一行歌词的内容:整行纯文本,或逐字时间轴。
///
/// 文本只存一份——逐字行的整行文本由 [`LineKind::text`] 按需拼接,不冗余存储。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineKind {
    /// 整行文本(行级有时间戳或无时间戳),无字级轴。
    Plain(String),

    /// 逐字时间轴(yrc):行时长 + 字单元序列。
    Words {
        /// 行时长(毫秒)。
        dur_ms: u64,

        /// 字单元序列(按 `start_ms` 升序)。
        words: Vec<Word>,
    },
}

impl Default for LineKind {
    /// 默认空文本行,给容器的 `#[derive(Default)]` 兜底。
    fn default() -> Self {
        Self::Plain(String::new())
    }
}

impl LineKind {
    /// 整行文本:[`Plain`](LineKind::Plain) 零拷贝借用;[`Words`](LineKind::Words)
    /// 拼接各 [`Word`] 文本(按需分配)。
    ///
    /// # Return:
    ///   整行文本,`Plain` 借用 / `Words` 拥有。
    pub fn text(&self) -> Cow<'_, str> {
        match self {
            Self::Plain(s) => Cow::Borrowed(s),
            Self::Words { words, .. } => {
                Cow::Owned(words.iter().map(|w| w.text.as_str()).collect())
            }
        }
    }

    /// 字单元序列;[`Plain`](LineKind::Plain) 返回空切片(判定"是否逐字行"用
    /// `.is_empty()`,wipe 渲染直接遍历)。
    ///
    /// # Return:
    ///   字单元切片,`Plain` 为空。
    pub fn words(&self) -> &[Word] {
        match self {
            Self::Plain(_) => &[],
            Self::Words { words, .. } => words,
        }
    }
}

/// 一行歌词:可选行级时间戳 + 内容。
///
/// 三种时间态由两字段正交表达:`time_ms` 区分有无行级时间戳(`None` = 纯文本 / 前奏白 /
/// 尾注 / 无 `t` 的 credits);`kind` 区分仅整行文本([`Plain`](LineKind::Plain))还是有
/// 字级时间轴([`Words`](LineKind::Words),yrc)。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LyricLine {
    /// 行起始时间(绝对毫秒);`None` = 该行无时间戳。
    pub time_ms: Option<u64>,

    /// 行内容。
    pub kind: LineKind,
}

impl LyricLine {
    /// 带时间戳的整行文本行。
    ///
    /// # Params:
    ///   - `time_ms`: 行起始(绝对毫秒)
    ///   - `text`: 行文本
    ///
    /// # Return:
    ///   `Plain` 行,`time_ms = Some`。
    pub fn timed(time_ms: u64, text: impl Into<String>) -> Self {
        Self {
            time_ms: Some(time_ms),
            kind: LineKind::Plain(text.into()),
        }
    }

    /// 无时间戳的整行文本行(纯文本 / credits / 前奏白)。
    ///
    /// # Params:
    ///   - `text`: 行文本
    ///
    /// # Return:
    ///   `Plain` 行,`time_ms = None`。
    pub fn untimed(text: impl Into<String>) -> Self {
        Self {
            time_ms: None,
            kind: LineKind::Plain(text.into()),
        }
    }
}

/// 一首歌的歌词:原文 + 翻译 + 罗马音,各是一条 [`LyricLine`] 有序序列。
///
/// channel 层已完成「反序列化 + 清洗」:内部完全结构化、协议无关,消费方直接渲染;要喂
/// 外部系统(如 MPRIS)时再序列化成对应协议。原文一条流里行级 / 逐字 / 有时间 / 无时间
/// 混排,翻译 / 罗马音实践中恒 [`Plain`](LineKind::Plain)。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lyrics {
    /// 原文:单一有序序列,行级 / 逐字 / 有时间 / 无时间混在一条流里。
    pub original: Vec<LyricLine>,

    /// 行级翻译。
    pub translation: Vec<LyricLine>,

    /// 行级罗马音。
    pub romanization: Vec<LyricLine>,
}
