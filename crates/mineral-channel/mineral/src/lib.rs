//! 跨源聚合 channel(source = `mineral`):把 persist 里的全源收藏投影成一张
//! synthetic 歌单,供上层与普通歌单同等浏览 / 下钻。

mod channel;

pub use channel::{MineralChannel, favorites_playlist_id};
