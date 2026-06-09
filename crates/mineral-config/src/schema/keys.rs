//! 键位重映射段(动作 → 键),挂在 `TuiConfig` 下。
//!
//! 方向是【动作 → 键】(非「键 → 动作」):与深合并语义强耦合——用户覆盖
//! `keys.play_pause = "x"` 即干净替换;若反向,旧键无法用 Lua `nil` 删除。
//! 字段集 = 内建动作展开后的稳定命令名;每字段值为单键或键数组(数组整体替换)。
//! 键字符串 → 和弦的解析复用 [`crate::keys::KeyChord::parse`],不在此重复定义。

use rustc_hash::FxHashMap;
use serde::Deserialize;

use crate::keys::KeyChord;

/// 键位重映射表:每个字段是一个内建动作的稳定命令名,值为绑定到它的键。
///
/// 字段集与渲染层内建键表一一对应(无参动作直名;带参动作按方向/幅度展开)。
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取(如 `keys.play_pause()`)。
/// 本段只承载强类型绑定;把命令名 + 步长参数组装成可执行动作是 client 接线的事。
#[derive(Clone, Debug, Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct KeysConfig {
    /// 暂停 / 恢复。
    play_pause: KeyBinding,

    /// 下一首。
    next: KeyBinding,

    /// 上一首 / 回开头。
    prev: KeyBinding,

    /// 进 / 退全屏播放态。
    toggle_fullscreen: KeyBinding,

    /// 打开浮动播放队列。
    open_queue: KeyBinding,

    /// 打开退出确认浮层。
    quit: KeyBinding,

    /// 循环歌词副语言。
    cycle_lyric: KeyBinding,

    /// 进入搜索输入态。
    enter_search: KeyBinding,

    /// 在当前视图「进入」。
    activate: KeyBinding,

    /// 在当前视图「返回」(搜索非空时先清搜索)。
    back: KeyBinding,

    /// 循环播放模式。
    cycle_mode: KeyBinding,

    /// 音量增(步长见 `behavior.volume_step`)。
    volume_up: KeyBinding,

    /// 音量减(步长见 `behavior.volume_step`)。
    volume_down: KeyBinding,

    /// 快进(步长见 `behavior.seek_step_secs`)。
    seek_forward: KeyBinding,

    /// 快退(步长见 `behavior.seek_step_secs`)。
    seek_backward: KeyBinding,

    /// 大步快进(步长见 `behavior.seek_big_step_secs`)。
    seek_forward_big: KeyBinding,

    /// 大步快退(步长见 `behavior.seek_big_step_secs`)。
    seek_backward_big: KeyBinding,

    /// 列表光标下移一行。
    move_down: KeyBinding,

    /// 列表光标上移一行。
    move_up: KeyBinding,

    /// 列表光标大步下移(行数见 `behavior.list_jump_rows`)。
    move_down_big: KeyBinding,

    /// 列表光标大步上移(行数见 `behavior.list_jump_rows`)。
    move_up_big: KeyBinding,

    /// 列表光标跳首行。
    move_first: KeyBinding,

    /// 列表光标跳末行。
    move_last: KeyBinding,

    /// 切换选中曲的 ♥。
    love: KeyBinding,

    /// 下载当前视图选中项。
    download: KeyBinding,

    /// 全屏歌词逐行下滚(行数见 `lyrics.line_scroll_rows`)。
    lyric_line_down: KeyBinding,

    /// 全屏歌词逐行上滚(行数见 `lyrics.line_scroll_rows`)。
    lyric_line_up: KeyBinding,

    /// 全屏歌词翻页下滚(行数见 `lyrics.page_scroll_rows`)。
    lyric_page_down: KeyBinding,

    /// 全屏歌词翻页上滚(行数见 `lyrics.page_scroll_rows`)。
    lyric_page_up: KeyBinding,

    /// 脚本动作绑定:`mineral.action` 注册名 → 键(开放映射,默认空)。
    /// 与内建动作不同,这里的键集合由用户脚本决定,client 触发时经
    /// daemon 转投脚本线程执行。
    script: FxHashMap<String, KeyBinding>,
}

/// 一个动作的键绑定:单键(`"space"`)或键数组(`{"n", "j"}`)。
///
/// 反序列化时每个键字符串经 [`KeyChord::parse`] 归一;数组语义是整体替换
/// (与深合并一致,见主设计 D3)。内部存 `Vec<KeyChord>`,经 [`Self::chords`] 读。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyBinding {
    /// 绑定到该动作的归一化和弦(可多键)。
    chords: Vec<KeyChord>,
}

impl KeyBinding {
    /// 取绑定的全部和弦。
    ///
    /// # Return:
    ///   归一化和弦切片(可能为空,表示用户清空了该绑定)
    pub fn chords(&self) -> &[KeyChord] {
        &self.chords
    }
}

impl<'de> Deserialize<'de> for KeyBinding {
    /// 接受单个键字符串或键字符串数组,逐元素 [`KeyChord::parse`];
    /// 解析失败返 `de::Error`(经 `serde_path_to_error` 带字段路径)。
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(BindingVisitor)
    }
}

/// `KeyBinding` 的反序列化访问器:容忍标量字符串与字符串序列两种形态。
struct BindingVisitor;

impl<'de> serde::de::Visitor<'de> for BindingVisitor {
    type Value = KeyBinding;

    /// 期望形态描述(serde 错误信息用)。
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("键字符串或键字符串数组")
    }

    /// 单键形态:`"space"` → 单和弦绑定。
    fn visit_str<E>(self, value: &str) -> Result<KeyBinding, E>
    where
        E: serde::de::Error,
    {
        let chord = KeyChord::parse(value).map_err(|e| E::custom(format!("{e}")))?;
        Ok(KeyBinding {
            chords: vec![chord],
        })
    }

    /// 数组形态:`{"n", "j"}` → 多和弦绑定(整体替换)。
    fn visit_seq<A>(self, mut seq: A) -> Result<KeyBinding, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut chords = Vec::<KeyChord>::new();
        while let Some(raw) = seq.next_element::<String>()? {
            let chord =
                KeyChord::parse(&raw).map_err(|e| serde::de::Error::custom(format!("{e}")))?;
            chords.push(chord);
        }
        Ok(KeyBinding { chords })
    }
}

#[cfg(test)]
mod tests {
    use super::KeyBinding;
    use crate::keys::{Key, KeyChord};

    #[test]
    fn single_key_parses() -> color_eyre::Result<()> {
        let b: KeyBinding = serde_json::from_value(serde_json::json!("<Space>"))?;
        assert_eq!(b.chords(), &[KeyChord::plain(Key::Char(' '))]);
        Ok(())
    }

    #[test]
    fn array_parses_as_multiple() -> color_eyre::Result<()> {
        let b: KeyBinding = serde_json::from_value(serde_json::json!(["n", "j"]))?;
        assert_eq!(
            b.chords(),
            &[
                KeyChord::plain(Key::Char('n')),
                KeyChord::plain(Key::Char('j')),
            ]
        );
        Ok(())
    }

    #[test]
    fn invalid_key_errors() {
        assert!(
            serde_json::from_value::<KeyBinding>(serde_json::json!("nope")).is_err(),
            "未知键名应报错"
        );
    }
}
