#[derive(Debug, Clone)]
pub struct Song {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: u32, // 秒
}
