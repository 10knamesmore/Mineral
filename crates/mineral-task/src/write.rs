//! 歌单写操作的任务载荷与跨进程错误形态。

use mineral_model::{PlaylistId, SongId, SourceKind};
use serde::{Deserialize, Serialize};

/// 一次歌单写操作。server 边界解开成对应 `MusicChannel` 方法调用。
///
/// `Create` 需要显式 `source`(还没有歌单 id 可派生 namespace);其余操作的
/// 目标 channel 一律从 `id` 的 namespace 派生,不另带字段(单一事实源)。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlaylistWriteOp {
    /// 创建歌单。
    Create {
        /// 目标 channel。
        source: SourceKind,

        /// 歌单名。
        name: String,
    },

    /// 删除自己创建的歌单。
    Delete {
        /// 歌单 id(自带 namespace)。
        id: PlaylistId,
    },

    /// 向歌单追加歌曲(须与歌单同 namespace,server 边界校验)。
    AddSongs {
        /// 歌单 id。
        id: PlaylistId,

        /// 待追加歌曲。
        songs: Vec<SongId>,
    },

    /// 从歌单移除歌曲(须与歌单同 namespace,server 边界校验)。
    RemoveSongs {
        /// 歌单 id。
        id: PlaylistId,

        /// 待移除歌曲。
        songs: Vec<SongId>,
    },

    /// 歌单改名。
    Rename {
        /// 歌单 id。
        id: PlaylistId,

        /// 新名字。
        name: String,
    },

    /// 修改歌单描述。
    SetDescription {
        /// 歌单 id。
        id: PlaylistId,

        /// 新描述。
        desc: String,
    },
}

impl PlaylistWriteOp {
    /// 该写操作针对的 channel(lane 路由 + 边界校验用)。
    pub fn target_source(&self) -> SourceKind {
        match self {
            Self::Create { source, .. } => *source,
            Self::Delete { id }
            | Self::AddSongs { id, .. }
            | Self::RemoveSongs { id, .. }
            | Self::Rename { id, .. }
            | Self::SetDescription { id, .. } => id.namespace(),
        }
    }

    /// 涉及的歌曲列表(同源校验用;无歌曲的操作返回空)。
    pub fn songs(&self) -> &[SongId] {
        match self {
            Self::AddSongs { songs, .. } | Self::RemoveSongs { songs, .. } => songs,
            Self::Create { .. }
            | Self::Delete { .. }
            | Self::Rename { .. }
            | Self::SetDescription { .. } => &[],
        }
    }

    /// dedup key 的可变部分。完全相同参数的写操作在飞时共享是良性的
    /// (用户连按两下"加入歌单"只发一次请求);参数有任何差异即不同 key。
    pub(crate) fn dedup_part(&self) -> String {
        match self {
            Self::Create { source, name } => format!("create:{source:?}:{name}"),
            Self::Delete { id } => format!("delete:{}", id.qualified()),
            Self::AddSongs { id, songs } => {
                format!("add:{}:{}", id.qualified(), join_qualified(songs))
            }
            Self::RemoveSongs { id, songs } => {
                format!("remove:{}:{}", id.qualified(), join_qualified(songs))
            }
            Self::Rename { id, name } => format!("rename:{}:{name}", id.qualified()),
            Self::SetDescription { id, desc } => format!("desc:{}:{desc}", id.qualified()),
        }
    }
}

/// 拼接歌曲 qualified id(dedup key 用)。
fn join_qualified(songs: &[SongId]) -> String {
    songs
        .iter()
        .map(SongId::qualified)
        .collect::<Vec<String>>()
        .join(",")
}

/// 写操作失败的跨进程错误形态(`channel::Error` 不可序列化,在 worker 边界映射)。
/// TUI 按变体翻译成用户语言的 toast。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteError {
    /// 需要登录(网易云 301)。
    AuthRequired,

    /// 风控或容量限制(网易云 512),稍后再试。
    RateLimited,

    /// 该 channel 不支持歌单写操作。
    NotSupported,

    /// channel 业务错误透传(如加歌重复的 502 →「已在歌单中」)。
    Api {
        /// channel 自定义错误 code。
        code: i64,

        /// 错误描述。
        message: String,
    },

    /// 其余失败(网络/解析等),携带展开后的错误链文本。
    Other(String),
}

impl WriteError {
    /// 从 channel 层错误映射(在 worker 边界调用,保住结构化语义)。
    pub fn from_channel(e: &mineral_channel_core::Error) -> Self {
        match e {
            mineral_channel_core::Error::AuthRequired => Self::AuthRequired,
            mineral_channel_core::Error::RateLimited => Self::RateLimited,
            mineral_channel_core::Error::NotSupported => Self::NotSupported,
            mineral_channel_core::Error::Api { code, message } => Self::Api {
                code: *code,
                message: message.clone(),
            },
            other => Self::Other(mineral_log::chain(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use mineral_channel_core::Error;
    use mineral_model::{PlaylistId, SongId, SourceKind};

    use super::{PlaylistWriteOp, WriteError};

    /// 目标 channel:Create 用显式 source,其余从歌单 id namespace 派生。
    #[test]
    fn target_source_derives_from_id_namespace() {
        let create = PlaylistWriteOp::Create {
            source: SourceKind::NETEASE,
            name: String::from("x"),
        };
        assert_eq!(create.target_source(), SourceKind::NETEASE);

        let delete = PlaylistWriteOp::Delete {
            id: PlaylistId::new(SourceKind::LOCAL, "p1"),
        };
        assert_eq!(delete.target_source(), SourceKind::LOCAL);
    }

    /// dedup:同参数同 key(连按合并),任何参数差异即不同 key。
    #[test]
    fn dedup_part_distinguishes_params() {
        let pl = PlaylistId::new(SourceKind::NETEASE, "1");
        let song_a = SongId::new(SourceKind::NETEASE, "a");
        let song_b = SongId::new(SourceKind::NETEASE, "b");
        let add = |songs: Vec<SongId>| PlaylistWriteOp::AddSongs {
            id: pl.clone(),
            songs,
        };
        assert_eq!(
            add(vec![song_a.clone()]).dedup_part(),
            add(vec![song_a.clone()]).dedup_part()
        );
        assert_ne!(
            add(vec![song_a.clone()]).dedup_part(),
            add(vec![song_b]).dedup_part()
        );
        assert_ne!(
            add(vec![song_a]).dedup_part(),
            PlaylistWriteOp::Delete { id: pl }.dedup_part()
        );
    }

    /// channel Error → WriteError 的结构化映射(301/512/NotSupported/Api/其他)。
    #[test]
    fn write_error_maps_channel_error() {
        assert_eq!(
            WriteError::from_channel(&Error::AuthRequired),
            WriteError::AuthRequired
        );
        assert_eq!(
            WriteError::from_channel(&Error::RateLimited),
            WriteError::RateLimited
        );
        assert_eq!(
            WriteError::from_channel(&Error::NotSupported),
            WriteError::NotSupported
        );
        assert_eq!(
            WriteError::from_channel(&Error::Api {
                code: 502,
                message: String::from("歌曲已存在")
            }),
            WriteError::Api {
                code: 502,
                message: String::from("歌曲已存在")
            }
        );
        assert!(matches!(
            WriteError::from_channel(&Error::Network(String::from("timeout"))),
            WriteError::Other(_)
        ));
    }
}
