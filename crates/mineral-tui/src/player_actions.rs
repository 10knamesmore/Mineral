//! 「转 Client」的领域动作执行器:播放控制 / love / 下载 / 脚本动作。
//!
//! 从 app 模块拆出的 `App` 方法集(单文件体量约束);执行点仍是
//! `App::dispatch`,这里只放函数体。

use mineral_model::Song;
use mineral_protocol::DownloadTarget;
use mineral_task::TaskEvent;

use crate::app::App;
use crate::components::popup::{ContainerRef, MenuAction};
use crate::components::toast::notifications::{TextTint, tinted_text_item};
use crate::runtime::action::ScriptSlot;
use crate::runtime::state::{ActiveLayer, DetailFetch, View};

/// 容器入队模式:替换队列起播 / 追加到队尾 / 按序插播(由 `PlayContainer` /
/// `AppendContainer` / `PlayNextContainer` 决定)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlayMode {
    /// 替换队列并起播首曲。
    Replace,

    /// 追加到队尾。
    Append,

    /// 按原顺序插到当前曲之后(下一首起连播);本地队列为空时退化为追加。
    InsertNext,
}

/// 复制成功 toast 里展示的内容上限(字符);超出截断加省略号,防长模板把顶栏挤爆。
const COPY_TOAST_MAX_CHARS: usize = 48;

impl App {
    /// 触发 `tui.keys.script` 绑定的脚本动作:槽位 → 注册名 → daemon;
    /// daemon 报错(未注册 / 脚本未启用 / 执行失败)时 toast 提示。
    pub(crate) fn invoke_script_action(&mut self, slot: ScriptSlot) {
        // owned 拷贝:松开对 keymap 的借用,下面才能可变借用 notifications。
        let Some(name) = self.keymap.script_action(slot).map(str::to_owned) else {
            return;
        };
        let ctx = self.collect_key_context();
        if let Some(err) = self.client.invoke_action(&name, Some(ctx)) {
            use crate::components::toast::notifications::{TextTint, tinted_text_item};
            self.notifications
                .flash(tinted_text_item(err, TextTint::Error));
        }
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
        // Library 列表取选中行(SongView 已装饰)。
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
                                .into_iter()
                                .nth(self.state.browse.nav.track.sel());
                            let loved = sel.as_ref().map(|sv| sv.loved);
                            (ViewKind::Tracks, sel.map(|sv| sv.data), loved)
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

    /// 在当前音量上加/减 `delta`,clamp 到 0..=100,本地立即更新避免 UI 滞后。
    pub(crate) fn nudge_volume(&mut self, delta: i16) {
        let cur = i16::from(self.state.playback.volume_pct);
        let new = cur.saturating_add(delta).clamp(0, 100);
        let pct = u8::try_from(new).unwrap_or(self.state.playback.volume_pct);
        self.client.set_volume(pct);
        self.state.playback.volume_pct = pct;
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

    /// 切换选中曲的 ♥:转发持久化意图 + 本地乐观翻转。仅 Library 有曲可选;全屏态屏蔽。
    pub(crate) fn toggle_love_selection(&mut self) {
        if self.state.browse.fullscreen.on()
            || !matches!(self.state.browse.view.current(), View::Library)
        {
            return;
        }
        let filtered = self.state.filtered_tracks();
        if let Some(song) = filtered
            .get(self.state.browse.nav.track.sel())
            .map(|sv| sv.data.clone())
        {
            // 触发持久化(daemon 写本地 + 远端,整首传入顺手落 meta);in-proc fire-and-forget。
            self.client.toggle_love(song.clone());
            // 乐观翻转:♥ 立即变,不等 server 确认。
            self.state.toggle_loved_local(&song);
        }
    }

    /// 执行 PopMenu 确认的动作(队列操作转 client;复制走系统剪贴板)。
    pub(crate) fn run_menu_action(&mut self, action: MenuAction) {
        match action {
            // 替换队列并起播两步(漏 play_song 会换队不响,见 nav PlayQueue 注释);空 queue
            // 退化为单曲队列,绝不给 set_queue 空列。
            MenuAction::Play {
                song,
                queue,
                context,
            } => {
                let target = song.id.clone();
                let queue = if queue.is_empty() {
                    vec![(*song).clone()]
                } else {
                    queue
                };
                self.client.set_queue(queue, target, context);
                self.client.play_song(*song);
            }
            MenuAction::PlayNext(song) => self
                .client
                .queue_insert_next(*song, mineral_protocol::QueueContextWire::Manual),
            MenuAction::Append(song) => self
                .client
                .queue_append(*song, mineral_protocol::QueueContextWire::Manual),
            MenuAction::Download(song) => self.client.download(DownloadTarget::Song(song)),
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
            // 同步等 daemon 渲染(IPC 往返 + Lua 执行,看门狗 hard wall 封顶):
            // 复制是低频操作,与 invoke_action 同款阻塞语义。
            MenuAction::CopyTemplate { index, ctx } => {
                match self.client.render_copy_template(index, ctx) {
                    Ok(text) => self.copy_to_clipboard(&text),
                    Err(msg) => {
                        self.notifications
                            .flash(tinted_text_item(msg, TextTint::Error));
                    }
                }
            }
        }
    }

    /// 把文本写进系统剪贴板:成功 flash `Copied: …`(超长截断),失败 error toast。
    /// 句柄懒初始化、终身持有(理由见字段文档)。
    fn copy_to_clipboard(&mut self, text: &str) {
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
                    .map(|sv| sv.data.clone());
                if let Some(song) = song {
                    self.client.download(DownloadTarget::Song(Box::new(song)));
                }
            }
        }
    }

    /// 容器「播放全部 / 加入队列 / 按序插播」入口:已加载曲目直接入队;未加载则派发详情拉取 +
    /// 登记待兑现意图,`*Fetched` 到货由 [`Self::fulfill_pending_container`] 入队。
    fn start_container_play(&mut self, container: &ContainerRef, mode: PlayMode) {
        // 先 owned 取出已加载曲目(释放对 state 的借用),再碰 client / pending。
        if let Some(songs) = self.container_loaded_songs(container) {
            self.enqueue_songs(songs, mode, container_context(container));
            return;
        }
        let fetch = container_fetch(container);
        crate::runtime::prefetch::submit_detail_tasks(&*self.client, fetch.clone());
        self.pending_container.insert(fetch.dedup_key(), mode);
    }

    /// 容器曲目若已在手则返回(免冗余拉取):歌单退查 library 缓存;专辑 / artist 本地无缓存,
    /// 恒 `None`(走拉取)。
    fn container_loaded_songs(&self, container: &ContainerRef) -> Option<Vec<Song>> {
        match container {
            ContainerRef::Playlist(p) => {
                let views = self.state.library.tracks.get(&p.id)?;
                (!views.is_empty()).then(|| {
                    views
                        .iter()
                        .map(|sv| sv.data.clone())
                        .collect::<Vec<Song>>()
                })
            }
            ContainerRef::Album(_) | ContainerRef::Artist(_) => None,
        }
    }

    /// 按模式入队一组曲目:Replace = 替换队列 + 起播首曲(空则 no-op,绝不发空 set_queue);
    /// Append = 逐曲追加(无批量 API);InsertNext = 按原顺序插到当前曲之后。`context` 是这批
    /// 曲目的来源语境(容器身份),Replace 落队列级、Append / InsertNext 逐曲带上(整张专辑
    /// 插入,每首都归该专辑而非笼统 Manual)。
    fn enqueue_songs(
        &self,
        songs: Vec<Song>,
        mode: PlayMode,
        context: mineral_protocol::QueueContextWire,
    ) {
        match mode {
            PlayMode::Replace => {
                let Some(first) = songs.first().cloned() else {
                    return;
                };
                let target = first.id.clone();
                self.client.set_queue(songs, target, context);
                self.client.play_song(first);
            }
            PlayMode::Append => {
                for song in songs {
                    self.client.queue_append(song, context.clone());
                }
            }
            PlayMode::InsertNext => {
                // insert_next 恒插在当前曲后一位:倒序逐曲喂入才把整组还原成原序连播
                // (正序会逐首顶到最前、把顺序翻过来)。本地队列为空时插入点数学退化
                // (无当前曲、queue_sel 悬空),改逐曲 append 保序(同 Append,不起播)。
                if self.state.player.queue.is_empty() {
                    for song in songs {
                        self.client.queue_append(song, context.clone());
                    }
                } else {
                    for song in songs.into_iter().rev() {
                        self.client.queue_insert_next(song, context.clone());
                    }
                }
            }
        }
    }

    /// 容器播放意图兑现:`*Fetched` 事件按 [`DetailFetch::dedup_key`] 与登记意图配对,命中则从
    /// **事件载荷**(非 detail 帧——帧可能已切走)取曲目入队、清意图。artist 只认热门曲那路
    /// (`ArtistDetailFetched`),`ArtistAlbumsFetched` 是专辑壳、与播放无关,不响应。
    pub(crate) fn fulfill_pending_container(&mut self, ev: &TaskEvent) {
        use mineral_protocol::QueueContextWire;
        // 语境由到货事件的实体 id 直接定出(专辑 / 歌单 / artist),与登记意图时的容器同一身份。
        let (key, songs, context) = match ev {
            TaskEvent::AlbumDetailFetched { id, album } => (
                DetailFetch::AlbumDetail(id.clone()).dedup_key(),
                album.songs.clone(),
                QueueContextWire::Album {
                    id: id.clone(),
                    name: Some(album.name.clone()),
                },
            ),
            TaskEvent::PlaylistDetailFetched { id, playlist } => (
                DetailFetch::PlaylistDetail(id.clone()).dedup_key(),
                playlist.songs.clone(),
                QueueContextWire::Playlist {
                    id: id.clone(),
                    name: Some(playlist.name.clone()),
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

    /// 造带 `n` 首曲的专辑(容器播放测试素材)。
    fn album_with_songs(raw: &str, n: usize) -> Album {
        Album::builder()
            .id(AlbumId::new(SourceKind::NETEASE, raw))
            .name(format!("album {raw}"))
            .songs(endserenading(n))
            .build()
    }

    /// 容器播放全部(专辑曲目未加载)→ 先派发拉取 + 挂 pending、无即时入队;AlbumDetailFetched
    /// 到货 fulfill → set_queue(全曲) + play_song(首曲)两步。
    #[test]
    fn container_play_all_fetches_then_enqueues() -> color_eyre::Result<()> {
        let (mut app, queue_ops) = app_with_library_probed(/*len*/ 1, /*sel_track*/ 0)?;
        let album = album_with_songs("al1", 3);
        let first_id = album
            .songs
            .first()
            .map(|s| s.id.qualified())
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
            vec![
                ("set_queue", format!("3:{first_id}")),
                ("play_song", first_id.clone()),
            ],
            "到货后整专辑替换队列 + 起播首曲"
        );
        assert!(app.pending_container.is_empty(), "兑现后意图清除");
        Ok(())
    }

    /// 容器加入队列(Append 模式)→ fulfill 后逐曲 queue_append、保序。
    #[test]
    fn container_append_all_enqueues_each() -> color_eyre::Result<()> {
        let (mut app, queue_ops) = app_with_library_probed(/*len*/ 1, /*sel_track*/ 0)?;
        let album = album_with_songs("al1", 2);
        let want: Vec<(&str, String)> = album
            .songs
            .iter()
            .map(|s| ("append", s.id.qualified()))
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
        assert_eq!(*ops, want, "逐曲 append 保序");
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
            Some(&("set_queue", format!("2:{first}"))),
            "热门曲路到货才起播"
        );
        Ok(())
    }

    /// F1 回归:容器播放(专辑)起播记 Album 语境——此前 `enqueue_songs` 硬编码 Unknown,
    /// albums-via-context 统计因此恒空。
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
                "set_queue",
                mineral_protocol::QueueContextWire::Album {
                    id: album.id.clone(),
                    name: Some(album.name.clone()),
                }
            )],
            "容器专辑起播 set_queue 带 Album 语境(带标题快照)"
        );
        Ok(())
    }

    /// InsertNext 容器(本地队列非空):倒序逐曲 `queue_insert_next`——server 恒插当前曲
    /// 后一位,倒序喂入恰把专辑还原成原序连播;语境逐曲带 Album 身份。
    #[test]
    fn container_play_next_inserts_reversed_keeping_order() -> color_eyre::Result<()> {
        let (mut app, _ops) = app_with_library_probed(/*len*/ 1, /*sel_track*/ 0)?;
        // 本地队列非空 → 走倒序 insert_next 分支。
        app.state.player.queue = endserenading(1);
        let queue_ops = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let queue_contexts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        app.client = std::sync::Arc::new(crate::test_support::TestClient {
            queue_ops: std::sync::Arc::clone(&queue_ops),
            queue_contexts: std::sync::Arc::clone(&queue_contexts),
            ..crate::test_support::TestClient::default()
        });
        let album = album_with_songs("al1", 3);
        let want_reversed: Vec<(&str, String)> = album
            .songs
            .iter()
            .rev()
            .map(|s| ("insert_next", s.id.qualified()))
            .collect();
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
        assert_eq!(*ops, want_reversed, "倒序 insert_next 恰按专辑原序连播");
        let ctxs = queue_contexts
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("queue_contexts 锁中毒: {e}"))?;
        assert!(
            ctxs.iter().all(|(op, ctx)| *op == "insert_next"
                && *ctx
                    == QueueContextWire::Album {
                        id: album.id.clone(),
                        name: Some(album.name.clone()),
                    }),
            "插播语境逐曲归 Album 身份,非笼统 Manual"
        );
        Ok(())
    }

    /// InsertNext 容器(本地队列为空):插入点数学退化,改逐曲 `queue_append` 正序——
    /// 与 Append all 同款保序,不自动起播。
    #[test]
    fn container_play_next_on_empty_queue_appends_in_order() -> color_eyre::Result<()> {
        let (mut app, queue_ops) = app_with_library_probed(/*len*/ 1, /*sel_track*/ 0)?;
        assert!(app.state.player.queue.is_empty(), "前置:本地队列应为空");
        let album = album_with_songs("al1", 2);
        let want: Vec<(&str, String)> = album
            .songs
            .iter()
            .map(|s| ("append", s.id.qualified()))
            .collect();
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
        assert_eq!(*ops, want, "空队列退化为逐曲 append 正序");
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
            .map(|sv| sv.data.clone());
        let ctx = app.collect_key_context();
        assert_eq!(*ctx.view(), ViewKind::Tracks);
        let want_sel = app
            .state
            .filtered_tracks()
            .get(1)
            .map(|sv| sv.data.id.clone());
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

    /// channel 搜索布局态:脚本 ctx 报 Search(回归 bug④——曾漏 channel_search 分支,
    /// 在 channel 搜索里误报成看穿到的下层主视图)。
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

    /// 非 queue 浮层(确认框)对脚本 ctx 透明:看穿到下层布局层。channel 搜索态上叠确认框,
    /// ctx 仍报 Search(回归:active_layer 重构不得改这条透明语义)。
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
