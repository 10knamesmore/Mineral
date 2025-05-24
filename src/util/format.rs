pub(crate) fn format_duration(secs: u32) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}
