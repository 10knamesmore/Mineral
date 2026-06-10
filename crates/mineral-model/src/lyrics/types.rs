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

/// 一行歌词:可选行级时间戳 + 内容 + 已配对的副轨文本。
///
/// 三种时间态由两字段正交表达:`time_ms` 区分有无行级时间戳(`None` = 纯文本 / 前奏白 /
/// 尾注 / 无 `t` 的 credits);`kind` 区分仅整行文本([`Plain`](LineKind::Plain))还是有
/// 字级时间轴([`Words`](LineKind::Words),yrc)。
///
/// 翻译 / 罗马音在 [`Lyrics::assemble`] 装配时按时间配对进行内,副轨缺行(credits /
/// 未翻句)即 `None`——消费方直接读字段,不再自行对齐。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LyricLine {
    /// 行起始时间(绝对毫秒);`None` = 该行无时间戳。
    pub time_ms: Option<u64>,

    /// 行内容。
    pub kind: LineKind,

    /// 本行的行级翻译;`None` = 翻译轨没有与本行对应的行。
    pub translation: Option<String>,

    /// 本行的行级罗马音;`None` = 罗马音轨没有与本行对应的行。
    pub romanization: Option<String>,
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
            translation: None,
            romanization: None,
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
            translation: None,
            romanization: None,
        }
    }
}

/// 一首歌的歌词:单一有序行序列,翻译 / 罗马音已按时间配对内嵌在各行上。
///
/// channel 层已完成「反序列化 + 清洗 + 配对」([`Lyrics::assemble`]):内部完全结构化、
/// 协议无关,消费方直接渲染;要喂外部系统(如 MPRIS)时再序列化成对应协议
/// ([`Self::translation_lines`] / [`Self::romanization_lines`] 重建独立副轨)。
/// 一条流里行级 / 逐字 / 有时间 / 无时间混排。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lyrics {
    /// 行序列:行级 / 逐字 / 有时间 / 无时间混在一条流里,副轨文本内嵌在行上。
    pub lines: Vec<LyricLine>,
}

impl Lyrics {
    /// 把原文与独立解析的翻译 / 罗马音轨装配成配对完成的歌词。
    ///
    /// 配对按**互最近邻**:原文行找时间最近的副轨行,且该副轨行时间最近的原文行也是
    /// 它,才算配上。单向最近邻会"永远给个答案"——副轨缺行(credits / 未翻句)时把
    /// 邻句的副轨文本错借过来(网易 credits 行在 0~2s、翻译轨首行是十几秒外的正文,
    /// 单向最近就把正文首句翻译配给了每个 credits 行)。副轨时间戳与原文有 ~0.1s
    /// 量级抖动且方向不定、行间隔为秒级,互最近性对此天然稳健,无需容差常数。
    ///
    /// # Params:
    ///   - `original`: 原文行序列(行级 / 逐字混排,直接成为 `lines`)
    ///   - `translation`: 行级翻译轨(独立时间戳,配对后即弃)
    ///   - `romanization`: 行级罗马音轨(同上)
    ///
    /// # Return:
    ///   副轨已内嵌的 [`Lyrics`]。
    pub fn assemble(
        original: Vec<LyricLine>,
        translation: &[LyricLine],
        romanization: &[LyricLine],
    ) -> Self {
        let mut lines = original;
        pair_secondary(&mut lines, translation, |line, text| {
            line.translation = Some(text);
        });
        pair_secondary(&mut lines, romanization, |line, text| {
            line.romanization = Some(text);
        });
        Self { lines }
    }

    /// 是否任一行配有翻译(决定 UI 的副歌词切换提示)。
    pub fn has_translation(&self) -> bool {
        self.lines.iter().any(|l| l.translation.is_some())
    }

    /// 是否任一行配有罗马音。
    pub fn has_romanization(&self) -> bool {
        self.lines.iter().any(|l| l.romanization.is_some())
    }

    /// 重建独立翻译轨(MPRIS 等外部协议导出用):取配有翻译的行,时间戳用原文行的——
    /// 与原文轨严格对齐(原始副轨的抖动时间戳在装配时已弃)。
    ///
    /// # Return:
    ///   行级 [`LyricLine`] 序列,无翻译时为空。
    pub fn translation_lines(&self) -> Vec<LyricLine> {
        rebuild_track(&self.lines, |l| l.translation.as_ref())
    }

    /// 重建独立罗马音轨(同 [`Self::translation_lines`])。
    ///
    /// # Return:
    ///   行级 [`LyricLine`] 序列,无罗马音时为空。
    pub fn romanization_lines(&self) -> Vec<LyricLine> {
        rebuild_track(&self.lines, |l| l.romanization.as_ref())
    }
}

/// 把一条副轨按互最近邻配对到原文行上,配上的行经 `set` 写入文本。
///
/// 只有带时间戳的行参与对齐;副轨文本为空的行不配(空行无展示意义)。
fn pair_secondary(
    lines: &mut [LyricLine],
    track: &[LyricLine],
    mut set: impl FnMut(&mut LyricLine, String),
) {
    let timed = |seq: &[LyricLine]| {
        seq.iter()
            .enumerate()
            .filter_map(|(i, l)| l.time_ms.map(|t| (i, t)))
            .collect::<Vec<(usize, u64)>>()
    };
    let orig_times = timed(lines);
    let track_times = timed(track);
    for &(i, t) in &orig_times {
        let Some(&(j, tj)) = nearest(&track_times, t) else {
            continue;
        };
        let Some(&(back, _)) = nearest(&orig_times, tj) else {
            continue;
        };
        if back != i {
            continue;
        }
        let Some(text) = track.get(j).map(|l| l.kind.text()) else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        if let Some(line) = lines.get_mut(i) {
            set(line, text.into_owned());
        }
    }
}

/// 在 `(index, time_ms)` 表里找时间与 `t` 最接近的项;空表返回 `None`。
fn nearest(items: &[(usize, u64)], t: u64) -> Option<&(usize, u64)> {
    items.iter().min_by_key(|(_, ts)| ts.abs_diff(t))
}

/// 从合并行序列重建一条独立副轨:取 `pick` 命中的带时间戳行,文本为副轨的、时间戳为
/// 原文行的。
fn rebuild_track(
    lines: &[LyricLine],
    pick: impl Fn(&LyricLine) -> Option<&String>,
) -> Vec<LyricLine> {
    lines
        .iter()
        .filter_map(|l| {
            let t = l.time_ms?;
            pick(l).map(|text| LyricLine::timed(t, text.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{LyricLine, Lyrics};

    /// 《迷星叫》真实形状:原文开头 3 行 credits(0/1/2s,翻译轨无对应行),正文从
    /// 16.64s 起。互最近邻下 credits 行不得借走正文首句翻译(单向最近邻的错配 bug)。
    #[test]
    fn assemble_credits_lines_get_no_translation() {
        let original = vec![
            LyricLine::timed(0, "作词 : 藤原優樹"),
            LyricLine::timed(1_000, "作曲 : 長谷川大介"),
            LyricLine::timed(2_000, "编曲 : 長谷川大介"),
            LyricLine::timed(16_640, "交差点の真ん中 急ぐ人に紛れて"),
            LyricLine::timed(21_540, "僕だけがあてもなく 漂うみたいだ"),
        ];
        let translation = vec![
            LyricLine::timed(16_640, "置身十字路口正中央 混入熙来熙往的人群"),
            LyricLine::timed(21_540, "唯独我漫无目的 有如漂流者一般"),
        ];
        let l = Lyrics::assemble(original, &translation, /*romanization*/ &[]);
        let trans = l
            .lines
            .iter()
            .map(|line| line.translation.as_deref())
            .collect::<Vec<Option<&str>>>();
        assert_eq!(
            trans,
            vec![
                None,
                None,
                None,
                Some("置身十字路口正中央 混入熙来熙往的人群"),
                Some("唯独我漫无目的 有如漂流者一般"),
            ]
        );
    }

    /// 副轨时间戳带 ~0.1s 量级抖动(方向不定)仍配对成功。
    #[test]
    fn assemble_pairs_despite_jitter() {
        let original = vec![
            LyricLine::timed(10_000, "一行目"),
            LyricLine::timed(14_000, "二行目"),
        ];
        let translation = vec![
            LyricLine::timed(10_120, "第一行"), // 偏晚 120ms
            LyricLine::timed(13_900, "第二行"), // 偏早 100ms
        ];
        let l = Lyrics::assemble(original, &translation, &[]);
        assert_eq!(
            l.lines.first().and_then(|x| x.translation.as_deref()),
            Some("第一行")
        );
        assert_eq!(
            l.lines.get(1).and_then(|x| x.translation.as_deref()),
            Some("第二行")
        );
    }

    /// 副轨缺中间行:对应原文行为 None,且不把邻句借过来。
    #[test]
    fn assemble_missing_middle_line_stays_none() {
        let original = vec![
            LyricLine::timed(10_000, "a"),
            LyricLine::timed(14_000, "b"),
            LyricLine::timed(18_000, "c"),
        ];
        let translation = vec![
            LyricLine::timed(10_000, "甲"),
            LyricLine::timed(18_000, "丙"),
        ];
        let l = Lyrics::assemble(original, &translation, &[]);
        let trans = l
            .lines
            .iter()
            .map(|line| line.translation.as_deref())
            .collect::<Vec<Option<&str>>>();
        assert_eq!(trans, vec![Some("甲"), None, Some("丙")]);
    }

    /// 翻译 / 罗马音双轨各自独立配对;无时间戳的原文行不参与对齐。
    #[test]
    fn assemble_both_tracks_and_untimed_lines() {
        let original = vec![
            LyricLine::untimed("无戳尾注"),
            LyricLine::timed(5_000, "歌詞"),
        ];
        let translation = vec![LyricLine::timed(5_050, "歌词")];
        let romanization = vec![LyricLine::timed(4_950, "ka shi")];
        let l = Lyrics::assemble(original, &translation, &romanization);
        let first = l.lines.first();
        assert_eq!(first.and_then(|x| x.translation.as_deref()), None);
        assert_eq!(first.and_then(|x| x.romanization.as_deref()), None);
        let second = l.lines.get(1);
        assert_eq!(second.and_then(|x| x.translation.as_deref()), Some("歌词"));
        assert_eq!(
            second.and_then(|x| x.romanization.as_deref()),
            Some("ka shi")
        );
        assert!(l.has_translation());
        assert!(l.has_romanization());
    }

    /// 空副轨 → 全 None;`has_*` 为 false。
    #[test]
    fn assemble_empty_tracks() {
        let l = Lyrics::assemble(vec![LyricLine::timed(1_000, "x")], &[], &[]);
        assert!(!l.has_translation());
        assert!(!l.has_romanization());
    }

    /// 重建副轨:时间戳取原文行(与原文严格对齐),未配对行不出现。
    #[test]
    fn rebuilt_track_uses_original_timestamps() {
        let original = vec![
            LyricLine::timed(0, "作词 : 某人"),
            LyricLine::timed(10_000, "歌詞"),
        ];
        let translation = vec![LyricLine::timed(10_080, "歌词")];
        let l = Lyrics::assemble(original, &translation, &[]);
        let rebuilt = l.translation_lines();
        assert_eq!(
            rebuilt,
            vec![LyricLine::timed(10_000, "歌词")],
            "时间戳为原文行的 10_000 而非副轨的 10_080;credits 行不出现"
        );
    }
}
