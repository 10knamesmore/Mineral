//! 键位重映射段(动作 → 键),挂在 `TuiConfig` 下。
//!
//! 方向是【动作 → 键】(非「键 → 动作」):与深合并语义强耦合——用户覆盖
//! `keys.play_pause = "x"` 即干净替换;若反向,旧键无法用 Lua `nil` 删除。
//! 字段集 = 内建动作展开后的稳定命令名;每字段值为单键或键数组(数组整体替换)。
//! 键字符串 → 和弦的解析复用 [`crate::config::key_syntax::KeyChord::parse`],不在此重复定义。

use mineral_config_macros::config_section;
use serde::Deserialize;

use crate::config::key_syntax::KeyChord;

/// 动作对应按键，空数组解绑
#[config_section]
pub struct KeysConfig {
    /// 暂停 / 恢复。
    play_pause: KeyBinding,

    /// 下一首。
    next: KeyBinding,

    /// 上一首或回曲首，分界见 prev_restart_threshold_ms
    prev: KeyBinding,

    /// 切换全屏播放
    toggle_fullscreen: KeyBinding,

    /// 切换搜索页，全屏时无效
    open_search: KeyBinding,

    /// 切换播放队列浮层
    open_queue: KeyBinding,

    /// 切换下载浮层
    open_downloads: KeyBinding,

    /// 切换音频设置浮层
    open_audio_settings: KeyBinding,

    /// 退出确认
    quit: KeyBinding,

    /// 切换按键帮助
    open_help: KeyBinding,

    /// 切换原文、翻译、罗马音
    cycle_lyric: KeyBinding,

    /// 输入搜索词，全屏时无效
    enter_search: KeyBinding,

    /// 进入歌单或播放选中曲
    activate: KeyBinding,

    /// 返回；搜索非空时先清空
    back: KeyBinding,

    /// 进入选中项详情
    drill_into: KeyBinding,

    /// 切换详情分区
    cycle_detail_section: KeyBinding,

    /// 循环播放模式。
    cycle_mode: KeyBinding,

    /// 增大音量
    volume_up: KeyBinding,

    /// 减小音量
    volume_down: KeyBinding,

    /// 快进
    seek_forward: KeyBinding,

    /// 快退
    seek_backward: KeyBinding,

    /// 大步快进
    seek_forward_big: KeyBinding,

    /// 大步快退
    seek_backward_big: KeyBinding,

    /// 列表光标下移一行。
    move_down: KeyBinding,

    /// 列表光标上移一行。
    move_up: KeyBinding,

    /// 光标大步下移
    move_down_big: KeyBinding,

    /// 光标大步上移
    move_up_big: KeyBinding,

    /// 列表光标跳首行。
    move_first: KeyBinding,

    /// 列表光标跳末行。
    move_last: KeyBinding,

    /// 切换歌曲收藏状态
    love: KeyBinding,

    /// 队列条目下移
    reorder_down: KeyBinding,

    /// 队列条目上移
    reorder_up: KeyBinding,

    /// 定位当前播放条目
    jump_to_current: KeyBinding,

    /// 下载当前视图选中项。
    download: KeyBinding,

    /// 关闭最早的驻留通知
    dismiss_notice: KeyBinding,

    /// 打开操作菜单
    open_action_menu: KeyBinding,

    /// 打开复制菜单
    open_copy_menu: KeyBinding,

    /// 逐行下滚
    scroll_line_down: KeyBinding,

    /// 逐行上滚
    scroll_line_up: KeyBinding,

    /// 向下翻页
    scroll_page_down: KeyBinding,

    /// 向上翻页
    scroll_page_up: KeyBinding,
}

/// 一个动作的键绑定:单键(`"space"`)或键数组(`{"n", "j"}`)。
///
/// 反序列化时每个键字符串经 [`KeyChord::parse`] 归一;数组语义是整体替换
/// (与配置深合并的数组规则一致)。内部存 `Vec<KeyChord>`,经 [`Self::chords`] 读。
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
    use crate::config::key_syntax::{Key, KeyChord};

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
