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

/// 0-1 的比例折 0-255 亮度差(clamp + round),喂 [`ensure_bg_contrast`]。
#[allow(clippy::as_conversions)] // reason: 已 clamp 进 0..=255 且 round,转换语义无损
pub fn luma255_of(ratio: f32) -> u8 {
    (ratio.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// 保证 `color` 与背景的亮度差 ≥ `min_luma`(0-255,定点舍入误差 ≤ 2):不足时把色
/// 向白 / 黑极 lerp 到恰好达标——只动明度,色相尽量保留。方向优先「远离背景亮度」的
/// 自然侧,该侧头顶余量不足(背景近纯白 / 纯黑)时换另一侧;两侧都不够(`min_luma`
/// 过半且背景居中)保持自然侧尽力。已达标、`min_luma` 为 0、任一非真彩时原样返回。
pub fn ensure_bg_contrast(color: Color, bg: Color, min_luma: u8) -> Color {
    let (Color::Rgb(r, g, b), Color::Rgb(bg_r, bg_g, bg_b)) = (color, bg) else {
        return color;
    };
    let fg_luma = luma(r, g, b);
    let bg_luma = luma(bg_r, bg_g, bg_b);
    let min = i32::from(min_luma);
    if (fg_luma - bg_luma).abs() >= min {
        return color;
    }
    let natural_white = if fg_luma == bg_luma {
        bg_luma < 128
    } else {
        fg_luma > bg_luma
    };
    let white_reachable = 255 - bg_luma >= min;
    let black_reachable = bg_luma >= min;
    let toward_white = match (natural_white, white_reachable, black_reachable) {
        (true, true, _) | (false, true, false) => true,
        (true, false, true) | (false, _, true) => false,
        (natural, false, false) => natural,
    };
    let (pole, pole_luma) = if toward_white {
        (Color::Rgb(255, 255, 255), 255)
    } else {
        (Color::Rgb(0, 0, 0), 0)
    };
    let target = if toward_white {
        (bg_luma + min).min(255)
    } else {
        (bg_luma - min).max(0)
    };
    let denom = (pole_luma - fg_luma).abs();
    if denom == 0 {
        return pole;
    }
    // 亮度对 lerp 线性,解出恰达标的比例(分子分母必同号,按绝对值算);
    // 向上取整抵消定点截断的欠拉。
    let num = (target - fg_luma).abs();
    let t = ((num * 1000 + denom - 1) / denom).clamp(0, 1000);
    lerp_color(color, pole, u64::try_from(t).unwrap_or(0), 1000)
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

    /// `ensure_bg_contrast`:已达标 / 关闭 / 非真彩原样;撞色时向远离背景亮度的极
    /// 拉到恰好达标;背景近极点时停在极点(尽力)。
    #[test]
    fn ensure_bg_contrast_lifts_clashing_colors() {
        use super::{ensure_bg_contrast, luma255_of};

        let bg = Color::Rgb(100, 100, 100);
        let bright = Color::Rgb(250, 250, 250);
        assert_eq!(ensure_bg_contrast(bright, bg, 64), bright, "已达标原样返回");
        assert_eq!(ensure_bg_contrast(bg, bg, 0), bg, "min = 0 关闭保底");
        assert_eq!(
            ensure_bg_contrast(Color::Red, bg, 64),
            Color::Red,
            "非真彩原样返回"
        );

        // 同色撞同底(中灰):底在暗半区 → 拉白,达到最小亮度差(定点舍入误差 ≤ 2)。
        let lifted = ensure_bg_contrast(bg, bg, 64);
        let Color::Rgb(r, g, b) = lifted else {
            unreachable!("真彩入参恒真彩出参");
        };
        let diff = (299 * i32::from(r) + 587 * i32::from(g) + 114 * i32::from(b)) / 1000 - 100;
        assert!(
            (62..=66).contains(&diff),
            "拉开后的亮度差应 ≈ min,got {diff}"
        );
        assert!(r >= 100 && g >= 100 && b >= 100, "拉白方向单调不降");

        // 背景近纯白:亮侧头顶余量不足,换暗侧拉开。
        let stuck = ensure_bg_contrast(Color::Rgb(255, 255, 255), Color::Rgb(250, 250, 250), 64);
        let Color::Rgb(sr, sg, sb) = stuck else {
            unreachable!("真彩入参恒真彩出参");
        };
        let stuck_luma = (299 * i32::from(sr) + 587 * i32::from(sg) + 114 * i32::from(sb)) / 1000;
        assert!(
            250 - stuck_luma >= 62,
            "亮侧无余量应换暗侧拉开,got luma {stuck_luma}"
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
    }
}
