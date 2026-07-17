//! 队列上下文:一个队列「来自哪」,随队列建立、被其后每个 plays 行继承。
//!
//! 与 [`crate::PlayOrigin`](发起方式)分层——单一 origin 有归属漏洞(从歌单点第一
//! 首后自动连播 20 首,后 19 行只知道 AutoAdvance,「最常听的歌单」就断了)。上下文
//! 层补齐队列级归属。落库拍平成 `context_kind` + `context_ref` 双列。

use mineral_model::{AlbumId, ArtistId, PlaylistId};

/// 队列的填充来源。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueueContext {
    /// 搜索结果(携带搜索词)。
    Search {
        /// 触发该队列的搜索词;`None` = 按 `search_queries` 隐私档略去(kind 仍记
        /// search,reference 落 NULL)。Hashed 档存的是散列串而非原文。
        query: Option<String>,
    },

    /// 歌单 tracks(含聚合收藏这类 synthetic 歌单)。
    Playlist {
        /// 歌单 ID。
        id: PlaylistId,
    },

    /// 专辑详情。
    Album {
        /// 专辑 ID。
        id: AlbumId,
    },

    /// 艺人详情。
    Artist {
        /// 艺人 ID。
        id: ArtistId,
    },

    /// 手动攒的队列(insert_next / append 散曲)。
    Manual,

    /// 未标注(旧 client 缺省)。
    Unknown,
}

impl QueueContext {
    /// 拍平成 `(context_kind, context_ref)` 两列的落库值。
    ///
    /// `context_ref` 对 [`QueueContext::Search`] 存搜索词(经隐私档处理后的值,可缺席),
    /// 对歌单 / 专辑 / 艺人存 `qualified()`(`namespace:value` 全局唯一串),对
    /// [`QueueContext::Manual`] / [`QueueContext::Unknown`] 为 `None`(落 NULL)。
    ///
    /// # Return:
    ///   `(kind, reference)`——kind 是 `'static` 词,reference 需借用 self 里的 id/词
    ///   现算 `String`,故按值返回 `Option<String>`。
    pub fn to_columns(&self) -> (&'static str, Option<String>) {
        match self {
            Self::Search { query } => ("search", query.clone()),
            Self::Playlist { id } => ("playlist", Some(id.qualified())),
            Self::Album { id } => ("album", Some(id.qualified())),
            Self::Artist { id } => ("artist", Some(id.qualified())),
            Self::Manual => ("manual", None),
            Self::Unknown => ("unknown", None),
        }
    }

    /// 按 `search_queries` 隐私档处理 [`QueueContext::Search`] 的搜索词(其余变体原样):
    /// Raw 保留原文、Hashed 换成 [`crate::query_hash`] 散列串、Off 丢词(reference 落
    /// NULL,kind 仍记 search)。plays.context_ref 与 searches 表同受该档约束——搜索词
    /// 不能经起播语境旁路落库。
    ///
    /// # Params:
    ///   - `mode`: 搜索词落库档
    ///
    /// # Return:
    ///   处理后的语境
    #[must_use]
    pub fn redact_search(self, mode: crate::SearchQueryMode) -> Self {
        let Self::Search { query } = self else {
            return self;
        };
        let query = match mode {
            crate::SearchQueryMode::Raw => query,
            crate::SearchQueryMode::Hashed => query.map(|q| crate::query_hash(&q)),
            crate::SearchQueryMode::Off => None,
        };
        Self::Search { query }
    }
}

#[cfg(test)]
mod tests {
    use super::QueueContext;
    use mineral_model::{AlbumId, ArtistId, PlaylistId, SourceKind};

    #[test]
    fn search_keeps_query_verbatim() {
        let ctx = QueueContext::Search {
            query: Some("李志".to_owned()),
        };
        assert_eq!(ctx.to_columns(), ("search", Some("李志".to_owned())));
        let redacted = QueueContext::Search { query: None };
        assert_eq!(redacted.to_columns(), ("search", None));
    }

    /// redact_search:Raw 原样、Hashed 换稳定散列(与 searches 表 query_hash 同算法可
    /// 关联)、Off 丢词保 kind;非 Search 变体不受影响。
    #[test]
    fn redact_search_honors_privacy_mode() {
        use crate::SearchQueryMode;
        let search = || QueueContext::Search {
            query: Some("李志".to_owned()),
        };
        assert_eq!(
            search().redact_search(SearchQueryMode::Raw).to_columns().1,
            Some("李志".to_owned())
        );
        let hashed = search()
            .redact_search(SearchQueryMode::Hashed)
            .to_columns()
            .1;
        assert_eq!(
            hashed,
            Some(crate::query_hash("李志")),
            "散列与 searches 表同算法"
        );
        assert_eq!(
            search().redact_search(SearchQueryMode::Off).to_columns(),
            ("search", None),
            "Off 丢词但保 kind"
        );
        let playlist = QueueContext::Playlist {
            id: PlaylistId::new(SourceKind::NETEASE, "1"),
        };
        assert_eq!(
            playlist.clone().redact_search(SearchQueryMode::Off),
            playlist,
            "非 Search 变体原样"
        );
    }

    #[test]
    fn entity_contexts_store_qualified_id() {
        let playlist = QueueContext::Playlist {
            id: PlaylistId::new(SourceKind::NETEASE, "123"),
        };
        assert_eq!(
            playlist.to_columns(),
            ("playlist", Some("netease:123".to_owned()))
        );

        let album = QueueContext::Album {
            id: AlbumId::new(SourceKind::BILIBILI, "BV1x"),
        };
        assert_eq!(
            album.to_columns(),
            ("album", Some("bilibili:BV1x".to_owned()))
        );

        let artist = QueueContext::Artist {
            id: ArtistId::new(SourceKind::LOCAL, "abc"),
        };
        assert_eq!(
            artist.to_columns(),
            ("artist", Some("local:abc".to_owned()))
        );
    }

    #[test]
    fn manual_and_unknown_have_no_ref() {
        assert_eq!(QueueContext::Manual.to_columns(), ("manual", None));
        assert_eq!(QueueContext::Unknown.to_columns(), ("unknown", None));
    }
}
