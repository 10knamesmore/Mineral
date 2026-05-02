use serde::{Deserialize, Serialize};

/// 一首歌的歌词集合。
///
/// 所有字段都是可选的——某些 channel 拿不到的格式给 `None` / 空 `String`,
/// 上层渲染时按 `yrc → lrc → 空` 的优先级降级。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lyrics {
    /// LRC 行歌词。
    pub lrc: Option<String>,
    /// LRC 翻译。
    pub translation: Option<String>,
    /// LRC 罗马音。
    pub romanization: Option<String>,
    /// 逐字歌词(网易 YRC 等)。
    pub yrc: Option<String>,
    /// 逐字翻译。
    pub yrc_translation: Option<String>,
    /// 逐字罗马音。
    pub yrc_romanization: Option<String>,
}
