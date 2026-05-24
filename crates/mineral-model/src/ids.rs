use mineral_macros::define_id;

use crate::source::SourceKind;

define_id!(SongId, SourceKind);
define_id!(AlbumId, SourceKind);
define_id!(ArtistId, SourceKind);
define_id!(PlaylistId, SourceKind);
define_id!(UserId, SourceKind);
