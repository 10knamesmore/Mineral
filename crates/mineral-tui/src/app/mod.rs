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
mod push_events;
mod queue_edit;
mod spectrum_feed;

#[cfg(test)]
mod transport_input_tests;

pub use application::App;
