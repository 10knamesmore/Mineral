//! 艺人专辑与 UP 主投稿的分页、导航和渲染回归。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use color_eyre::eyre::eyre;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use mineral_channel_core::{ArtistSectionKind, ArtistSections, ChannelCaps, Page};
use mineral_model::{Album, AlbumId, Artist, ArtistId, SearchKind, SourceKind};
use mineral_task::{ChannelFetchKind, SearchPayload, TaskEvent, TaskKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::App;
use crate::runtime::scroll::list::ScrollMotion;
use crate::runtime::state::{ArtistSection, DetailFrame, EntityRef};

/// 已进入艺人 Albums 区的应用，后端探针保留实际任务提交次数。
struct ArtistAlbumsTest {
    /// 真实按键与回包消费入口。
    app: App,

    /// 待验证的艺人身份，包含来源 namespace。
    artist_id: ArtistId,

    /// 后端请求记录，不执行 scheduler 去重。
    submitted: Arc<Mutex<Vec<TaskKind>>>,
}

impl ArtistAlbumsTest {
    /// 从搜索结果进入详情，走驻留预取请求首页，再注入相应来源的首批专辑。
    fn new(source: SourceKind, count: u32, has_more: bool) -> color_eyre::Result<Self> {
        let (mut app, submitted) = crate::test_support::app_with_channel_search_probed(vec![
            SearchKind::Artist,
            SearchKind::Album,
        ])?;
        app.state.caps.insert(
            SourceKind::BILIBILI,
            ChannelCaps::builder()
                .searchable(vec![SearchKind::Artist, SearchKind::Album])
                .playlist_edit(false)
                .artist_sections(ArtistSections::new(vec![ArtistSectionKind::Albums]))
                .build(),
        );
        app.state
            .channel_search
            .switch_source(source, &app.state.caps);
        app.state.channel_search.select_kind(SearchKind::Artist);
        let artist_id = ArtistId::new(source, "artist");
        let mut test = Self {
            app,
            artist_id,
            submitted,
        };
        test.press(KeyCode::Char('q'));
        test.press(KeyCode::Enter);
        let artist = Artist::builder()
            .id(test.artist_id.clone())
            .name("Artist".to_owned())
            .songs(crate::test_support::endserenading(3))
            .build();
        test.app.state.apply(&TaskEvent::SearchResults {
            source,
            kind: SearchKind::Artist,
            query: "q".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Artists(vec![artist.clone()]),
            has_more: Some(false),
        });
        test.press(KeyCode::Char('l'));
        test.app.state.channel_search.last_sel_change = Instant::now()
            .checked_sub(Duration::from_secs(3600))
            .ok_or_else(|| eyre!("无法设置驻留时间"))?;
        crate::runtime::prefetch::tick(&mut test.app.state, &*test.app.client, Vec::new());
        assert_eq!(
            test.requested_pages()?,
            vec![Page::default()],
            "驻留预取首页"
        );
        test.app.state.apply(&TaskEvent::ArtistDetailFetched {
            id: test.artist_id.clone(),
            artist: Box::new(artist),
        });
        test.receive(Page::default(), count, has_more);
        if source == SourceKind::NETEASE {
            test.press(KeyCode::Char(']'));
        }
        assert_eq!(test.frame()?.section, ArtistSection::Albums);
        Ok(test)
    }

    /// 通过终端事件入口派发按键。
    fn press(&mut self, code: KeyCode) {
        self.app
            .handle_event(&Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    /// 返回详情父帧或上一级面板。
    fn back(&mut self) {
        self.app.handle_event(&Event::Key(KeyEvent::new(
            KeyCode::Char('h'),
            KeyModifiers::CONTROL,
        )));
    }

    /// 构造来源内唯一的专辑 ID；第二页不会复用第一页身份。
    fn album_id(&self, index: u32) -> AlbumId {
        AlbumId::new(self.artist_id.namespace(), format!("album-{index}"))
    }

    /// 按请求页投递专辑，count 可小于 limit，包括来源明确还有下一页的空页。
    fn receive(&mut self, page: Page, count: u32, has_more: bool) {
        let albums = (page.offset..page.offset + count)
            .map(|index| {
                Album::builder()
                    .id(self.album_id(index))
                    .name(format!("Album {index}"))
                    .build()
            })
            .collect();
        self.app.state.apply(&TaskEvent::ArtistAlbumsFetched {
            id: self.artist_id.clone(),
            page,
            albums,
            has_more: Some(has_more),
        });
    }

    /// 当前栈顶详情。
    fn frame(&self) -> color_eyre::Result<&DetailFrame> {
        self.app
            .state
            .channel_search
            .active_results()
            .and_then(|results| results.detail.current())
            .ok_or_else(|| eyre!("缺少详情帧"))
    }

    /// 当前栈顶详情，可设置测试所需的既有导航位置。
    fn frame_mut(&mut self) -> color_eyre::Result<&mut DetailFrame> {
        self.app
            .state
            .channel_search
            .active_results_mut()
            .and_then(|results| results.detail.current_mut())
            .ok_or_else(|| eyre!("缺少详情帧"))
    }

    /// 艺人专辑任务携带的实际页参数，包含驻留预取的首页。
    fn requested_pages(&self) -> color_eyre::Result<Vec<Page>> {
        Ok(self
            .submitted
            .lock()
            .map_err(|e| eyre!("任务探针锁中毒: {e}"))?
            .iter()
            .filter_map(|task| match task {
                TaskKind::ChannelFetch(ChannelFetchKind::ArtistAlbums { id, page })
                    if *id == self.artist_id =>
                {
                    Some(*page)
                }
                _ => None,
            })
            .collect())
    }

    /// 校验完整列表顺序，检测重复追加、漏页及整表替换。
    fn assert_album_ids(&self, indices: impl IntoIterator<Item = u32>) -> color_eyre::Result<()> {
        let albums = self
            .frame()?
            .current_album_list()
            .ok_or_else(|| eyre!("缺少专辑列表"))?;
        assert_eq!(
            albums
                .items()
                .iter()
                .map(|album| album.id.clone())
                .collect::<Vec<_>>(),
            indices
                .into_iter()
                .map(|index| self.album_id(index))
                .collect::<Vec<_>>()
        );
        Ok(())
    }

    /// 渲染真实详情面板，读取底边的位置、更多页及 loading 提示。
    fn footer(&mut self) -> color_eyre::Result<String> {
        let mut terminal = Terminal::new(TestBackend::new(120, 44))?;
        for _ in 0..64 {
            self.app.state.channel_search.tick();
        }
        terminal.draw(|frame| crate::view::draw(frame, &self.app))?;
        let area = crate::components::layout::shared::compute::compute_search(
            self.app.state.frame_area.get(),
            self.app.state.cfg.tui().layout(),
        )
        .right
        .ok_or_else(|| eyre!("缺少详情面板区域"))?;
        let buffer = terminal.backend().buffer();
        Ok((area.x..area.right())
            .filter_map(|x| buffer.cell((x, area.bottom() - 1)))
            .map(ratatui::buffer::Cell::symbol)
            .collect())
    }
}

/// 两个来源都能加载第二页并下钻其中的专辑；追加保留选择，重复/超前回包不改分页进度。
#[test]
fn artist_and_uploader_pages_append_and_remain_navigable() -> color_eyre::Result<()> {
    let limit = Page::default().limit;
    for source in [SourceKind::NETEASE, SourceKind::BILIBILI] {
        let mut test = ArtistAlbumsTest::new(source, limit, true)?;
        assert!(test.footer()?.contains("1 / 30+"));
        test.press(KeyCode::Char('G'));
        test.frame_mut()?.list_mut().place(29, 22);
        let second = Page::new(limit, limit);
        for key in ['k', 'j', 'G', 'j'] {
            test.press(KeyCode::Char(key));
        }
        assert_eq!(test.requested_pages()?, vec![Page::default(), second]);
        assert!(test.footer()?.contains("loading"));
        let before_offset = test.frame()?.list().offset(30, 6, ScrollMotion::Frozen);
        test.receive(Page::default(), limit, false);
        test.receive(Page::new(2 * limit, limit), limit, false);
        test.receive(Page::new(limit, limit + 1), limit, false);
        test.assert_album_ids(0..limit)?;
        test.receive(second, limit, false);
        test.receive(second, limit, false);
        test.assert_album_ids(0..2 * limit)?;
        assert_eq!(test.frame()?.list().sel(), 29);
        assert_eq!(
            test.frame()?.list().offset(60, 6, ScrollMotion::Frozen),
            before_offset
        );
        test.receive(Page::default(), limit, true);
        assert_eq!(
            test.frame()?.list().offset(60, 6, ScrollMotion::Frozen),
            before_offset
        );
        let footer = test.footer()?;
        assert!(footer.contains("30 / 60"));
        assert!(!footer.contains('+'));
        assert!(!footer.contains("loading"));
        for _ in 0..5 {
            test.press(KeyCode::Char('j'));
        }
        let expected = test.album_id(34);
        assert!(
            matches!(test.frame()?.row_entity(), Some(EntityRef::Album(album)) if album.id == expected)
        );
        test.press(KeyCode::Char('l'));
        assert!(matches!(&test.frame()?.entity, EntityRef::Album(album) if album.id == expected));
        assert_eq!(test.requested_pages()?, vec![Page::default(), second]);
    }
    Ok(())
}

/// 显式更多页信号优先于短页和过滤后的空页；offset 按请求页大小推进。
#[test]
fn short_and_empty_album_pages_with_more_do_not_end_pagination() -> color_eyre::Result<()> {
    let limit = Page::default().limit;
    for first_count in [0, 2] {
        let mut test = ArtistAlbumsTest::new(SourceKind::BILIBILI, first_count, true)?;
        assert!(test.footer()?.contains(&format!("/ {first_count}+")));
        test.press(KeyCode::Char('j'));
        let second = Page::new(limit, limit);
        assert_eq!(test.requested_pages()?, vec![Page::default(), second]);
        test.receive(second, 0, true);
        assert!(test.footer()?.contains('+'));
        test.press(KeyCode::Char('G'));
        let third = Page::new(2 * limit, limit);
        assert_eq!(
            test.requested_pages()?,
            vec![Page::default(), second, third]
        );
        test.receive(third, 1, false);
        test.press(KeyCode::Char('G'));
        test.assert_album_ids((0..first_count).chain([2 * limit]))?;
        assert!(
            matches!(test.frame()?.row_entity(), Some(EntityRef::Album(album)) if album.id == test.album_id(2 * limit))
        );
        assert!(!test.footer()?.contains('+'));
        assert_eq!(
            test.requested_pages()?,
            vec![Page::default(), second, third]
        );
    }
    Ok(())
}

/// 续页回包在下钻及切 source/kind 后仍更新原艺人父帧，返回时保留光标和分页进度。
#[test]
fn album_page_reaches_retained_parent_across_source_and_kind_switches() -> color_eyre::Result<()> {
    let limit = Page::default().limit;
    let mut test = ArtistAlbumsTest::new(SourceKind::NETEASE, limit, true)?;
    let second = Page::new(limit, limit);
    test.press(KeyCode::Char('G'));
    test.press(KeyCode::Char('l'));
    test.app.state.channel_search.select_kind(SearchKind::Album);
    test.app
        .state
        .channel_search
        .switch_source(SourceKind::BILIBILI, &test.app.state.caps);
    test.receive(second, limit, true);
    assert!(test.app.state.channel_search.active_results().is_none());
    test.app
        .state
        .channel_search
        .switch_source(SourceKind::NETEASE, &test.app.state.caps);
    test.app
        .state
        .channel_search
        .select_kind(SearchKind::Artist);
    assert!(matches!(&test.frame()?.entity, EntityRef::Album(_)));
    assert!(test.frame()?.data.is_none());
    test.back();
    test.assert_album_ids(0..2 * limit)?;
    assert_eq!(test.frame()?.list().sel(), 29);
    assert_eq!(test.frame()?.section, ArtistSection::Albums);
    test.press(KeyCode::Char('G'));
    assert_eq!(
        test.requested_pages()?,
        vec![Page::default(), second, Page::new(2 * limit, limit)]
    );
    Ok(())
}

/// 失败只释放完整艺人 ID 与页匹配的续页；离屏失败可重试，旧失败不能释放新页。
#[test]
fn failed_album_page_can_retry_after_returning_to_artist() -> color_eyre::Result<()> {
    let limit = Page::default().limit;
    let mut test = ArtistAlbumsTest::new(SourceKind::NETEASE, limit, true)?;
    let second = Page::new(limit, limit);
    let third = Page::new(2 * limit, limit);
    test.press(KeyCode::Char('G'));
    for (id, page) in [
        (ArtistId::new(SourceKind::BILIBILI, "artist"), second),
        (test.artist_id.clone(), third),
    ] {
        test.app
            .state
            .apply(&TaskEvent::ArtistAlbumsPageFailed { id, page });
        test.press(KeyCode::Char('j'));
        assert_eq!(test.requested_pages()?, vec![Page::default(), second]);
    }
    test.press(KeyCode::Char('l'));
    let failure = TaskEvent::ArtistAlbumsPageFailed {
        id: test.artist_id.clone(),
        page: second,
    };
    test.app.state.apply(&failure);
    test.back();
    test.assert_album_ids(0..limit)?;
    assert!(!test.footer()?.contains("loading"));
    test.press(KeyCode::Char('G'));
    assert_eq!(
        test.requested_pages()?,
        vec![Page::default(), second, second]
    );
    test.receive(second, limit, true);
    test.press(KeyCode::Char('G'));
    test.app.state.apply(&failure);
    test.press(KeyCode::Char('j'));
    assert_eq!(
        test.requested_pages()?,
        vec![Page::default(), second, second, third]
    );
    test.receive(third, 1, false);
    test.assert_album_ids(0..2 * limit + 1)?;
    Ok(())
}

/// 热门曲和结果列不触发艺人专辑分页；专辑预取半径读取当前热更后的配置。
#[test]
fn album_prefetch_uses_current_radius_and_only_the_albums_section() -> color_eyre::Result<()> {
    let mut test = ArtistAlbumsTest::new(SourceKind::NETEASE, Page::default().limit, true)?;
    test.press(KeyCode::Char(']'));
    test.press(KeyCode::Char('G'));
    assert_eq!(test.frame()?.section, ArtistSection::Hot);
    assert_eq!(test.requested_pages()?, vec![Page::default()]);
    test.back();
    test.press(KeyCode::Char('G'));
    assert_eq!(test.requested_pages()?, vec![Page::default()]);
    test.press(KeyCode::Char('l'));
    test.press(KeyCode::Char(']'));
    let tree = mineral_config::merge_tree(
        mineral_config::default_tree()?,
        serde_json::json!({"tui":{"behavior":{"search_prefetch_rows":2}}}),
    );
    test.app
        .apply_pushed_config(mineral_protocol::BusValue::from_json(tree));
    test.frame_mut()?.list_mut().set_sel(25);
    test.press(KeyCode::Char('j'));
    assert_eq!(test.requested_pages()?, vec![Page::default()]);
    test.press(KeyCode::Char('j'));
    assert_eq!(
        test.requested_pages()?,
        vec![Page::default(), Page::new(30, 30)]
    );
    Ok(())
}
