//! 索引:扫描结果 ↔ 持久索引 ↔ model 的桥。
//!
//! reconcile 把扫描并入 persist(rename 复用 uuid);row 做行 ↔ Song 互转与派生 ID。

mod group;
mod ingest;
mod reconcile;
mod row;

pub use ingest::scan_and_index;
pub use reconcile::reconcile;
pub(crate) use group::{playlist_detail_from_rows, playlists_from_rows};
pub(crate) use row::row_to_song;
