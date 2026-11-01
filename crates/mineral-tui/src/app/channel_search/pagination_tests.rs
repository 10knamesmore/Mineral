//! 搜索续页的按键、请求与回包时序回归。

use std::sync::Arc;

use color_eyre::eyre::eyre;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use mineral_model::{SearchKind, Song, SongId, SourceKind};
use mineral_protocol::{SubscriptionDemand, SubscriptionTopic};
use mineral_task::{SearchPayload, TaskEvent};
use rustc_hash::FxHashSet;

use crate::App;
use crate::test_support::TestClient;

const BATCH_SONGS: u64 = 30;

/// 真实按键入口与后端请求探针；回包由测试显式投递以控制收包时序。
struct SearchTest {
    /// 已进入 Search、收到首页的应用。
    app: App,

    /// 后端收到的订阅和推进命令。
    submitted: Arc<TestClient>,
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
        test.app.state.apply(&results(0, true));
        Ok(test)
    }

    /// 经终端事件入口派发按键。
    fn press(&mut self, code: KeyCode) {
        self.app
            .handle_event(&Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    /// 排除每个订阅的首批，核对后续请求确实复用该订阅。
    fn requested_queries(&self) -> color_eyre::Result<Vec<String>> {
        let subscriptions = self
            .submitted
            .subscriptions
            .lock()
            .map_err(|e| eyre!("{e}"))?;
        let requests = self.submitted.advances.lock().map_err(|e| eyre!("{e}"))?;
        let mut seen = FxHashSet::default();
        let mut queries = Vec::new();
        for request in requests.iter() {
            assert_eq!(request.demand, SubscriptionDemand::Next);
            if !seen.insert(request.subscription)
                && let Some((_, SubscriptionTopic::Search { query, .. })) = subscriptions
                    .iter()
                    .find(|(id, _)| *id == request.subscription)
            {
                queries.push(query.clone());
            }
        }
        Ok(queries)
    }

    /// 核对完整结果顺序；重复追加或缺页均会改变 ID 序列。
    fn assert_song_ids(&self, end: u64) -> color_eyre::Result<()> {
        let SearchPayload::Songs(songs) = &self
            .app
            .state
            .ui
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
fn song_id(index: u64) -> SongId {
    SongId::new(SourceKind::NETEASE, format!("result-{index}"))
}

/// 每批使用独立的歌曲 ID 区间。
fn results(batch: u64, has_more: bool) -> TaskEvent {
    let start = batch * BATCH_SONGS;
    TaskEvent::SearchResults {
        source: SourceKind::NETEASE,
        kind: SearchKind::Song,
        query: "q".to_owned(),
        batch,
        payload: SearchPayload::Songs(
            (start..start + BATCH_SONGS)
                .map(|index| {
                    Song::builder()
                        .id(song_id(index))
                        .name(format!("song {index}"))
                        .build()
                })
                .collect(),
        ),
        complete: !has_more,
    }
}

/// 连按期间仅推进一次；重复或超前的回包不追加，也不释放当前等待。
#[test]
fn repeated_navigation_and_duplicate_replies_do_not_skip_pages() -> color_eyre::Result<()> {
    let mut test = SearchTest::new()?;
    let limit = BATCH_SONGS;
    let second = 1;
    let third = 2;
    let second_result = results(second, true);
    test.app.state.apply(&second_result);
    test.assert_song_ids(limit)?;
    test.press(KeyCode::Char('G'));
    // 回包已构造但尚未交给 TUI，模拟 worker 已完成、按键先于收包的窗口。
    for key in ['k', 'j', 'G', 'j'] {
        test.press(KeyCode::Char(key));
    }
    assert_eq!(test.requested_queries()?, vec!["q"]);

    test.app.state.apply(&results(third, false));
    test.press(KeyCode::Char('G'));
    assert_eq!(test.requested_queries()?, vec!["q"]);
    test.assert_song_ids(limit)?;

    test.app.state.apply(&second_result);
    test.app.state.apply(&second_result);
    test.assert_song_ids(2 * limit)?;
    test.press(KeyCode::Char('G'));
    assert_eq!(test.requested_queries()?, vec!["q", "q"]);
    test.app.state.apply(&second_result);
    test.press(KeyCode::Char('j'));
    assert_eq!(test.requested_queries()?, vec!["q", "q"]);
    test.assert_song_ids(2 * limit)?;

    test.app.state.apply(&results(third, false));
    test.press(KeyCode::Char('G'));
    test.assert_song_ids(3 * limit)?;
    assert_eq!(test.requested_queries()?, vec!["q", "q"]);
    Ok(())
}

/// 只有匹配的失败事件释放续页；切到别的 kind 后失败仍允许原桶重试相同页。
#[test]
fn failed_page_retries_without_advancing_or_clearing_other_requests() -> color_eyre::Result<()> {
    let mut test = SearchTest::new()?;
    let limit = BATCH_SONGS;
    let second = 1;
    test.press(KeyCode::Char('G'));
    for (source, kind, query, batch) in [
        (SourceKind::BILIBILI, SearchKind::Song, "q", second),
        (SourceKind::NETEASE, SearchKind::Album, "q", second),
        (SourceKind::NETEASE, SearchKind::Song, "old", second),
        (SourceKind::NETEASE, SearchKind::Song, "q", 2),
    ] {
        test.app.state.apply(&TaskEvent::SearchPageFailed {
            source,
            kind,
            query: query.to_owned(),
            batch,
        });
        test.press(KeyCode::Char('j'));
        assert_eq!(test.requested_queries()?, vec!["q"]);
    }
    test.app
        .state
        .ui
        .channel_search
        .select_kind(SearchKind::Album);
    let failure = TaskEvent::SearchPageFailed {
        source: SourceKind::NETEASE,
        kind: SearchKind::Song,
        query: "q".to_owned(),
        batch: second,
    };
    test.app.state.apply(&failure);
    test.app
        .state
        .ui
        .channel_search
        .select_kind(SearchKind::Song);
    test.assert_song_ids(limit)?;
    test.press(KeyCode::Char('G'));
    assert_eq!(test.requested_queries()?, vec!["q", "q"]);
    test.app.state.apply(&results(second, true));
    test.press(KeyCode::Char('G'));
    let third = 2;
    test.app.state.apply(&failure);
    test.press(KeyCode::Char('j'));
    assert_eq!(test.requested_queries()?, vec!["q", "q", "q"]);
    test.app.state.apply(&results(third, false));
    test.assert_song_ids(3 * limit)?;
    Ok(())
}

/// 切 source 和 kind 不丢失待收取页；离屏成功回包追加原桶，返回后继续下一页。
#[test]
fn pending_page_survives_source_and_kind_switches() -> color_eyre::Result<()> {
    let mut test = SearchTest::new()?;
    let limit = BATCH_SONGS;
    let second = 1;
    test.press(KeyCode::Char('G'));
    let caps = test
        .app
        .state
        .models
        .caps
        .get(&SourceKind::NETEASE)
        .cloned()
        .ok_or_else(|| eyre!("缺少 caps"))?;
    test.app
        .state
        .models
        .caps
        .insert(SourceKind::BILIBILI, caps);
    test.app
        .state
        .ui
        .channel_search
        .select_kind(SearchKind::Album);
    test.app
        .state
        .ui
        .channel_search
        .switch_source(SourceKind::BILIBILI, &test.app.state.models.caps);
    test.app.state.apply(&results(second, true));
    assert!(test.app.state.ui.channel_search.active_results().is_none());
    test.app
        .state
        .ui
        .channel_search
        .switch_source(SourceKind::NETEASE, &test.app.state.models.caps);
    assert!(test.app.state.ui.channel_search.active_results().is_none());
    test.app
        .state
        .ui
        .channel_search
        .select_kind(SearchKind::Song);
    test.assert_song_ids(2 * limit)?;
    test.press(KeyCode::Char('G'));
    assert_eq!(test.requested_queries()?, vec!["q", "q"]);
    Ok(())
}

/// 编辑 query 销毁续页状态；迟到的旧词成功与失败回包不能影响新词结果及其待收取页。
#[test]
fn editing_query_discards_pending_page_and_old_replies() -> color_eyre::Result<()> {
    let mut test = SearchTest::new()?;
    let limit = BATCH_SONGS;
    let second = 1;
    test.press(KeyCode::Char('G'));
    test.press(KeyCode::Char('/'));
    test.press(KeyCode::Char('x'));
    test.press(KeyCode::Enter);
    let mut first = results(0, true);
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
        batch: second,
    });
    test.press(KeyCode::Char('j'));
    test.assert_song_ids(limit)?;
    assert_eq!(test.requested_queries()?, vec!["q", "qx"]);
    let mut next = results(second, false);
    if let TaskEvent::SearchResults { query, .. } = &mut next {
        *query = "qx".to_owned();
    }
    test.app.state.apply(&next);
    test.assert_song_ids(2 * limit)?;
    Ok(())
}
