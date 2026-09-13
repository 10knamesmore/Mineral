//! 右栏 Now Playing detail:Playlists 视图显示歌单 meta,Library 视图显示
//! 当前选中曲目 meta；一律包含图片区与 KV 区。

mod cover_transition;
pub(crate) mod main_cover;
mod panel;
pub mod playlist;
pub mod track;

pub use panel::draw;

#[cfg(test)]
mod tests;
