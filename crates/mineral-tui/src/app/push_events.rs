//! 分流 server 主动推送，并把数据与生命周期事件收敛进应用状态。

use super::App;

impl App {
    /// 取走 server 主动推送的 event 缓冲并逐条消费。
    ///
    /// 数据事件进入 [`crate::runtime::state::AppState`]；生命周期事件维护对应 cache；
    /// 视觉通知交给 toast 层。
    pub(super) fn drain_push_events(&mut self) {
        for event in self.client.drain_events() {
            match event {
                mineral_protocol::Event::Task(task) => {
                    self.state.apply(&task);
                    // apply 落数据后，容器播放意图在此兑现；入队需要 client，state 碰不到。
                    self.fulfill_pending_container(&task);
                }
                mineral_protocol::Event::ScriptReloaded => self.refresh_script_binds(),
                mineral_protocol::Event::ConfigChanged { config } => {
                    self.apply_pushed_config(config);
                }
                mineral_protocol::Event::WindowTitleOverride { text } => {
                    self.state.window_title_override = text;
                }
                mineral_protocol::Event::TrackFinished { song_id, reason } => {
                    self.state.apply_track_finished(&song_id, reason);
                }
                other => {
                    crate::components::toast::push::apply_event(&mut self.notifications, other);
                }
            }
        }
    }
}
