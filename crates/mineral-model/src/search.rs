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

    /// 单独的字形图标(`label` 的前缀部分,如 `◉`)。
    ///
    /// # Return:
    ///   该类型的字形图标(`&'static str`)。
    pub const fn icon(&self) -> &'static str {
        match self {
            Self::Song => "♪",
            Self::Album => "◉",
            Self::Artist => "✦",
            Self::Playlist => "▤",
            Self::User => "☻",
        }
    }

    /// 单数名词(如 `album`)。
    ///
    /// # Return:
    ///   该类型的单数名词(`&'static str`)。
    pub const fn singular(&self) -> &'static str {
        match self {
            Self::Song => "song",
            Self::Album => "album",
            Self::Artist => "artist",
            Self::Playlist => "playlist",
            Self::User => "user",
        }
    }

    /// 全部变体,按声明序。穷举消费点(测试 / 文档生成)用它,新增 variant 须同步。
    pub const ALL: [Self; 5] = [
        Self::Song,
        Self::Album,
        Self::Artist,
        Self::Playlist,
        Self::User,
    ];
}

#[cfg(test)]
mod tests {
    use super::SearchKind;

    /// 五个变体(穷举用,取自类型自身声明)。
    const ALL: [SearchKind; 5] = SearchKind::ALL;

    /// icon / singular 与 label 同源:label 必以 icon 起头、且含 singular 词干(复数仅多个尾字符)。
    /// 守卫三处词表(label/icon/singular)别各改一处漂移。
    #[test]
    fn label_composes_from_icon_and_singular() {
        for kind in ALL {
            assert!(
                kind.label().starts_with(kind.icon()),
                "{kind:?}: label 应以 icon 起头"
            );
            assert!(
                kind.label().contains(kind.singular()),
                "{kind:?}: label 应含 singular 词干"
            );
        }
    }

    /// singular 是复数 label 去掉图标与尾 `s`(纯文案锚点,改词表时一并对齐)。
    #[test]
    fn singular_forms_are_expected() {
        assert_eq!(SearchKind::Song.singular(), "song");
        assert_eq!(SearchKind::Album.singular(), "album");
        assert_eq!(SearchKind::Artist.singular(), "artist");
        assert_eq!(SearchKind::Playlist.singular(), "playlist");
        assert_eq!(SearchKind::User.singular(), "user");
    }
}
