//! 模糊搜索过滤器:fzf 风格子序列 + 中文拼音 / 首字母联合匹配。
//!
//! 给 `state.search.query` 做本地过滤用。每条候选文本(歌名 / 艺人 / 专辑 / 歌单名)
//! 预处理成 [`MatchableText`]:把原文、全拼、首字母三段拼成一个 char 数组喂给
//! nucleo,既覆盖了「输入 `cry` 命中『春日影』」「输入 `chunying` 命中『春日影』」
//! 这种中文场景,也保留了纯 ASCII 文本的 fzf 模糊匹配能力。
//!
//! 命中下标(nucleo 给的是 haystack 内的 char 下标)会再被反向映射回 `original`
//! 的 char 下标,渲染端按这些下标做汉字级高亮。

use std::sync::Arc;

use nucleo_matcher::Utf32Str;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};
use pinyin::ToPinyin;
use smallvec::SmallVec;

/// 一条预处理过的可匹配文本:原文 + 拼音 + 首字母 + 反向映射索引。
///
/// 用 [`MatchableText::new`] 构造一次,后续每次查询变化时只跑 [`FuzzyMatcher::score`],
/// 不再重算拼音。
pub struct MatchableText {
    /// 拼接好的 haystack:`<original>\0<pinyin>\0<initials>`。
    /// 直接以 char 数组形态存,匹配时 0 拷贝交给 nucleo 的 [`Utf32Str::Unicode`]。
    haystack: Box<[char]>,

    /// 三段在 `haystack` 中的字符下标边界(单位:char)。
    bounds: Bounds,

    /// 拼音段内第 i 个音节在 `haystack` 中的字符范围(已加 `bounds.pinyin_start`
    /// 偏移,可与 nucleo 返回的命中下标直接比较)。
    syllable_ranges: Box<[(u32, u32)]>,

    /// 第 i 个 Han 字符在 `original` 内的字符下标。
    /// 与 `syllable_ranges`、首字母段三者一一对齐 —— 索引 `i` 都指同一个 Han 字符。
    han_to_orig: Box<[u32]>,
}

/// 三段 haystack 各自的 char 下标边界。
#[derive(Clone, Copy)]
struct Bounds {
    /// 原文段的 `[0, orig_end)`。
    orig_end: u32,

    /// 拼音段 `[pinyin_start, pinyin_end)`。
    pinyin_start: u32,

    /// 拼音段右开端。
    pinyin_end: u32,

    /// 首字母段 `[initials_start, initials_end)`。
    initials_start: u32,

    /// 首字母段右开端。
    initials_end: u32,
}

impl MatchableText {
    /// 预处理 `text`,产出可复用的匹配载荷。
    ///
    /// # Params:
    ///   - `text`: 待处理的原文。非 Han 字符(空格 / Latin / 标点 / emoji)只出现
    ///     在原文段,不参与拼音 / 首字母段。
    ///
    /// # Return:
    ///   Arc 包裹的预处理结果。Arc 是为了让同一份文本在多个 view 间复用(同一首
    ///   歌的歌名可能出现在 library 与 queue 两处)。
    pub fn new(text: &str) -> Arc<Self> {
        let orig_chars: Vec<char> = text.chars().collect();

        // 逐字处理 Han:拿默认读音(plain,小写),记录音节起止 + 首字母 + 反向映射。
        let mut pinyin_chars: Vec<char> = Vec::new();
        let mut initials_chars: Vec<char> = Vec::new();
        let mut syllable_ranges_rel: Vec<(u32, u32)> = Vec::new();
        let mut han_to_orig_acc: Vec<u32> = Vec::new();

        for (idx, ch) in orig_chars.iter().enumerate() {
            let mut buf = [0u8; 4];
            let one_char_str: &str = ch.encode_utf8(&mut buf);
            // ToPinyin 对非 Han 字符返回 None,直接跳过。
            let Some(py) = one_char_str.to_pinyin().next().flatten() else {
                continue;
            };
            let plain: &'static str = py.plain();
            let Some(first) = plain.chars().next() else {
                continue;
            };
            let Ok(orig_idx_u32) = u32::try_from(idx) else {
                // 文本超过 2^32 char(不可能)。该字段不参与拼音/首字母,但
                // 不影响原文段匹配,跳过即可。
                continue;
            };

            initials_chars.push(first.to_ascii_lowercase());

            let Ok(start) = u32::try_from(pinyin_chars.len()) else {
                continue;
            };
            for c in plain.chars() {
                pinyin_chars.push(c.to_ascii_lowercase());
            }
            let Ok(end) = u32::try_from(pinyin_chars.len()) else {
                continue;
            };
            syllable_ranges_rel.push((start, end));
            han_to_orig_acc.push(orig_idx_u32);
        }

        // 拼 haystack 并算各段的绝对 char 下标边界。
        let mut haystack: Vec<char> =
            Vec::with_capacity(orig_chars.len() + pinyin_chars.len() + initials_chars.len() + 2);
        haystack.extend(orig_chars.iter().copied());
        let orig_end = u32::try_from(haystack.len()).unwrap_or(u32::MAX);

        haystack.push('\0');
        let pinyin_start = orig_end.saturating_add(1);
        haystack.extend(pinyin_chars.iter().copied());
        let pinyin_end = u32::try_from(haystack.len()).unwrap_or(u32::MAX);

        haystack.push('\0');
        let initials_start = pinyin_end.saturating_add(1);
        haystack.extend(initials_chars.iter().copied());
        let initials_end = u32::try_from(haystack.len()).unwrap_or(u32::MAX);

        // 把音节相对下标(pinyin 段内)平移成 haystack 绝对下标,与 nucleo 输出对齐。
        let syllable_ranges: Box<[(u32, u32)]> = syllable_ranges_rel
            .into_iter()
            .map(|(s, e)| {
                (
                    s.saturating_add(pinyin_start),
                    e.saturating_add(pinyin_start),
                )
            })
            .collect();

        Arc::new(Self {
            haystack: haystack.into_boxed_slice(),
            bounds: Bounds {
                orig_end,
                pinyin_start,
                pinyin_end,
                initials_start,
                initials_end,
            },
            syllable_ranges,
            han_to_orig: han_to_orig_acc.into_boxed_slice(),
        })
    }

    /// haystack 的 nucleo Utf32 视图(零拷贝)。
    fn haystack_view(&self) -> Utf32Str<'_> {
        Utf32Str::Unicode(&self.haystack)
    }

    /// 把 nucleo 给出的命中 char 下标(在拼接好的 haystack 内)反向映射回
    /// `original` 的 char 下标。
    ///
    /// - 原文段直接采纳。
    /// - 拼音段查 `syllable_ranges` 找到对应 Han 字符,再查 `han_to_orig`。
    /// - 首字母段一字符对一 Han,直接查 `han_to_orig`。
    ///
    /// 返回前已 sort + dedup,方便渲染端直接用。
    fn map_back(&self, hits: &[u32]) -> SmallVec<[u32; 8]> {
        let b = self.bounds;
        let mut out: SmallVec<[u32; 8]> = SmallVec::new();
        for &i in hits {
            if i < b.orig_end {
                out.push(i);
            } else if i >= b.pinyin_start && i < b.pinyin_end {
                let Some(syl_idx) = self
                    .syllable_ranges
                    .iter()
                    .position(|&(s, e)| i >= s && i < e)
                else {
                    continue;
                };
                if let Some(&orig) = self.han_to_orig.get(syl_idx) {
                    out.push(orig);
                }
            } else if i >= b.initials_start && i < b.initials_end {
                let Some(rel) = usize::try_from(i.saturating_sub(b.initials_start)).ok() else {
                    continue;
                };
                if let Some(&orig) = self.han_to_orig.get(rel) {
                    out.push(orig);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// 一次匹配的成功结果。
#[derive(Debug, Clone)]
pub struct Match {
    /// nucleo 排序分(越高越靠前)。
    pub score: u32,

    /// 已 sort + dedup 的 `original` 字符下标,供高亮渲染。
    pub hits: SmallVec<[u32; 8]>,
}

/// 模糊匹配器。持有 nucleo Matcher、当前 Pattern、indices buffer 等可变 buf,
/// 便于在多帧 / 多候选间复用分配。
pub struct FuzzyMatcher {
    /// nucleo 核心 matcher。
    matcher: Matcher,

    /// 当前 query 编译出的 Pattern;空 query 时 `atoms` 为空,[`Self::is_active`]
    /// 返回 false,调用方据此走 fast path。
    pattern: Pattern,

    /// 当前 pattern 对应的原始 query 字符串,用于判等避免每次重 parse。
    last_query: String,

    /// `Pattern::indices` 的复用 buffer。
    indices_buf: Vec<u32>,
}

impl FuzzyMatcher {
    /// 构造新匹配器。
    pub fn new() -> Self {
        Self {
            matcher: Matcher::new(Config::DEFAULT),
            // 用空串 parse 出空 Pattern;后续 `set_query` 通过 `reparse` 原地刷新。
            pattern: Pattern::parse("", CaseMatching::Ignore, Normalization::Smart),
            last_query: String::new(),
            indices_buf: Vec::new(),
        }
    }

    /// 设置 / 切换查询串。
    ///
    /// 与上次相同则什么也不做。空串 / 全空白 query 会让 [`Self::is_active`] 转 false,
    /// 调用方据此跳过整轮打分。
    pub fn set_query(&mut self, query: &str) {
        if query == self.last_query {
            return;
        }
        self.last_query.clear();
        self.last_query.push_str(query);
        self.pattern
            .reparse(query, CaseMatching::Ignore, Normalization::Smart);
    }

    /// 当前 query 是否非空。
    pub fn is_active(&self) -> bool {
        !self.pattern.atoms.is_empty()
    }

    /// 对一条预处理过的文本打分 + 拿命中下标。
    ///
    /// # Return:
    ///   - `Some(Match)`:命中。`hits` 已映射回 `original` 的 char 下标,sort + dedup。
    ///   - `None`:未命中,或当前 query 为空(调用方应自行 short-circuit)。
    pub fn score(&mut self, text: &MatchableText) -> Option<Match> {
        if !self.is_active() {
            return None;
        }
        self.indices_buf.clear();
        let score = self.pattern.indices(
            text.haystack_view(),
            &mut self.matcher,
            &mut self.indices_buf,
        )?;
        // pattern.indices 对每个 atom 追加且不 sort / dedup,要在反向映射前先排一遍。
        self.indices_buf.sort_unstable();
        self.indices_buf.dedup();
        let hits = text.map_back(&self.indices_buf);
        Some(Match { score, hits })
    }
}

impl Default for FuzzyMatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{FuzzyMatcher, MatchableText};

    /// 纯 ASCII:无 Han 时拼音 / 首字母段都为空,只原文段参与匹配。
    #[test]
    fn matchable_ascii_only() -> color_eyre::Result<()> {
        let mt = MatchableText::new("MyGO!!!!!");
        assert!(mt.syllable_ranges.is_empty());
        assert!(mt.han_to_orig.is_empty());
        // haystack = "MyGO!!!!!\0\0":原文 9 char + 2 个 \0。
        assert_eq!(mt.haystack.len(), 11);
        Ok(())
    }

    /// 纯 Han:三个字 → 三个音节 / 三个首字母 / 三段映射。
    #[test]
    fn matchable_pure_han() -> color_eyre::Result<()> {
        let mt = MatchableText::new("春日影");
        assert_eq!(&*mt.han_to_orig, &[0u32, 1, 2]);
        // 春=chun(4) 日=ri(2) 影=ying(4),共 10 char 的拼音段。
        // bounds:orig 3 char + \0 + pinyin 10 char + \0 + initials 3 char。
        assert_eq!(mt.bounds.orig_end, 3);
        assert_eq!(mt.bounds.pinyin_start, 4);
        assert_eq!(mt.bounds.pinyin_end, 14);
        assert_eq!(mt.bounds.initials_start, 15);
        assert_eq!(mt.bounds.initials_end, 18);
        // 音节区间(haystack 绝对下标):chun=[4,8) ri=[8,10) ying=[10,14)。
        assert_eq!(&*mt.syllable_ranges, &[(4u32, 8), (8, 10), (10, 14)]);
        Ok(())
    }

    /// 混排:Han 与非 Han 交替,han_to_orig 只记录 Han 字符的 char 下标。
    #[test]
    fn matchable_mixed() -> color_eyre::Result<()> {
        let mt = MatchableText::new("a春日");
        // 原文 char 下标:a=0, 春=1, 日=2。
        assert_eq!(&*mt.han_to_orig, &[1u32, 2]);
        Ok(())
    }

    /// 空串:三段均空,bounds 退化但仍合法(全 0 / 仅两个 \0)。
    #[test]
    fn matchable_empty() -> color_eyre::Result<()> {
        let mt = MatchableText::new("");
        // haystack = "\0\0",仅两个分隔符。
        assert_eq!(mt.haystack.len(), 2);
        assert!(mt.syllable_ranges.is_empty());
        assert!(mt.han_to_orig.is_empty());
        Ok(())
    }

    /// 首字母 query:`cry` → 命中「春日影」的全部三个 Han。
    #[test]
    fn fuzzy_initials_hit() -> color_eyre::Result<()> {
        let mut m = FuzzyMatcher::new();
        m.set_query("cry");
        let mt = MatchableText::new("春日影");
        let r = m
            .score(&mt)
            .ok_or_else(|| color_eyre::eyre::eyre!("应命中"))?;
        assert_eq!(r.hits.as_slice(), &[0u32, 1, 2]);
        Ok(())
    }

    /// 全拼 query 子串:`chunying` 在「春日影」中,c-h-u-n 落在春,y-i-n-g 落在影,
    /// 日的 ri 没字符命中 → hits = [0, 2]。
    #[test]
    fn fuzzy_full_pinyin_partial_hit() -> color_eyre::Result<()> {
        let mut m = FuzzyMatcher::new();
        m.set_query("chunying");
        let mt = MatchableText::new("春日影");
        let r = m
            .score(&mt)
            .ok_or_else(|| color_eyre::eyre::eyre!("应命中"))?;
        assert_eq!(r.hits.as_slice(), &[0u32, 2]);
        Ok(())
    }

    /// ASCII query 命中原文段:`mygo` 在「春日影 MyGO!!!!!」中,hits = ASCII 字符下标。
    #[test]
    fn fuzzy_ascii_in_original() -> color_eyre::Result<()> {
        let mut m = FuzzyMatcher::new();
        m.set_query("mygo");
        let mt = MatchableText::new("春日影 MyGO!!!!!");
        let r = m
            .score(&mt)
            .ok_or_else(|| color_eyre::eyre::eyre!("应命中"))?;
        // M=4, y=5, G=6, O=7
        assert_eq!(r.hits.as_slice(), &[4u32, 5, 6, 7]);
        Ok(())
    }

    /// 多 atom(空格分词):`cry mygo` 在「春日影 MyGO!!!!!」 → Han 三字 + ASCII 四字。
    #[test]
    fn fuzzy_multi_atom_mixed() -> color_eyre::Result<()> {
        let mut m = FuzzyMatcher::new();
        m.set_query("cry mygo");
        let mt = MatchableText::new("春日影 MyGO!!!!!");
        let r = m
            .score(&mt)
            .ok_or_else(|| color_eyre::eyre::eyre!("应命中"))?;
        assert_eq!(r.hits.as_slice(), &[0u32, 1, 2, 4, 5, 6, 7]);
        Ok(())
    }

    /// 不可命中的 query:返回 None。
    #[test]
    fn fuzzy_no_match() -> color_eyre::Result<()> {
        let mut m = FuzzyMatcher::new();
        m.set_query("zzz");
        let mt = MatchableText::new("春日影");
        assert!(m.score(&mt).is_none());
        Ok(())
    }

    /// 空 query 不算激活,score 直接返回 None;调用方据此走 fast path。
    #[test]
    fn fuzzy_empty_query_inactive() -> color_eyre::Result<()> {
        let mut m = FuzzyMatcher::new();
        m.set_query("");
        assert!(!m.is_active());
        let mt = MatchableText::new("春日影");
        assert!(m.score(&mt).is_none());
        Ok(())
    }

    /// 大小写不敏感:`MYGO` 命中「mygo」。
    #[test]
    fn fuzzy_case_insensitive() -> color_eyre::Result<()> {
        let mut m = FuzzyMatcher::new();
        m.set_query("MYGO");
        let mt = MatchableText::new("mygo");
        let r = m
            .score(&mt)
            .ok_or_else(|| color_eyre::eyre::eyre!("应命中"))?;
        assert_eq!(r.hits.as_slice(), &[0u32, 1, 2, 3]);
        Ok(())
    }

    /// 切换 query 后 last_query 同步更新,新 query 与之前判等不再返回旧 Pattern。
    #[test]
    fn fuzzy_set_query_idempotent() -> color_eyre::Result<()> {
        let mut m = FuzzyMatcher::new();
        m.set_query("a");
        m.set_query("a");
        assert!(m.is_active());
        m.set_query("");
        assert!(!m.is_active());
        Ok(())
    }

    /// 排序合约:同一文本下,「连续命中」分数高于「散开命中」。
    /// 用作守门:nucleo bonus 规则版本漂移会被这里抓到。
    #[test]
    fn fuzzy_consecutive_scores_higher_than_scattered() -> color_eyre::Result<()> {
        let mut m = FuzzyMatcher::new();
        let mt_consec = MatchableText::new("MyGO");
        let mt_scattered = MatchableText::new("M_y_G_O");
        m.set_query("mygo");
        let s1 = m
            .score(&mt_consec)
            .ok_or_else(|| color_eyre::eyre::eyre!("连续:应命中"))?
            .score;
        let s2 = m
            .score(&mt_scattered)
            .ok_or_else(|| color_eyre::eyre::eyre!("散开:应命中"))?
            .score;
        assert!(s1 > s2, "连续 {s1} 应高于散开 {s2}");
        Ok(())
    }
}
