//! 应用状态、页面输入与同步主事件循环。

mod application;
mod backend_sync;
mod channel_search;
mod event_loop;
mod input;
mod menus;
mod nav;
mod page;
mod player_sync;
pub(crate) mod playlists;
mod push_events;
mod queue_edit;
mod spectrum_feed;

#[cfg(test)]
mod tui_script_tests;

#[cfg(test)]
mod transport_input_tests;

pub use application::App;

#[cfg(test)]
mod presentation_tests;

pub(crate) mod overlays;
pub(crate) use overlays::AppOverlay;
