//! 字词分别渐入提亮的强调色、退为已唱色；暂停与 seek 不会截断颜色交接。

use std::cell::{RefCell, RefMut};
use std::time::Instant;

use mineral_config::LyricsConfig;
use mineral_model::{SongId, Word};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use rustc_hash::FxHashMap;

use super::panel::LyricMode;
use crate::render::color::lerp_color;
use crate::render::theme::{Theme, permille_of};

/// 当前歌曲两个歌词面板的颜色动画；渲染时更新目标，重复绘制不推进额外帧。
#[derive(Default)]
pub(crate) struct LyricColors {
    /// 与 marquee 相同，允许只持 `&AppState` 的绘制路径维护显示状态。
    state: RefCell<ColorState>,
}

/// 普通与全屏面板各自的歌词颜色过渡。
#[derive(Default)]
struct ColorState {
    /// 动画所属的歌曲，不跨歌曲沿用。
    song: Option<SongId>,

    /// 普通面板的可见字词。
    compact: FxHashMap<WordKey, ColorFade>,

    /// 全屏面板的可见字词。
    immersive: FxHashMap<WordKey, ColorFade>,
}

/// 字词在原文时间轴中的位置，不随居中、滚动与面板尺寸变化。
#[derive(PartialEq, Eq, Hash)]
struct WordKey {
    /// 原文行索引。
    line: usize,

    /// 行内时间片索引。
    word: usize,
}

/// 一次面板绘制使用的配置与时钟快照，不保存配置副本到运行时状态。
pub(super) struct LyricPaint<'a> {
    /// 该面板可见字词的动画。
    colors: RefMut<'a, FxHashMap<WordKey, ColorFade>>,

    /// 本帧的歌词明暗层级与过渡时长。
    cfg: &'a LyricsConfig,

    /// 同一面板的所有单元使用同一动画时刻。
    now: Instant,
}

impl LyricColors {
    /// 保留可见行的动画；首次出现的单元直接显示其初始颜色，之后的目标变化才渐变。
    pub(super) fn begin<'a>(
        &'a self,
        song: Option<&SongId>,
        cfg: &'a LyricsConfig,
        motion: LyricMode,
        visible_lines: impl Iterator<Item = usize>,
    ) -> LyricPaint<'a> {
        let mut state = self.state.borrow_mut();
        if state.song.as_ref() != song {
            *state = ColorState {
                song: song.cloned(),
                ..ColorState::default()
            };
        }
        let mut colors = RefMut::map(state, |state| match motion {
            LyricMode::Compact => &mut state.compact,
            LyricMode::Immersive => &mut state.immersive,
        });
        let visible_lines = visible_lines.collect::<Vec<_>>();
        colors.retain(|key, _| visible_lines.contains(&key.line));
        LyricPaint {
            colors,
            cfg,
            now: Instant::now(),
        }
    }
}

impl LyricPaint<'_> {
    /// 整行和逐字跟唱共用浅强调色；非 RGB 主题保留原强调色。
    pub(super) fn highlight(&self, theme: &Theme) -> Color {
        match theme.accent {
            Color::Rgb(..) => lerp_color(
                theme.accent,
                Color::Rgb(255, 255, 255),
                u64::from(permille_of(*self.cfg.highlight_white_mix())),
                1000,
            ),
            color => color,
        }
    }

    /// 当前行按未唱、正在唱、已唱选择目标色；非当前行退到邻行色，仍保留逐单元动画。
    pub(super) fn line<'a>(
        &mut self,
        line_index: usize,
        words: &'a [Word],
        position_ms: Option<u64>,
        inactive: Color,
        theme: &Theme,
        row_bg: Color,
    ) -> Line<'a> {
        // 未唱部分应与已唱部分保持明显区分。
        let unlit = theme
            .text_over(row_bg, permille_of(*self.cfg.text_alpha().unsung()))
            .unwrap_or(theme.overlay);
        let sung = theme
            .text_over(row_bg, permille_of(*self.cfg.text_alpha().sung()))
            .unwrap_or(theme.text);
        let highlight = self.highlight(theme);
        let mut spans = Vec::<Span<'a>>::with_capacity(words.len());
        for (word_index, word) in words.iter().enumerate() {
            let end_ms = word.start_ms.saturating_add(word.dur_ms);
            let (target, transition) = match position_ms {
                None => (inactive, ColorTransition::LineChange),
                Some(position) if position >= end_ms => (sung, ColorTransition::Release),
                Some(position) if position >= word.start_ms => (highlight, ColorTransition::Attack),
                Some(_) => (unlit, ColorTransition::LineChange),
            };
            let key = WordKey {
                line: line_index,
                word: word_index,
            };
            let fade = self
                .colors
                .entry(key)
                .or_insert_with(|| ColorFade::new(target, self.now, transition));
            let color = fade.animate_to(target, transition, self.now, self.cfg);
            spans.push(Span::styled(word.text.as_str(), Style::new().fg(color)));
        }
        Line::from(spans)
    }
}

/// 字词跟唱与整行切换各自的颜色过渡。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorTransition {
    /// 字词开始演唱。
    Attack,

    /// 字词演唱结束。
    Release,

    /// 歌词行进入或退出当前行。
    LineChange,
}

impl ColorTransition {
    /// 当前生效的过渡时长（毫秒）。
    fn duration_ms(self, cfg: &LyricsConfig) -> u64 {
        match self {
            Self::Attack => *cfg.attack_ms(),
            Self::Release => *cfg.release_ms(),
            Self::LineChange => *cfg.scroll_ms(),
        }
    }
}

/// 每个词独立完成颜色过渡，中途变色时保持连续。
struct ColorFade {
    /// 本次过渡开始时的显示色，可能来自尚未完成的上一段动画。
    from: Color,

    /// 当前属性目标色。
    to: Color,

    /// 目标最近一次改变的单调时钟时刻，与播放位置无关。
    started: Instant,

    /// 本次过渡的阶段，入场和退场互不影响。
    transition: ColorTransition,
}

impl ColorFade {
    /// 初次绑定颜色不播放过渡。
    fn new(color: Color, now: Instant, transition: ColorTransition) -> Self {
        Self {
            from: color,
            to: color,
            started: now,
            transition,
        }
    }

    /// 阶段或目标变化时保留当前颜色；暂停期间也继续完成过渡。
    fn animate_to(
        &mut self,
        target: Color,
        transition: ColorTransition,
        now: Instant,
        cfg: &LyricsConfig,
    ) -> Color {
        if self.to != target || self.transition != transition {
            self.from = self.current(now, cfg);
            self.to = target;
            self.started = now;
            self.transition = transition;
        }
        self.current(now, cfg)
    }

    /// 取当前颜色；时长为零时直接显示目标色。
    fn current(&self, now: Instant, cfg: &LyricsConfig) -> Color {
        let duration_ms = self.transition.duration_ms(cfg);
        if duration_ms == 0 {
            return self.to;
        }
        let elapsed_ms =
            u64::try_from(now.duration_since(self.started).as_millis()).unwrap_or(duration_ms);
        lerp_color(self.from, self.to, elapsed_ms, duration_ms)
    }
}
