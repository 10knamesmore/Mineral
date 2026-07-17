//! `o`(上下文操作)/ `y`(复制)两个 PopMenu 的构造:内容由「光标实体 × 所在视图」
//! 决定,确认产出 [`MenuAction`] 回 [`App::run_menu_action`](super::App) 执行。
//!
//! 锚点 = 选中行的屏幕矩形,由上一帧面积([`AppState::frame_area`])重算布局 +
//! 列表滚动态的只读 offset 还原;菜单贴行下方弹出(`Placement::Below`)。

use mineral_config::{CopyContext, CopyTemplate};
use mineral_model::{Album, Artist, Song};
use mineral_protocol::CopyTemplateCtx;
use mineral_task::SearchPayload;
use ratatui::layout::Rect;

use crate::components::layout::search::detail::detail_list_area;
use crate::components::layout::shared::compute::{compute, compute_search};
use crate::components::popup::{
    ContainerRef, MenuAction, MenuItem, OverlayKind, Placement, PopMenu,
};
use crate::runtime::scroll::list::{ScrollList, ScrollMotion};
use crate::runtime::scroll::viewport::pin_cursor;
use crate::runtime::state::{DetailFrame, EntityRef, SearchFocus, View};

use super::App;

/// 要开哪种行级菜单:复制(`y`)/ 操作(`o`)。两者共用 [`App::current_list_selection`]
/// 解析活跃 list,只在「实体 → 菜单项」的构造器上分流。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuKind {
    /// 复制菜单(把选中实体的字段塞剪贴板)。
    Copy,

    /// 操作菜单(队列动作 / 导航)。
    Action,
}

/// 当前活跃 list 面的种类:决定 `o` 操作项的导航语义(查看专辑只在有 detail 栈处、
/// 容器「进入」browse=激活歌单 ≠ search=下钻)。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SurfaceKind {
    /// 浏览态 Library 曲目列表。
    BrowseLibrary,

    /// 浏览态 Playlists 歌单列表。
    BrowsePlaylists,

    /// 搜索态结果列。
    SearchResults,

    /// 搜索态 detail 面板焦点的当前区列表（曲目 / 歌手某区）。
    SearchDetail,
}

/// 一次行级菜单的落点:活跃 list 选中行的实体、屏幕锚点矩形、所在面种类。
///
/// 这是「共同行为」的核心载体——每个 list 面在 [`App::current_list_selection`] 注册成一条
/// arm 产出它,`y`/`o` 都消费同一个,保证两键在每个面成对、一致地出现。
pub(crate) struct ListSelection {
    /// 选中行的实体(四类型统一,行级菜单据此构造项)。
    pub entity: EntityRef,

    /// 菜单贴出的屏幕矩形(选中行正下方弹)。
    pub anchor: Rect,

    /// 所在 list 面(操作项的导航语义按它分流)。
    pub surface: SurfaceKind,
}

impl App {
    /// 解析当前活跃 list 面的选中行 → 实体 + 锚点 + 面种类。全屏态 / 无活跃 list / 无选中
    /// 实体 → `None`。**共同行为的单一入口**:每个 list 面在此注册一条 arm,`y`/`o` 共用。
    ///
    /// queue 浮层不在此(它走 overlay 路由,见 [`App::open_queue_copy_menu`]),是唯一接缝。
    pub(crate) fn current_list_selection(&self) -> Option<ListSelection> {
        if self.state.browse.fullscreen.on() {
            return None;
        }
        if self.state.channel_search.active.on() {
            let kr = self.state.channel_search.active_results()?;
            return match self.state.channel_search.focus {
                SearchFocus::Results => Some(ListSelection {
                    entity: EntityRef::from_payload(&kr.results, kr.sel())?,
                    anchor: self.search_row_anchor()?,
                    surface: SurfaceKind::SearchResults,
                }),
                // detail 焦点:当前区选中行的实体（曲目 / 歌手某区），锚到 detail 面板内
                // 该列表区的选中行下方。
                SearchFocus::Detail => Some(ListSelection {
                    entity: kr.detail.current()?.row_entity()?,
                    anchor: self.search_detail_row_anchor()?,
                    surface: SurfaceKind::SearchDetail,
                }),
                // prompt 是模态文本输入,无行级实体。
                SearchFocus::Prompt => None,
            };
        }
        match self.state.browse.view.current() {
            View::Library => Some(ListSelection {
                entity: EntityRef::Song(Box::new(self.selected_track_song()?)),
                anchor: self.library_row_anchor(),
                surface: SurfaceKind::BrowseLibrary,
            }),
            View::Playlists => Some(ListSelection {
                entity: EntityRef::Playlist(Box::new(self.state.selected_playlist()?.data.clone())),
                anchor: self.playlist_row_anchor(),
                surface: SurfaceKind::BrowsePlaylists,
            }),
        }
    }

    /// 打开行级菜单(`y` 复制 / `o` 操作):解析活跃 list 选中行 → 按 `kind` 构造项 → 贴行下方
    /// 弹出。全屏态 / 无选中 / 空项静默。`y`/`o` 唯一消费者。
    pub(crate) fn open_menu(&mut self, kind: MenuKind) {
        let Some(sel) = self.current_list_selection() else {
            return;
        };
        let items = match kind {
            MenuKind::Copy => self.copy_items(&sel.entity),
            MenuKind::Action => self.action_items(&sel.entity, sel.surface),
        };
        if items.is_empty() {
            return;
        }
        let label = match kind {
            MenuKind::Copy => "copy",
            MenuKind::Action => "actions",
        };
        self.overlays.push(OverlayKind::menu(PopMenu::new(
            label,
            items,
            sel.anchor,
            Placement::Below,
        )));
    }

    /// queue 浮层 `y` 的落地:为队列第 `idx` 项构造复制菜单,贴 `anchor`(队列行下方)弹在
    /// queue 浮层**之上**(不关 queue)。空队列下标 / 空项静默。复用 [`Self::copy_items`]——
    /// 与全站复制同一套;queue 是 [`Self::current_list_selection`] resolver 之外的唯一接缝
    /// (浮层持私有光标,锚点只能由它自算后随动作带回)。
    pub(crate) fn open_queue_copy_menu(&mut self, idx: usize, anchor: Rect) {
        let Some(song) = self.state.player.queue.get(idx).cloned() else {
            return;
        };
        let items = self.copy_items(&EntityRef::Song(Box::new(song)));
        if items.is_empty() {
            return;
        }
        self.overlays.push(OverlayKind::menu(PopMenu::new(
            "copy",
            items,
            anchor,
            Placement::Below,
        )));
    }

    /// 选中实体的 `o` 操作项(按实体类型 + 面种类)。歌曲给队列动作(`p` 替换队列起播取所在
    /// 列表整列作上下文);容器(专辑/歌单/歌手)给播放全部 / 加入队列(见 [`container_action_items`])。
    fn action_items(&self, entity: &EntityRef, surface: SurfaceKind) -> Vec<MenuItem> {
        match entity {
            EntityRef::Song(song) => vec![
                MenuItem::keyed(
                    'p',
                    "Play",
                    MenuAction::Play {
                        song: song.clone(),
                        queue: self.surface_song_queue(surface),
                        context: self.surface_play_context(surface),
                    },
                ),
                MenuItem::keyed('n', "Play next", MenuAction::PlayNext(song.clone())),
                MenuItem::keyed('a', "Append to queue", MenuAction::Append(song.clone())),
                MenuItem::keyed('d', "Download", MenuAction::Download(song.clone())),
            ],
            EntityRef::Album(album) => container_action_items(ContainerRef::Album(album.clone())),
            EntityRef::Playlist(playlist) => {
                container_action_items(ContainerRef::Playlist(playlist.clone()))
            }
            EntityRef::Artist(artist) => {
                container_action_items(ContainerRef::Artist(artist.clone()))
            }
        }
    }

    /// 某 list 面的「整列歌曲」(`Play` 的队列上下文,语义同该面 activate 起播):
    /// Library 取当前全列曲目(非过滤投影,与 Enter 一致)、search 结果列取整列结果(仅歌曲
    /// kind),其余面无歌曲列表给空(落地时退化为单曲队列)。
    fn surface_song_queue(&self, surface: SurfaceKind) -> Vec<Song> {
        match surface {
            SurfaceKind::BrowseLibrary => self
                .state
                .current_tracks_slot()
                .map(|v| v.iter().map(|sv| sv.data.clone()).collect::<Vec<Song>>())
                .unwrap_or_default(),
            SurfaceKind::SearchResults => self
                .state
                .channel_search
                .active_results()
                .and_then(|kr| match &kr.results {
                    SearchPayload::Songs(v) => Some(v.clone()),
                    SearchPayload::Albums(_)
                    | SearchPayload::Playlists(_)
                    | SearchPayload::Artists(_) => None,
                })
                .unwrap_or_default(),
            // detail 面板当前区的整列歌曲（专辑/歌单曲目、歌手热门曲；Albums 区行是容器，
            // 走容器动作不取此）。
            SurfaceKind::SearchDetail => self
                .state
                .channel_search
                .active_results()
                .and_then(|kr| kr.detail.current())
                .map(DetailFrame::song_list)
                .unwrap_or_default(),
            SurfaceKind::BrowsePlaylists => Vec::new(),
        }
    }

    /// 某 list 面「整列起播」的队列语境(埋点 provenance,与 [`Self::surface_song_queue`] 取的
    /// 队列同源):Library 归当前歌单、search 结果列归搜索词、detail 面板归当前容器身份;
    /// Playlists 面选中的是容器本身(不在此起单曲队列)→ `Unknown`。
    fn surface_play_context(&self, surface: SurfaceKind) -> mineral_protocol::QueueContextWire {
        match surface {
            SurfaceKind::BrowseLibrary => self.state.selected_playlist().map_or(
                mineral_protocol::QueueContextWire::Unknown,
                |p| mineral_protocol::QueueContextWire::Playlist {
                    id: p.data.id.clone(),
                },
            ),
            SurfaceKind::SearchResults => self.state.channel_search.search_context(),
            SurfaceKind::SearchDetail => self.state.channel_search.detail_context(),
            SurfaceKind::BrowsePlaylists => mineral_protocol::QueueContextWire::Unknown,
        }
    }

    /// 选中实体的 `y` 复制项(按实体类型),后随 Lua 自定义模板项。歌曲 / 歌单带网页链接 +
    /// 模板;专辑 / 歌手只内置项。全站(browse / search results / search detail / queue)共用。
    fn copy_items(&self, entity: &EntityRef) -> Vec<MenuItem> {
        let templates = self.state.cfg.tui().copy().templates();
        let caps = self.state.caps.get(&entity_source(entity));
        match entity {
            EntityRef::Song(song) => {
                let url = caps.and_then(|c| c.song_web_url().as_deref());
                let mut items = song_copy_items(song, url);
                if let Some(item) = self.stream_copy_item(&song.id) {
                    items.push(item);
                }
                append_template_items(&mut items, templates, CopyContext::Song, || {
                    CopyTemplateCtx::Song(song.clone())
                });
                items
            }
            EntityRef::Playlist(playlist) => {
                let url = caps.and_then(|c| c.playlist_web_url().as_deref());
                let mut items = playlist_copy_items(playlist, url);
                // 模板实参带上已加载曲目:实体自带 songs 优先,空则退查 library 缓存。
                let mut pl = (**playlist).clone();
                if pl.songs.is_empty() {
                    pl.songs = self
                        .state
                        .library
                        .tracks
                        .get(&pl.id)
                        .map(|views| {
                            views
                                .iter()
                                .map(|sv| sv.data.clone())
                                .collect::<Vec<Song>>()
                        })
                        .unwrap_or_default();
                }
                append_template_items(&mut items, templates, CopyContext::Playlist, || {
                    CopyTemplateCtx::Playlist(Box::new(pl.clone()))
                });
                items
            }
            EntityRef::Album(album) => album_copy_items(album),
            EntityRef::Artist(artist) => artist_copy_items(artist),
        }
    }

    /// 选中歌**恰是当前在播歌**时,给出其音频流地址的复制项:带取流头的源(如 B 站需
    /// Referer / UA,裸链接贴出去 403)拼成 curl 片段,无头源给裸 URL / 本地路径。
    /// 非在播歌 client 手里没有已解析的 PlayUrl,不出该项——按需解析得走 server 往返,
    /// 不值得为一个复制项发请求。
    fn stream_copy_item(&self, song_id: &mineral_model::SongId) -> Option<MenuItem> {
        let play_url = self.state.playback.play_url.as_ref()?;
        if &play_url.song_id != song_id {
            return None;
        }
        Some(if play_url.stream_headers.is_empty() {
            MenuItem::keyed(
                's',
                "Copy stream URL",
                MenuAction::Copy(play_url.url.to_string()),
            )
        } else {
            MenuItem::keyed(
                's',
                "Copy stream (curl)",
                MenuAction::Copy(stream_curl(play_url)),
            )
        })
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

    /// Search detail 面板当前区选中行的屏幕矩形（detail 焦点 `y`/`o` 贴行下方弹）。
    ///
    /// 走 detail 面板专属几何：右面板去外框 → [`detail_list_area`] 取列表区（该区无自己的
    /// block 边框，[`borderless_row_anchor`] 据此还原行 y）。`.right` 为 `Option`（搜索布局恒
    /// `Some`）、空栈无栈顶帧 → `None`。
    fn search_detail_row_anchor(&self) -> Option<Rect> {
        let kr = self.state.channel_search.active_results()?;
        let dframe = kr.detail.current()?;
        let panel =
            compute_search(self.state.frame_area.get(), self.state.cfg.tui().layout()).right?;
        let is_artist = matches!(dframe.entity, EntityRef::Artist(_));
        let list_area = detail_list_area(panel_inner(panel), is_artist);
        Some(borderless_row_anchor(
            list_area,
            dframe.list(),
            dframe.list_len(),
        ))
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

/// detail 面板内（列表区**无**自己的 block 边框）选中行的屏幕矩形：视口 = 区高 − 表头一行，
/// 行 y = 区顶 + 表头 + (钳后光标 − offset)。区别于带边框面板的 [`row_anchor`]（那里另算上下
/// 边框行）；offset 走只读 `Frozen` 快照，平移途中 `pin_cursor` 钳边与渲染端一致。
///
/// # Params:
///   - `area`: 列表区矩形（已去面板外框 + 歌手 Tab 行，见 [`detail_list_area`]）
///   - `list`: 该列表的光标 + 视口滚动态
///   - `len`: 当前区列表总行数
fn borderless_row_anchor(area: Rect, list: &ScrollList, len: usize) -> Rect {
    let viewport = usize::from(area.height.saturating_sub(1));
    let offset = list.offset(len, viewport, ScrollMotion::Frozen);
    let pinned = pin_cursor(list.sel(), offset, viewport);
    let dy = u16::try_from(pinned.saturating_sub(offset)).unwrap_or(0);
    Rect::new(
        area.x,
        // +1 表头（无边框，故不加边框行）。
        area.y.saturating_add(1).saturating_add(dy),
        area.width,
        1,
    )
}

/// 去四周 1 格边框后的内区（detail 面板 Borders::ALL）。
fn panel_inner(r: Rect) -> Rect {
    Rect::new(
        r.x.saturating_add(1),
        r.y.saturating_add(1),
        r.width.saturating_sub(2),
        r.height.saturating_sub(2),
    )
}

/// 按源声明的模板渲染网页分享链接(占位语义——`{id}` 整段 / `{0}` 按 `:` 分段——见
/// [`mineral_channel_core::render_web_url`];源无模板给 `None`,不渲染该项)。
fn render_web_url(template: Option<&str>, raw_id: &str) -> Option<String> {
    template.map(|t| mineral_channel_core::render_web_url(t, raw_id))
}

/// POSIX shell 单引号包裹:内嵌 `'` 换成 `'\''`。复制出的 curl 片段要能原样粘进 shell——
/// header / url 可经 Lua before_stream hook 自定义,不能假设无引号。
fn shell_squote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// 带取流头的流地址拼成可直接执行的 curl 片段。B 站等源的音频直链缺 Referer / UA 会 403,
/// 裸 URL 贴到浏览器 / curl 里没用,复制就得连头一起给。
fn stream_curl(play_url: &mineral_model::PlayUrl) -> String {
    let headers = play_url
        .stream_headers
        .iter()
        .map(|(k, v)| format!("-H {} ", shell_squote(&format!("{k}: {v}"))))
        .collect::<String>();
    format!("curl {headers}{}", shell_squote(&play_url.url.to_string()))
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
            tint: None,
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

/// 容器(专辑/歌单/歌手)的 `o` 操作项:`p` 播放全部、`a` 加入队列(歌手取热门曲那路)。
/// 两项各持容器副本,落地经 [`App::start_container_play`] 拉取→入队。
fn container_action_items(container: ContainerRef) -> Vec<MenuItem> {
    let (play_label, append_label) = match container {
        ContainerRef::Artist(_) => ("Play top songs", "Append top songs"),
        ContainerRef::Album(_) | ContainerRef::Playlist(_) => ("Play all", "Append all to queue"),
    };
    vec![
        MenuItem::keyed(
            'p',
            play_label,
            MenuAction::PlayContainer(Box::new(container.clone())),
        ),
        MenuItem::keyed(
            'a',
            append_label,
            MenuAction::AppendContainer(Box::new(container)),
        ),
    ]
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
        press(&mut app, KeyCode::Char('n'));
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
            "n=插播 a=追加,都作用于选中歌"
        );
        Ok(())
    }

    /// `o` → `p` = Play:整列(当前全列曲目)替换队列 + 起播选中曲,记 set_queue + play_song
    /// 两步(canonical:p=Play 主操作,顶掉原 p=插播)。
    #[test]
    fn o_menu_play_replaces_queue_and_plays() -> color_eyre::Result<()> {
        let (mut app, queue_ops) = app_with_library_probed(/*len*/ 3, /*sel_track*/ 1)?;
        let want_id = app
            .state
            .filtered_tracks()
            .get(1)
            .map(|sv| sv.data.id.qualified())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 应有第 2 首"))?;
        draw_once(&app)?;
        press(&mut app, KeyCode::Char('o'));
        press(&mut app, KeyCode::Char('p'));
        let ops = queue_ops
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("queue_ops 锁中毒: {e}"))?;
        assert_eq!(
            *ops,
            vec![
                ("set_queue", format!("3:{want_id}")),
                ("play_song", want_id.clone()),
            ],
            "p=Play:整列(3)替换队列 + 起播选中曲,两步齐"
        );
        Ok(())
    }

    /// F1 回归:Library 上 o→Play 记当前歌单语境(此前菜单单曲 Play 硬编码 Unknown)。
    #[test]
    fn o_menu_play_carries_playlist_context() -> color_eyre::Result<()> {
        let (mut app, _ops) = app_with_library_probed(/*len*/ 3, /*sel_track*/ 1)?;
        let contexts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        app.client = std::sync::Arc::new(crate::test_support::TestClient {
            queue_contexts: std::sync::Arc::clone(&contexts),
            ..crate::test_support::TestClient::default()
        });
        draw_once(&app)?;
        press(&mut app, KeyCode::Char('o'));
        press(&mut app, KeyCode::Char('p'));
        let got = contexts
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("queue_contexts 锁中毒: {e}"))?;
        assert_eq!(
            *got,
            vec![(
                "set_queue",
                mineral_protocol::QueueContextWire::Playlist {
                    id: PlaylistId::new(SourceKind::NETEASE, "p1")
                }
            )],
            "库内 o→Play 记当前歌单语境"
        );
        Ok(())
    }

    /// `o` 在 Playlists 视图弹容器菜单(Play all / Add to queue);全屏态仍屏蔽。
    #[test]
    fn o_on_playlists_opens_container_menu_fullscreen_silent() -> color_eyre::Result<()> {
        let (mut app, _ops) = app_with_playlists_probed()?;
        draw_once(&app)?;
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(app.overlays.len(), 1, "Playlists 歌单 o 弹容器操作菜单");

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

    /// 复合裸 id(B 站 `bvid:page`)经位置占位模板渲染出正确视频页 URL。
    #[test]
    fn song_copy_url_renders_positional_segments() -> color_eyre::Result<()> {
        let mut song = mineral_test::song("ignored");
        song.id = mineral_model::SongId::new(SourceKind::BILIBILI, "BV1rvkMYgEtT:2");
        let items = song_copy_items(&song, Some("https://www.bilibili.com/video/{0}?p={1}"));
        let url = items
            .iter()
            .find(|it| it.label == "Copy URL")
            .and_then(|it| it.action.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("应有 Copy URL 项"))?;
        assert_eq!(
            url,
            MenuAction::Copy("https://www.bilibili.com/video/BV1rvkMYgEtT?p=2".to_owned()),
            "bvid:page 按 {{0}}/{{1}} 拆段填入"
        );
        Ok(())
    }

    /// 流地址复制项只在「选中歌 == 当前在播歌」时出现:带取流头拼 curl 片段,其余选中歌无此项。
    #[test]
    fn stream_copy_item_gated_to_playing_song() -> color_eyre::Result<()> {
        let mut app = app_with_library(/*len*/ 3, /*sel_track*/ 0)?;
        let playing = app
            .state
            .filtered_tracks()
            .first()
            .map(|sv| sv.data.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 应有首曲"))?;
        app.state.playback.play_url = Some(mineral_model::PlayUrl {
            song_id: playing.id.clone(),
            url: mineral_model::MediaUrl::remote("https://cdn.example/a.m4s")?,
            bitrate_bps: None,
            quality: mineral_model::BitRate::Standard,
            size: None,
            format: Some(mineral_model::AudioFormat::Aac),
            bit_depth: None,
            stream_headers: vec![("Referer".to_owned(), "https://www.bilibili.com".to_owned())],
            layout: mineral_model::StreamLayout::Contiguous,
            substituted: false,
        });
        let items = app.copy_items(&crate::runtime::state::EntityRef::Song(Box::new(playing)));
        let stream = items
            .iter()
            .find(|it| it.label == "Copy stream (curl)")
            .and_then(|it| it.action.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("在播歌应有流地址项"))?;
        assert_eq!(
            stream,
            MenuAction::Copy(
                "curl -H 'Referer: https://www.bilibili.com' 'https://cdn.example/a.m4s'"
                    .to_owned()
            ),
            "取流头进 curl 片段,裸 URL 单独可用不了"
        );

        // 另一首(非在播)不出流地址项。
        let other = app
            .state
            .filtered_tracks()
            .get(1)
            .map(|sv| sv.data.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 应有第 2 首"))?;
        let items = app.copy_items(&crate::runtime::state::EntityRef::Song(Box::new(other)));
        assert!(
            !items.iter().any(|it| it.label.starts_with("Copy stream")),
            "非在播歌无已解析 PlayUrl,不出流地址项"
        );
        Ok(())
    }

    /// curl 片段的 shell 单引号转义:值内嵌 `'` 时粘进 shell 不截断。
    #[test]
    fn stream_curl_escapes_embedded_single_quote() {
        assert_eq!(
            super::shell_squote("it's"),
            r"'it'\''s'",
            "内嵌单引号按 POSIX 惯例断开重接"
        );
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
                /*has_more*/ None,
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

    /// 内聚 resolver 落地:search 结果列 Song(队列动作)与 Album 容器(播放全部/加入队列)
    /// 上 `o` 都弹操作菜单(此前 active.on() 早退静默)。
    #[test]
    fn o_in_search_results_opens_action_menu_for_song_and_container() -> color_eyre::Result<()> {
        // Song 结果:o 弹菜单。
        let (mut app, _submitted) = app_with_channel_search_probed(vec![SearchKind::Song])?;
        if let Some(session) = app.state.channel_search.current_mut() {
            session.set_kind(SearchKind::Song);
            session.apply_page(
                SearchKind::Song,
                SearchPayload::Songs(endserenading(5)),
                Page::default(),
                /*has_more*/ None,
            );
        }
        app.state.channel_search.set_focus(SearchFocus::Results);
        draw_once(&app)?;
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(app.overlays.len(), 1, "结果列 Song 的 o 应弹操作菜单");

        // Album 容器结果:o 弹容器菜单(Play all / Append all)。
        let (mut app, _submitted) = app_with_channel_search_probed(vec![SearchKind::Album])?;
        if let Some(session) = app.state.channel_search.current_mut() {
            session.set_kind(SearchKind::Album);
            session.apply_page(
                SearchKind::Album,
                SearchPayload::Albums(vec![
                    mineral_model::Album::builder()
                        .id(AlbumId::new(SourceKind::NETEASE, "al1"))
                        .name("EndSerenading".to_owned())
                        .build(),
                ]),
                Page::default(),
                /*has_more*/ None,
            );
        }
        app.state.channel_search.set_focus(SearchFocus::Results);
        draw_once(&app)?;
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(app.overlays.len(), 1, "Album 容器 o 应弹容器操作菜单");
        Ok(())
    }

    /// 造一张带 `n` 首曲目的专辑（曲目取 EndSerenading 前 n 首）。
    fn album_with_tracks(id: &AlbumId, n: usize) -> Album {
        Album::builder()
            .id(id.clone())
            .name("EndSerenading".to_owned())
            .songs(endserenading(n))
            .build()
    }

    /// 构造：album 结果 + 专辑详情（3 曲）到货 + detail 焦点，并画一帧回写 `frame_area`。
    /// detail 列表此时是专辑曲目，光标在首行。
    fn app_in_album_detail() -> color_eyre::Result<App> {
        let (mut app, _submitted) = app_with_channel_search_probed(vec![SearchKind::Album])?;
        let al_id = AlbumId::new(SourceKind::NETEASE, "al1");
        if let Some(session) = app.state.channel_search.current_mut() {
            session.set_kind(SearchKind::Album);
            session.apply_page(
                SearchKind::Album,
                SearchPayload::Albums(vec![
                    Album::builder()
                        .id(al_id.clone())
                        .name("EndSerenading".to_owned())
                        .build(),
                ]),
                Page::default(),
                /*has_more*/ None,
            );
            if let Some(kr) = session.kind_results_mut() {
                kr.fill_album_detail(&al_id, Box::new(album_with_tracks(&al_id, 3)));
            }
        }
        app.state.channel_search.set_focus(SearchFocus::Detail);
        draw_once(&app)?;
        Ok(app)
    }

    /// detail 焦点:专辑详情曲目行上 `o` 弹操作菜单(Song 队列动作)。回归:detail 焦点
    /// 此前 `active.on()` 早退静默。
    #[test]
    fn o_in_search_detail_opens_action_menu_for_track() -> color_eyre::Result<()> {
        let mut app = app_in_album_detail()?;
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(app.overlays.len(), 1, "detail 曲目行 o 弹操作菜单");
        Ok(())
    }

    /// detail 焦点:曲目行 `y` 弹复制菜单,且锚点落在右(detail)面板内、表头之下。
    #[test]
    fn y_in_search_detail_anchors_copy_menu_in_detail_panel() -> color_eyre::Result<()> {
        let app = app_in_album_detail()?;
        let anchor = app
            .search_detail_row_anchor()
            .ok_or_else(|| color_eyre::eyre::eyre!("有详情应能算出锚点"))?;
        let right = compute_search(app.state.frame_area.get(), app.state.cfg.tui().layout())
            .right
            .ok_or_else(|| color_eyre::eyre::eyre!("应有 detail 面板"))?;
        assert!(
            anchor.x > right.x && anchor.x < right.x + right.width,
            "锚点 x 落在 detail 面板内(去左边框)"
        );
        assert!(
            anchor.y > right.y + 1,
            "锚点 y 在面板内、头部之下(detail 头图区占顶部)"
        );

        let mut app = app;
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(app.overlays.len(), 1, "detail 曲目行 y 弹复制菜单");
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
