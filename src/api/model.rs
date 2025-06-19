pub enum Method {
    Get,
    Post,
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
