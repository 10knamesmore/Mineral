use mineral_macros::{define_id, define_uuid};

use crate::source::SourceKind;

// SongId 额外要 `new_uuid`:shelf 本地文件用随机 uuid 当裸值(路径当 id 会让 rename 断链)。
define_uuid!(SongId, SourceKind);
define_id!(AlbumId, SourceKind);
define_id!(ArtistId, SourceKind);
define_id!(PlaylistId, SourceKind);
define_id!(UserId, SourceKind);
