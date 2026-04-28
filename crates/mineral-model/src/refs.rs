use serde::{Deserialize, Serialize};

use crate::ids::{AlbumId, ArtistId};

/// 引用一个艺人(用在 [`crate::Song`] / [`crate::Album`] 等里)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistRef {
    pub id: ArtistId,
    pub name: String,
}

/// 引用一张专辑。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumRef {
    pub id: AlbumId,
    pub name: String,
}
