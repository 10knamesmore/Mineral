//! Apply daemon business capabilities without changing local client preferences.

impl crate::app::App {
    /// Replace daemon operations; changed statistics availability invalidates decoration
    /// caches and re-queries the selected song when its source is still available.
    pub(crate) fn apply_service_info(&mut self, info: mineral_protocol::ServiceInfo) {
        let counts_changed = self.state.models.service_info.play_counts != info.play_counts;
        mineral_log::info!(
            target: "tui",
            queue_transforms = info.queue_transforms.len(),
            play_counts_enabled = info.play_counts.enabled,
            counts_changed,
            "daemon service capabilities updated"
        );
        self.state.models.service_info = info;
        if counts_changed {
            self.state.clear_local_play_counts();
            crate::runtime::prefetch::request_play_count(&mut self.state, &*self.client);
        }
    }
}
