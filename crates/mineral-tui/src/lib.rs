//! Terminal UI client for Mineral.

#[cfg(windows)]
compile_error!("Windows 暂不支持");

mod app;
mod color;
mod components;
mod layout;
mod lrc;
mod playback;
mod prefetch;
mod state;
mod theme;
mod tui;
mod view;
mod view_model;
mod yrc;

use std::sync::Arc;

use mineral_audio::AudioHandle;
use mineral_channel_core::MusicChannel;
use mineral_task::{ChannelFetchKind, Priority, Scheduler, TaskKind};
use ratatui_image::picker::Picker;

use app::App;
use tui::Tui;

/// 启动 TUI。
///
/// # Params:
///   - `channels`: 已构造好的所有音乐源。TUI 平等对待,扔给 [`Scheduler`] 后
///     scheduler 会逐个 channel 拉 `my_playlists`。空 vec 也合法(纯 UI 演示)。
///
/// # Return:
///   主循环正常退出返回 `Ok(())`;终端 raw mode / 渲染失败返回 `Err`。
pub async fn run(channels: Vec<Arc<dyn MusicChannel>>) -> color_eyre::Result<()> {
    let scheduler = Scheduler::new(&channels);
    submit_initial_loads(&scheduler, &channels);
    let (audio, spectrum_tap) = AudioHandle::spawn()?;

    let mut tui = Tui::new()?;
    tui.enter()?;
    // Picker::from_query_stdio 必须在进 alternate screen 之后、读 events 之前调,
    // 因为它会临时往 stdio 写探测 escape 序列读响应。失败 fallback 到 8x16 fixed
    // font 用 halfblocks 渲染,不阻塞启动。
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::from_fontsize((8, 16)));
    let mut app = App::new(scheduler, audio, spectrum_tap, picker);
    let result = app.run(&mut tui);
    tui.exit()?;
    result
}

fn submit_initial_loads(scheduler: &Scheduler, channels: &[Arc<dyn MusicChannel>]) {
    for ch in channels {
        scheduler.submit(
            TaskKind::ChannelFetch(ChannelFetchKind::MyPlaylists {
                source: ch.source(),
            }),
            Priority::User,
        );
    }
}
