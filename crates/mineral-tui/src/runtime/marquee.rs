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

    /// 下载浮层选中行。
    DownloadSelected,

    /// transport 面板顶行(当前曲)。
    Transport,

    /// now_playing 面板标题行(选中曲)。
    NowPlaying,
}

/// 一个槽的滚动相位:显示身份 + 起始拍。
struct SlotPhase {
    /// 槽当前显示对象的稳定 ID；变化即重置相位。
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
    ///   - `identity`: 当前显示对象的稳定 ID
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
