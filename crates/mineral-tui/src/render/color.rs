//! 颜色工具:RGB 之间整数 lerp + HSV 色相旋转,被歌词渐变 / 频谱渐变 / 频谱色相漂移共用。

use ratatui::style::Color;

/// 在两个 `Color::Rgb` 之间按 `num/denom` 整数比例 lerp。非 RGB 主题降级为二态。
pub fn lerp_color(from: Color, to: Color, num: u64, denom: u64) -> Color {
    match (from, to) {
        (Color::Rgb(fr, fg, fb), Color::Rgb(tr, tg, tb)) => Color::Rgb(
            lerp_byte(fr, tr, num, denom),
            lerp_byte(fg, tg, num, denom),
            lerp_byte(fb, tb, num, denom),
        ),
        _ => {
            if num.saturating_mul(2) >= denom {
                to
            } else {
                from
            }
        }
    }
}

/// `(a*(d-n) + b*n) / d`,纯整数,不踩 `as_conversions` lint。
///
/// 中间乘积用 `u128`:`255 * u64::MAX` 远超 `u64`,小输入(帧数 / 比例)行为不变,
/// 大 `denom` 也不会溢出 panic。
pub fn lerp_byte(a: u8, b: u8, num: u64, denom: u64) -> u8 {
    let denom = u128::from(denom.max(1));
    let num = u128::from(num).min(denom);
    let a128 = u128::from(a);
    let b128 = u128::from(b);
    let res = (a128 * (denom - num) + b128 * num) / denom;
    u8::try_from(res).unwrap_or(0)
}

/// 在两个千分比 alpha 间按 `num/denom` 整数比例 lerp(歌词距离淡出的 alpha 阶梯:
/// 近端 `strong` 渐至远端 `ghost`)。结果 clamp 进 `0..=1000`。
pub fn lerp_permille(from: u16, to: u16, num: u64, denom: u64) -> u16 {
    let denom = i64::try_from(denom.max(1)).unwrap_or(i64::MAX);
    let num = i64::try_from(num).unwrap_or(i64::MAX).clamp(0, denom);
    let (a, b) = (i64::from(from), i64::from(to));
    let v = a + (b - a) * num / denom;
    u16::try_from(v.clamp(0, 1000)).unwrap_or(1000)
}

/// BT.601 亮度(0-255 定点)。
fn luma(r: u8, g: u8, b: u8) -> i32 {
    (299 * i32::from(r) + 587 * i32::from(g) + 114 * i32::from(b)) / 1000
}

/// 0-1 的比例折 0-255 亮度差(clamp + round),喂 [`soften_over_bg`]。
#[allow(clippy::as_conversions)] // reason: 已 clamp 进 0..=255 且 round,转换语义无损
pub fn luma255_of(ratio: f32) -> u8 {
    (ratio.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// 盲文点阵落笔色对**实际背景**的平滑亮度分离:点与背景亮度差不足 `min_luma` 时,
/// 按缺口 `min_luma − |Δluma|` 把点**向白**提亮;缺口随差值连续归零,故背景平滑
/// 变化时输出连续、无跳变。**只往亮抬、绝不压暗**——不制造调色板外的近黑色。
///
/// 单向连续是刻意的:若按「远离背景亮度」双向拉开,点比背景暗时会被推向黑,且方向
/// 在 `fg=bg` 处翻转、输出出现亮度断崖;平滑漂动的氛围场扫过这道断崖就是一条会移动
/// 的黑边。单向只提亮无此突变,代价是点与背景亮度极接近的一瞬只被抬到「刚好分开」
/// (柔和淡入,而非硬顶强对比)。
///
/// 已充分分离(`|Δluma| ≥ min_luma`)、`min_luma = 0`、点已近纯白、任一非真彩时原样返回。
///
/// # Params:
///   - `color`: 点的原始色(封面色场采样)
///   - `bg`: 该点**实际**落处的背景色(氛围场,逐列现采)
///   - `min_luma`: 目标最小亮度差(0-255,`dot_bg_contrast` 折算;`0` = 关闭)
///
/// # Return:
///   提亮后的点色(色相尽量保留,只沿向白方向抬亮度)。
pub fn soften_over_bg(color: Color, bg: Color, min_luma: u8) -> Color {
    let (Color::Rgb(r, g, b), Color::Rgb(bg_r, bg_g, bg_b)) = (color, bg) else {
        return color;
    };
    let fg_luma = luma(r, g, b);
    let bg_luma = luma(bg_r, bg_g, bg_b);
    // 缺口:亮度差不足 min 的部分。连续、非负,充分分离时为 0。
    let deficit = (i32::from(min_luma) - (fg_luma - bg_luma).abs()).max(0);
    let headroom = 255 - fg_luma;
    if deficit == 0 || headroom <= 0 {
        return color; // 已够分开,或点已近纯白、无从更亮。
    }
    // 只往白抬:目标亮度 = fg + 缺口(封顶纯白)。lerp 比例 = 缺口 / (255 − fg)。
    let num = deficit.min(headroom);
    let t = (num * 1000) / headroom;
    lerp_color(
        color,
        Color::Rgb(255, 255, 255),
        u64::try_from(t).unwrap_or(0),
        1000,
    )
}

/// 把 `Color::Rgb` 在 HSV 色环上旋转 `deg` 度,饱和度 / 明度不变。
/// 非 RGB 主题色直接返回原值。
#[allow(clippy::as_conversions)]
pub fn rotate_hue(color: Color, deg: f32) -> Color {
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    let (h, s, v) = rgb_to_hsv(r, g, b);
    let new_h = (h + deg).rem_euclid(360.0);
    let (nr, ng, nb) = hsv_to_rgb(new_h, s, v);
    Color::Rgb(nr, ng, nb)
}

/// RGB → HSV(h ∈ [0, 360),s/v ∈ [0, 1])。
#[allow(clippy::as_conversions)]
fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let r = f32::from(r) / 255.0;
    let g = f32::from(g) / 255.0;
    let b = f32::from(b) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let v = max;
    let s = if max > 0.0 { delta / max } else { 0.0 };
    let h = if delta < f32::EPSILON {
        0.0
    } else if (max - r).abs() < f32::EPSILON {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if (max - g).abs() < f32::EPSILON {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    (h, s, v)
}

/// HSV → RGB(h ∈ [0, 360),s/v ∈ [0, 1])。clamp 到 0..=255 兜底浮点误差。
#[allow(clippy::as_conversions)]
fn hsv_to_rgb(h_deg: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let h_prime = h_deg / 60.0;
    let x = c * (1.0 - ((h_prime % 2.0) - 1.0).abs());
    let (r1, g1, b1) = match h_prime as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let to_byte = |f: f32| ((f + m) * 255.0).clamp(0.0, 255.0) as u8;
    (to_byte(r1), to_byte(g1), to_byte(b1))
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::{lerp_byte, lerp_color, lerp_permille, rotate_hue};

    /// `lerp_byte`:端点 / 中点 / num 越界被 clamp 到 denom。
    #[test]
    fn lerp_byte_endpoints_and_midpoint() {
        assert_eq!(lerp_byte(10, 20, 0, 1), 10);
        assert_eq!(lerp_byte(10, 20, 1, 1), 20);
        assert_eq!(lerp_byte(0, 255, 1, 2), 127);
        assert_eq!(lerp_byte(0, 100, 5, 2), 100);
    }

    /// `lerp_permille`:端点 / 中点、降序(strong→ghost)方向、num 越界 clamp 到 denom。
    #[test]
    fn lerp_permille_endpoints_and_descending() {
        assert_eq!(lerp_permille(780, 110, 0, 5), 780, "近端");
        assert_eq!(lerp_permille(780, 110, 5, 5), 110, "远端");
        assert_eq!(lerp_permille(780, 110, 1, 2), 445, "中点(降序)");
        assert_eq!(lerp_permille(100, 900, 1, 2), 500, "升序同样成立");
        assert_eq!(lerp_permille(780, 110, 9, 5), 110, "num 越界 clamp");
    }

    /// 真彩亮度(测试内联,与 `color::luma` 同式);非真彩给 -1。
    fn tl(c: Color) -> i32 {
        let Color::Rgb(r, g, b) = c else {
            return -1;
        };
        (299 * i32::from(r) + 587 * i32::from(g) + 114 * i32::from(b)) / 1000
    }

    /// `soften_over_bg`:关闭 / 充分分离 / 非真彩原样;撞色时只向白提亮、**绝不压暗**;
    /// 且对背景**连续**——`fg=bg` 两侧无亮度断崖(旧双向保底会在此处骤降到近黑)。
    #[test]
    fn soften_over_bg_lifts_toward_light_only() {
        use super::{luma255_of, soften_over_bg};

        let mid = Color::Rgb(100, 100, 100);
        assert_eq!(soften_over_bg(mid, mid, 0), mid, "min = 0 关闭");
        assert_eq!(
            soften_over_bg(Color::Red, mid, 64),
            Color::Red,
            "非真彩原样"
        );
        assert_eq!(
            soften_over_bg(Color::Rgb(10, 10, 10), Color::Rgb(200, 200, 200), 64),
            Color::Rgb(10, 10, 10),
            "亮度差已 ≥ min,原样"
        );

        // 暗点撞更亮背景(旧双向会把它压到近黑 luma≈6):只往亮,不暗于原色、不落近黑。
        let fg = Color::Rgb(18, 18, 26);
        let out = soften_over_bg(fg, Color::Rgb(70, 70, 70), 64);
        assert!(tl(out) >= tl(fg), "永不压暗:{} >= {}", tl(out), tl(fg));
        assert!(tl(out) > 8, "绝不落近黑(旧双向到 ~6),got {}", tl(out));

        // 同色撞:抬开约 min(柔和分离,定点误差内)。
        let lifted = soften_over_bg(mid, mid, 64);
        assert!(
            (tl(lifted) - 100 - 64).abs() <= 3,
            "同色应抬开约 min,got Δ{}",
            tl(lifted) - 100
        );

        // 连续性:背景跨过 fg 亮度两侧,输出不跳变(旧双向此处骤降 ~128)。
        let f = Color::Rgb(80, 80, 80);
        let below = soften_over_bg(f, Color::Rgb(79, 79, 79), 64);
        let above = soften_over_bg(f, Color::Rgb(81, 81, 81), 64);
        assert!(
            (tl(below) - tl(above)).abs() <= 4,
            "fg=bg 两侧应连续,got {} vs {}",
            tl(below),
            tl(above)
        );

        assert_eq!(luma255_of(0.25), 64, "0-1 比例折 0-255 亮度差");
    }

    /// `lerp_color`:Rgb 逐分量 lerp;非 Rgb 降级二态(< 半 → from,≥ 半 → to)。
    #[test]
    fn lerp_color_rgb_and_fallback() {
        let black = Color::Rgb(0, 0, 0);
        let white = Color::Rgb(255, 255, 255);
        assert_eq!(lerp_color(black, white, 0, 1), black);
        assert_eq!(lerp_color(black, white, 1, 1), white);
        assert_eq!(lerp_color(black, white, 1, 2), Color::Rgb(127, 127, 127));
        assert_eq!(lerp_color(Color::Red, Color::Blue, 0, 1), Color::Red);
        assert_eq!(lerp_color(Color::Red, Color::Blue, 1, 2), Color::Blue);
    }

    /// `rotate_hue`:0° / 360° 不变,180° 反相(红→青);非 Rgb 原样返回。
    #[test]
    fn rotate_hue_identity_and_opposite() {
        let red = Color::Rgb(255, 0, 0);
        assert_eq!(rotate_hue(red, 0.0), red);
        assert_eq!(rotate_hue(red, 360.0), red);
        assert_eq!(rotate_hue(red, 180.0), Color::Rgb(0, 255, 255));
        assert_eq!(rotate_hue(Color::Red, 123.0), Color::Red);
    }

    use proptest::prelude::{any, proptest};

    proptest! {
        /// `lerp_byte` 结果恒在 [min(a,b), max(a,b)] 内(任意比例 / denom)。
        #[test]
        fn lerp_byte_in_range(a in any::<u8>(), b in any::<u8>(), num in any::<u64>(), denom in any::<u64>()) {
            let r = lerp_byte(a, b, num, denom);
            proptest::prop_assert!(r >= a.min(b) && r <= a.max(b));
        }

        /// `rotate_hue` 对 Rgb 输入恒返回 Rgb(任意角度都不 panic / 不掉变体 / 留在色彩域)。
        #[test]
        fn rotate_hue_keeps_rgb(r in any::<u8>(), g in any::<u8>(), b in any::<u8>(), deg in -1000.0_f32..1000.0) {
            let out = rotate_hue(Color::Rgb(r, g, b), deg);
            proptest::prop_assert!(matches!(out, Color::Rgb(..)));
        }

        /// `soften_over_bg` 永不压暗:任意点色 / 背景 / min,输出亮度恒 ≥ 原色亮度
        /// (根除「往黑压」的核心不变量)。
        #[test]
        fn soften_never_darkens(fr in any::<u8>(), fgc in any::<u8>(), fb in any::<u8>(),
                                br in any::<u8>(), bgc in any::<u8>(), bb in any::<u8>(),
                                min in any::<u8>()) {
            let color = Color::Rgb(fr, fgc, fb);
            let out = super::soften_over_bg(color, Color::Rgb(br, bgc, bb), min);
            proptest::prop_assert!(tl(out) >= tl(color));
        }
    }
}
