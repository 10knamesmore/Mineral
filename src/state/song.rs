#[derive(Debug, Clone)]
pub struct Song {
    pub id: u64,
    pub name: String,
    pub artist: String,
    pub album: String,
    pub duration: u32, // 秒
}

pub trait SongList {
    fn get_song_list(&self) -> &[Song];
}
