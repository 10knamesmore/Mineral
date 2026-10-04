//! 应用事件与歌词组件滚动行为的接线。

use super::super::AppState;
use crate::runtime::action::ScrollStep;

impl AppState {
    /// 全屏页面将滚动意图交给歌词组件。
    pub(crate) fn scroll_lyrics(&mut self, scroll: ScrollStep) {
        if !self.ui.browse.fullscreen.on() {
            return;
        }
        let input = crate::view::lyrics::input(&self.models.playback, &self.models.library);
        self.ui
            .browse
            .lyrics
            .scroll_lyrics(scroll, input, &self.cfg);
    }

    /// 主循环推进歌词组件自己的滚动生命周期。
    pub(crate) fn tick_lyric_scroll(&mut self) {
        let input = crate::view::lyrics::input(&self.models.playback, &self.models.library);
        self.ui.browse.lyrics.tick_lyric_scroll(input, &self.cfg);
    }

    /// 当前手动浏览的焦点行。
    pub(crate) fn manual_lyric_focus_line(&self) -> Option<usize> {
        self.ui.browse.lyrics.manual_lyric_focus_line()
    }

    /// 按焦点行生成需要发送的 seek 位置。
    pub(crate) fn lyric_focus_seek_target(&self) -> Option<u64> {
        self.ui
            .browse
            .lyrics
            .lyric_focus_seek_target(crate::view::lyrics::input(
                &self.models.playback,
                &self.models.library,
            ))
    }

    /// seek 已发出，组件等待对应播放进度到达。
    pub(crate) fn hold_lyric_anchor_for_seek(&mut self, line: usize) {
        self.ui.browse.lyrics.hold_lyric_anchor_for_seek(line);
    }
}
