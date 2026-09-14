//! 「转 Client」的领域动作执行器:播放控制 / love / 下载 / 脚本动作。
//!
//! 实现 `App` 的播放控制、love、下载与脚本动作；入口由 `App::dispatch` 调用。

use mineral_model::Song;
use mineral_protocol::DownloadTarget;
use mineral_task::TaskEvent;

use crate::app::App;
use crate::components::popup::{ContainerRef, MenuAction};
use crate::components::toast::notifications::{TextTint, tinted_text_item};
use crate::runtime::action::ScriptSlot;
use crate::runtime::state::{ActiveLayer, DetailFetch, EntityRef, PageKind, View};

/// 容器入队模式:替换队列起播 / 追加到队尾 / 按序插播(由 `PlayContainer` /
/// `AppendContainer` / `PlayNextContainer` 决定)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlayMode {
    /// 替换队列并起播首曲。
    Replace,

    /// 追加到队尾。
    Append,

    /// 按原顺序插到当前曲之后；daemon 队列为空时从头插入，不自动起播。
    InsertNext,
}

/// 复制成功 toast 里展示的内容上限(字符);超出截断加省略号,防长模板把顶栏挤爆。
const COPY_TOAST_MAX_CHARS: usize = 48;

/// 等待补齐后执行的歌单起播；位置使用原始 relation index，过滤词在发起时固定。
pub(crate) struct PendingPlaylistPlay {
    /// 发起起播的歌单。
    playlist: mineral_model::PlaylistId,

    /// 发起操作时的条目；补齐后同时核对原始位置和歌曲身份。
    target: mineral_model::PlaylistEntry,

    /// 仅播放匹配项时使用的查询词；None 表示整张歌单。
    query: Option<String>,

    /// 发起操作时的展示名称。
    name: Option<String>,
}

impl PendingPlaylistPlay {
    /// 在完整结果中按原位置定位，并复用本地搜索的匹配和稳定排序语义。
    fn resolve(&self, entries: &[mineral_model::PlaylistEntry]) -> Option<(Vec<Song>, usize)> {
        let mut selected = entries.iter().collect::<Vec<_>>();
        if let Some(query) = &self.query {
            let mut search = crate::runtime::state::SearchState::new();
            for character in query.chars() {
                search.edit(crate::runtime::line_input::InputRequest::Insert(character));
            }
            let mut scored = selected
                .into_iter()
                .filter_map(|entry| search.song_score(&entry.song).map(|score| (score, entry)))
                .collect::<Vec<_>>();
            scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
            selected = scored.into_iter().map(|(_, entry)| entry).collect();
        }
        let target = selected.iter().position(|entry| {
            entry.index == self.target.index && entry.song.id == self.target.song.id
        })?;
        Some((
            selected
                .into_iter()
                .map(|entry| entry.song.clone())
                .collect(),
            target,
        ))
    }
}

impl App {
    /// 触发 `tui.keys.script` 绑定的脚本动作:槽位 → 注册名 → daemon;
    /// daemon 报错(未注册 / 脚本未启用 / 执行失败)时 toast 提示。
    pub(crate) fn invoke_script_action(&mut self, slot: ScriptSlot) {
        // owned 拷贝:松开对 keymap 的借用,下面才能可变借用 notifications。
        let Some(name) = self.keymap.script_action(slot).map(str::to_owned) else {
            return;
        };
        let ctx = self.collect_key_context();
        // 结论异步回流(完成事件带动作名),失败时提示;按键立即继续。
        self.client.invoke_action(&name, Some(ctx));
    }

    /// 采集按键瞬间的上下文快照(脚本动作的 `ctx` 实参)。
    ///
    /// view 判定与 `handle_key` 路由共用 `active_layer`(队列浮层光标特例除外),优先级
    /// 队列浮层 > 搜索 > 全屏 >
    /// 主视图映射(`Playlists` → Playlists,`Library` → Tracks)。选中歌只在
    /// 「有歌列表光标」的视图采(Library 列表 / 队列浮层光标),其余为 `None`;
    /// `selected_loved` 随选中歌给(♥ 装饰缓存),`search_query` 空词为 `None`。
    pub(crate) fn collect_key_context(&self) -> mineral_protocol::KeyContext {
        use mineral_protocol::{KeyContext, PlaylistRef, ViewKind};
        let now_playing = self.state.player.current.clone().map(Box::new);
        let selected_playlist = self.state.selected_playlist().map(|p| PlaylistRef {
            id: p.data.id.clone(),
            name: p.data.name.clone(),
        });
        let search_query = if self.state.browse.search.query().is_empty() {
            None
        } else {
            Some(self.state.browse.search.query().to_owned())
        };
        // 选中歌 + 其 ♥ 态:队列浮层取光标条目(♥ 查 liked_ids 缓存),
        // Library 列表取选中行(PlaylistEntryView 已装饰)。
        let (view, selected_song, selected_loved) =
            if let Some(cursor) = self.overlays.active_queue_cursor(&self.state) {
                // 队列浮层:唯一带脚本选中的浮层(取光标条目)。
                let song = self.state.player.queue.get(cursor).cloned();
                let loved = song.as_ref().map(|s| {
                    self.state
                        .library
                        .liked_ids
                        .get(&s.id.namespace())
                        .is_some_and(|ids| ids.contains(&s.id))
                });
                (ViewKind::Queue, song, loved)
            } else {
                // 其余浮层对脚本 ctx 透明,看穿到下层布局层(与 handle_key 路由共用 active_layer)。
                match self.state.active_layer() {
                    ActiveLayer::SearchSession | ActiveLayer::DeepSearch => {
                        (ViewKind::Search, None, None)
                    }
                    ActiveLayer::Fullscreen => (ViewKind::Fullscreen, None, None),
                    ActiveLayer::Browse => match self.state.browse.view.current() {
                        View::Playlists => (ViewKind::Playlists, None, None),
                        View::Library => {
                            let sel = self
                                .state
                                .filtered_tracks()
                                .get(self.state.browse.nav.track.sel());
                            let loved = sel.as_ref().map(|entry| entry.loved);
                            (
                                ViewKind::Tracks,
                                sel.map(|entry| entry.data.song.clone()),
                                loved,
                            )
                        }
                    },
                }
            };
        KeyContext::builder()
            .view(view)
            .selected_song(selected_song.map(Box::new))
            .selected_playlist(selected_playlist)
            .now_playing(now_playing)
            .selected_loved(selected_loved)
            .search_query(search_query)
            .build()
    }

    /// 空格键:有当前曲目时在 pause/resume 间切换;没歌时无动作。
    pub(crate) fn toggle_play_pause(&mut self) {
        if self.state.playback.track.is_none() {
            return;
        }
        if self.state.playback.playing {
            self.client.pause();
        } else {
            self.client.resume();
        }
    }

    /// 在当前音量上加/减 `delta`,clamp 到 0..=100,只发命令。
    pub(crate) fn nudge_volume(&mut self, delta: i16) {
        let cur = i16::from(self.state.playback.volume_pct);
        let new = cur.saturating_add(delta).clamp(0, 100);
        let pct = u8::try_from(new).unwrap_or(self.state.playback.volume_pct);
        self.client.set_volume(pct);
    }

    /// 相对当前位置跳 `delta_s` 秒,clamp 到 [0, duration];时长未知时无法 clamp,不跳。
    pub(crate) fn seek_relative(&mut self, delta_s: i64) {
        let Some(dur_ms) = self.state.playback.duration_ms() else {
            return;
        };
        let cur = i64::try_from(self.state.playback.position_ms).unwrap_or(0);
        let max = i64::try_from(dur_ms).unwrap_or(0);
        let new_ms = cur
            .saturating_add(delta_s.saturating_mul(1000))
            .clamp(0, max);
        let new_u = u64::try_from(new_ms).unwrap_or(0);
        self.client.seek(new_u);
    }

    /// 持久化并乐观切换当前页面选中歌曲的喜欢态。Search 取结果或详情曲目，Browse 取
    /// Library 曲目；容器行、空行和全屏态不操作。
    pub(crate) fn toggle_love_selection(&mut self) {
        let song = match self.state.page_kind() {
            PageKind::Search => match self.state.channel_search.selected_entity() {
                Some(EntityRef::Song(song)) => Some(*song),
                Some(EntityRef::Album(_) | EntityRef::Artist(_) | EntityRef::Playlist(_))
                | None => None,
            },
            PageKind::Browse => {
                if self.state.browse.fullscreen.on()
                    || self.state.browse.view.current() != View::Library
                {
                    return;
                }
                self.state
                    .filtered_tracks()
                    .get(self.state.browse.nav.track.sel())
                    .map(|entry| entry.data.song.clone())
            }
        };
        let Some(song) = song else {
            return;
        };
        mineral_log::info!(
            target: "tui",
            song_id = %song.id,
            page = ?self.state.page_kind(),
            "切换当前页面选中歌曲的喜欢状态"
        );
        // 整首交给 daemon 持久化歌曲元信息及喜欢态，界面不等待异步回执。
        self.client.toggle_love(song.clone());
        self.state.toggle_loved_local(&song);
    }

    /// 执行 PopMenu 确认的动作(队列操作转 client;复制走系统剪贴板)。
    pub(crate) fn run_menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::Play {
                queue,
                target,
                context,
            } => self.play_queue(queue, target, context),
            MenuAction::PlayNext { song, context } => {
                self.client.queue_insert_next(vec![*song], context);
            }
            MenuAction::Append { song, context } => {
                self.client.queue_append(vec![*song], context);
            }
            MenuAction::Download(song) => {
                self.client.download(DownloadTarget::Song(song));
            }
            MenuAction::QueueEdit(op) => self.apply_queue_edit(op),
            MenuAction::PlayContainer(container) => {
                self.start_container_play(&container, PlayMode::Replace);
            }
            MenuAction::AppendContainer(container) => {
                self.start_container_play(&container, PlayMode::Append);
            }
            MenuAction::PlayNextContainer(container) => {
                self.start_container_play(&container, PlayMode::InsertNext);
            }
            MenuAction::Copy(text) => self.copy_to_clipboard(&text),
            // 渲染在 daemon 脚本运行时,结论异步回流(低频操作,按键不等)。
            MenuAction::CopyTemplate { index, ctx } => {
                self.client.render_copy_template(index, ctx);
            }
        }
    }

    /// 把文本写进系统剪贴板:成功 flash `Copied: …`(超长截断),失败 error toast。
    /// 句柄懒初始化、终身持有(理由见字段文档)。
    pub(crate) fn copy_to_clipboard(&mut self, text: &str) {
        if self.clipboard.is_none() {
            match arboard::Clipboard::new() {
                Ok(cb) => self.clipboard = Some(cb),
                Err(e) => {
                    mineral_log::warn!(target: "tui", error = mineral_log::chain(&e), "剪贴板初始化失败");
                    self.notifications.flash(tinted_text_item(
                        "clipboard unavailable".to_owned(),
                        TextTint::Error,
                    ));
                    return;
                }
            }
        }
        let Some(cb) = self.clipboard.as_mut() else {
            return;
        };
        match cb.set_text(text) {
            Ok(()) => {
                let shown = if text.chars().count() > COPY_TOAST_MAX_CHARS {
                    let head = text.chars().take(COPY_TOAST_MAX_CHARS).collect::<String>();
                    format!("Copied: {head}…")
                } else {
                    format!("Copied: {text}")
                };
                self.notifications
                    .flash(tinted_text_item(shown, TextTint::Normal));
            }
            Err(e) => {
                mineral_log::warn!(target: "tui", error = mineral_log::chain(&e), "写剪贴板失败");
                self.notifications
                    .flash(tinted_text_item("copy failed".to_owned(), TextTint::Error));
            }
        }
    }

    /// 下载当前视图选中项:Playlists 整张歌单 / Library 单曲。全屏态屏蔽。
    pub(crate) fn download_selection(&mut self) {
        if self.state.browse.fullscreen.on() {
            return;
        }
        match self.state.browse.view.current() {
            View::Playlists => {
                let id = self
                    .state
                    .filtered_playlists()
                    .get(self.state.browse.nav.playlist.sel())
                    .map(|p| p.data.id.clone());
                if let Some(id) = id {
                    self.client.download(DownloadTarget::Playlist(id));
                }
            }
            View::Library => {
                let song = self
                    .state
                    .filtered_tracks()
                    .get(self.state.browse.nav.track.sel())
                    .map(|entry| entry.data.song.clone());
                if let Some(song) = song {
                    self.client.download(DownloadTarget::Song(Box::new(song)));
                }
            }
        }
    }

    /// 容器「播放全部 / 加入队列 / 按序插播」入口:已加载曲目直接入队;未加载则派发详情拉取 +
    /// 登记待兑现意图,`*Fetched` 到货由 [`Self::fulfill_pending_container`] 入队。
    fn start_container_play(&mut self, container: &ContainerRef, mode: PlayMode) {
        if mode == PlayMode::Replace {
            self.pending_playlist_play = None;
        }
        // 先 owned 取出已加载曲目(释放对 state 的借用),再碰 client / pending。
        if let Some(songs) = self.container_loaded_songs(container) {
            self.enqueue_songs(songs, mode, container_context(container));
            return;
        }
        let fetch = container_fetch(container);
        crate::runtime::prefetch::submit_detail_tasks(
            &*self.client,
            &mut self.state.library,
            fetch.clone(),
        );
        self.pending_container.insert(fetch.dedup_key(), mode);
    }

    /// 容器曲目若已在手则返回(免冗余拉取):歌单退查 library 缓存;专辑 / artist 本地无缓存,
    /// 恒 `None`(走拉取)。
    fn container_loaded_songs(&self, container: &ContainerRef) -> Option<Vec<Song>> {
        match container {
            ContainerRef::Playlist(p) => {
                let views = self.state.library.tracks.get(&p.id)?;
                if !views.complete {
                    return None;
                }
                Some(views.iter().map(|entry| entry.data.song.clone()).collect())
            }
            ContainerRef::Album(_) | ContainerRef::Artist(_) => None,
        }
    }

    /// 原子替换 queue 并起播 exact target;structured reject 由完成事件提示。
    pub(crate) fn play_queue(
        &mut self,
        songs: Vec<Song>,
        target: usize,
        context: mineral_protocol::QueueContextWire,
    ) {
        self.pending_playlist_play = None;
        if let mineral_protocol::QueueContextWire::Playlist { id, name } = &context
            && let Some(tracks) = self
                .state
                .library
                .tracks
                .get(id)
                .filter(|tracks| !tracks.complete)
            && let Some(selected) = songs.get(target)
        {
            // 相同 SongId 的第几次出现必须保留，不能把重复歌曲都定位到首个 occurrence。
            let occurrence = songs
                .iter()
                .take(target)
                .filter(|song| song.id == selected.id)
                .count();
            if let Some(entry) = tracks
                .iter()
                .filter(|entry| entry.data.song.id == selected.id)
                .nth(occurrence)
            {
                let query = (self
                    .state
                    .cfg
                    .tui()
                    .behavior()
                    .filter_play_scope()
                    .matches_only()
                    && !self.state.browse.search.query().is_empty())
                .then(|| self.state.browse.search.query().to_owned());
                self.pending_playlist_play = Some(PendingPlaylistPlay {
                    playlist: id.clone(),
                    target: entry.data.clone(),
                    query,
                    name: name.clone(),
                });
                crate::runtime::prefetch::submit_detail_tasks(
                    &*self.client,
                    &mut self.state.library,
                    DetailFetch::PlaylistDetail(id.clone()),
                );
                mineral_log::info!(target: "tui", playlist = %id, "等待完整歌单后起播");
                return;
            }
        }
        self.client.play_queue(songs, target, context);
    }

    /// 将容器曲目作为一次请求提交，空容器不操作。
    /// Replace 替换队列并起播首曲；Append / InsertNext 由 daemon 按原序整组入队。
    /// `context` 随整组传递，使每首歌起播时归属该容器。
    fn enqueue_songs(
        &mut self,
        songs: Vec<Song>,
        mode: PlayMode,
        context: mineral_protocol::QueueContextWire,
    ) {
        if songs.is_empty() {
            return;
        }
        mineral_log::info!(
            target: "tui",
            mode = ?mode,
            count = songs.len(),
            context = ?context,
            "enqueue container songs"
        );
        match mode {
            PlayMode::Replace => self.play_queue(songs, 0, context),
            PlayMode::Append => self.client.queue_append(songs, context),
            PlayMode::InsertNext => self.client.queue_insert_next(songs, context),
        }
    }

    /// 容器播放意图兑现:`*Fetched` 事件按 [`DetailFetch::dedup_key`] 与登记意图配对,命中则从
    /// **事件载荷**(非 detail 帧——帧可能已切走)取曲目入队、清意图。artist 只认热门曲那路
    /// (`ArtistDetailFetched`),`ArtistAlbumsFetched` 是专辑壳、与播放无关,不响应。
    pub(crate) fn fulfill_pending_container(&mut self, ev: &TaskEvent) {
        use mineral_protocol::QueueContextWire;
        if let TaskEvent::PlaylistDetailFailed {
            id,
            load: mineral_channel_core::PlaylistLoad::Complete,
        } = ev
        {
            let container = self
                .pending_container
                .remove(&DetailFetch::PlaylistDetail(id.clone()).dedup_key())
                .is_some();
            let selection = self
                .pending_playlist_play
                .as_ref()
                .is_some_and(|pending| pending.playlist == *id);
            if selection {
                self.pending_playlist_play = None;
            }
            if container || selection {
                self.notifications.flash(tinted_text_item(
                    "Could not load the complete playlist".to_owned(),
                    TextTint::Error,
                ));
            }
            return;
        }
        if let TaskEvent::PlaylistDetailFetched { id, detail, .. } = ev
            && detail.complete
            && self
                .pending_playlist_play
                .as_ref()
                .is_some_and(|pending| pending.playlist == *id)
            && let Some(pending) = self.pending_playlist_play.take()
        {
            if let Some((songs, target)) = pending.resolve(&detail.playlist.entries) {
                self.client.play_queue(
                    songs,
                    target,
                    QueueContextWire::Playlist {
                        id: id.clone(),
                        name: pending.name,
                    },
                );
            } else {
                mineral_log::warn!(target: "tui", playlist = %id, song = %pending.target.song.id, "歌单补齐后起播目标已变化");
                self.notifications.flash(tinted_text_item(
                    "The selected playlist entry has changed".to_owned(),
                    TextTint::Error,
                ));
            }
        }
        // 语境由到货事件的实体 id 直接定出(专辑 / 歌单 / artist),与登记意图时的容器同一身份。
        let (key, songs, context) = match ev {
            TaskEvent::AlbumDetailFetched { id, album } => (
                DetailFetch::AlbumDetail(id.clone()).dedup_key(),
                album
                    .tracks
                    .iter()
                    .map(|track| track.song.clone())
                    .collect(),
                QueueContextWire::Album {
                    id: id.clone(),
                    name: Some(album.name.clone()),
                },
            ),
            TaskEvent::PlaylistDetailFetched { id, detail, .. } if detail.complete => (
                DetailFetch::PlaylistDetail(id.clone()).dedup_key(),
                detail
                    .playlist
                    .entries
                    .iter()
                    .map(|entry| entry.song.clone())
                    .collect(),
                QueueContextWire::Playlist {
                    id: id.clone(),
                    name: Some(detail.playlist.name.clone()),
                },
            ),
            TaskEvent::ArtistDetailFetched { id, artist } => (
                DetailFetch::Artist(id.clone()).dedup_key(),
                artist.songs.clone(),
                QueueContextWire::Artist {
                    id: id.clone(),
                    name: Some(artist.name.clone()),
                },
            ),
            // 其余事件(含 ArtistAlbumsFetched)不兑现容器播放意图。
            _ => return,
        };
        if let Some(mode) = self.pending_container.remove(&key) {
            self.enqueue_songs(songs, mode, context);
        }
    }
}

/// 容器 → 其详情拉取目标(`DetailFetch`,跨类型 dedup_key 不碰撞)。
fn container_fetch(container: &ContainerRef) -> DetailFetch {
    match container {
        ContainerRef::Album(a) => DetailFetch::AlbumDetail(a.id.clone()),
        ContainerRef::Playlist(p) => DetailFetch::PlaylistDetail(p.id.clone()),
        ContainerRef::Artist(a) => DetailFetch::Artist(a.id.clone()),
    }
}

/// 容器 → 其起播队列语境(埋点 provenance:整张专辑 / 整个歌单 / artist 热门曲各归其身份)。
/// 曲目已加载时立即入队走它;未加载走拉取,到货后由 [`App::fulfill_pending_container`] 按
/// 事件 id 重新定出同一语境。
fn container_context(container: &ContainerRef) -> mineral_protocol::QueueContextWire {
    use mineral_protocol::QueueContextWire;
    match container {
        ContainerRef::Album(a) => QueueContextWire::Album {
            id: a.id.clone(),
            name: Some(a.name.clone()),
        },
        ContainerRef::Playlist(p) => QueueContextWire::Playlist {
            id: p.id.clone(),
            name: Some(p.name.clone()),
        },
        ContainerRef::Artist(a) => QueueContextWire::Artist {
            id: a.id.clone(),
            name: Some(a.name.clone()),
        },
    }
}

#[cfg(test)]
mod tests {
    use mineral_channel_core::Page;
    use mineral_model::{Album, AlbumId, Artist, ArtistId, SourceKind};
    use mineral_protocol::{QueueContextWire, ViewKind};
    use mineral_task::TaskEvent;

    use super::PlayMode;
    use crate::components::popup::{ContainerRef, MenuAction};
    use crate::runtime::state::DetailFetch;
    use crate::test_support::{
        app_with_library, app_with_library_probed, app_with_queue, endserenading,
    };

    /// 首批起播等待完整结果；补进中间缺口后仍定位到同一重复歌曲 occurrence。
    #[test]
    fn partial_playlist_waits_for_complete_queue_and_preserves_occurrence() -> color_eyre::Result<()>
    {
        use mineral_channel_core::{PlaylistDetail, PlaylistLoad};
        use mineral_model::{CollectionIndex, Playlist, PlaylistEntry, PlaylistId};
        let (mut app, queue_ops) = app_with_library_probed(2, 1)?;
        let id = PlaylistId::new(SourceKind::NETEASE, "p1");
        let repeated = mineral_test::song("repeat");
        let preview_entries = [0, 2]
            .into_iter()
            .map(|index| {
                PlaylistEntry::builder()
                    .index(CollectionIndex::new(index))
                    .song(repeated.clone())
                    .build()
            })
            .collect::<Vec<_>>();
        let preview = TaskEvent::PlaylistDetailFetched {
            id: id.clone(),
            load: PlaylistLoad::Preview,
            detail: Box::new(PlaylistDetail {
                playlist: Playlist::builder()
                    .id(id.clone())
                    .name("p".to_owned())
                    .track_count(3)
                    .entries(preview_entries)
                    .build(),
                complete: false,
                next_offset: None,
            }),
        };
        // Replace the fixture's complete data with an actual first response.
        app.state.library.tracks.remove(&id);
        app.state.apply(&preview);
        app.play_queue(
            vec![repeated.clone(), repeated.clone()],
            1,
            QueueContextWire::Playlist {
                id: id.clone(),
                name: Some("p".to_owned()),
            },
        );
        app.fulfill_pending_container(&preview);
        assert!(
            queue_ops
                .lock()
                .map_err(|e| color_eyre::eyre::eyre!("{e}"))?
                .is_empty(),
            "首批不得入队"
        );
        let full = TaskEvent::PlaylistDetailFetched {
            id: id.clone(),
            load: PlaylistLoad::Complete,
            detail: Box::new(PlaylistDetail::complete(
                Playlist::builder()
                    .id(id)
                    .name("p".to_owned())
                    .track_count(3)
                    .entries(PlaylistEntry::enumerate(vec![
                        repeated.clone(),
                        mineral_test::song("middle"),
                        repeated.clone(),
                    ]))
                    .build(),
            )),
        };
        app.state.apply(&full);
        app.fulfill_pending_container(&full);
        assert_eq!(
            *queue_ops
                .lock()
                .map_err(|e| color_eyre::eyre::eyre!("{e}"))?,
            vec![("play_queue", format!("3:2:{}", repeated.id.qualified()))]
        );
        assert!(app.pending_playlist_play.is_none());
        Ok(())
    }

    /// 造带 `n` 首曲的专辑(容器播放测试素材)。
    fn album_with_songs(raw: &str, n: usize) -> Album {
        Album::builder()
            .id(AlbumId::new(SourceKind::NETEASE, raw))
            .name(format!("album {raw}"))
            .tracks(mineral_model::AlbumTrack::enumerate(endserenading(n)))
            .build()
    }

    /// 容器播放全部(专辑曲目未加载)→ 先派发拉取 + 挂 pending、无即时入队;AlbumDetailFetched
    /// 到货 fulfill → atomic PlayQueue(全曲,target 0)。
    #[test]
    fn container_play_all_fetches_then_enqueues() -> color_eyre::Result<()> {
        let (mut app, queue_ops) = app_with_library_probed(/*len*/ 1, /*sel_track*/ 0)?;
        let album = album_with_songs("al1", 3);
        let first_id = album
            .tracks
            .first()
            .map(|track| track.song.id.qualified())
            .ok_or_else(|| color_eyre::eyre::eyre!("素材应有曲"))?;
        // 结果列专辑只有壳(无 songs)→ 触发拉取。
        let shell = Album::builder()
            .id(album.id.clone())
            .name(album.name.clone())
            .build();
        app.run_menu_action(MenuAction::PlayContainer(Box::new(ContainerRef::Album(
            Box::new(shell),
        ))));
        assert!(
            queue_ops
                .lock()
                .map_err(|e| color_eyre::eyre::eyre!("锁中毒: {e}"))?
                .is_empty(),
            "拉取前不入队"
        );
        assert!(
            app.pending_container
                .contains_key(&DetailFetch::AlbumDetail(album.id.clone()).dedup_key()),
            "已挂 pending 意图"
        );
        app.fulfill_pending_container(&TaskEvent::AlbumDetailFetched {
            id: album.id.clone(),
            album: Box::new(album.clone()),
        });
        let ops = queue_ops
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("锁中毒: {e}"))?;
        assert_eq!(
            *ops,
            vec![("play_queue", format!("3:0:{first_id}"))],
            "到货后整专辑与 target 0 原子提交"
        );
        assert!(app.pending_container.is_empty(), "兑现后意图清除");
        Ok(())
    }

    /// 容器详情到货后，追加请求保留全部歌曲的顺序。
    #[test]
    fn container_append_all_keeps_order() -> color_eyre::Result<()> {
        let (mut app, queue_ops) = app_with_library_probed(/*len*/ 1, /*sel_track*/ 0)?;
        let album = album_with_songs("al1", 2);
        let want: Vec<(&str, String)> = album
            .tracks
            .iter()
            .map(|track| ("append", track.song.id.qualified()))
            .collect();
        app.pending_container.insert(
            DetailFetch::AlbumDetail(album.id.clone()).dedup_key(),
            PlayMode::Append,
        );
        app.fulfill_pending_container(&TaskEvent::AlbumDetailFetched {
            id: album.id.clone(),
            album: Box::new(album),
        });
        let ops = queue_ops
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("锁中毒: {e}"))?;
        assert_eq!(*ops, want, "追加保留容器原序");
        Ok(())
    }

    /// 千首歌单的追加和插播各提交一次，避免歌曲数消耗请求额度。
    #[test]
    fn large_cached_playlist_submits_one_queue_request() -> color_eyre::Result<()> {
        use std::sync::Arc;

        use crate::test_support::{TestClient, app_with_long_library};

        for mode in [PlayMode::Append, PlayMode::InsertNext] {
            let mut app = app_with_long_library(/*len*/ 1000, /*sel_track*/ 0)?;
            let client = Arc::new(TestClient::default());
            app.client = client.clone();
            let playlist = app
                .state
                .library
                .playlists
                .first()
                .ok_or_else(|| color_eyre::eyre::eyre!("应有测试歌单"))?
                .data
                .clone();
            let songs = app
                .state
                .library
                .tracks
                .get(&playlist.id)
                .ok_or_else(|| color_eyre::eyre::eyre!("应有缓存曲目"))?
                .iter()
                .map(|entry| entry.data.song.clone())
                .collect::<Vec<_>>();
            app.state.player.queue = songs.iter().take(500).cloned().collect();
            let container = Box::new(ContainerRef::Playlist(Box::new(playlist.clone())));
            let (operation, action) = if mode == PlayMode::Append {
                ("append", MenuAction::AppendContainer(container))
            } else {
                ("insert_next", MenuAction::PlayNextContainer(container))
            };
            app.run_menu_action(action);
            let operations = client
                .queue_ops
                .lock()
                .map_err(|e| color_eyre::eyre::eyre!("queue_ops 锁中毒: {e}"))?;
            assert_eq!(
                *operations,
                songs
                    .iter()
                    .map(|song| (operation, song.id.qualified()))
                    .collect::<Vec<_>>(),
                "{mode:?} 应保留全部 1000 首及其顺序"
            );
            let contexts = client
                .queue_contexts
                .lock()
                .map_err(|e| color_eyre::eyre::eyre!("queue_contexts 锁中毒: {e}"))?;
            assert_eq!(
                *contexts,
                vec![(
                    operation,
                    QueueContextWire::Playlist {
                        id: playlist.id,
                        name: Some(playlist.name),
                    }
                )],
                "{mode:?} 整张歌单只提交一次"
            );
        }
        Ok(())
    }

    /// pending 按 id 配对:登记 al1 意图,到货 al2 不兑现、al1 意图保留。
    #[test]
    fn container_intent_matches_by_id() -> color_eyre::Result<()> {
        let (mut app, queue_ops) = app_with_library_probed(/*len*/ 1, /*sel_track*/ 0)?;
        let key1 = DetailFetch::AlbumDetail(AlbumId::new(SourceKind::NETEASE, "al1")).dedup_key();
        app.pending_container
            .insert(key1.clone(), PlayMode::Replace);
        let al2 = album_with_songs("al2", 2);
        app.fulfill_pending_container(&TaskEvent::AlbumDetailFetched {
            id: al2.id.clone(),
            album: Box::new(al2),
        });
        assert!(
            queue_ops
                .lock()
                .map_err(|e| color_eyre::eyre::eyre!("锁中毒: {e}"))?
                .is_empty(),
            "非匹配 id 不入队"
        );
        assert!(app.pending_container.contains_key(&key1), "al1 意图仍在");
        Ok(())
    }

    /// artist 播放只认热门曲那路:ArtistAlbumsFetched 不兑现、ArtistDetailFetched(带热门曲)才入队。
    #[test]
    fn artist_play_only_fulfills_detail_path() -> color_eyre::Result<()> {
        let (mut app, queue_ops) = app_with_library_probed(/*len*/ 1, /*sel_track*/ 0)?;
        let aid = ArtistId::new(SourceKind::NETEASE, "ar1");
        app.pending_container.insert(
            DetailFetch::Artist(aid.clone()).dedup_key(),
            PlayMode::Replace,
        );
        // 专辑那路到货:不兑现。
        app.fulfill_pending_container(&TaskEvent::ArtistAlbumsFetched {
            id: aid.clone(),
            page: Page::default(),
            albums: Vec::new(),
            has_more: None,
        });
        assert!(
            queue_ops
                .lock()
                .map_err(|e| color_eyre::eyre::eyre!("锁中毒: {e}"))?
                .is_empty(),
            "ArtistAlbums 路不兑现播放意图"
        );
        // 详情那路(带热门曲)到货:入队起播。
        let artist = Artist::builder()
            .id(aid.clone())
            .name("A".to_owned())
            .songs(endserenading(2))
            .build();
        let first = artist
            .songs
            .first()
            .map(|s| s.id.qualified())
            .ok_or_else(|| color_eyre::eyre::eyre!("热门曲应有"))?;
        app.fulfill_pending_container(&TaskEvent::ArtistDetailFetched {
            id: aid.clone(),
            artist: Box::new(artist),
        });
        let ops = queue_ops
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("锁中毒: {e}"))?;
        assert_eq!(
            ops.first(),
            Some(&("play_queue", format!("2:0:{first}"))),
            "热门曲路到货才起播"
        );
        Ok(())
    }

    /// 容器播放从专辑起播时必须携带 Album 语境，否则 albums-via-context 统计无法归属。
    #[test]
    fn container_play_carries_album_context() -> color_eyre::Result<()> {
        let (mut app, _ops) = app_with_library_probed(/*len*/ 1, /*sel_track*/ 0)?;
        let contexts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        app.client = std::sync::Arc::new(crate::test_support::TestClient {
            queue_contexts: std::sync::Arc::clone(&contexts),
            ..crate::test_support::TestClient::default()
        });
        let album = album_with_songs("al1", 3);
        // 结果列专辑只有壳(无 songs)→ 触发拉取 + 挂 pending,到货再入队。
        let shell = Album::builder()
            .id(album.id.clone())
            .name(album.name.clone())
            .build();
        app.run_menu_action(MenuAction::PlayContainer(Box::new(ContainerRef::Album(
            Box::new(shell),
        ))));
        app.fulfill_pending_container(&TaskEvent::AlbumDetailFetched {
            id: album.id.clone(),
            album: Box::new(album.clone()),
        });
        let got = contexts
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("queue_contexts 锁中毒: {e}"))?;
        assert_eq!(
            *got,
            vec![(
                "play_queue",
                mineral_protocol::QueueContextWire::Album {
                    id: album.id.clone(),
                    name: Some(album.name.clone()),
                }
            )],
            "容器专辑起播 PlayQueue 带 Album 语境(带标题快照)"
        );
        Ok(())
    }

    /// 整张专辑按原序提交一次插播请求，并携带 Album 语境。
    #[test]
    fn container_play_next_submits_one_batch_in_order() -> color_eyre::Result<()> {
        let (mut app, _ops) = app_with_library_probed(/*len*/ 1, /*sel_track*/ 0)?;
        app.state.player.queue = endserenading(1);
        let queue_ops = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let queue_contexts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        app.client = std::sync::Arc::new(crate::test_support::TestClient {
            queue_ops: std::sync::Arc::clone(&queue_ops),
            queue_contexts: std::sync::Arc::clone(&queue_contexts),
            ..crate::test_support::TestClient::default()
        });
        let album = album_with_songs("al1", 3);
        let want = album
            .tracks
            .iter()
            .map(|track| ("insert_next", track.song.id.qualified()))
            .collect::<Vec<_>>();
        app.pending_container.insert(
            DetailFetch::AlbumDetail(album.id.clone()).dedup_key(),
            PlayMode::InsertNext,
        );
        app.fulfill_pending_container(&TaskEvent::AlbumDetailFetched {
            id: album.id.clone(),
            album: Box::new(album.clone()),
        });
        let ops = queue_ops
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("queue_ops 锁中毒: {e}"))?;
        assert_eq!(*ops, want, "插播请求保留专辑原序");
        let ctxs = queue_contexts
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("queue_contexts 锁中毒: {e}"))?;
        assert_eq!(
            *ctxs,
            vec![(
                "insert_next",
                QueueContextWire::Album {
                    id: album.id.clone(),
                    name: Some(album.name.clone()),
                },
            )],
            "整张专辑只提交一次，语境归 Album 身份"
        );
        Ok(())
    }

    /// 本地队列为空时仍提交原序插播请求，由 daemon 决定插入位置。
    #[test]
    fn container_play_next_on_empty_queue_keeps_insert_intent() -> color_eyre::Result<()> {
        let (mut app, queue_ops) = app_with_library_probed(/*len*/ 1, /*sel_track*/ 0)?;
        assert!(app.state.player.queue.is_empty(), "前置:本地队列应为空");
        let album = album_with_songs("al1", 2);
        let want = album
            .tracks
            .iter()
            .map(|track| ("insert_next", track.song.id.qualified()))
            .collect::<Vec<_>>();
        app.pending_container.insert(
            DetailFetch::AlbumDetail(album.id.clone()).dedup_key(),
            PlayMode::InsertNext,
        );
        app.fulfill_pending_container(&TaskEvent::AlbumDetailFetched {
            id: album.id.clone(),
            album: Box::new(album),
        });
        let ops = queue_ops
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("queue_ops 锁中毒: {e}"))?;
        assert_eq!(*ops, want, "本地空队列不改变插播意图或歌曲顺序");
        Ok(())
    }

    /// Library 视图:view 映射 Tracks,选中歌 / 所在歌单 / 在播全采到。
    #[test]
    fn keyctx_library_view_collects_selection() -> color_eyre::Result<()> {
        let mut app = app_with_library(/*len*/ 3, /*sel_track*/ 1)?;
        app.state.player.current = app
            .state
            .filtered_tracks()
            .first()
            .map(|entry| entry.data.song.clone());
        let ctx = app.collect_key_context();
        assert_eq!(*ctx.view(), ViewKind::Tracks);
        let want_sel = app
            .state
            .filtered_tracks()
            .get(1)
            .map(|entry| entry.data.song.id.clone());
        assert_eq!(ctx.selected_song().as_ref().map(|s| s.id.clone()), want_sel);
        assert_eq!(
            *ctx.selected_loved(),
            Some(false),
            "选中歌的 ♥ 态随投影给(测试装饰默认 false)"
        );
        assert!(
            ctx.selected_playlist()
                .as_ref()
                .is_some_and(|p| !p.name.is_empty()),
            "Library 视图下所在歌单也算选中,且带名字"
        );
        assert_eq!(
            ctx.now_playing().as_ref().map(|s| s.id.clone()),
            app.state.player.current.as_ref().map(|s| s.id.clone())
        );
        assert_eq!(*ctx.search_query(), None, "无过滤词为 None");
        Ok(())
    }

    /// Playlists 视图:选中歌单命中、选中歌为 None。
    #[test]
    fn keyctx_playlists_view_selects_playlist_only() -> color_eyre::Result<()> {
        let mut app = app_with_library(/*len*/ 3, /*sel_track*/ 0)?;
        app.state
            .browse
            .view
            .switch_to(crate::runtime::state::View::Playlists);
        app.state.player.current = None;
        let ctx = app.collect_key_context();
        assert_eq!(*ctx.view(), ViewKind::Playlists);
        assert!(ctx.selected_song().is_none());
        assert!(ctx.selected_playlist().is_some());
        assert!(ctx.now_playing().is_none(), "停止态在播为 None");
        Ok(())
    }

    /// 队列浮层开着:view 报 Queue,选中歌取浮层光标所指的队列条目。
    #[test]
    fn keyctx_queue_overlay_selects_cursor_entry() -> color_eyre::Result<()> {
        let mut app = app_with_queue(/*len*/ 3, /*current_idx*/ 0)?;
        app.overlays
            .push(crate::components::popup::OverlayKind::queue(/*sel*/ 2));
        let ctx = app.collect_key_context();
        assert_eq!(*ctx.view(), ViewKind::Queue);
        assert_eq!(
            ctx.selected_song().as_ref().map(|s| s.id.clone()),
            app.state.player.queue.get(2).map(|s| s.id.clone()),
            "浮层光标所指条目算选中"
        );
        assert_eq!(
            *ctx.selected_loved(),
            Some(false),
            "队列条目 ♥ 态查 liked_ids 缓存(测试无 liked 记录 = false)"
        );
        Ok(())
    }

    /// 全屏态:view 报 Fullscreen,无列表选中,在播照常。
    #[test]
    fn keyctx_fullscreen_reports_now_playing() -> color_eyre::Result<()> {
        let mut app = app_with_queue(/*len*/ 2, /*current_idx*/ 1)?;
        app.state.browse.fullscreen.set(true);
        let ctx = app.collect_key_context();
        assert_eq!(*ctx.view(), ViewKind::Fullscreen);
        assert!(ctx.selected_song().is_none());
        assert_eq!(
            ctx.now_playing().as_ref().map(|s| s.id.clone()),
            app.state.player.current.as_ref().map(|s| s.id.clone())
        );
        Ok(())
    }

    /// channel 搜索布局态的脚本上下文必须报 Search，不能看穿到下层主视图。
    #[test]
    fn keyctx_channel_search_reports_search() -> color_eyre::Result<()> {
        let mut app = app_with_library(/*len*/ 3, /*sel_track*/ 0)?;
        app.state.channel_search.active.set(true);
        let ctx = app.collect_key_context();
        assert_eq!(
            *ctx.view(),
            ViewKind::Search,
            "channel 搜索态应报 Search,而非看穿到下层主视图"
        );
        Ok(())
    }

    /// 非 queue 浮层(确认框)对脚本上下文透明；channel 搜索态上叠确认框仍报 Search。
    #[test]
    fn keyctx_non_queue_overlay_is_transparent() -> color_eyre::Result<()> {
        let mut app = app_with_library(/*len*/ 3, /*sel_track*/ 0)?;
        app.state.channel_search.active.set(true);
        app.overlays
            .push(crate::components::popup::OverlayKind::confirm());
        let ctx = app.collect_key_context();
        assert_eq!(
            *ctx.view(),
            ViewKind::Search,
            "非 queue 浮层透明,看穿到下层 channel 搜索 = Search"
        );
        Ok(())
    }
}
