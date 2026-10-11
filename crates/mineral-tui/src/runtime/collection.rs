//! 曲库中歌单与专辑的共同浏览接口，身份与业务模型始终保留各自类型。

use mineral_model::{AlbumId, ArtistRef, MediaUrl, PlaylistId, SourceKind};
use mineral_protocol::QueueContextWire;

use crate::runtime::state::{CollectionTracks, EntityRef, LibraryData};
use crate::runtime::view_model::{AlbumView, PlaylistView};

/// 曲库项目的身份；同一来源内同值的歌单和专辑互不混淆。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CollectionId {
    /// 一个歌单。
    Playlist(PlaylistId),

    /// 一张专辑。
    Album(AlbumId),
}

impl From<PlaylistId> for CollectionId {
    fn from(id: PlaylistId) -> Self {
        Self::Playlist(id)
    }
}

impl From<AlbumId> for CollectionId {
    fn from(id: AlbumId) -> Self {
        Self::Album(id)
    }
}

impl From<&PlaylistId> for CollectionId {
    fn from(id: &PlaylistId) -> Self {
        Self::Playlist(id.clone())
    }
}

impl From<&AlbumId> for CollectionId {
    fn from(id: &AlbumId) -> Self {
        Self::Album(id.clone())
    }
}

impl From<&Self> for CollectionId {
    fn from(id: &Self) -> Self {
        id.clone()
    }
}

/// 曲库可以浏览的两种内容类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum CollectionGroup {
    /// 用户可以浏览的歌单。
    Playlists,

    /// 账号收藏的专辑。
    Albums,
}

impl CollectionGroup {
    /// 界面中的类型标签。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Playlists => "playlists",
            Self::Albums => "albums",
        }
    }
}

impl CollectionId {
    /// 持久化使用的稳定类型名称。
    pub(crate) fn kind_name(&self) -> &'static str {
        match self {
            Self::Playlist(_) => "playlist",
            Self::Album(_) => "album",
        }
    }

    /// 项目来源。
    pub(crate) fn source(&self) -> SourceKind {
        match self {
            Self::Playlist(id) => id.namespace(),
            Self::Album(id) => id.namespace(),
        }
    }

    /// 来源内的原始身份。
    pub(crate) fn value(&self) -> &str {
        match self {
            Self::Playlist(id) => id.value(),
            Self::Album(id) => id.value(),
        }
    }

    /// 日志中的带类型身份，避免歌单与专辑同值时混淆。
    pub(crate) fn qualified(&self) -> String {
        format!(
            "{}:{}:{}",
            self.kind_name(),
            self.source().name(),
            self.value()
        )
    }

    /// 项目所属的曲库类型。
    pub(crate) fn group(&self) -> CollectionGroup {
        match self {
            Self::Playlist(_) => CollectionGroup::Playlists,
            Self::Album(_) => CollectionGroup::Albums,
        }
    }

    /// 在共享曲库中定位同一项目。
    pub(crate) fn resolve<'a>(&self, library: &'a LibraryData) -> Option<CollectionRef<'a>> {
        match self {
            Self::Playlist(id) => library
                .playlists
                .iter()
                .find(|p| &p.data.id == id)
                .map(CollectionRef::Playlist),
            Self::Album(id) => library
                .albums
                .iter()
                .find(|a| &a.data.id == id)
                .map(CollectionRef::Album),
        }
    }

    /// 当前已加载的曲目；没有缓存时为等待数据状态。
    pub(crate) fn tracks<'a>(&self, library: &'a LibraryData) -> Option<&'a CollectionTracks> {
        match self {
            Self::Playlist(id) => library.tracks.get(id),
            Self::Album(id) => library.album_tracks.get(id),
        }
    }
}

/// 借用一个项目，统一导航和预览，不修改底层模型。
#[derive(Clone, Copy)]
pub(crate) enum CollectionRef<'a> {
    /// 歌单元信息。
    Playlist(&'a PlaylistView),

    /// 专辑元信息。
    Album(&'a AlbumView),
}

impl<'a> CollectionRef<'a> {
    /// 保留实体类型的项目身份。
    pub(crate) fn id(self) -> CollectionId {
        match self {
            Self::Playlist(p) => p.data.id.clone().into(),
            Self::Album(a) => a.data.id.clone().into(),
        }
    }

    /// 来源提供的展示名称。
    pub(crate) fn name(self) -> &'a str {
        match self {
            Self::Playlist(p) => &p.data.name,
            Self::Album(a) => &a.data.name,
        }
    }

    /// 音乐来源。
    pub(crate) fn source(self) -> SourceKind {
        match self {
            Self::Playlist(p) => p.data.source(),
            Self::Album(a) => a.data.source(),
        }
    }

    /// 来源提供的曲目总数，尚未知晓时为空。
    pub(crate) fn track_count(self) -> Option<u64> {
        match self {
            Self::Playlist(p) => Some(p.data.track_count),
            Self::Album(a) => a.data.track_count,
        }
    }

    /// 来源提供的封面，歌单拼贴由图片层另行处理。
    pub(crate) fn cover_url(self) -> Option<&'a MediaUrl> {
        match self {
            Self::Playlist(p) => p.data.cover_url.as_ref(),
            Self::Album(a) => a.data.cover_url.as_ref(),
        }
    }

    /// 来源提供的简介。
    pub(crate) fn description(self) -> &'a str {
        match self {
            Self::Playlist(p) => &p.data.description,
            Self::Album(a) => &a.data.description,
        }
    }

    /// 专辑艺人；歌单没有艺人归属。
    pub(crate) fn artists(self) -> &'a [ArtistRef] {
        match self {
            Self::Playlist(_) => &[],
            Self::Album(a) => &a.data.artists,
        }
    }

    /// 将浏览选择交给已有的实体操作菜单。
    pub(crate) fn entity(self) -> EntityRef {
        match self {
            Self::Playlist(p) => EntityRef::Playlist(Box::new(p.data.clone())),
            Self::Album(a) => EntityRef::Album(Box::new(a.data.clone())),
        }
    }

    /// 从曲库播放时保留专辑或歌单语境。
    pub(crate) fn play_context(self) -> QueueContextWire {
        match self {
            Self::Playlist(p) => QueueContextWire::Playlist {
                id: p.data.id.clone(),
                name: Some(p.data.name.clone()),
            },
            Self::Album(a) => QueueContextWire::Album {
                id: a.data.id.clone(),
                name: Some(a.data.name.clone()),
            },
        }
    }
}
