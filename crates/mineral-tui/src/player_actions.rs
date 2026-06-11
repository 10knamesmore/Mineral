//! 「转 Client」的领域动作执行器:播放控制 / love / 下载 / 脚本动作。
//!
//! 从 app 模块拆出的 `App` 方法集(单文件体量约束);执行点仍是
//! `App::dispatch`,这里只放函数体。

use mineral_protocol::DownloadTarget;

use crate::app::App;
use crate::runtime::action::ScriptSlot;
use crate::runtime::state::View;

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
    /// view 判定优先级对齐 `handle_key` 的吃键顺序:队列浮层 > 搜索态 > 全屏 >
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
        let search_query = if self.state.search.query.is_empty() {
            None
        } else {
            Some(self.state.search.query.clone())
        };
        // 选中歌 + 其 ♥ 态:队列浮层取光标条目(♥ 查 liked_ids 缓存),
        // Library 列表取选中行(SongView 已装饰)。
        let (view, selected_song, selected_loved) =
            if let Some(cursor) = self.overlays.active_queue_cursor() {
                let song = self.state.player.queue.get(cursor).cloned();
                let loved = song.as_ref().map(|s| {
                    self.state
                        .library
                        .liked_ids
                        .get(&s.id.namespace())
                        .is_some_and(|ids| ids.contains(&s.id))
                });
                (ViewKind::Queue, song, loved)
            } else if self.state.search.typing {
                (ViewKind::Search, None, None)
            } else if self.state.fullscreen.on() {
                (ViewKind::Fullscreen, None, None)
            } else {
                match self.state.view {
                    View::Playlists => (ViewKind::Playlists, None, None),
                    View::Library => {
                        let sel = self
                            .state
                            .filtered_tracks()
                            .into_iter()
                            .nth(self.state.nav.sel_track);
                        let loved = sel.as_ref().map(|sv| sv.loved);
                        (ViewKind::Tracks, sel.map(|sv| sv.data), loved)
                    }
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

    /// 相对当前位置跳 `delta_s` 秒,clamp 到 [0, duration]。
    pub(crate) fn seek_relative(&mut self, delta_s: i64) {
        let dur_ms = self.state.playback.duration_ms();
        if dur_ms == 0 {
            return;
        }
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
        if self.state.fullscreen.on() || !matches!(self.state.view, View::Library) {
            return;
        }
        let filtered = self.state.filtered_tracks();
        if let Some(song) = filtered
            .get(self.state.nav.sel_track)
            .map(|sv| sv.data.clone())
        {
            // 触发持久化(daemon 写本地 + 远端);in-proc fire-and-forget。
            self.client.toggle_love(song.id.clone());
            // 乐观翻转:♥ 立即变,不等 server 确认。
            self.state.toggle_loved_local(&song);
        }
    }

    /// 下载当前视图选中项:Playlists 整张歌单 / Library 单曲。全屏态屏蔽。
    pub(crate) fn download_selection(&mut self) {
        if self.state.fullscreen.on() {
            return;
        }
        match self.state.view {
            View::Playlists => {
                let id = self
                    .state
                    .filtered_playlists()
                    .get(self.state.nav.sel_playlist)
                    .map(|p| p.data.id.clone());
                if let Some(id) = id {
                    self.client.download(DownloadTarget::Playlist(id));
                }
            }
            View::Library => {
                let song = self
                    .state
                    .filtered_tracks()
                    .get(self.state.nav.sel_track)
                    .map(|sv| sv.data.clone());
                if let Some(song) = song {
                    self.client.download(DownloadTarget::Song(Box::new(song)));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use mineral_protocol::ViewKind;

    use crate::test_support::{app_with_library, app_with_queue};

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
        app.state.view = crate::runtime::state::View::Playlists;
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
        app.state.fullscreen.set(true);
        let ctx = app.collect_key_context();
        assert_eq!(*ctx.view(), ViewKind::Fullscreen);
        assert!(ctx.selected_song().is_none());
        assert_eq!(
            ctx.now_playing().as_ref().map(|s| s.id.clone()),
            app.state.player.current.as_ref().map(|s| s.id.clone())
        );
        Ok(())
    }
}
