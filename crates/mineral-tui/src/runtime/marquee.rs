//! 单个组件的标题滚动状态；身份绑定由准备入口更新，绘制只读采样。

use std::time::Instant;

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
enum Mode {
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
struct Tempo {
    /// 滚动方式(含各方式独有节奏)。
    pub(crate) mode: Mode,

    /// 每前进 1 列的拍数(0 视作 1)。
    pub(crate) step_ticks: u32,

    /// 起步 / 重置后的停顿拍数。
    pub(crate) pause_ticks: u32,

    /// 边缘 fade 渐入拍数;0 = 关闭边缘 fade。
    pub(crate) fade_in_ticks: u32,
}

impl Tempo {
    /// 按本次配置折算节奏，仅作为相位采样的局部值。
    fn from_config(cfg: &mineral_config::MarqueeConfig, tick_ms: u64) -> Self {
        use crate::render::anim::ticks16_from_ms;
        Self {
            mode: marquee_mode(cfg, tick_ms),
            step_ticks: u32::from(ticks16_from_ms(*cfg.step_ms(), tick_ms)),
            pause_ticks: u32::from(ticks16_from_ms(*cfg.pause_ms(), tick_ms)),
            // ticks16_from_ms 下限 1 拍,fade_ms = 0(关闭)需保住 0 语义。
            fade_in_ticks: if *cfg.fade_ms() == 0 {
                0
            } else {
                u32::from(ticks16_from_ms(*cfg.fade_ms(), tick_ms))
            },
        }
    }
}

/// 一个组件当前显示的标题；没有绑定时保持静止。
#[derive(Default, Clone, Debug)]
pub(crate) struct Marquee {
    /// 显示身份和本次起步时间，组件销毁时一起释放。
    binding: Option<Binding>,

    /// 最近准备的窗口是否确实需要滚动。
    overflowing: bool,
}

/// 一次标题显示的起点。
#[derive(Clone, Debug)]
struct Binding {
    /// 调用方提供的稳定身份。
    identity: String,

    /// 由主循环明确采样的起步时间。
    start: Instant,
}

impl Marquee {
    /// 身份改变或内容完整可见时重置起点；不自行读取时钟。
    pub(crate) fn prepare(&mut self, identity: &str, content_w: u16, window_w: u16, now: Instant) {
        self.overflowing = content_w > window_w;
        let phase = self.binding.get_or_insert_with(|| Binding {
            identity: identity.to_owned(),
            start: now,
        });
        if phase.identity != identity {
            phase.identity = identity.to_owned();
            phase.start = now;
        }
        if content_w <= window_w {
            phase.start = now;
        }
    }

    /// 只有可见标题溢出且滚动开启时，绘制结果才依赖本帧时间。
    pub(crate) fn dependencies(
        &self,
        inputs: &mut crate::render::memo::Dependencies<'_>,
        anim: &mineral_config::AnimationConfig,
        now: Instant,
    ) {
        inputs.observe(&self.overflowing);
        if self.overflowing && *anim.marquee().mode() != mineral_config::MarqueeMode::Off {
            inputs.time(now);
        }
    }

    /// 按本帧时钟和当前配置采样相位，不缓存配置也不改变显示身份。
    pub(crate) fn phase(
        &self,
        identity: &str,
        content_w: u16,
        window_w: u16,
        gap_w: u16,
        anim: &mineral_config::AnimationConfig,
        now: Instant,
    ) -> Phase {
        let tick_ms = *anim.frame_tick_ms();
        let tempo = Tempo::from_config(anim.marquee(), tick_ms);
        const STILL: Phase = Phase {
            offset: 0,
            fade_permille: 0,
        };
        if matches!(tempo.mode, Mode::Off) {
            return STILL;
        }
        let Some(phase) = self.binding.as_ref() else {
            return STILL;
        };
        if phase.identity != identity || content_w <= window_w {
            return STILL;
        }
        let elapsed = u64::try_from(
            now.saturating_duration_since(phase.start).as_millis() / u128::from(tick_ms.max(1)),
        )
        .unwrap_or(u64::MAX);

        let fade_permille = if tempo.fade_in_ticks == 0 {
            0
        } else {
            u16::try_from((elapsed.saturating_mul(1000) / u64::from(tempo.fade_in_ticks)).min(1000))
                .unwrap_or(1000)
        };
        let Some(scrolled) = elapsed.checked_sub(u64::from(tempo.pause_ticks)) else {
            return Phase {
                offset: 0,
                fade_permille,
            };
        };
        let step = u64::from(tempo.step_ticks);
        let offset = match tempo.mode {
            Mode::Off => 0,
            // 循环:模「内容 + gap」周期,窗口滚过末尾经 gap 回绕到开头。
            Mode::Loop => scrolled / step % u64::from(u32::from(content_w) + u32::from(gap_w)),
            // 往返:三角波 0→max→0(max = 溢出列数 ≥ 1,不经过 gap),两端各停
            // `edge_hold_ticks` 拍再折返。tick 域分段:正向 → 右停 → 反向 → 左停。
            Mode::Bounce { edge_hold_ticks } => {
                let max_off = u64::from(content_w - window_w);
                let leg = max_off * step;
                let hold = u64::from(edge_hold_ticks);
                let pos = scrolled % (2 * (leg + hold));
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
