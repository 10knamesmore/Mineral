//! 右栏封面编码请求的去重与接管测试。

use std::sync::Arc;

use color_eyre::eyre::eyre;
use mineral_model::MediaUrl;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

use crate::render::theme::Theme;
use crate::runtime::state::{AppState, View};
use crate::test_support::{
    default_theme, entry_views, solid_cover, song, state_with_playlists, with_name,
};

use super::{draw, main_cover};

/// 面板尺寸用于驱动真实编码请求。
const PANEL: Rect = Rect::new(4, 3, 40, 20);

/// 两端有不同标题和封面；曲目选择第二首，避免把入口曲误当成当前选中项。
fn state_with_covers(cache_playlist: bool, cache_track: bool) -> color_eyre::Result<AppState> {
    let mut state = state_with_playlists()?;
    let from = MediaUrl::remote("https://example.com/playlist.jpg")?;
    let to = MediaUrl::remote("https://example.com/selected-track.jpg")?;
    let playlist = state
        .library
        .playlists
        .first_mut()
        .ok_or_else(|| eyre!("缺少歌单"))?;
    playlist.data.name = "Playlist detail".to_owned();
    playlist.data.cover_url = Some(from.clone());
    let playlist_id = playlist.data.id.clone();
    let mut selected = with_name(song("selected"), "Track detail");
    selected.cover_url = Some(to.clone());
    state.browse.nav.opened_playlist = Some(playlist_id.clone());
    state.library.tracks.insert(
        playlist_id,
        crate::runtime::state::PlaylistTracks {
            entries: entry_views(vec![song("entry"), selected]),
            complete: true,
            next_offset: None,
        },
    );
    state.browse.nav.track.set_sel(1);
    state.browse.view.retempo(4);
    if cache_playlist {
        state
            .images
            .cache
            .insert_test(&from, Arc::new(solid_cover(200, 0, 0)));
    }
    if cache_track {
        state
            .images
            .cache
            .insert_test(&to, Arc::new(solid_cover(0, 0, 200)));
    }
    Ok(state)
}

/// 通过生产入口绘制右栏以触发封面编码请求。
fn render(state: &AppState, theme: &Theme, cover_in_flight: bool) -> color_eyre::Result<()> {
    let mut terminal = Terminal::new(TestBackend::new(52, 28))?;
    terminal.draw(|frame| draw(frame, PANEL, state, theme, cover_in_flight))?;
    Ok(())
}

/// 读取指定端点的真实封面身份。
fn cover_url(state: &AppState, view: View) -> color_eyre::Result<MediaUrl> {
    main_cover::url_for_view(state, view).ok_or_else(|| eyre!("测试选中项应有封面"))
}

/// 两端图片只预热一次，后续过渡帧不重复提交编码。
#[test]
fn covers_prewarm_both_endpoints_without_churn() -> color_eyre::Result<()> {
    let mut state = state_with_covers(true, true)?;
    let theme = default_theme()?;
    let from = cover_url(&state, View::Playlists)?;
    let to = cover_url(&state, View::Library)?;
    state.browse.view.switch_to(View::Library);
    state.browse.view.tick();
    render(&state, &theme, false)?;
    let pending = state.images.encode_pending.borrow().clone();
    assert_eq!(pending.len(), 2, "只预热当前选中的两张图");
    assert!(pending.iter().any(|key| key.matches_url(&from)));
    assert!(pending.iter().any(|key| key.matches_url(&to)));
    state.browse.view.tick();
    render(&state, &theme, false)?;
    assert_eq!(
        *state.images.encode_pending.borrow(),
        pending,
        "固定尺寸不重复提交编码"
    );
    Ok(())
}

/// 飞行层接管时，本地详情不提交封面编码。
#[test]
fn cover_in_flight_suppresses_local_encode_requests() -> color_eyre::Result<()> {
    let mut state = state_with_covers(true, true)?;
    let theme = default_theme()?;
    state.browse.view.switch_to(View::Library);
    for _ in 0..=4 {
        render(&state, &theme, true)?;
        assert!(
            state.images.encode_pending.borrow().is_empty(),
            "被接管的两端不应自画或预热"
        );
        state.browse.view.tick();
    }
    Ok(())
}
