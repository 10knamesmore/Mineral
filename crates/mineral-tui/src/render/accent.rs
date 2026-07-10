//! 封面驱动的动态 accent:把封面派生的一对强调色渐变进 effective theme。
//!
//! [`AccentFade`] 是一个极小的颜色过渡状态机(仿频谱色场过渡的 from/to/frame 范式):
//! app 层每帧 `tick` 推进、draw 前用 [`AccentFade::apply`] 把 base theme 合成为
//! effective theme——只覆写 `accent` / `accent_2` 两个 token,其余字段原样透传。
//! 目标为 `None` 表示回落 base(无封面 / 取色失败 / 功能关闭),回落同样走渐变;
//! 回落终点在 `apply` 时现读 base,故渐变途中热更主题也会追到新 token 色。

use ratatui::style::Color;

use crate::render::color::lerp_color;
use crate::render::palette::Rgb;
use crate::render::theme::Theme;

/// 封面派生的一对强调色(sRGB,派生时已做可读性明度 clamp 与色相分离)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccentPair {
    /// 主强调色(渐变终点上的 `Theme::accent`)。
    pub accent: Rgb,

    /// 副强调色(渐变终点上的 `Theme::accent_2`)。
    pub accent_2: Rgb,
}

impl AccentPair {
    /// 转成渲染色对 `(accent, accent_2)`。
    fn colors(self) -> (Color, Color) {
        (
            Color::Rgb(self.accent.r, self.accent.g, self.accent.b),
            Color::Rgb(self.accent_2.r, self.accent_2.g, self.accent_2.b),
        )
    }
}

/// accent 渐变状态机:从「切换那刻的可见色」线性过渡到目标色对。
///
/// 与频谱色场过渡并行驱动(同一个封面身份 diff 触发),但时长独立
/// (配置 `theme.dynamic.fade_ms`)。打断(渐变途中换目标)时把当前插值色
/// 冻结为新起点,不跳变。
#[derive(Clone, Debug)]
pub struct AccentFade {
    /// 过渡起点色对(设目标那刻的可见色,已冻结为具体色)。
    from: (Color, Color),

    /// 过渡目标:`Some` = 封面派生色,`None` = 回落 base theme 的静态 token。
    to: Option<AccentPair>,

    /// 已过渡拍数,推进到 `fade_ticks` 后静止。
    frame: u32,

    /// 全程拍数(`fade_ms` 按帧率折算,恒 ≥ 1)。
    fade_ticks: u32,
}

impl AccentFade {
    /// 构造一个已静止在 base 上的状态机(启动初态:无封面色,theme 原样)。
    ///
    /// # Params:
    ///   - `fade_ticks`: 渐变全程拍数(`theme.dynamic.fade_ms` 折算;`0` 提为 `1`)
    pub fn new(fade_ticks: u32) -> Self {
        let fade_ticks = fade_ticks.max(1);
        Self {
            from: (Color::Reset, Color::Reset),
            to: None,
            frame: fade_ticks,
            fade_ticks,
        }
    }

    /// 设置新的渐变目标:把**当前可见色**冻结为起点,进度归零。
    ///
    /// 目标与现行目标相同时是空操作(不重启渐变)——身份 diff 在 app 层,
    /// 这里再兜一层防热更等路径重复投喂。
    ///
    /// # Params:
    ///   - `to`: `Some` = 封面派生色对;`None` = 渐变回 base 静态 token
    ///   - `base`: 现行 base theme(冻结当前可见色用)
    pub fn set_target(&mut self, to: Option<AccentPair>, base: &Theme) {
        if to == self.to {
            return;
        }
        self.from = self.current(base);
        self.to = to;
        self.frame = 0;
    }

    /// 推进一拍(到程后饱和,静止不再变化)。
    pub fn tick(&mut self) {
        self.frame = self.frame.saturating_add(1).min(self.fade_ticks);
    }

    /// 重设全程拍数而**保留相位**(进度比例不变):配置热更 `fade_ms` 时调用,
    /// 渐变不回跳、只换后续速度。
    ///
    /// # Params:
    ///   - `fade_ticks`: 新全程拍数(`0` 提为 `1`)
    pub fn retempo(&mut self, fade_ticks: u32) {
        let fade_ticks = fade_ticks.max(1);
        let scaled = u64::from(self.frame).saturating_mul(u64::from(fade_ticks))
            / u64::from(self.fade_ticks.max(1));
        self.frame = u32::try_from(scaled).unwrap_or(fade_ticks).min(fade_ticks);
        self.fade_ticks = fade_ticks;
    }

    /// 用 base theme 合成 effective theme:覆写 `accent` / `accent_2`,其余原样。
    ///
    /// 已静止且无封面目标时原样返回(常态零开销)。注:`search_hit_color` 在
    /// `Theme::from_config` 时已按静态 token 解析定型,不随动态 accent 联动。
    ///
    /// # Params:
    ///   - `base`: 配置落地的 base theme
    ///
    /// # Return:
    ///   effective theme(值拷贝,`Theme` 是 `Copy`)。
    pub fn apply(&self, base: Theme) -> Theme {
        if self.settled() && self.to.is_none() {
            return base;
        }
        let (accent, accent_2) = self.current(&base);
        let mut effective = base;
        effective.accent = accent;
        effective.accent_2 = accent_2;
        effective
    }

    /// 当前可见色对:静止在终点,或起点→终点按 `frame/fade_ticks` 线性插值。
    /// 终点为 `None` 时现读 base 的静态 token(渐变途中热更主题即追新色)。
    fn current(&self, base: &Theme) -> (Color, Color) {
        let end = match self.to {
            Some(pair) => pair.colors(),
            None => (base.accent, base.accent_2),
        };
        if self.settled() {
            return end;
        }
        let num = u64::from(self.frame);
        let denom = u64::from(self.fade_ticks.max(1));
        (
            lerp_color(self.from.0, end.0, num, denom),
            lerp_color(self.from.1, end.1, num, denom),
        )
    }

    /// 渐变是否已到程(静止)。
    fn settled(&self) -> bool {
        self.frame >= self.fade_ticks
    }
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::{AccentFade, AccentPair};
    use crate::render::palette::Rgb;
    use crate::render::theme::Theme;

    /// 测试目标色对:红 / 蓝(与 mocha 默认 accent 都拉得开)。
    fn red_blue() -> AccentPair {
        AccentPair {
            accent: Rgb::new(200, 40, 40),
            accent_2: Rgb::new(40, 40, 200),
        }
    }

    /// 初态(无封面目标)是恒等合成:effective 与 base 逐字段一致。
    #[test]
    fn settled_none_is_identity() {
        let base = Theme::mocha_mauve();
        let fade = AccentFade::new(/*fade_ticks*/ 10);
        let effective = fade.apply(base);
        assert_eq!(format!("{effective:?}"), format!("{base:?}"));
    }

    /// 设目标后:起点 = base 色,中点 = 线性插值,到程 = 目标色;其余 token 不动。
    #[test]
    fn fades_to_target_then_settles() {
        let base = Theme::mocha_mauve();
        let mut fade = AccentFade::new(/*fade_ticks*/ 10);
        fade.set_target(Some(red_blue()), &base);
        assert_eq!(
            fade.apply(base).accent,
            base.accent,
            "frame 0 应从 base 起步"
        );

        for _ in 0..5 {
            fade.tick();
        }
        let mid = fade.apply(base);
        assert_eq!(
            mid.accent,
            crate::render::color::lerp_color(base.accent, Color::Rgb(200, 40, 40), 5, 10),
            "中点应是线性插值"
        );
        assert_eq!(mid.base, base.base, "非 accent token 不受染");

        for _ in 0..5 {
            fade.tick();
        }
        let done = fade.apply(base);
        assert_eq!(done.accent, Color::Rgb(200, 40, 40));
        assert_eq!(done.accent_2, Color::Rgb(40, 40, 200));
    }

    /// 打断:渐变途中换目标,起点冻结为打断那刻的可见色——换目标前后 apply 同色,不跳变。
    #[test]
    fn retarget_freezes_current_color_no_jump() {
        let base = Theme::mocha_mauve();
        let mut fade = AccentFade::new(/*fade_ticks*/ 10);
        fade.set_target(Some(red_blue()), &base);
        for _ in 0..4 {
            fade.tick();
        }
        let before = fade.apply(base).accent;
        fade.set_target(
            Some(AccentPair {
                accent: Rgb::new(10, 200, 10),
                accent_2: Rgb::new(200, 200, 10),
            }),
            &base,
        );
        assert_eq!(fade.apply(base).accent, before, "打断那帧不应跳色");
    }

    /// 回落:静止在封面色上后目标置 `None`,渐变回 base 静态 token。
    #[test]
    fn fades_back_to_base() {
        let base = Theme::mocha_mauve();
        let mut fade = AccentFade::new(/*fade_ticks*/ 10);
        fade.set_target(Some(red_blue()), &base);
        for _ in 0..10 {
            fade.tick();
        }
        fade.set_target(/*to*/ None, &base);
        let mid = fade.apply(base).accent;
        assert_ne!(mid, base.accent, "回落起点是封面色,不瞬跳");
        for _ in 0..10 {
            fade.tick();
        }
        assert_eq!(fade.apply(base).accent, base.accent, "到程应回到 base");
    }

    /// retempo 保相位:半程改时长,进度比例不变(apply 结果那一帧不动)。
    #[test]
    fn retempo_preserves_phase() {
        let base = Theme::mocha_mauve();
        let mut fade = AccentFade::new(/*fade_ticks*/ 10);
        fade.set_target(Some(red_blue()), &base);
        for _ in 0..5 {
            fade.tick();
        }
        let before = fade.apply(base).accent;
        fade.retempo(/*fade_ticks*/ 20);
        assert_eq!(
            fade.apply(base).accent,
            before,
            "retempo 不应改变当前帧颜色"
        );
    }

    /// 同目标重复投喂是空操作:进度不归零(防热更路径重启渐变)。
    #[test]
    fn same_target_does_not_restart() {
        let base = Theme::mocha_mauve();
        let mut fade = AccentFade::new(/*fade_ticks*/ 10);
        fade.set_target(Some(red_blue()), &base);
        for _ in 0..7 {
            fade.tick();
        }
        let before = fade.apply(base).accent;
        fade.set_target(Some(red_blue()), &base);
        assert_eq!(fade.apply(base).accent, before, "同目标不应重启渐变");
    }
}
