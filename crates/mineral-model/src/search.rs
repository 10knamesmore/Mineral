use serde::{Deserialize, Serialize};

/// 搜索接口要返回的目标类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchKind {
    /// 单曲。
    Song,
    /// 专辑。
    Album,
    /// 艺人。
    Artist,
    /// 歌单。
    Playlist,
    /// 用户。
    User,
}

impl SearchKind {
    /// UI 展示标签:字形图标 + 复数名词(如 `♪ songs`)。
    ///
    /// 与 [`SourceKind::label`](crate::SourceKind::label) 对称——展示元数据内建在类型上,
    /// channel 策略只声明支持哪些类型,不持有其图标。
    ///
    /// # Return:
    ///   含字形图标的展示标签(`&'static str`)。
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Song => "♪ songs",
            Self::Album => "◉ albums",
            Self::Artist => "✦ artists",
            Self::Playlist => "▤ playlists",
            Self::User => "☻ users",
        }
    }
}
