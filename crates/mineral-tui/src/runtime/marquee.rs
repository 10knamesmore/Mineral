//! 溢出标题滚动(marquee)的相位状态:槽 → (显示身份, 起始拍) 的 reconciliation。
//!
//! 渲染端每帧对自己的槽声明「现在显示的是谁」([`Marquees::phase`]),身份变化
//! (选中移动 / 切歌 / 列表内容变化)即重置相位——三种触发统一为一个机制,零事件通知。
//! 帧计数走按键外的 tick 路径(`&mut`),相位查询走渲染路径(`&self` + 内部
//! `RefCell`),与 `ScrollList` 的两路分工同款。切片本身在渲染层纯函数(marquee_line)。

use std::cell::RefCell;

use rustc_hash::FxHashMap;

/// marquee 槽:每个「同一时刻至多滚一行」的渲染位一个槽。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Slot {
    /// browse 曲目表选中行。
    BrowseSelected,

    /// search 结果列选中行。
    SearchResults,

    /// search detail 曲目表选中行。
    SearchDetailSelected,

    /// 队列浮层选中行。
    QueueSelected,

    /// transport 面板顶行(当前曲)。
    Transport,

    /// now_playing 面板标题行(选中曲)。
    NowPlaying,
}

/// 一个槽的滚动相位:显示身份 + 起始拍。
struct SlotPhase {
    /// 槽当前显示对象的身份(歌的 `qualified()` id);变化即重置相位。
    identity: String,

    /// 相位起点(全局帧计数值)。
    start: u32,
}

/// 一次相位查询的结果。
pub(crate) struct Phase {
    /// 滚动相位(显示列,已模周期);停顿期 / 不溢出为 0。
    pub(crate) offset: u16,

    /// 窗口边缘 fade 的渐入强度(0..=1000):相位重置起线性升满,选中后缓缓变暗
    /// 不突变;不溢出 / fade 关闭恒 0。
    pub(crate) fade_permille: u16,
}

/// 滚动方式(配置 `animation.marquee.mode` 的映射,各方式独有节奏已折算成拍)。
#[derive(Clone, Copy)]
pub(crate) enum Mode {
    /// 循环:文本首尾相接(中间夹 gap)向左匀速循环。
    Loop,

    /// 来回往返:三角波 0→max→0,不经过 gap。
    Bounce {
        /// 到达两端后的停顿拍数(读完首 / 尾再折返);0 = 直接折返。
        edge_hold_ticks: u32,
    },

    /// 关闭:恒零相位,溢出标题维持静态截断。
    Off,
}

/// 把配置的滚动方式映射成 [`Mode`](各方式独有节奏一并折算成拍)。
fn marquee_mode(cfg: &mineral_config::MarqueeConfig, tick_ms: u64) -> Mode {
    match *cfg.mode() {
        mineral_config::MarqueeMode::Loop => Mode::Loop,
        mineral_config::MarqueeMode::Bounce => Mode::Bounce {
            // ticks16_from_ms 下限 1 拍,edge_pause_ms = 0(直接折返)需保住 0 语义。
            edge_hold_ticks: if *cfg.bounce().edge_pause_ms() == 0 {
                0
            } else {
                u32::from(crate::render::anim::ticks16_from_ms(
                    *cfg.bounce().edge_pause_ms(),
                    tick_ms,
                ))
            },
        },
        mineral_config::MarqueeMode::Off => Mode::Off,
    }
}

/// 折算好的滚动节奏(配置 `animation.marquee` 按帧率折算成拍)。
#[derive(Clone, Copy)]
pub(crate) struct Tempo {
    /// 滚动方式(含各方式独有节奏)。
    pub(crate) mode: Mode,

    /// 每前进 1 列的拍数(0 视作 1)。
    pub(crate) step_ticks: u32,

    /// 起步 / 重置后的停顿拍数。
    pub(crate) pause_ticks: u32,

    /// 边缘 fade 渐入拍数;0 = 关闭边缘 fade。
    pub(crate) fade_in_ticks: u32,
}

/// 全部 marquee 槽的相位状态(挂在 `AppState`)。
pub(crate) struct Marquees {
    /// 全局帧计数(App tick 每帧 +1;wrapping,配合 `wrapping_sub` 求 elapsed)。
    now: u32,

    /// 滚动节奏(方式 + 各拍数)。
    tempo: Tempo,

    /// 槽表(渲染路径 `&self` 更新,内部可变)。
    slots: RefCell<FxHashMap<Slot, SlotPhase>>,
}

impl Marquees {
    /// 构造:注入已按帧率折算好的节奏。
    pub(crate) fn new(tempo: Tempo) -> Self {
        Self {
            now: 0,
            tempo: Tempo {
                step_ticks: tempo.step_ticks.max(1),
                ..tempo
            },
            slots: RefCell::new(FxHashMap::default()),
        }
    }

    /// 从配置段折算节奏并构造(启动与配置热重载共用;重载 = 整体重建,槽相位
    /// 清零从头带停顿起步)。
    ///
    /// # Params:
    ///   - `cfg`: 配置 `animation.marquee` 段
    ///   - `tick_ms`: 主循环帧间隔(拍数折算分母,`animation.frame_tick_ms`)
    pub(crate) fn from_config(cfg: &mineral_config::MarqueeConfig, tick_ms: u64) -> Self {
        use crate::render::anim::ticks16_from_ms;
        Self::new(Tempo {
            mode: marquee_mode(cfg, tick_ms),
            step_ticks: u32::from(ticks16_from_ms(*cfg.step_ms(), tick_ms)),
            pause_ticks: u32::from(ticks16_from_ms(*cfg.pause_ms(), tick_ms)),
            // ticks16_from_ms 下限 1 拍,fade_ms = 0(关闭)需保住 0 语义。
            fade_in_ticks: if *cfg.fade_ms() == 0 {
                0
            } else {
                u32::from(ticks16_from_ms(*cfg.fade_ms(), tick_ms))
            },
        })
    }

    /// 测试构造:Loop 模式、无 fade 的最小节奏。
    #[cfg(test)]
    pub(crate) fn test_loop(step_ticks: u32, pause_ticks: u32) -> Self {
        Self::new(Tempo {
            mode: Mode::Loop,
            step_ticks,
            pause_ticks,
            fade_in_ticks: 0,
        })
    }

    /// 推进一帧(App tick 路径,`&mut`)。
    pub(crate) fn tick(&mut self) {
        self.now = self.now.wrapping_add(1);
    }

    /// 渲染路径:查询 `slot` 当前的滚动相位与边缘 fade 强度。
    ///
    /// 身份与槽存的不同即重置相位;不溢出(`content_w ≤ window_w`)恒返零相位并重置——
    /// 这样 resize 变窄再度溢出时从头带停顿起步,而不是落在滚动中段。
    ///
    /// # Params:
    ///   - `slot`: 渲染位
    ///   - `identity`: 当前显示对象身份(歌的 `qualified()` id)
    ///   - `content_w`: 标题内容显示宽(列)
    ///   - `window_w`: 可用窗口宽(列)
    ///   - `gap_w`: 循环间隔串显示宽(列)
    ///
    /// # Return:
    ///   [`Phase`]:滚动列(已模周期 `content_w + gap_w`,停顿期为 0)+ fade 渐入强度。
    pub(crate) fn phase(
        &self,
        slot: Slot,
        identity: &str,
        content_w: u16,
        window_w: u16,
        gap_w: u16,
    ) -> Phase {
        const STILL: Phase = Phase {
            offset: 0,
            fade_permille: 0,
        };
        if matches!(self.tempo.mode, Mode::Off) {
            return STILL;
        }
        let mut slots = self.slots.borrow_mut();
        let phase = slots.entry(slot).or_insert_with(|| SlotPhase {
            identity: identity.to_owned(),
            start: self.now,
        });
        if phase.identity != identity {
            phase.identity = identity.to_owned();
            phase.start = self.now;
        }
        if content_w <= window_w {
            phase.start = self.now;
            return STILL;
        }
        let elapsed = self.now.wrapping_sub(phase.start);
        let fade_permille = if self.tempo.fade_in_ticks == 0 {
            0
        } else {
            u16::try_from(
                (u64::from(elapsed) * 1000 / u64::from(self.tempo.fade_in_ticks)).min(1000),
            )
            .unwrap_or(1000)
        };
        let Some(scrolled) = elapsed.checked_sub(self.tempo.pause_ticks) else {
            return Phase {
                offset: 0,
                fade_permille,
            };
        };
        let step = u64::from(self.tempo.step_ticks);
        let offset = match self.tempo.mode {
            Mode::Off => 0,
            // 循环:模「内容 + gap」周期,窗口滚过末尾经 gap 回绕到开头。
            Mode::Loop => {
                u64::from(scrolled) / step % u64::from(u32::from(content_w) + u32::from(gap_w))
            }
            // 往返:三角波 0→max→0(max = 溢出列数 ≥ 1,不经过 gap),两端各停
            // `edge_hold_ticks` 拍再折返。tick 域分段:正向 → 右停 → 反向 → 左停。
            Mode::Bounce { edge_hold_ticks } => {
                let max_off = u64::from(content_w - window_w);
                let leg = max_off * step;
                let hold = u64::from(edge_hold_ticks);
                let pos = u64::from(scrolled) % (2 * (leg + hold));
                if pos < leg {
                    pos / step
                } else if pos < leg + hold {
                    max_off
                } else if pos < 2 * leg + hold {
                    max_off - (pos - leg - hold) / step
                } else {
                    0
                }
            }
        };
        Phase {
            offset: u16::try_from(offset).unwrap_or(0),
            fade_permille,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Marquees, Mode, Slot, Tempo};

    /// 溢出场景的标准参数:内容 10 列、窗口 6 列、gap 2 列(周期 12)。
    fn offset_of(m: &Marquees, slot: Slot, identity: &str) -> u16 {
        m.phase(
            slot, identity, /*content_w*/ 10, /*window_w*/ 6, /*gap_w*/ 2,
        )
        .offset
    }

    /// 推进 n 帧。
    fn advance(m: &mut Marquees, n: u32) {
        for _ in 0..n {
            m.tick();
        }
    }

    /// 停顿期内恒 0,过停顿后每 step_ticks 前进 1 列。
    #[test]
    fn pauses_then_steps() {
        let mut m = Marquees::test_loop(/*step_ticks*/ 2, /*pause_ticks*/ 4);
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 0, "起步即停顿");
        advance(&mut m, 4);
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 0, "停顿最后一拍仍 0");
        advance(&mut m, 2);
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 1, "过停顿一步进 1 列");
        advance(&mut m, 2);
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 2);
    }

    /// 相位模周期回绕:走满 content_w + gap_w 列后回 0。
    #[test]
    fn wraps_modulo_cycle() {
        let mut m = Marquees::test_loop(/*step_ticks*/ 1, /*pause_ticks*/ 0);
        // 槽在首次查询时建档,先建档;周期 = 10 + 2 = 12 列,12 拍走满一周。
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 0, "首查建档");
        advance(&mut m, 12);
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 0, "走满一周回绕到 0");
        advance(&mut m, 3);
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 3);
    }

    /// 身份变化(选中移动 / 切歌)即重置:回 0 并重新停顿。
    #[test]
    fn identity_change_resets_phase() {
        let mut m = Marquees::test_loop(/*step_ticks*/ 1, /*pause_ticks*/ 2);
        // 槽在首次查询时建档(start = 当时帧),先建档再推进。
        assert_eq!(offset_of(&m, Slot::BrowseSelected, "a"), 0, "首查建档");
        advance(&mut m, 6);
        assert_eq!(
            offset_of(&m, Slot::BrowseSelected, "a"),
            4,
            "a 已在滚动中段"
        );
        assert_eq!(offset_of(&m, Slot::BrowseSelected, "b"), 0, "换 b 立即回 0");
        advance(&mut m, 2);
        assert_eq!(offset_of(&m, Slot::BrowseSelected, "b"), 0, "b 重新停顿");
        advance(&mut m, 1);
        assert_eq!(offset_of(&m, Slot::BrowseSelected, "b"), 1, "b 停顿后起步");
    }

    /// 不溢出恒 0 且重置相位:resize 变窄再溢出时从头带停顿起步,不落中段。
    #[test]
    fn fitting_returns_zero_and_rearms() {
        let mut m = Marquees::test_loop(/*step_ticks*/ 1, /*pause_ticks*/ 2);
        advance(&mut m, 10);
        assert_eq!(
            m.phase(
                Slot::Transport,
                "a",
                /*content_w*/ 5,
                /*window_w*/ 6,
                /*gap_w*/ 2
            )
            .offset,
            0,
            "不溢出恒 0"
        );
        // 立刻变窄溢出:应从停顿起步而不是 elapsed=10 的中段。
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 0, "再溢出从停顿起步");
        advance(&mut m, 3);
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 1, "停顿后正常步进");
    }

    /// 边缘 fade 渐入:相位重置起线性升满(半程 500、满程钳 1000);
    /// 不溢出恒 0;fade_in_ticks = 0 关闭恒 0。
    #[test]
    fn fade_ramps_in_from_reset() {
        let mut m = Marquees::new(Tempo {
            mode: Mode::Loop,
            step_ticks: 1,
            pause_ticks: 0,
            fade_in_ticks: 10,
        });
        let fade = |m: &Marquees, identity: &str| {
            m.phase(
                Slot::Transport,
                identity,
                /*content_w*/ 10,
                /*window_w*/ 6,
                /*gap_w*/ 2,
            )
            .fade_permille
        };
        assert_eq!(fade(&m, "a"), 0, "建档帧强度 0");
        advance(&mut m, 5);
        assert_eq!(fade(&m, "a"), 500, "半程 500");
        advance(&mut m, 10);
        assert_eq!(fade(&m, "a"), 1000, "超时长钳到 1000");
        assert_eq!(fade(&m, "b"), 0, "换身份重置回 0");

        // 不溢出:强度恒 0。
        let overflow_free = m
            .phase(
                Slot::Transport,
                "b",
                /*content_w*/ 5,
                /*window_w*/ 6,
                /*gap_w*/ 2,
            )
            .fade_permille;
        assert_eq!(overflow_free, 0, "不溢出不 fade");

        // fade 关闭:任意 elapsed 恒 0。
        let mut off = Marquees::test_loop(/*step_ticks*/ 1, /*pause_ticks*/ 0);
        assert_eq!(fade(&off, "a"), 0);
        advance(&mut off, 50);
        assert_eq!(fade(&off, "a"), 0, "fade_in_ticks=0 应恒关");
    }

    /// bounce:三角波往返——正向到 max(溢出列数)后反向滚回 0,再正向;不经 gap 周期。
    #[test]
    fn bounce_reverses_at_ends() {
        let mut m = Marquees::new(Tempo {
            mode: Mode::Bounce { edge_hold_ticks: 0 },
            step_ticks: 1,
            pause_ticks: 0,
            fade_in_ticks: 0,
        });
        // content 10 / window 6 → max_off = 4,往返周期 8。
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 0, "首查建档");
        advance(&mut m, 4);
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 4, "正向到达右端");
        advance(&mut m, 2);
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 2, "反向往回");
        advance(&mut m, 2);
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 0, "回到开头");
        advance(&mut m, 1);
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 1, "再次正向");
    }

    /// bounce 端点停顿:到达两端后驻留 edge_hold_ticks 拍再折返。
    #[test]
    fn bounce_holds_at_edges() {
        let mut m = Marquees::new(Tempo {
            mode: Mode::Bounce { edge_hold_ticks: 3 },
            step_ticks: 1,
            pause_ticks: 0,
            fade_in_ticks: 0,
        });
        // content 10 / window 6 → max_off 4;单程 4 拍 + 每端停 3 拍,周期 14。
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 0, "首查建档");
        advance(&mut m, 4);
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 4, "到达右端");
        advance(&mut m, 3);
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 4, "右端驻留 3 拍");
        advance(&mut m, 1);
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 3, "驻留结束反向");
        advance(&mut m, 3);
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 0, "回到左端");
        advance(&mut m, 3);
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 0, "左端驻留 3 拍");
        advance(&mut m, 1);
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 1, "再次正向");
    }

    /// off:任意推进恒零相位零 fade(不滚动,维持静态截断)。
    #[test]
    fn off_mode_never_scrolls() {
        let mut m = Marquees::new(Tempo {
            mode: Mode::Off,
            step_ticks: 1,
            pause_ticks: 0,
            fade_in_ticks: 10,
        });
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 0);
        advance(&mut m, 50);
        let p = m.phase(
            Slot::Transport,
            "a",
            /*content_w*/ 10,
            /*window_w*/ 6,
            /*gap_w*/ 2,
        );
        assert_eq!((p.offset, p.fade_permille), (0, 0), "off 恒零相位零 fade");
    }

    /// 槽之间相位独立:一个槽换身份不影响另一个槽。
    #[test]
    fn slots_are_independent() {
        let mut m = Marquees::test_loop(/*step_ticks*/ 1, /*pause_ticks*/ 0);
        // 槽在首次查询时建档:先建 Transport,推进后再首查 QueueSelected。
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 0, "首查建档");
        advance(&mut m, 5);
        assert_eq!(offset_of(&m, Slot::Transport, "a"), 5);
        assert_eq!(
            offset_of(&m, Slot::QueueSelected, "b"),
            0,
            "新槽首查从零起步"
        );
        advance(&mut m, 2);
        assert_eq!(
            offset_of(&m, Slot::Transport, "a"),
            7,
            "老槽相位不受新槽影响"
        );
        assert_eq!(offset_of(&m, Slot::QueueSelected, "b"), 2);
    }
}
