//! 曲库类型抽屉与独立的歌单、专辑列表。

mod drawer;
mod paint;
mod state;
mod view;
mod window;

pub use state::CollectionList;
pub(crate) use view::{CollectionInput, CollectionView};

mod input;
