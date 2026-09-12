//! `mineral.library.*`:用户曲库的脚本出口(映射 channel 能力)。

pub(crate) mod love;
pub(crate) mod playlists;
pub(crate) mod search;
pub(crate) mod song_url;
mod table;
pub(crate) mod tracks;

pub(crate) use table::install;
