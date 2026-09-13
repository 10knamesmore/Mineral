//! 搜索续页的按键、请求与回包时序回归。

use std::sync::{Arc, Mutex};

use color_eyre::eyre::eyre;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use mineral_channel_core::Page;
use mineral_model::{SearchKind, Song, SongId, SourceKind};
use mineral_task::{ChannelFetchKind, SearchPayload, TaskEvent, TaskKind};

use crate::App;

/// 真实按键入口与后端请求探针；回包由测试显式投递以控制收包时序。
struct SearchTest {
    /// 已进入 Search、收到首页的应用。
    app: App,

    /// 后端收到的全部任务，探针不做 scheduler 去重。
    submitted: Arc<Mutex<Vec<TaskKind>>>,
}

impl SearchTest {
    /// 提交搜索并注入一页结果，后续各页使用不同歌曲 ID。
    fn new() -> color_eyre::Result<Self> {
        let (app, submitted) = crate::test_support::app_with_channel_search_probed(vec![
            SearchKind::Song,
            SearchKind::Album,
        ])?;
        let mut test = Self { app, submitted };
        test.press(KeyCode::Char('q'));
        test.press(KeyCode::Enter);
        test.app.state.apply(&results(Page::default(), true));
        Ok(test)
    }

    /// 经终端事件入口派发按键。
    fn press(&mut self, code: KeyCode) {
        self.app
            .handle_event(&Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    /// 读取已发出的搜索分页参数，排除首页和详情任务。
    fn requested_pages(&self) -> color_eyre::Result<Vec<Page>> {
        Ok(self
            .submitted
            .lock()
            .map_err(|e| eyre!("任务探针锁中毒: {e}"))?
            .iter()
            .filter_map(|task| match task {
                TaskKind::ChannelFetch(ChannelFetchKind::Search { page, .. })
                    if page.offset > 0 =>
                {
                    Some(*page)
                }
                _ => None,
            })
            .collect())
    }

    /// 核对完整结果顺序；重复追加或缺页均会改变 ID 序列。
    fn assert_song_ids(&self, end: u32) -> color_eyre::Result<()> {
        let SearchPayload::Songs(songs) = &self
            .app
            .state
            .channel_search
            .active_results()
            .ok_or_else(|| eyre!("缺少结果桶"))?
            .results
        else {
            return Err(eyre!("应为歌曲结果"));
        };
        assert_eq!(
            songs.iter().map(|song| song.id.clone()).collect::<Vec<_>>(),
            (0..end).map(song_id).collect::<Vec<_>>()
        );
        Ok(())
    }
}

/// 按页内绝对位置生成歌曲身份，用于检查跨页顺序。
fn song_id(index: u32) -> SongId {
    SongId::new(SourceKind::NETEASE, format!("result-{index}"))
}

/// 一页搜索结果，ID 区间与请求 offset 对齐。
fn results(page: Page, has_more: bool) -> TaskEvent {
    TaskEvent::SearchResults {
        source: SourceKind::NETEASE,
        kind: SearchKind::Song,
        query: "q".to_owned(),
        page,
        payload: SearchPayload::Songs(
            (page.offset..page.offset + page.limit)
                .map(|index| {
                    Song::builder()
                        .id(song_id(index))
                        .name(format!("song {index}"))
                        .build()
                })
                .collect(),
        ),
        has_more: Some(has_more),
    }
}

/// 连按期间仅发一页；重复、超前或 limit 不符的回包既不追加，也不释放正在等待的下一页。
#[test]
fn repeated_navigation_and_duplicate_replies_do_not_skip_pages() -> color_eyre::Result<()> {
    let mut test = SearchTest::new()?;
    let limit = Page::default().limit;
    let second = Page::new(limit, limit);
    let third = Page::new(2 * limit, limit);
    let second_result = results(second, true);
    test.app.state.apply(&second_result);
    test.assert_song_ids(limit)?;
    test.press(KeyCode::Char('G'));
    // 回包已构造但尚未交给 TUI，模拟 worker 已完成、按键先于收包的窗口。
    for key in ['k', 'j', 'G', 'j'] {
        test.press(KeyCode::Char(key));
    }
    assert_eq!(test.requested_pages()?, vec![second]);

    test.app.state.apply(&results(third, false));
    test.app
        .state
        .apply(&results(Page::new(limit, limit + 1), false));
    test.press(KeyCode::Char('G'));
    assert_eq!(test.requested_pages()?, vec![second]);
    test.assert_song_ids(limit)?;

    test.app.state.apply(&second_result);
    test.app.state.apply(&second_result);
    test.assert_song_ids(2 * limit)?;
    test.press(KeyCode::Char('G'));
    assert_eq!(test.requested_pages()?, vec![second, third]);
    test.app.state.apply(&second_result);
    test.press(KeyCode::Char('j'));
    assert_eq!(test.requested_pages()?, vec![second, third]);
    test.assert_song_ids(2 * limit)?;

    test.app.state.apply(&results(third, false));
    test.press(KeyCode::Char('G'));
    test.assert_song_ids(3 * limit)?;
    assert_eq!(test.requested_pages()?, vec![second, third]);
    Ok(())
}

/// 只有匹配的失败事件释放续页；切到别的 kind 后失败仍允许原桶重试相同页。
#[test]
fn failed_page_retries_without_advancing_or_clearing_other_requests() -> color_eyre::Result<()> {
    let mut test = SearchTest::new()?;
    let limit = Page::default().limit;
    let second = Page::new(limit, limit);
    test.press(KeyCode::Char('G'));
    for (source, kind, query, page) in [
        (SourceKind::BILIBILI, SearchKind::Song, "q", second),
        (SourceKind::NETEASE, SearchKind::Album, "q", second),
        (SourceKind::NETEASE, SearchKind::Song, "old", second),
        (
            SourceKind::NETEASE,
            SearchKind::Song,
            "q",
            Page::new(2 * limit, limit),
        ),
    ] {
        test.app.state.apply(&TaskEvent::SearchPageFailed {
            source,
            kind,
            query: query.to_owned(),
            page,
        });
        test.press(KeyCode::Char('j'));
        assert_eq!(test.requested_pages()?, vec![second]);
    }
    test.app.state.channel_search.select_kind(SearchKind::Album);
    let failure = TaskEvent::SearchPageFailed {
        source: SourceKind::NETEASE,
        kind: SearchKind::Song,
        query: "q".to_owned(),
        page: second,
    };
    test.app.state.apply(&failure);
    test.app.state.channel_search.select_kind(SearchKind::Song);
    test.assert_song_ids(limit)?;
    test.press(KeyCode::Char('G'));
    assert_eq!(test.requested_pages()?, vec![second, second]);
    test.app.state.apply(&results(second, true));
    test.press(KeyCode::Char('G'));
    let third = Page::new(2 * limit, limit);
    test.app.state.apply(&failure);
    test.press(KeyCode::Char('j'));
    assert_eq!(test.requested_pages()?, vec![second, second, third]);
    test.app.state.apply(&results(third, false));
    test.assert_song_ids(3 * limit)?;
    Ok(())
}

/// 切 source 和 kind 不丢失待收取页；离屏成功回包追加原桶，返回后继续下一页。
#[test]
fn pending_page_survives_source_and_kind_switches() -> color_eyre::Result<()> {
    let mut test = SearchTest::new()?;
    let limit = Page::default().limit;
    let second = Page::new(limit, limit);
    test.press(KeyCode::Char('G'));
    let caps = test
        .app
        .state
        .caps
        .get(&SourceKind::NETEASE)
        .cloned()
        .ok_or_else(|| eyre!("缺少 caps"))?;
    test.app.state.caps.insert(SourceKind::BILIBILI, caps);
    test.app.state.channel_search.select_kind(SearchKind::Album);
    test.app
        .state
        .channel_search
        .switch_source(SourceKind::BILIBILI, &test.app.state.caps);
    test.app.state.apply(&results(second, true));
    assert!(test.app.state.channel_search.active_results().is_none());
    test.app
        .state
        .channel_search
        .switch_source(SourceKind::NETEASE, &test.app.state.caps);
    assert!(test.app.state.channel_search.active_results().is_none());
    test.app.state.channel_search.select_kind(SearchKind::Song);
    test.assert_song_ids(2 * limit)?;
    test.press(KeyCode::Char('G'));
    assert_eq!(
        test.requested_pages()?,
        vec![second, Page::new(2 * limit, limit)]
    );
    Ok(())
}

/// 编辑 query 销毁续页状态；迟到的旧词成功与失败回包不能影响新词结果及其待收取页。
#[test]
fn editing_query_discards_pending_page_and_old_replies() -> color_eyre::Result<()> {
    let mut test = SearchTest::new()?;
    let limit = Page::default().limit;
    let second = Page::new(limit, limit);
    test.press(KeyCode::Char('G'));
    test.press(KeyCode::Char('/'));
    test.press(KeyCode::Char('x'));
    test.press(KeyCode::Enter);
    let mut first = results(Page::default(), true);
    if let TaskEvent::SearchResults { query, .. } = &mut first {
        *query = "qx".to_owned();
    }
    test.app.state.apply(&first);
    test.press(KeyCode::Char('G'));
    test.app.state.apply(&results(second, true));
    test.app.state.apply(&TaskEvent::SearchPageFailed {
        source: SourceKind::NETEASE,
        kind: SearchKind::Song,
        query: "q".to_owned(),
        page: second,
    });
    test.press(KeyCode::Char('j'));
    test.assert_song_ids(limit)?;
    assert_eq!(test.requested_pages()?, vec![second, second]);
    let mut next = results(second, false);
    if let TaskEvent::SearchResults { query, .. } = &mut next {
        *query = "qx".to_owned();
    }
    test.app.state.apply(&next);
    test.assert_song_ids(2 * limit)?;
    Ok(())
}
