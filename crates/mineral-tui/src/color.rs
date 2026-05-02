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
