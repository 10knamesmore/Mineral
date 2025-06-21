pub enum Method {
    Get,
    Post,
}

pub enum UserAgentType {
    Any,
    Custom(String),
    Mobile,
    PC,
}

pub enum SearchType {
    Song,
    Album,
    Artist,
    Playlist,
    User,
    MV,
    Lyrics,
    FM,
    Video,
}

pub enum BitRate {
    Low,
    Medium,
    High,
    SQ,
    HR,
}

impl From<BitRate> for &str {
    fn from(value: BitRate) -> Self {
        match value {
            BitRate::Low => "128000",
            BitRate::Medium => "192000",
            BitRate::High => "320000",
            BitRate::SQ => "999000",
            BitRate::HR => "1900000",
        }
    }
}

#[derive(Debug)]
pub struct SongUrl {
    pub id: u64,
    pub url: String,
    pub rate: u32,
}

impl From<SearchType> for String {
    fn from(value: SearchType) -> Self {
        let code = match value {
            SearchType::Song => 1,
            SearchType::Album => 10,
            SearchType::Artist => 100,
            SearchType::Playlist => 1000,
            SearchType::User => 1002,
            SearchType::MV => 1006,
            SearchType::Lyrics => 1006,
            SearchType::FM => 1009,
            SearchType::Video => 1014,
        };

        code.to_string()
    }
}
