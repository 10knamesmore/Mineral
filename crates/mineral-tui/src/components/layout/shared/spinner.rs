//! loading spinner 帧选取:按帧计数从配置的字形数组取当前旋转帧。
//!
//! 状态层只持帧计数(每帧 +1),字形与节奏在此层定——帧数组来自用户配置
//! `animation.spinner_frames`,空数组时不画字形(loading 文案仍在)。search「searching」与
//! detail 数据未到占位共用此一处选取,旋转观感一致。

/// 每几帧换一格(≈ 60fps / 5 = 12 spinner-fps,一周约 `frames.len()` / 12 秒)。
const STEP_TICKS: u32 = 5;

/// 取当前 spinner 字形:`counter` 每 [`STEP_TICKS`] 帧前进一格,在 `frames` 上循环。
///
/// # Params:
///   - `frames`: 配置的旋转帧字形(`animation.spinner_frames`)
///   - `counter`: 帧计数(`SearchPage::spinner_counter`)
///
/// # Return:
///   当前应画的字形;`frames` 空(用户配空数组) → `""`(不画字形)。
pub(crate) fn glyph(frames: &[String], counter: u32) -> &str {
    let len = u32::try_from(frames.len()).unwrap_or(0);
    if len == 0 {
        return "";
    }
    let idx = usize::try_from((counter / STEP_TICKS) % len).unwrap_or(0);
    frames.get(idx).map_or("", String::as_str)
}

#[cfg(test)]
mod tests {
    use super::glyph;

    /// 按帧计数循环取帧;每 STEP_TICKS 帧前进一格,到末尾回绕。
    #[test]
    fn glyph_cycles_through_frames() {
        let frames = ["a".to_owned(), "b".to_owned(), "c".to_owned()];
        assert_eq!(glyph(&frames, 0), "a", "首帧");
        assert_eq!(
            glyph(&frames, super::STEP_TICKS - 1),
            "a",
            "不足一格仍是首帧"
        );
        assert_eq!(glyph(&frames, super::STEP_TICKS), "b", "满一格进下一帧");
        assert_eq!(glyph(&frames, super::STEP_TICKS * 3), "a", "走满一周回绕");
    }

    /// 空帧数组 → 空串(不画字形,仅留 loading 文案)。
    #[test]
    fn glyph_empty_frames_yields_blank() {
        assert_eq!(glyph(&[], 42), "");
    }
}
