//! 全屏歌词「脱离播放」的手动滚动子系统:缓动平移锚点、边界过冲回弹(rubber-band)、
//! synced 歌空闲超时回锚。

use crate::render::anim::{Transition, ticks16_from_ms};
use crate::runtime::action::ScrollStep;

use super::AppState;

/// 手动滚动平移的阶段,决定 settle 后的去向。
#[derive(Clone, Copy, PartialEq, Eq)]
enum GlidePhase {
    /// 用户锚定平移;settle 后停在锚定行,空闲计时走向回锚。
    Manual,

    /// 边界过冲段(rubber-band):锚点暂越出内容界,settle 后自动起弹回平移。
    Overshoot {
        /// 弹回目标(clamp 到边界的 milli-line)。
        rest_milli: i64,
    },

    /// 回锚平移(目标已切回播放行);settle 后清回附着态。
    Reattach,
}

/// 全屏歌词「脱离播放」的手动滚动态。
///
/// detach 后窗口居中锚点由用户控制——播放推进不再驱动居中(高亮 / wipe 仍按真实播放
/// 位置走);锚点在 `from_milli` → `to_milli` 间按 cubic ease-out 平移,得到平滑滚动。
/// synced 歌空闲超时把目标切回当前播放行,**回锚同样走这条平移通道**;边界过冲的弹回
/// 也是——同一条曲线,只是目标不同。
pub(super) struct LyricGlide {
    /// 平移起点(milli-line = 原文行号 × 1000)。每设新目标时置为当前动画位置,故连按 /
    /// 中途回锚都从眼前位置接着滑、不跳。
    from_milli: i64,

    /// 平移目标(milli-line)。滚动键 = 用户锚定行;过冲段 = 界外过冲点;回锚 = 当前播放行。
    to_milli: i64,

    /// `from` → `to` 的缓动进度(`expanding`:0 起步推满)。
    glide: Transition,

    /// 当前平移阶段(settle 后的去向)。
    phase: GlidePhase,

    /// 手动滚动后的空闲拍数;synced 据此超时触发回锚(无时间戳歌不回)。
    idle: u16,
}

impl LyricGlide {
    /// 当前缓动锚点位置(milli-line):在 `from` → `to` 间按已缓动进度线性插值。
    fn pos_milli(&self) -> i64 {
        let eased = i64::from(self.glide.eased());
        self.from_milli + (self.to_milli - self.from_milli) * eased / 1000
    }

    /// 当前锚定的目标行(整数 line index)。过冲段以弹回目标计——锚定行始终在内容界内,
    /// 连按基准与焦点高亮都不受过冲影响。
    fn target_line(&self) -> i64 {
        match self.phase {
            GlidePhase::Overshoot { rest_milli } => rest_milli / 1000,
            GlidePhase::Manual | GlidePhase::Reattach => self.to_milli / 1000,
        }
    }
}

impl AppState {
    /// 全屏手动滚动歌词:按方向 + 档位(逐行 / 翻页)行数移动锚定行,**脱离播放**(播放推进
    /// 不再驱动居中);非全屏不接管(键不被吞,但无效果)。
    ///
    /// 锚定行钳到 `[0, 内容行数-1]`;撞墙的超出量按阻尼折算成短暂过冲(rubber-band),
    /// glide 滑到界外过冲点后由 tick 自动弹回边界。锚点位置从当前动画位置平滑滑向新
    /// 目标——连按 / 中途反向都从眼前位置接着滑,不跳变。
    ///
    /// # Params:
    ///   - `scroll`: 方向 + 档位
    pub(crate) fn scroll_lyrics(&mut self, scroll: ScrollStep) {
        if !self.fullscreen {
            return;
        }
        let len = self
            .current_lines()
            .map_or(0, <[mineral_model::LyricLine]>::len);
        let Some(max_line) = len.checked_sub(1).and_then(|m| i64::try_from(m).ok()) else {
            return;
        };
        let lyrics = self.cfg.tui().lyrics();
        let behavior = self.cfg.tui().behavior();
        let line = i64::try_from(*behavior.line_scroll_rows()).unwrap_or(0);
        let page = i64::try_from(*behavior.page_scroll_rows()).unwrap_or(0);
        let delta = match scroll {
            ScrollStep::LineDown => line,
            ScrollStep::LineUp => -line,
            ScrollStep::PageDown => page,
            ScrollStep::PageUp => -page,
        };
        // 基准:已脱离则用现有目标行,否则用播放当前行(脱离瞬间锚在眼前)。
        let base = match &self.lyric_scroll {
            Some(g) => g.target_line(),
            None => self.current_line_anchor(),
        };
        let from_milli = self
            .lyric_scroll
            .as_ref()
            .map_or(base * 1000, LyricGlide::pos_milli);
        let raw = base.saturating_add(delta);
        let clamped = raw.clamp(0, max_line);
        // 边界过冲(rubber-band):超出量按阻尼折算、设上限,glide 先滑到界外过冲点,
        // settle 后由 tick_lyric_scroll 自动起弹回平移滑回边界。
        let damping = i64::from(*lyrics.overshoot_damping()).max(1);
        let max_milli = i64::from(*lyrics.overshoot_max_permille());
        let overshoot_milli = (raw.saturating_sub(clamped).saturating_mul(1000) / damping)
            .clamp(-max_milli, max_milli);
        let phase = if overshoot_milli == 0 {
            GlidePhase::Manual
        } else {
            GlidePhase::Overshoot {
                rest_milli: clamped * 1000,
            }
        };
        let glide = Transition::expanding(self.glide_ticks());
        self.lyric_scroll = Some(LyricGlide {
            from_milli,
            to_milli: clamped * 1000 + overshoot_milli,
            glide,
            phase,
            idle: 0,
        });
    }

    /// 每帧推进歌词滚动生命周期:换歌清脱离态;推进缓动平移(手动滚动 / 过冲弹回 / 回锚
    /// 共用);过冲段 settle 后自动弹回 clamp 边界;有时间戳歌手动滚走后空闲超时平滑回锚
    /// 到当前播放行,无时间戳歌停在手动位置不回(无锚点可回)。
    pub(crate) fn tick_lyric_scroll(&mut self) {
        let changed =
            self.playback.track.as_ref().map(|s| &s.id) != self.lyric_scroll_song.as_ref();
        if changed {
            self.lyric_scroll_song = self.playback.track.as_ref().map(|s| s.id.clone());
            self.lyric_scroll = None;
            return;
        }
        if self.lyric_scroll.is_none() {
            return;
        }
        // 借用 lyric_scroll 前先把依赖 &self 的量算好,避免重叠借用。
        let synced = self.current_lines().is_some_and(mineral_model::has_timed);
        let reattach_ticks = ticks16_from_ms(
            *self.cfg.tui().lyrics().reattach_ms(),
            *self.cfg.tui().animation().frame_tick_ms(),
        );
        let cur_line = self.current_line_anchor();
        let glide_ticks = self.glide_ticks();
        let Some(g) = self.lyric_scroll.as_mut() else {
            return;
        };
        g.glide.tick();
        // 过冲段 settle → 起弹回平移滑回边界。放在 synced 检查之前:无时间戳歌不回锚,
        // 但边界回弹同样生效。
        if let GlidePhase::Overshoot { rest_milli } = g.phase
            && g.glide.settled()
        {
            let from_milli = g.pos_milli();
            let idle = g.idle;
            *g = LyricGlide {
                from_milli,
                to_milli: rest_milli,
                glide: Transition::expanding(glide_ticks),
                phase: GlidePhase::Manual,
                idle,
            };
        }
        // 无时间戳歌:无播放锚点可回,停在手动位置(缓动已推进,仍平滑)。
        if !synced {
            return;
        }
        if g.phase == GlidePhase::Reattach {
            if g.glide.settled() {
                self.lyric_scroll = None;
            }
            return;
        }
        g.idle = g.idle.saturating_add(1);
        if g.idle >= reattach_ticks {
            // 启动回锚平移:目标切回当前播放行,从眼前位置平滑滑回。
            let from_milli = g.pos_milli();
            *g = LyricGlide {
                from_milli,
                to_milli: cur_line * 1000,
                glide: Transition::expanding(glide_ticks),
                phase: GlidePhase::Reattach,
                idle: 0,
            };
        }
    }

    /// 当前播放位置对应的原文行索引(无时间戳 / 未进首句时落 `0`),作整数锚定行用。
    fn current_line_anchor(&self) -> i64 {
        self.current_lines()
            .and_then(|lines| mineral_model::current_line(lines, self.playback.position_ms))
            .and_then(|i| i64::try_from(i).ok())
            .unwrap_or(0)
    }

    /// 手动滚动 / 回锚平移的缓动拍数(复用 `tui.lyrics.scroll_ms` 过渡时长)。
    fn glide_ticks(&self) -> u16 {
        let scroll_ms = u32::try_from(*self.cfg.tui().lyrics().scroll_ms()).unwrap_or(u32::MAX);
        ticks16_from_ms(scroll_ms, *self.cfg.tui().animation().frame_tick_ms())
    }

    /// 全屏手动滚动当前的缓动锚点(milli-line = 原文行号 × 1000);`None` = 附着态
    /// (渲染跟随播放)。仅全屏沉浸态读取——紧凑面板恒附着,不吃手动偏移。
    /// 过冲段 / 弹回中锚点可短暂越出 `[0, 内容行数-1]`,渲染端沿行距外推画出回弹帧。
    pub(crate) fn manual_lyric_anchor_milli(&self) -> Option<i64> {
        self.lyric_scroll.as_ref().map(LyricGlide::pos_milli)
    }

    /// 脱离态当前锚定的原文行(渲染端给它半程高亮,标记手动浏览焦点);`None` = 附着态。
    /// 回锚平移期间目标即播放行,与 now-playing 高亮自然合一。
    pub(crate) fn manual_lyric_focus_line(&self) -> Option<usize> {
        self.lyric_scroll
            .as_ref()
            .and_then(|g| usize::try_from(g.target_line()).ok())
    }

    /// 测试辅助:把手动滚动直接置于「已 settle」状态,锚定在当前播放行 + `delta` 行。
    #[cfg(test)]
    pub(crate) fn debug_scroll_lyrics_to_settled(&mut self, delta: i64) {
        let line = self.current_line_anchor().saturating_add(delta).max(0);
        self.debug_scroll_lyrics_to_milli(line * 1000);
    }

    /// 测试辅助:把手动滚动直接置于「已 settle」的任意 milli-line 锚点(允许越出内容界,
    /// 模拟过冲中帧供渲染快照)。
    #[cfg(test)]
    pub(crate) fn debug_scroll_lyrics_to_milli(&mut self, milli: i64) {
        self.lyric_scroll = Some(LyricGlide {
            from_milli: milli,
            to_milli: milli,
            glide: Transition::expanding(1),
            phase: GlidePhase::Manual,
            idle: 0,
        });
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::{LyricLine, Lyrics};
    use mineral_test::{feiyu_song, qianzai_song};

    use crate::runtime::action::ScrollStep;
    use crate::runtime::state::AppState;

    use super::LyricGlide;

    /// 造一个全屏、缓存了 `original` 行(指定时间态)的 `AppState`,供手动滚动测试。
    fn fullscreen_with(original: Vec<LyricLine>) -> color_eyre::Result<AppState> {
        fullscreen_with_cfg(AppState::test_default()?, original)
    }

    /// 同 [`fullscreen_with`],但状态由调用方注入(自定义配置覆盖用)。
    fn fullscreen_with_cfg(
        mut s: AppState,
        original: Vec<LyricLine>,
    ) -> color_eyre::Result<AppState> {
        let song = qianzai_song();
        s.lyrics_cache.insert(
            song.id.clone(),
            Lyrics {
                original,
                ..Lyrics::default()
            },
        );
        s.playback.track = Some(song);
        s.fullscreen = true;
        Ok(s)
    }

    /// 20 行带时间戳(synced)。
    fn timed_lines() -> Vec<LyricLine> {
        (0..20u64)
            .map(|i| LyricLine::timed(i * 1000, "x"))
            .collect()
    }

    /// 20 行无时间戳(unsynced)。
    fn untimed_lines() -> Vec<LyricLine> {
        (0..20).map(|_| LyricLine::untimed("x")).collect()
    }

    /// 锚定目标行(整数 line index);附着态返回 `None`。
    fn target(s: &AppState) -> Option<i64> {
        s.lyric_scroll.as_ref().map(LyricGlide::target_line)
    }

    /// 平移目标(milli-line,过冲时越出内容界);附着态返回 `None`。
    fn to_milli(s: &AppState) -> Option<i64> {
        s.lyric_scroll.as_ref().map(|g| g.to_milli)
    }

    /// 步长随默认配置算(`behavior.page_scroll_rows` / `line_scroll_rows` 是手感旋钮,
    /// 调默认值不该改这条测试);前提:一页步长不超过 20 行 fixture 的末行。
    #[test]
    fn scroll_lyrics_gated_on_fullscreen() -> color_eyre::Result<()> {
        let mut s = fullscreen_with(timed_lines())?;
        let page = i64::try_from(*s.cfg.tui().behavior().page_scroll_rows())?;
        let line = i64::try_from(*s.cfg.tui().behavior().line_scroll_rows())?;
        assert!(page <= 19, "前提:默认翻页步长须落在 20 行 fixture 界内");
        s.fullscreen = false;
        s.scroll_lyrics(ScrollStep::PageDown);
        assert!(s.lyric_scroll.is_none(), "非全屏不接管滚动");
        s.fullscreen = true;
        // position 0 → 当前播放行 0;翻页锚定到 0 + page。
        s.scroll_lyrics(ScrollStep::PageDown);
        assert_eq!(target(&s), Some(page), "全屏翻页锚定行 = 0 + page");
        s.scroll_lyrics(ScrollStep::LineUp);
        assert_eq!(target(&s), Some(page - line), "逐行上滚 = line_scroll_rows");
        Ok(())
    }

    #[test]
    fn scroll_lyrics_clamps_to_content() -> color_eyre::Result<()> {
        let mut s = fullscreen_with(timed_lines())?;
        for _ in 0..5 {
            s.scroll_lyrics(ScrollStep::PageDown);
        }
        assert_eq!(target(&s), Some(19), "累加钳到末行(20 行 → 行号 19)");
        Ok(())
    }

    /// 期望值随默认配置算(damping / 上限 / 翻页步长是手感旋钮,调默认值不该改这条测试)。
    #[test]
    fn boundary_press_overshoots_with_damping() -> color_eyre::Result<()> {
        let mut s = fullscreen_with(timed_lines())?;
        let damping = i64::from(*s.cfg.tui().lyrics().overshoot_damping()).max(1);
        let cap = i64::from(*s.cfg.tui().lyrics().overshoot_max_permille());
        let page = i64::try_from(*s.cfg.tui().behavior().page_scroll_rows())?;
        assert!(page <= 19 && page * 2 > 19, "前提:一页在界内、两页撞底");
        s.scroll_lyrics(ScrollStep::PageDown); // 0 → page,界内无过冲
        assert_eq!(to_milli(&s), Some(page * 1000), "界内滚动无过冲");
        s.scroll_lyrics(ScrollStep::PageDown); // raw 2*page → clamp 19
        assert_eq!(target(&s), Some(19), "锚定行仍钳在末行");
        let over_first = page * 2 - 19;
        assert_eq!(
            to_milli(&s),
            Some(19_000 + (over_first * 1000 / damping).min(cap)),
            "超出 {over_first} 行 → 过冲 = 超出量/damping(不超上限)"
        );
        s.scroll_lyrics(ScrollStep::PageDown); // 已在末行,整页全是超出量
        assert_eq!(
            to_milli(&s),
            Some(19_000 + (page * 1000 / damping).min(cap)),
            "超出整页 → 过冲更大(不超上限),比逐行撞墙弹得远"
        );
        Ok(())
    }

    #[test]
    fn top_press_overshoots_negative() -> color_eyre::Result<()> {
        let mut s = fullscreen_with(timed_lines())?;
        let damping = i64::from(*s.cfg.tui().lyrics().overshoot_damping()).max(1);
        let cap = i64::from(*s.cfg.tui().lyrics().overshoot_max_permille());
        s.scroll_lyrics(ScrollStep::LineUp); // 行 0 再上滚:超出 -1 行
        assert_eq!(target(&s), Some(0), "锚定行钳在首行");
        assert_eq!(
            to_milli(&s),
            Some(-(1000 / damping).min(cap)),
            "顶部过冲为负(滚出内容上界)"
        );
        Ok(())
    }

    /// 用户配置把过冲上限压到 0.1 行:确认上限真实参与夹取——默认上限远大于单次按键
    /// 能产生的超出量,常规路径永远命中不了 clamp 分支。
    #[test]
    fn overshoot_clamped_by_config_cap() -> color_eyre::Result<()> {
        let path =
            std::env::temp_dir().join(format!("mineral-glide-cap-{}.lua", std::process::id()));
        std::fs::write(
            &path,
            "return { tui = { lyrics = { overshoot_max_permille = 100 } } }",
        )?;
        let loaded = mineral_config::load(&path);
        std::fs::remove_file(&path).ok();
        let (cfg, warnings) = loaded?;
        assert!(warnings.is_empty(), "覆盖配置应干净落型: {warnings:?}");
        let mut s = fullscreen_with_cfg(AppState::new(std::sync::Arc::new(cfg)), timed_lines())?;
        s.scroll_lyrics(ScrollStep::LineUp); // 行 0 再上滚:阻尼后 333 → 夹到 100
        assert_eq!(to_milli(&s), Some(-100), "过冲被配置上限夹住");
        Ok(())
    }

    #[test]
    fn overshoot_bounces_back_to_boundary() -> color_eyre::Result<()> {
        // 无时间戳歌隔离回锚路径:settle 后若仍停在过冲点说明没弹回。
        let mut s = fullscreen_with(untimed_lines())?;
        s.tick_lyric_scroll();
        s.scroll_lyrics(ScrollStep::PageDown);
        s.scroll_lyrics(ScrollStep::PageDown); // 撞底过冲
        for _ in 0..200 {
            s.tick_lyric_scroll();
        }
        assert_eq!(target(&s), Some(19), "无时间戳歌弹回后停在末行不回锚");
        assert_eq!(
            s.manual_lyric_anchor_milli(),
            Some(19_000),
            "过冲 settle 后自动弹回 clamp 边界"
        );
        Ok(())
    }

    #[test]
    fn synced_overshoot_still_reattaches() -> color_eyre::Result<()> {
        let mut s = fullscreen_with(timed_lines())?;
        s.tick_lyric_scroll();
        s.scroll_lyrics(ScrollStep::LineUp); // 顶部过冲
        for _ in 0..3000 {
            s.tick_lyric_scroll();
        }
        assert!(s.lyric_scroll.is_none(), "过冲弹回后空闲超时仍正常回锚");
        Ok(())
    }

    #[test]
    fn synced_reattaches_after_timeout() -> color_eyre::Result<()> {
        let mut s = fullscreen_with(timed_lines())?;
        let page = i64::try_from(*s.cfg.tui().behavior().page_scroll_rows())?;
        s.tick_lyric_scroll(); // 先注册当前歌
        s.scroll_lyrics(ScrollStep::PageDown);
        assert_eq!(target(&s), Some(page), "脱离锚定行 = 一页步长");
        for _ in 0..3000 {
            s.tick_lyric_scroll();
        }
        assert!(
            s.lyric_scroll.is_none(),
            "synced 歌空闲超时平滑回锚后归附着"
        );
        Ok(())
    }

    #[test]
    fn unsynced_never_reattaches() -> color_eyre::Result<()> {
        let mut s = fullscreen_with(untimed_lines())?;
        let page = i64::try_from(*s.cfg.tui().behavior().page_scroll_rows())?;
        s.tick_lyric_scroll();
        s.scroll_lyrics(ScrollStep::PageDown);
        for _ in 0..3000 {
            s.tick_lyric_scroll();
        }
        assert_eq!(target(&s), Some(page), "无时间戳歌停在手动锚定行不回锚");
        Ok(())
    }

    #[test]
    fn song_change_resets_scroll() -> color_eyre::Result<()> {
        let mut s = fullscreen_with(untimed_lines())?;
        let page = i64::try_from(*s.cfg.tui().behavior().page_scroll_rows())?;
        s.tick_lyric_scroll();
        s.scroll_lyrics(ScrollStep::PageDown);
        assert_eq!(target(&s), Some(page));
        s.playback.track = Some(feiyu_song());
        s.tick_lyric_scroll();
        assert!(s.lyric_scroll.is_none(), "换歌清脱离态");
        Ok(())
    }

    #[test]
    fn manual_scroll_does_not_drift_with_playback() -> color_eyre::Result<()> {
        // detach 后推进播放位置 + tick,锚点目标行不应被播放推动(完全独立)。
        let mut s = fullscreen_with(timed_lines())?;
        let page = i64::try_from(*s.cfg.tui().behavior().page_scroll_rows())?;
        s.tick_lyric_scroll();
        s.scroll_lyrics(ScrollStep::PageDown);
        assert_eq!(target(&s), Some(page));
        // 播放推进到第 5 行附近,但仍在 reattach 超时窗口内。
        for step in 1..50u64 {
            s.playback.position_ms = step * 1000;
            s.tick_lyric_scroll();
        }
        assert_eq!(target(&s), Some(page), "脱离态锚点不随播放漂移");
        Ok(())
    }
}
