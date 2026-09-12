//! 应用全局状态、页面状态与数据镜像。

mod application_state;
mod browse;
mod channel_search;
mod detail;
mod library;
mod library_projection;
mod lifecycle;
mod lyric;
mod nav;
mod overlay_reveal;
mod player;
mod search;
mod search_updates;
pub(crate) mod search_whitelist;
mod task_event;
mod track_filter;
mod view_context;
mod view_switch;

#[cfg(test)]
pub use crate::image::CoverTransition;
pub use crate::image::ImageEngine;
pub use application_state::AppState;
pub(crate) use application_state::spectrum_params;
pub use browse::BrowsePage;
pub(crate) use browse::{BrowseModel, LibraryQueueProjection};
pub use channel_search::{PromptSegment, SearchFocus, SearchPage, SearchSession};
pub use detail::{ArtistSection, DetailData, DetailFetch, DetailFrame, EntityRef};
pub use library::LibraryData;
pub use lyric::LyricExtra;
pub use overlay_reveal::OverlayReveal;
pub use player::PlayerMirror;
pub use search::SearchState;
pub use view_context::{ActiveLayer, PageKind, View};
