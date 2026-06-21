//! `o`(上下文操作)/ `y`(复制)两个 PopMenu 的构造:内容由「光标实体 × 所在视图」
//! 决定,确认产出 [`MenuAction`] 回 [`App::run_menu_action`](super::App) 执行。
//!
//! 锚点 = 选中行的屏幕矩形,由上一帧面积([`AppState::frame_area`])重算布局 +
//! 列表滚动态的只读 offset 还原;菜单贴行下方弹出(`Placement::Below`)。

use mineral_config::{CopyContext, CopyTemplate};
use mineral_model::{Album, Artist, Song};
use mineral_protocol::CopyTemplateCtx;
use ratatui::layout::Rect;

use crate::components::layout::shared::compute::{compute, compute_search};
use crate::components::popup::{MenuAction, MenuItem, OverlayKind, Placement, PopMenu};
use crate::runtime::scroll::list::{ScrollList, ScrollMotion};
use crate::runtime::scroll::viewport::pin_cursor;
use crate::runtime::state::{EntityRef, SearchFocus, View};

use super::App;

impl App {
    /// 打开上下文操作菜单(全屏态屏蔽;无可作用实体时静默)。
    pub(crate) fn open_action_menu(&mut self) {
        if self.state.browse.fullscreen.on() {
            return;
        }
        // Search 布局态的操作菜单(插播/下载等)随 §5 接入;此处不落浏览态实体,静默。
        if self.state.channel_search.active.on() {
            return;
        }
        let built = match self.state.browse.view.current() {
            View::Library => self.selected_track_song().map(|song| {
                let items = vec![
                    MenuItem::keyed(
                        'p',
                        "Play next",
                        MenuAction::PlayNext(Box::new(song.clone())),
                    ),
                    MenuItem::keyed(
                        'a',
                        "Append to queue",
                        MenuAction::Append(Box::new(song.clone())),
                    ),
                    MenuItem::keyed('d', "Download", MenuAction::Download(Box::new(song))),
                ];
                (items, self.library_row_anchor())
            }),
            // Playlists 菜单全是歌单写操作项,随管理流程接入。
            View::Playlists => None,
        };
        if let Some((items, anchor)) = built {
            self.overlays.push(OverlayKind::menu(PopMenu::new(
                "actions",
                items,
                anchor,
                Placement::Below,
            )));
        }
    }

    /// 打开复制菜单(全屏态屏蔽;无可作用实体时静默)。
    pub(crate) fn open_copy_menu(&mut self) {
        if self.state.browse.fullscreen.on() {
            return;
        }
        // Search 布局态走自己的实体 + 锚点(浏览态布局/选中与搜索无关,不能借用)。
        if self.state.channel_search.active.on() {
            self.open_search_copy_menu();
            return;
        }
        let templates = self.state.cfg.tui().copy().templates();
        let built = match self.state.browse.view.current() {
            View::Library => self.selected_track_song().map(|song| {
                let url = self
                    .state
                    .caps
                    .get(&song.source())
                    .and_then(|c| c.song_web_url().as_deref());
                let mut items = song_copy_items(&song, url);
                append_template_items(&mut items, templates, CopyContext::Song, || {
                    CopyTemplateCtx::Song(Box::new(song.clone()))
                });
                (items, self.library_row_anchor())
            }),
            View::Playlists => self.state.selected_playlist().map(|p| {
                let url = self
                    .state
                    .caps
                    .get(&p.data.source())
                    .and_then(|c| c.playlist_web_url().as_deref());
                let mut items = playlist_copy_items(&p.data, url);
                // 模板实参带上已加载曲目(songs 字段;未拉取过为空数组)。
                let mut playlist = p.data.clone();
                playlist.songs = self
                    .state
                    .library
                    .tracks
                    .get(&playlist.id)
                    .map(|views| {
                        views
                            .iter()
                            .map(|sv| sv.data.clone())
                            .collect::<Vec<Song>>()
                    })
                    .unwrap_or_default();
                append_template_items(&mut items, templates, CopyContext::Playlist, || {
                    CopyTemplateCtx::Playlist(Box::new(playlist.clone()))
                });
                (items, self.playlist_row_anchor())
            }),
        };
        if let Some((items, anchor)) = built {
            self.overlays.push(OverlayKind::menu(PopMenu::new(
                "copy",
                items,
                anchor,
                Placement::Below,
            )));
        }
    }

    /// Search 布局态的复制菜单:复制结果列选中实体(song/album/artist/playlist),菜单贴
    /// 结果行正下方弹出(强制 `Left`,不吃全局右对齐)。
    ///
    /// 仅结果列焦点开;detail 焦点(下钻帧实体复制)随 §5 操作菜单接入,此处静默。
    fn open_search_copy_menu(&mut self) {
        if self.state.channel_search.focus != SearchFocus::Results {
            return;
        }
        let templates = self.state.cfg.tui().copy().templates();
        let Some(entity) = self
            .state
            .channel_search
            .active_results()
            .and_then(|kr| EntityRef::from_payload(&kr.results, kr.sel()))
        else {
            return;
        };
        let caps = self.state.caps.get(&entity_source(&entity));
        let items = match &entity {
            EntityRef::Song(song) => {
                let url = caps.and_then(|c| c.song_web_url().as_deref());
                let mut items = song_copy_items(song, url);
                append_template_items(&mut items, templates, CopyContext::Song, || {
                    CopyTemplateCtx::Song(song.clone())
                });
                items
            }
            EntityRef::Playlist(playlist) => {
                let url = caps.and_then(|c| c.playlist_web_url().as_deref());
                let mut items = playlist_copy_items(playlist, url);
                append_template_items(&mut items, templates, CopyContext::Playlist, || {
                    CopyTemplateCtx::Playlist(playlist.clone())
                });
                items
            }
            EntityRef::Album(album) => album_copy_items(album),
            EntityRef::Artist(artist) => artist_copy_items(artist),
        };
        let Some(anchor) = self.search_row_anchor() else {
            return;
        };
        self.overlays.push(OverlayKind::menu(PopMenu::new(
            "copy",
            items,
            anchor,
            Placement::Below,
        )));
    }

    /// Search 结果列选中行的屏幕矩形。
    ///
    /// 走结果列的 [`ScrollList`] 只读 offset 还原(与左栏 row_anchor 同款),滚动到任意位置都对——
    /// 不再假设 offset 恒 0。
    fn search_row_anchor(&self) -> Option<Rect> {
        let panel = compute_search(self.state.frame_area.get(), self.state.cfg.tui().layout()).left;
        let kr = self.state.channel_search.active_results()?;
        Some(row_anchor(panel, kr.list(), kr.len()))
    }

    /// Library 视图选中歌(过滤投影后的当前行)。
    fn selected_track_song(&self) -> Option<Song> {
        self.state
            .filtered_tracks()
            .get(self.state.browse.nav.track.sel())
            .map(|sv| sv.data.clone())
    }

    /// Library 选中行的屏幕矩形。
    fn library_row_anchor(&self) -> Rect {
        row_anchor(
            self.left_panel(),
            &self.state.browse.nav.track,
            self.state.filtered_tracks().len(),
        )
    }

    /// Playlists 选中行的屏幕矩形。
    fn playlist_row_anchor(&self) -> Rect {
        row_anchor(
            self.left_panel(),
            &self.state.browse.nav.playlist,
            self.state.filtered_playlists().len(),
        )
    }

    /// 由上一帧面积重算浏览态布局,取左栏面板矩形。
    fn left_panel(&self) -> Rect {
        compute(self.state.frame_area.get(), self.state.cfg.tui().layout()).left
    }
}

/// 选中行在屏幕上的矩形:面板内容区(去边框)第 `表头 + (sel - offset)` 行。
/// offset 走滚动态的只读快照(`Frozen`,不推进动画);平移途中光标可能在视口外,
/// `pin_cursor` 与渲染端同款钳边,保证锚点恒落在面板内。
///
/// # Params:
///   - `panel`: 列表面板矩形(含边框)
///   - `list`: 该列表的光标 + 视口滚动态
///   - `len`: 列表总行数
fn row_anchor(panel: Rect, list: &ScrollList, len: usize) -> Rect {
    // 与渲染端同款视口数学:高 - 上下边框 - 表头。
    let viewport = usize::from(panel.height.saturating_sub(3));
    let offset = list.offset(len, viewport, ScrollMotion::Frozen);
    let pinned = pin_cursor(list.sel(), offset, viewport);
    let dy = u16::try_from(pinned.saturating_sub(offset)).unwrap_or(0);
    Rect::new(
        panel.x.saturating_add(1),
        // +1 边框 +1 表头。
        panel.y.saturating_add(2).saturating_add(dy),
        panel.width.saturating_sub(2),
        1,
    )
}

/// 按源声明的模板渲染网页分享链接(`{id}` 填裸 id;源无模板给 `None`,不渲染该项)。
fn render_web_url(template: Option<&str>, raw_id: &str) -> Option<String> {
    template.map(|t| t.replace("{id}", raw_id))
}

/// 把 config 的自定义模板项追加到内置项之后:按 `context` 过滤,与已有项
/// (内置或先前用户项)同字母时顶掉对方的快捷位——用户显式写了 key 就是想要
/// 这个键;被顶的项仍渲染,仅失去直达字母。
///
/// # Params:
///   - `items`: 已装好内置项的菜单
///   - `templates`: config `copy.templates` 全量
///   - `context`: 当前菜单的上下文(song / playlist)
///   - `make_ctx`: 构造该模板实参的闭包(每个模板项各持一份实体快照)
fn append_template_items(
    items: &mut Vec<MenuItem>,
    templates: &[CopyTemplate],
    context: CopyContext,
    make_ctx: impl Fn() -> CopyTemplateCtx,
) {
    for (index, t) in templates.iter().enumerate() {
        if *t.context() != context {
            continue;
        }
        if let Some(k) = t.key() {
            for it in items.iter_mut() {
                if it.hotkey == Some(*k) {
                    it.hotkey = None;
                }
            }
        }
        items.push(MenuItem {
            hotkey: *t.key(),
            label: t.label().clone(),
            action: Some(MenuAction::CopyTemplate {
                index,
                ctx: make_ctx(),
            }),
            destructive: false,
        });
    }
}

/// 歌单的复制菜单内置项。
///
/// # Params:
///   - `playlist`: 选中歌单
///   - `web_url`: 该源的歌单网页模板(caps 声明)
fn playlist_copy_items(playlist: &mineral_model::Playlist, web_url: Option<&str>) -> Vec<MenuItem> {
    let mut items = vec![
        MenuItem::keyed('n', "Copy name", MenuAction::Copy(playlist.name.clone())),
        MenuItem::keyed(
            'd',
            "Copy description",
            MenuAction::Copy(playlist.description.clone()),
        ),
        MenuItem::keyed('i', "Copy id", MenuAction::Copy(playlist.id.qualified())),
    ];
    if let Some(url) = render_web_url(web_url, playlist.id.value()) {
        items.push(MenuItem::keyed('u', "Copy URL", MenuAction::Copy(url)));
    }
    if let Some(cover) = &playlist.cover_url {
        items.push(MenuItem::keyed(
            'c',
            "Copy cover URL",
            MenuAction::Copy(cover.to_string()),
        ));
    }
    items
}

/// 歌曲的复制菜单内置项(后随 Lua 自定义模板,接入见 `copy.templates`)。
///
/// # Params:
///   - `song`: 选中歌
///   - `web_url`: 该源的歌曲网页模板(caps 声明)
fn song_copy_items(song: &Song, web_url: Option<&str>) -> Vec<MenuItem> {
    let mut items = vec![MenuItem::keyed(
        't',
        "Copy title",
        MenuAction::Copy(song.name.clone()),
    )];
    if !song.artists.is_empty() {
        let joined = song
            .artists
            .iter()
            .map(|a| a.name.clone())
            .collect::<Vec<String>>()
            .join(", ");
        items.push(MenuItem::keyed(
            'a',
            "Copy artist",
            MenuAction::Copy(joined),
        ));
    }
    if let Some(album) = &song.album {
        items.push(MenuItem::keyed(
            'b',
            "Copy album",
            MenuAction::Copy(album.name.clone()),
        ));
    }
    items.push(MenuItem::keyed(
        'i',
        "Copy id",
        MenuAction::Copy(song.id.qualified()),
    ));
    if let Some(url) = render_web_url(web_url, song.id.value()) {
        items.push(MenuItem::keyed('u', "Copy URL", MenuAction::Copy(url)));
    }
    if let Some(cover) = &song.cover_url {
        items.push(MenuItem::keyed(
            'c',
            "Copy cover URL",
            MenuAction::Copy(cover.to_string()),
        ));
    }
    items
}

/// 专辑的复制菜单内置项(name / id / 封面;caps 未声明专辑网页模板,故无「复制链接」项)。
fn album_copy_items(album: &Album) -> Vec<MenuItem> {
    let mut items = vec![
        MenuItem::keyed('n', "Copy name", MenuAction::Copy(album.name.clone())),
        MenuItem::keyed('i', "Copy id", MenuAction::Copy(album.id.qualified())),
    ];
    if let Some(cover) = &album.cover_url {
        items.push(MenuItem::keyed(
            'c',
            "Copy cover URL",
            MenuAction::Copy(cover.to_string()),
        ));
    }
    items
}

/// 歌手的复制菜单内置项(name / id / 头像;caps 未声明歌手网页模板,故无「复制链接」项)。
fn artist_copy_items(artist: &Artist) -> Vec<MenuItem> {
    let mut items = vec![
        MenuItem::keyed('n', "Copy name", MenuAction::Copy(artist.name.clone())),
        MenuItem::keyed('i', "Copy id", MenuAction::Copy(artist.id.qualified())),
    ];
    if let Some(avatar) = &artist.avatar_url {
        items.push(MenuItem::keyed(
            'c',
            "Copy avatar URL",
            MenuAction::Copy(avatar.to_string()),
        ));
    }
    items
}

/// 结果实体的来源(由各自 id 的 namespace 派生);供查 caps 取网页模板。
fn entity_source(entity: &EntityRef) -> mineral_model::SourceKind {
    match entity {
        EntityRef::Song(s) => s.id.namespace(),
        EntityRef::Album(a) => a.id.namespace(),
        EntityRef::Artist(a) => a.id.namespace(),
        EntityRef::Playlist(p) => p.id.namespace(),
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use mineral_channel_core::Page;
    use mineral_model::{Album, AlbumId, AlbumRef, Playlist, PlaylistId, SearchKind, SourceKind};
    use mineral_task::SearchPayload;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    use super::{
        album_copy_items, append_template_items, playlist_copy_items, row_anchor, song_copy_items,
    };
    use crate::app::App;
    use crate::components::layout::shared::compute::compute_search;
    use crate::components::popup::MenuAction;
    use crate::runtime::scroll::list::ScrollList;
    use crate::runtime::state::SearchFocus;
    use crate::test_support::{
        app_with_channel_search_probed, app_with_library, app_with_library_probed,
        app_with_playlists_probed, endserenading,
    };

    /// 经 serde 造一个 CopyTemplate(schema 字段私有,只能落型构造)。
    fn template(json: serde_json::Value) -> color_eyre::Result<mineral_config::CopyTemplate> {
        Ok(serde_json::from_value(json)?)
    }

    /// 喂一个 Press 键给 App(走真实事件入口 `handle_event`)。
    fn press(app: &mut App, code: KeyCode) {
        app.handle_event(&Event::Key(KeyEvent::new(code, KeyModifiers::empty())));
    }

    /// 先画一帧把 `frame_area` 回写(锚点计算依赖),再返回终端供后续快照。
    fn draw_once(app: &App) -> color_eyre::Result<Terminal<TestBackend>> {
        let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
        terminal.draw(|f| crate::view::draw(f, app))?;
        Ok(terminal)
    }

    /// `o` @ Library:菜单弹出,快捷字母 `p`/`a` 把选中歌转成对应队列操作。
    #[test]
    fn o_menu_hotkeys_run_queue_ops() -> color_eyre::Result<()> {
        let (mut app, queue_ops) = app_with_library_probed(/*len*/ 3, /*sel_track*/ 1)?;
        let want_id = app
            .state
            .filtered_tracks()
            .get(1)
            .map(|sv| sv.data.id.qualified())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 应有第 2 首"))?;
        draw_once(&app)?;
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(app.overlays.len(), 1, "o 应弹出操作菜单");
        press(&mut app, KeyCode::Char('p'));
        press(&mut app, KeyCode::Char('o'));
        press(&mut app, KeyCode::Char('a'));
        let ops = queue_ops
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("queue_ops 锁中毒: {e}"))?;
        assert_eq!(
            *ops,
            vec![
                ("insert_next", want_id.clone()),
                ("append", want_id.clone())
            ],
            "p=插播 a=追加,都作用于选中歌"
        );
        Ok(())
    }

    /// `o` 在 Playlists 视图(写操作未接入)与全屏态都不弹菜单。
    #[test]
    fn o_noop_on_playlists_and_fullscreen() -> color_eyre::Result<()> {
        let (mut app, _ops) = app_with_playlists_probed()?;
        draw_once(&app)?;
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(app.overlays.len(), 0, "Playlists 视图暂无操作菜单");

        let mut app = app_with_library(/*len*/ 3, /*sel_track*/ 0)?;
        app.state.browse.fullscreen.set(true);
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(app.overlays.len(), 0, "全屏态屏蔽操作菜单");
        Ok(())
    }

    /// `y` @ Library 弹复制菜单;`y` @ Playlists 也弹(Copy name)。
    #[test]
    fn y_opens_copy_menu_in_both_views() -> color_eyre::Result<()> {
        let mut app = app_with_library(/*len*/ 3, /*sel_track*/ 0)?;
        draw_once(&app)?;
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(app.overlays.len(), 1, "Library 歌曲上 y 应弹复制菜单");

        let (mut app, _ops) = app_with_playlists_probed()?;
        draw_once(&app)?;
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(app.overlays.len(), 1, "Playlists 歌单上 y 也应弹复制菜单");
        Ok(())
    }

    /// 歌曲复制项:title/id 恒有;artist 多人 `, ` join;album/URL/封面缺源数据不渲染。
    #[test]
    fn song_copy_items_compose() -> color_eyre::Result<()> {
        let mut song = mineral_test::song("s1");
        song.name = "Mineral".to_owned();
        song.artists = vec![
            mineral_model::ArtistRef {
                id: mineral_model::ArtistId::new(SourceKind::LOCAL, "a1"),
                name: "A".to_owned(),
            },
            mineral_model::ArtistRef {
                id: mineral_model::ArtistId::new(SourceKind::LOCAL, "a2"),
                name: "B".to_owned(),
            },
        ];
        song.album = Some(AlbumRef {
            id: AlbumId::new(SourceKind::LOCAL, "al"),
            name: "EndSerenading".to_owned(),
        });
        song.cover_url = Some(mineral_model::MediaUrl::remote(
            "https://img.example/c.jpg",
        )?);
        let items = song_copy_items(&song, Some("https://x.example/song?id={id}"));
        let got = items
            .iter()
            .filter_map(|it| it.action.clone().map(|a| (it.hotkey, a)))
            .collect::<Vec<(Option<char>, MenuAction)>>();
        assert_eq!(
            got,
            vec![
                (Some('t'), MenuAction::Copy("Mineral".to_owned())),
                (Some('a'), MenuAction::Copy("A, B".to_owned())),
                (Some('b'), MenuAction::Copy("EndSerenading".to_owned())),
                (Some('i'), MenuAction::Copy(song.id.qualified())),
                (
                    Some('u'),
                    MenuAction::Copy("https://x.example/song?id=s1".to_owned())
                ),
                (
                    Some('c'),
                    MenuAction::Copy("https://img.example/c.jpg".to_owned())
                ),
            ],
            "URL 模板 {{id}} 填裸 id;id 项给 qualified 形式"
        );

        song.album = None;
        song.artists = Vec::new();
        song.cover_url = None;
        let items = song_copy_items(&song, /*web_url*/ None);
        assert_eq!(items.len(), 2, "无艺人/专辑/模板/封面时剩 title + id");
        Ok(())
    }

    /// 歌单复制项:name/description/id 恒有(空描述也渲染);URL/封面缺源数据不渲染。
    #[test]
    fn playlist_copy_items_compose() {
        let playlist = Playlist::builder()
            .id(PlaylistId::new(SourceKind::LOCAL, "p1"))
            .name("歌单甲".to_owned())
            .build();
        let items = playlist_copy_items(&playlist, /*web_url*/ None);
        let got = items
            .iter()
            .filter_map(|it| it.action.clone().map(|a| (it.hotkey, a)))
            .collect::<Vec<(Option<char>, MenuAction)>>();
        assert_eq!(
            got,
            vec![
                (Some('n'), MenuAction::Copy("歌单甲".to_owned())),
                (Some('d'), MenuAction::Copy(String::new())),
                (Some('i'), MenuAction::Copy(playlist.id.qualified())),
            ],
            "空描述也渲染 description 项;无模板/封面不渲染 u/c"
        );
        let items = playlist_copy_items(&playlist, Some("https://x.example/pl/{id}"));
        assert_eq!(
            items.get(3).and_then(|it| it.action.clone()),
            Some(MenuAction::Copy("https://x.example/pl/p1".to_owned()))
        );
    }

    /// 专辑复制项:name/id 恒有;有封面则补 cover URL(caps 无专辑网页模板,故无 URL 项)。
    #[test]
    fn album_copy_items_compose() -> color_eyre::Result<()> {
        let album = Album::builder()
            .id(AlbumId::new(SourceKind::LOCAL, "al1"))
            .name("EndSerenading".to_owned())
            .cover_url(Some(mineral_model::MediaUrl::remote(
                "https://img.example/a.jpg",
            )?))
            .build();
        let got = album_copy_items(&album)
            .iter()
            .filter_map(|it| it.action.clone().map(|a| (it.hotkey, a)))
            .collect::<Vec<(Option<char>, MenuAction)>>();
        assert_eq!(
            got,
            vec![
                (Some('n'), MenuAction::Copy("EndSerenading".to_owned())),
                (Some('i'), MenuAction::Copy(album.id.qualified())),
                (
                    Some('c'),
                    MenuAction::Copy("https://img.example/a.jpg".to_owned())
                ),
            ]
        );
        Ok(())
    }

    /// Search 结果列按 `y`:复制菜单贴**结果行下方**弹出(锚到 search 布局的 results 面板,
    /// 不借浏览态布局/选中)。回归:结果区 y 曾错抓浏览态歌单、菜单飘到 detail 面板上。
    #[test]
    fn y_in_search_anchors_copy_menu_to_result_row() -> color_eyre::Result<()> {
        let (mut app, _submitted) = app_with_channel_search_probed(vec![SearchKind::Song])?;
        if let Some(session) = app.state.channel_search.current_mut() {
            session.set_kind(SearchKind::Song);
            session.apply_page(
                SearchKind::Song,
                SearchPayload::Songs(endserenading(5)),
                Page::default(),
            );
            if let Some(kr) = session.kind_results_mut() {
                kr.set_sel(2);
            }
        }
        app.state.channel_search.set_focus(SearchFocus::Results);
        // 画一帧回写 frame_area(锚点计算依赖)。
        draw_once(&app)?;

        let anchor = app
            .search_row_anchor()
            .ok_or_else(|| color_eyre::eyre::eyre!("有结果时应能算出锚点"))?;
        let results = compute_search(app.state.frame_area.get(), app.state.cfg.tui().layout()).left;
        assert_eq!(anchor.x, results.x + 1, "锚点在 results 面板内(去左边框)");
        assert_eq!(
            anchor.y,
            results.y + 2 + 2,
            "落在表头下第 3 行(offset 0、sel=2)"
        );

        press(&mut app, KeyCode::Char('y'));
        assert_eq!(app.overlays.len(), 1, "结果列 y 应弹复制菜单");
        Ok(())
    }

    /// 模板项追加:context 过滤、同字母顶掉内置快捷位、index 按全量数组序对位。
    #[test]
    fn template_items_append_and_override_hotkeys() -> color_eyre::Result<()> {
        use mineral_config::CopyContext;
        use mineral_protocol::CopyTemplateCtx;
        let song = mineral_test::song("s1");
        let templates = vec![
            // 与内置 Copy title 的 't' 同字母 → 顶掉内置的快捷位。
            template(serde_json::json!({"key": "t", "label": "Tpl A"}))?,
            // playlist 上下文,song 菜单里不出现;但 index 仍按全量数组序。
            template(serde_json::json!({"label": "Tpl P", "context": "playlist"}))?,
            // 无 key:仅导航可达。
            template(serde_json::json!({"label": "Tpl B"}))?,
        ];
        let mut items = song_copy_items(&song, /*web_url*/ None);
        append_template_items(&mut items, &templates, CopyContext::Song, || {
            CopyTemplateCtx::Song(Box::new(song.clone()))
        });
        let builtin_title = items
            .iter()
            .find(|it| it.label == "Copy title")
            .ok_or_else(|| color_eyre::eyre::eyre!("内置 title 项应仍在"))?;
        assert_eq!(builtin_title.hotkey, None, "同字母用户项顶掉内置快捷位");
        let got = items
            .iter()
            .filter(|it| it.label.starts_with("Tpl"))
            .filter_map(|it| {
                it.action
                    .as_ref()
                    .map(|a| (it.hotkey, it.label.as_str(), a))
            })
            .collect::<Vec<(Option<char>, &str, &MenuAction)>>();
        let (k0, l0, a0) = got
            .first()
            .copied()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有模板项"))?;
        assert_eq!((k0, l0), (Some('t'), "Tpl A"));
        assert!(
            matches!(a0, MenuAction::CopyTemplate { index: 0, .. }),
            "index 对位全量数组序"
        );
        let (k2, l2, a2) = got
            .get(1)
            .copied()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有第二个 song 模板项"))?;
        assert_eq!((k2, l2), (None, "Tpl B"));
        assert!(
            matches!(a2, MenuAction::CopyTemplate { index: 2, .. }),
            "playlist 模板被过滤但不挤占 index"
        );
        assert_eq!(got.len(), 2, "playlist 上下文的模板不进 song 菜单");
        Ok(())
    }

    /// CopyTemplate 确认:实体与下标发给 client 渲染;失败回 toast 不碰剪贴板。
    #[test]
    fn copy_template_action_calls_client() -> color_eyre::Result<()> {
        use mineral_protocol::CopyTemplateCtx;
        let (mut app, _ops) = app_with_library_probed(/*len*/ 1, /*sel_track*/ 0)?;
        let calls = {
            // TestClient 在 Arc<dyn Client> 后面,记录通道经构造前克隆持有——
            // probed helper 没暴露这支探针,这里直接重建一个带探针的 App。
            let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let client = crate::test_support::TestClient {
                copy_template_calls: std::sync::Arc::clone(&calls),
                ..crate::test_support::TestClient::default()
            };
            app.client = std::sync::Arc::new(client);
            calls
        };
        let song = mineral_test::song("s1");
        app.run_menu_action(MenuAction::CopyTemplate {
            index: 5,
            ctx: CopyTemplateCtx::Song(Box::new(song)),
        });
        let got = calls
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("calls 锁中毒: {e}"))?;
        assert_eq!(*got, vec![5], "下标原样到达 client");
        Ok(())
    }

    /// 行锚点数学:无滚动时第 `sel` 行落在面板内容区第 `sel` 行(边框+表头各 1)。
    #[test]
    fn row_anchor_maps_selection_to_panel_row() {
        let panel = Rect::new(10, 5, 40, 20);
        let mut list = ScrollList::new();
        list.set_sel(2);
        let got = row_anchor(panel, &list, /*len*/ 10);
        assert_eq!(got, Rect::new(11, 9, 38, 1), "y = 5 + 2(框+表头) + 2(行)");
    }

    /// 全帧快照:Library 上 `o` 菜单完全展开,贴选中行下方。
    #[test]
    fn o_menu_steady_frame_snapshot() -> color_eyre::Result<()> {
        let (mut app, _ops) = app_with_library_probed(/*len*/ 3, /*sel_track*/ 1)?;
        // 推满 Playlists → Library 的 sweep 过渡,否则左栏还画在起点视图。
        for _ in 0..40 {
            app.state.browse.view.tick();
        }
        let mut terminal = draw_once(&app)?;
        press(&mut app, KeyCode::Char('o'));
        // 推满弹出动画(popup_anim_ms / frame_tick_ms 拍,多 tick 几拍无害)。
        for _ in 0..40 {
            app.overlays.tick();
        }
        terminal.draw(|f| crate::view::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Library 视图 o 操作菜单稳态(贴选中行下方,p/a/d 三项)",
            terminal.backend()
        );
        Ok(())
    }
}
