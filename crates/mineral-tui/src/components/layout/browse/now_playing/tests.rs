//! 右栏视图过渡的文字、封面和页面飞行层接管回归测试。

use std::sync::Arc;

use color_eyre::eyre::eyre;
use mineral_model::MediaUrl;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Block;
use ratatui::{Frame, Terminal};

use crate::render::theme::Theme;
use crate::runtime::state::{AppState, View};
use crate::test_support::{
    default_theme, entry_views, solid_cover, song, state_with_playlists, with_name,
};

use super::{draw, main_cover, playlist, track};

/// 非零原点用于检查封面和文字沿用面板坐标。
const PANEL: Rect = Rect::new(4, 3, 40, 20);

/// 与测试封面不同的底色，用于检查透明区域和单端淡化。
const BACKGROUND: Color = Color::Rgb(20, 40, 60);

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
    state
        .library
        .tracks
        .insert(playlist_id, entry_views(vec![song("entry"), selected]));
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

/// 给每帧铺相同背景，返回包含颜色的完整结果。
fn paint(painter: impl FnOnce(&mut Frame<'_>)) -> color_eyre::Result<Buffer> {
    let mut terminal = Terminal::new(TestBackend::new(52, 28))?;
    terminal.draw(|frame| {
        frame.render_widget(
            Block::new().style(Style::new().bg(BACKGROUND)),
            frame.area(),
        );
        painter(frame);
    })?;
    Ok(terminal.backend().buffer().clone())
}

/// 通过生产入口绘制右栏，包含端点和过渡分支。
fn render(state: &AppState, theme: &Theme, cover_in_flight: bool) -> color_eyre::Result<Buffer> {
    paint(|frame| draw(frame, PANEL, state, theme, cover_in_flight))
}

/// 获取生产布局里的封面、标题和底行，避免测试另算位置。
fn sections() -> color_eyre::Result<[Rect; 3]> {
    main_cover::sections(PANEL).ok_or_else(|| eyre!("测试面板应能显示详情"))
}

/// 读取指定端点的真实封面身份。
fn cover_url(state: &AppState, view: View) -> color_eyre::Result<MediaUrl> {
    main_cover::url_for_view(state, view).ok_or_else(|| eyre!("测试选中项应有封面"))
}

/// 稳定端点保留原组件的完整输出，包括封面、标题和边框。
#[test]
fn endpoints_match_original_details() -> color_eyre::Result<()> {
    let mut state = state_with_covers(true, true)?;
    let theme = default_theme()?;
    for view in [View::Playlists, View::Library] {
        state.browse.view.switch_to(view);
        for _ in 0..4 {
            state.browse.view.tick();
        }
        let expected = match view {
            View::Playlists => {
                let selected = state
                    .selected_playlist()
                    .ok_or_else(|| eyre!("缺少选中歌单"))?;
                paint(|frame| playlist::draw(frame, PANEL, selected, &state, &theme, false))?
            }
            View::Library => {
                let entries = state.filtered_tracks();
                let selected = entries
                    .get(state.browse.nav.track.sel())
                    .ok_or_else(|| eyre!("缺少选中曲目"))?;
                paint(|frame| track::draw(frame, PANEL, selected, None, &state, &theme, false))?
            }
        };
        assert_eq!(
            render(&state, &theme, false)?,
            expected,
            "{view:?} 稳态应沿用原绘制"
        );
    }
    Ok(())
}

/// 目标刚切换但进度尚在原端点时，画面仍停在原视图。
#[test]
fn changing_target_at_endpoint_does_not_replace_details() -> color_eyre::Result<()> {
    let mut state = state_with_covers(true, true)?;
    let theme = default_theme()?;
    let playlists = render(&state, &theme, false)?;
    state.browse.view.switch_to(View::Library);
    assert_eq!(render(&state, &theme, false)?, playlists);
    for _ in 0..4 {
        state.browse.view.tick();
    }
    let tracks = render(&state, &theme, false)?;
    state.browse.view.switch_to(View::Playlists);
    assert_eq!(render(&state, &theme, false)?, tracks);
    Ok(())
}

/// 对照稳定标题行，确保文字位置没动且确实混入背景色。
fn assert_title_faded(endpoint: &Buffer, fading: &Buffer) -> color_eyre::Result<()> {
    let [_, title, _] = sections()?;
    let mut letters = 0;
    for x in title.left()..title.right() {
        let full = endpoint
            .cell((x, title.y))
            .ok_or_else(|| eyre!("标题格越界"))?;
        let dim = fading
            .cell((x, title.y))
            .ok_or_else(|| eyre!("过渡标题格越界"))?;
        assert_eq!(dim.symbol(), full.symbol(), "标题排版应固定");
        assert_eq!(dim.bg, BACKGROUND, "文字不能盖掉背景");
        if full.symbol() != " " {
            letters += 1;
            assert_ne!(dim.fg, full.fg, "途中标题必须发生淡化");
            assert_ne!(dim.fg, BACKGROUND, "此时标题仍应可见");
        }
    }
    assert!(letters > 0, "应实际检查到标题文字");
    assert_eq!(
        fading.cell((PANEL.x, PANEL.y)),
        endpoint.cell((PANEL.x, PANEL.y)),
        "边框不随文字淡化"
    );
    Ok(())
}

/// 歌单淡出和曲目淡入都保留各自稳定排版。
#[test]
fn outgoing_and_incoming_details_fade_without_moving() -> color_eyre::Result<()> {
    let mut state = state_with_covers(false, false)?;
    let theme = default_theme()?;
    let playlists = render(&state, &theme, false)?;
    state.browse.view.switch_to(View::Library);
    state.browse.view.tick();
    let outgoing = render(&state, &theme, false)?;
    state.browse.view.tick();
    state.browse.view.tick();
    let incoming = render(&state, &theme, false)?;
    state.browse.view.tick();
    let tracks = render(&state, &theme, false)?;
    assert_title_faded(&playlists, &outgoing)?;
    assert_title_faded(&tracks, &incoming)?;
    Ok(())
}

/// 反向那帧与再次经过同一进度的帧，文字和封面逐格一致。
#[test]
fn reversal_keeps_colors_and_retraces_the_same_frame() -> color_eyre::Result<()> {
    let mut state = state_with_covers(true, true)?;
    let theme = default_theme()?;
    state.browse.view.switch_to(View::Library);
    for _ in 0..3 {
        state.browse.view.tick();
    }
    let progress = state.browse.view.raw();
    let forward = render(&state, &theme, false)?;
    state.browse.view.switch_to(View::Playlists);
    assert_eq!(state.browse.view.raw(), progress);
    assert_eq!(
        render(&state, &theme, false)?,
        forward,
        "反向瞬间不能跳字、跳色或移动封面"
    );
    state.browse.view.tick();
    let backward = render(&state, &theme, false)?;
    state.browse.view.switch_to(View::Library);
    assert_eq!(render(&state, &theme, false)?, backward);
    state.browse.view.tick();
    assert_eq!(state.browse.view.raw(), progress);
    assert_eq!(
        render(&state, &theme, false)?,
        forward,
        "同一进度必须给出同一帧"
    );
    Ok(())
}

/// 两张纯色图中点为均值，封面区外不画图，两端都按同一稳定区域预热。
#[test]
fn covers_crossfade_in_place_and_prewarm_both_endpoints() -> color_eyre::Result<()> {
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
    let buffer = render(&state, &theme, false)?;
    assert_eq!(
        *state.images.encode_pending.borrow(),
        pending,
        "固定尺寸不重复提交编码"
    );
    let [cover, _, _] = sections()?;
    let square = state.images.square_area(cover);
    for y in cover.top()..cover.bottom() {
        for x in cover.left()..cover.right() {
            let cell = buffer.cell((x, y)).ok_or_else(|| eyre!("封面格越界"))?;
            if x >= square.left() && x < square.right() && y >= square.top() && y < square.bottom()
            {
                assert_eq!(cell.symbol(), "▀");
                assert_eq!(cell.fg, Color::Rgb(100, 0, 100), "中点应混合两张封面");
                assert_eq!(cell.bg, Color::Rgb(100, 0, 100));
            } else {
                assert_eq!(cell.symbol(), " ", "封面不能超出稳定正方区域");
                assert_eq!(cell.bg, BACKGROUND);
            }
        }
    }
    Ok(())
}

/// 页面切换中途的封面像素必须与端点稳态逐格一致(同一张封面不换画法)。
///
/// # Params:
///   - `state`: 两端指向同一张封面的状态，尚未切换视图
///   - `steady_cover_url`: 稳态端点实际贴的封面(切换中途也应贴它)
fn assert_steady_cover_through_switch(
    state: &mut AppState,
    theme: &Theme,
    steady_cover_url: &MediaUrl,
) -> color_eyre::Result<()> {
    let [cover, _, _] = sections()?;
    let square = state.images.square_area(cover);
    state
        .images
        .insert_test_terminal_image(steady_cover_url, (square.width, square.height));

    // 端点稳态帧：成品图就绪时贴的是终端图片本身。
    let steady = render(state, theme, false)?;
    state.browse.view.switch_to(View::Library);
    state.browse.view.tick();
    assert!(
        !state.browse.view.at_min() && !state.browse.view.at_max(),
        "前置：视图切换进行中"
    );
    let midway = render(state, theme, false)?;
    let mut pixels = 0_u16;
    for y in square.top()..square.bottom() {
        for x in square.left()..square.right() {
            let expected = steady.cell((x, y)).ok_or_else(|| eyre!("封面格越界"))?;
            let actual = midway.cell((x, y)).ok_or_else(|| eyre!("封面格越界"))?;
            if expected.symbol() != "▀" && actual.symbol() != "▀" {
                // 封面没画到的格子归背景与正文淡出,空格上的 fg 不可见。
                continue;
            }
            pixels = pixels.saturating_add(1);
            assert_eq!(
                (actual.symbol(), actual.fg, actual.bg),
                (expected.symbol(), expected.fg, expected.bg),
                "同一张封面在中途不应换一种画法"
            );
        }
    }
    assert!(pixels > 0, "前置:封面像素应落在测试区域");
    Ok(())
}

/// 歌单与曲目本就是同一张封面(同一 URL)时不做交叉渐变。
#[test]
fn identical_covers_keep_the_steady_image_through_the_switch() -> color_eyre::Result<()> {
    let mut state = state_with_covers(true, true)?;
    let theme = default_theme()?;
    let shared = cover_url(&state, View::Playlists)?;
    let playlist_id = state
        .selected_playlist()
        .ok_or_else(|| eyre!("缺少歌单"))?
        .data
        .id
        .clone();
    let selected = state
        .library
        .tracks
        .get_mut(&playlist_id)
        .and_then(|entries| entries.get_mut(1))
        .ok_or_else(|| eyre!("缺少曲目"))?;
    selected.data.song.cover_url = Some(shared.clone());
    assert_steady_cover_through_switch(&mut state, &theme, &shared)
}

/// URL 不同但图是同一张(Netease 那种尺寸变体)时同样不做交叉渐变。
#[test]
fn matching_cover_pixels_keep_the_steady_image_through_the_switch() -> color_eyre::Result<()> {
    let mut state = state_with_covers(true, true)?;
    let theme = default_theme()?;
    let playlist_url = cover_url(&state, View::Playlists)?;
    let track_url = cover_url(&state, View::Library)?;
    assert_ne!(playlist_url, track_url, "前置:两端 URL 不同");
    // 两图内容仅差一档压缩/重采样噪声，应被内容指纹认成同一张。
    state
        .images
        .cache
        .insert_test(&track_url, Arc::new(solid_cover(201, 0, 0)));
    assert_steady_cover_through_switch(&mut state, &theme, &playlist_url)
}

/// 一端没有 URL，或 URL 尚无完整图时，另一端在固定位置渐变到实际背景。
#[test]
fn single_available_cover_fades_against_background() -> color_eyre::Result<()> {
    let theme = default_theme()?;
    for (cache_playlist, cache_track, expected) in [
        (true, false, Color::Rgb(110, 20, 30)),
        (false, true, Color::Rgb(10, 20, 130)),
    ] {
        for missing_url in [false, true] {
            let mut state = state_with_covers(cache_playlist, cache_track)?;
            if missing_url {
                if cache_playlist {
                    let playlist = state
                        .selected_playlist()
                        .ok_or_else(|| eyre!("缺少歌单"))?
                        .data
                        .id
                        .clone();
                    let selected = state
                        .library
                        .tracks
                        .get_mut(&playlist)
                        .and_then(|entries| entries.get_mut(1))
                        .ok_or_else(|| eyre!("缺少曲目"))?;
                    selected.data.song.cover_url = None;
                } else {
                    state
                        .library
                        .playlists
                        .first_mut()
                        .ok_or_else(|| eyre!("缺少歌单"))?
                        .data
                        .cover_url = None;
                }
            }
            state.browse.view.switch_to(View::Library);
            state.browse.view.tick();
            state.browse.view.tick();
            let buffer = render(&state, &theme, false)?;
            let [cover, _, _] = sections()?;
            let square = state.images.square_area(cover);
            let cell = buffer
                .cell((square.x, square.y))
                .ok_or_else(|| eyre!("封面格越界"))?;
            assert_eq!(cell.symbol(), "▀", "单端图仍在固定位置");
            assert_eq!(cell.fg, expected, "单端图与背景按进度混合");
            assert_eq!(cell.bg, expected);
        }
    }
    Ok(())
}

/// 缺完整图时保留真实 preview；两端都无可用像素时不覆盖背景。
#[test]
fn missing_full_images_preserve_previews_and_empty_background() -> color_eyre::Result<()> {
    let mut state = state_with_covers(false, false)?;
    let theme = default_theme()?;
    state.browse.view.switch_to(View::Library);
    state.browse.view.tick();
    state.browse.view.tick();
    let empty = render(&state, &theme, false)?;
    let [cover, _, _] = sections()?;
    let square = state.images.square_area(cover);
    for y in cover.top()..cover.bottom() {
        for x in cover.left()..cover.right() {
            let cell = empty.cell((x, y)).ok_or_else(|| eyre!("封面格越界"))?;
            assert_eq!(cell.symbol(), " ");
            assert_eq!(cell.bg, BACKGROUND);
        }
    }
    let url = cover_url(&state, View::Playlists)?;
    state
        .images
        .insert_test_preview(&url, (square.width, square.height));
    let preview = render(&state, &theme, false)?;
    let cell = preview
        .cell((square.x, square.y))
        .ok_or_else(|| eyre!("preview 格越界"))?;
    assert_eq!(cell.symbol(), "▀", "完整图缺席时保留真实 preview");
    assert_eq!(
        cell.fg,
        Color::Rgb(10, 20, 30),
        "黑色测试 preview 在中点混入背景"
    );
    assert_eq!(cell.bg, Color::Rgb(10, 20, 30));
    Ok(())
}

/// 页面飞行层接管后，稳定和过渡帧都只画详情，也不从离屏端点提交本地封面编码。
#[test]
fn cover_in_flight_suppresses_local_images_but_keeps_details() -> color_eyre::Result<()> {
    let mut state = state_with_covers(true, true)?;
    let theme = default_theme()?;
    state.browse.view.switch_to(View::Library);
    let [cover, title, _] = sections()?;
    for _ in 0..=4 {
        let buffer = render(&state, &theme, true)?;
        for y in cover.top()..cover.bottom() {
            for x in cover.left()..cover.right() {
                let cell = buffer.cell((x, y)).ok_or_else(|| eyre!("封面格越界"))?;
                assert_eq!(cell.symbol(), " ", "页面飞行层接管时不能双画");
                assert_eq!(cell.bg, BACKGROUND);
            }
        }
        assert!(
            (title.left()..title.right()).any(|x| buffer
                .cell((x, title.y))
                .is_some_and(|cell| cell.symbol() != " " && cell.fg != BACKGROUND)),
            "接管封面不能抹掉详情文字"
        );
        assert!(
            state.images.encode_pending.borrow().is_empty(),
            "被接管的两端不应自画或预热"
        );
        state.browse.view.tick();
    }
    Ok(())
}
