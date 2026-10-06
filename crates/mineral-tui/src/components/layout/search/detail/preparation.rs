//! 详情组件的布局准备：滚动边界、标题相位和图片需求在绘制之前确定。

use super::{
    body::{TrackList, album_widths},
    geometry, meta,
    track_table::TrackColumns,
};
use crate::components::frame::PrepareCx;
use crate::components::layout::shared::{marquee, thumbnails};
use crate::image::{ImageNeeds, ImageRenderPhase};
use crate::runtime::scroll::list::ScrollMotion;
use crate::runtime::state::{ArtistSection, DetailData, DetailFrame, EntityRef, SearchPage};
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders};

/// 准备当前详情帧及仍参与过渡的出发帧。
pub(crate) fn prepare(
    area: Rect,
    page: &mut SearchPage,
    cx: &mut PrepareCx<'_>,
    cover_in_flight: bool,
) {
    let inner = Block::new().borders(Borders::ALL).inner(area);
    if inner.height < 2 || inner.width == 0 {
        return;
    }
    let phase = cx.image_phase;
    let motion = cx.motion;
    let ticks = cx.frame.cursor_ticks();
    let thumbnail_phase = thumbnails::thumbnail_phase(
        phase,
        motion,
        cx.frame.now,
        page.last_sel_change,
        std::time::Duration::from_millis(*cx.frame.config.cover().debounce_ms()),
    );
    let now = cx.frame.now;
    let theme = cx.frame.theme;
    let advance = cx.advance;
    let images = &mut cx.images;
    let Some(results) = page.active_results_mut() else {
        return;
    };
    let show_back = results.detail.depth() > 0;
    results.detail.prepare_visible(|frame, stable| {
        let (head, _, body) = geometry::split_frame(inner);
        let artist = matches!(frame.entity, EntityRef::Artist(_));
        let (cover, metadata, selected_cover) = geometry::split_head(head, artist);
        let phase = if stable {
            phase
        } else {
            ImageRenderPhase::Offscreen
        };
        if !cover_in_flight {
            images.display(frame.entity.cover(), cover, phase);
        }
        if let Some(area) = selected_cover {
            images.display(frame.selected_cover(), area, phase);
        }
        meta::prepare(metadata, frame, theme, stable && show_back);
        let single = frame
            .artist_sections()
            .is_some_and(|sections| sections.kinds().len() < 2);
        let list_area = if artist && !single {
            geometry::split_artist_body(body).1
        } else {
            body
        };
        let motion = if stable && frame.section_eased().is_none() {
            motion
        } else {
            ScrollMotion::Frozen
        };
        let total = frame.list_len();
        frame.list_mut().prepare(
            total,
            usize::from(list_area.height.saturating_sub(1)),
            motion,
            ticks,
            advance,
        );
        let phase = if matches!(motion, ScrollMotion::Frozen) {
            ImageRenderPhase::Offscreen
        } else {
            thumbnail_phase
        };
        if artist && frame.section_eased().is_some() {
            prepare_rows(list_area, frame, ArtistSection::Hot, images, phase, now);
            prepare_rows(list_area, frame, ArtistSection::Albums, images, phase, now);
        } else {
            prepare_rows(list_area, frame, frame.section, images, phase, now);
        }
    });
}

/// 按该帧现有列布局准备一个可见窗口，未加载的数据不产生行需求。
fn prepare_rows(
    area: Rect,
    frame: &mut DetailFrame,
    section: ArtistSection,
    images: &mut ImageNeeds<'_>,
    phase: ImageRenderPhase,
    now: std::time::Instant,
) {
    let show_cover = images.ready().supports_thumbnails();
    let mut songs = TrackList::Songs(&[]);
    let mut albums = None;
    let mut cols = TrackColumns::new(true, false);
    match &frame.data {
        Some(DetailData::Album(album)) => songs = TrackList::Album(&album.tracks),
        Some(DetailData::PlaylistEntries(entries)) => {
            cols = TrackColumns::new(true, true);
            songs = TrackList::Playlist(entries);
        }
        Some(DetailData::Artist {
            detail,
            albums: available,
        }) => match section {
            ArtistSection::Hot => {
                cols = TrackColumns::new(false, true);
                if let Some(detail) = detail {
                    songs = TrackList::Songs(&detail.songs);
                }
            }
            ArtistSection::Albums => {
                albums = available
                    .as_ref()
                    .map(crate::runtime::state::ArtistAlbums::items);
            }
        },
        None => return,
    }
    let cols = cols.with_thumbnails(show_cover).for_width(area.width);
    let widths = if albums.is_some() {
        album_widths(show_cover)
    } else {
        cols.widths()
    };
    let columns = marquee::resolve_column_rects(area, &widths, 2);
    let total = albums.map_or(songs.len(), <[mineral_model::Album]>::len);
    let viewport = usize::from(area.height.saturating_sub(1));
    let offset = frame.list().offset(total, viewport);
    let visible = offset..offset.saturating_add(viewport).min(total);
    let selected = frame.list().sel();
    if albums.is_none()
        && visible.contains(&selected)
        && let Some(song) = songs.song(selected)
    {
        marquee::prepare_song(
            &mut frame.title,
            song,
            columns
                .get(cols.title_index())
                .map_or(0, |column| column.width),
            now,
        );
    }
    let cover_index = usize::from(albums.is_none());
    if show_cover && let Some(column) = columns.get(cover_index) {
        thumbnails::declare_table_thumbnails(
            images,
            *column,
            visible.map(|index| match albums {
                Some(albums) => albums.get(index).and_then(|album| album.cover_url.as_ref()),
                None => songs.song(index).and_then(|song| song.cover_url.as_ref()),
            }),
            phase,
        );
    }
}
