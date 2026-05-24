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
pub fn lerp_byte(a: u8, b: u8, num: u64, denom: u64) -> u8 {
    let denom = denom.max(1);
    let num = num.min(denom);
    let a64 = u64::from(a);
    let b64 = u64::from(b);
    let res = (a64 * (denom - num) + b64 * num) / denom;
    u8::try_from(res).unwrap_or(0)
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

    use super::{lerp_byte, lerp_color, rotate_hue};

    /// `lerp_byte`:端点 / 中点 / num 越界被 clamp 到 denom。
    #[test]
    fn lerp_byte_endpoints_and_midpoint() {
        assert_eq!(lerp_byte(10, 20, 0, 1), 10);
        assert_eq!(lerp_byte(10, 20, 1, 1), 20);
        assert_eq!(lerp_byte(0, 255, 1, 2), 127);
        assert_eq!(lerp_byte(0, 100, 5, 2), 100);
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
}
