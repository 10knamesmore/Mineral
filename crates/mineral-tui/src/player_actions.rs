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
        if let Some(err) = self.client.invoke_action(&name) {
            use crate::components::toast::notifications::{TextTint, tinted_text_item};
            self.notifications
                .flash(tinted_text_item(err, TextTint::Error));
        }
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
        if self.state.fullscreen || !matches!(self.state.view, View::Library) {
            return;
        }
        let filtered = self.state.filtered_tracks();
        if let Some(song) = filtered.get(self.state.sel_track).map(|sv| sv.data.clone()) {
            // 触发持久化(daemon 写本地 + 远端);in-proc fire-and-forget。
            self.client.toggle_love(song.id.clone());
            // 乐观翻转:♥ 立即变,不等 server 确认。
            self.state.toggle_loved_local(&song);
        }
    }

    /// 下载当前视图选中项:Playlists 整张歌单 / Library 单曲。全屏态屏蔽。
    pub(crate) fn download_selection(&mut self) {
        if self.state.fullscreen {
            return;
        }
        match self.state.view {
            View::Playlists => {
                let id = self
                    .state
                    .filtered_playlists()
                    .get(self.state.sel_playlist)
                    .map(|p| p.data.id.clone());
                if let Some(id) = id {
                    self.client.download(DownloadTarget::Playlist(id));
                }
            }
            View::Library => {
                let song = self
                    .state
                    .filtered_tracks()
                    .get(self.state.sel_track)
                    .map(|sv| sv.data.clone());
                if let Some(song) = song {
                    self.client.download(DownloadTarget::Song(Box::new(song)));
                }
            }
        }
    }
}
