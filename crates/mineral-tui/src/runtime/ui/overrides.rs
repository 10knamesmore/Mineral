//! 脚本 UI 旋钮覆盖的 client 边缘映射 —— 全链路**唯一**认识 key 的地方。
//!
//! daemon 对 `mineral.ui.override` 零解释,只把 `(key, value)` 经
//! `Event::UiOverride` 转发到这里;本模块把 opaque 对校验形状后落到具体
//! 渲染旋钮,未知 key / 形状不符 warn + 丢。`value = None` 撤销覆盖,
//! 渲染处回落配置值。session 级:daemon 重启 / 撤销即回原样,不碰配置文件。

use mineral_protocol::BusValue;

/// 当前生效的旋钮覆盖。每个字段对应一个旋钮 key,`None` = 无覆盖
/// (渲染处读配置值)。加新可覆盖旋钮 = 加字段 + [`Self::apply`] 一臂。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct UiOverrides {
    /// `"lyrics.fullscreen_line_gap"`:全屏沉浸态歌词行间距(行)。
    pub fullscreen_line_gap: Option<usize>,

    /// `"lyrics.compact_line_gap"`:非全屏紧凑态歌词行间距(行)。
    pub compact_line_gap: Option<usize>,

    /// `"window_title.text"`:整串覆盖窗口标题(脚本自渲染;`None` = 回落结构化模板)。
    pub window_title_text: Option<String>,
}

impl UiOverrides {
    /// 应用一条覆盖:已知 key 校验值形状落字段;未知 key warn + 丢。
    ///
    /// # Params:
    ///   - `key`: 旋钮键(约定 = 配置路径)
    ///   - `value`: 覆盖值;`None` = 撤销
    pub fn apply(&mut self, key: &str, value: Option<&BusValue>) {
        match key {
            "lyrics.fullscreen_line_gap" => self.fullscreen_line_gap = parse_usize(key, value),
            "lyrics.compact_line_gap" => self.compact_line_gap = parse_usize(key, value),
            "window_title.text" => self.window_title_text = parse_string(key, value),
            other => {
                mineral_log::warn!(target: "script", key = other, "未知 UI 覆盖键,忽略");
            }
        }
    }
}

/// 非负整数旋钮的值校验:非负 `Int` 落覆盖;`None`(撤销)清覆盖;
/// 其余形状 warn + 视同撤销(回落配置最不骗人——保持旧覆盖会让脚本
/// 误以为新值生效了)。
fn parse_usize(key: &str, value: Option<&BusValue>) -> Option<usize> {
    let v = value?;
    if let BusValue::Int(n) = v
        && let Ok(n) = usize::try_from(*n)
    {
        return Some(n);
    }
    mineral_log::warn!(
        target: "script",
        key,
        value = ?v,
        "UI 覆盖值应为非负整数,视同撤销"
    );
    None
}

/// 字符串旋钮的值校验:`Str` 落覆盖;`None`(撤销)清覆盖;其余形状 warn + 视同撤销。
fn parse_string(key: &str, value: Option<&BusValue>) -> Option<String> {
    let v = value?;
    if let BusValue::Str(s) = v {
        return Some(s.clone());
    }
    mineral_log::warn!(
        target: "script",
        key,
        value = ?v,
        "UI 覆盖值应为字符串,视同撤销"
    );
    None
}

#[cfg(test)]
mod tests {
    use mineral_protocol::BusValue;

    use super::UiOverrides;

    #[test]
    fn apply_sets_clears_and_rejects() {
        let mut o = UiOverrides::default();
        // 落覆盖。
        o.apply("lyrics.fullscreen_line_gap", Some(&BusValue::Int(2)));
        assert_eq!(o.fullscreen_line_gap, Some(2));
        // 撤销。
        o.apply("lyrics.fullscreen_line_gap", None);
        assert_eq!(o.fullscreen_line_gap, None);
        // 形状不符:视同撤销。
        o.apply("lyrics.compact_line_gap", Some(&BusValue::Int(1)));
        o.apply(
            "lyrics.compact_line_gap",
            Some(&BusValue::Str("x".to_owned())),
        );
        assert_eq!(o.compact_line_gap, None, "形状不符应回落配置(撤销)");
        // 负数:视同撤销。
        o.apply("lyrics.fullscreen_line_gap", Some(&BusValue::Int(-1)));
        assert_eq!(o.fullscreen_line_gap, None);
        // 未知 key:不动任何字段。
        o.apply("no.such.knob", Some(&BusValue::Int(9)));
        assert_eq!(o, UiOverrides::default());
    }

    /// `window_title.text`:Str 落覆盖;非 Str 视同撤销;nil 撤销。
    #[test]
    fn window_title_text_string_knob() {
        let mut o = UiOverrides::default();
        o.apply(
            "window_title.text",
            Some(&BusValue::Str("⏸ 歌名".to_owned())),
        );
        assert_eq!(o.window_title_text.as_deref(), Some("⏸ 歌名"));
        // 非字符串:视同撤销。
        o.apply("window_title.text", Some(&BusValue::Int(1)));
        assert_eq!(o.window_title_text, None, "非字符串应回落(撤销)");
        // 撤销。
        o.apply("window_title.text", Some(&BusValue::Str("x".to_owned())));
        o.apply("window_title.text", None);
        assert_eq!(o.window_title_text, None);
    }
}
