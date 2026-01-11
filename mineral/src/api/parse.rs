use std::{fs::File, io::Write};

use anyhow::{anyhow, Context, Ok, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    api::model::{LoginInfo, LoginQrCode, Message, SongUrl},
    app::{Album, PlayList, Song},
};

trait DeVal<'a>: Sized {
    fn dval(v: &'a Value) -> Result<Self>;
}

impl DeVal<'_> for bool {
    fn dval(v: &Value) -> Result<Self> {
        Ok(Self::deserialize(v)?)
    }
}

impl DeVal<'_> for i64 {
    fn dval(v: &Value) -> Result<Self> {
        Ok(Self::deserialize(v)?)
    }
}

impl DeVal<'_> for u64 {
    fn dval(v: &Value) -> Result<Self> {
        Ok(Self::deserialize(v)?)
    }
}

impl DeVal<'_> for i32 {
    fn dval(v: &Value) -> Result<Self> {
        Ok(Self::deserialize(v)?)
    }
}

impl DeVal<'_> for u32 {
    fn dval(v: &Value) -> Result<Self> {
        Ok(Self::deserialize(v)?)
    }
}

impl DeVal<'_> for String {
    fn dval(v: &Value) -> Result<Self> {
        Ok(Self::deserialize(v)?)
    }
}

impl<'a> DeVal<'a> for &'a Vec<Value> {
    fn dval(v: &'a Value) -> Result<Self> {
        match v {
            Value::Array(v) => Ok(v),
            _ => Err(anyhow!("json not a array")),
        }
    }
}

impl<'a> DeVal<'a> for &'a Value {
    fn dval(v: &'a Value) -> Result<Self> {
        Ok(v)
    }
}

macro_rules! json_val {
    (@as $t:ty, $v:expr, $($n:expr),+) => {
        get_val_chain::<$t>($v, &[$($n),+]).context(format!("at {}:{}", file!(), line!()))
    };
    ($v:expr, $($n:expr),+) => {
        get_val_chain($v, &[$($n),+]).context(format!("at {}:{}", file!(), line!()))
    };
}

fn get_val_chain<'a, T>(value: &'a Value, path: &[&str]) -> Result<T>
where
    T: DeVal<'a>,
{
    let mut current = value;
    let mut full_path = String::new();

    for key in path {
        if !full_path.is_empty() {
            full_path.push('.');
        }
        full_path.push_str(key);

        current = current
            .get(key)
            .with_context(|| format!("missing key '{}'", full_path))?;
    }

    T::dval(current)
}

pub(super) fn parse_song_search(json: String) -> anyhow::Result<Vec<Song>> {
    let value: Value = serde_json::from_str(&json)?;

    let code: i64 = json_val!(&value, "code")?;

    if code == 200 {
        let songs_value: &Vec<Value> = json_val!(&value, "result", "songs")?;

        let songs: Vec<Song> = songs_value
            .iter()
            .map(|song_value| -> Result<Song> {
                let (artist_id, artist_name): (u64, String) = {
                    let artists: &Vec<Value> = json_val!(song_value, "artists")?;

                    if let Some(artist_value) = artists.first() {
                        let id = json_val!(@as u64, artist_value, "id")?;
                        let name = json_val!(@as String, artist_value, "name")?;
                        (id, name)
                    } else {
                        // 默认值
                        (0, "未知Artist".to_string())
                    }
                };

                let (album_id, album_name, pic_url): (u64, String, String) = {
                    let album_value: &Value = json_val!(song_value, "album")?;

                    (
                        json_val!(album_value, "id")?,
                        json_val!(album_value, "name").unwrap_or(String::from("未知专辑名")),
                        json_val!(album_value, "picUrl").unwrap_or_default(),
                    )
                };

                Ok(Song {
                    id: json_val!(song_value, "id")?,
                    name: json_val!(song_value, "name")?,
                    // HACK: 对多名artists的支持
                    artist_id,
                    artist_name,
                    album_id,
                    album_name,
                    pic_url,
                    song_url: String::new(),
                    local_path: None,
                    duration: json_val!(song_value, "duration")?,
                })
            })
            .collect::<Result<_, _>>()?;

        Ok(songs)
    } else {
        Err(anyhow!("api code 没有返回200"))
    }
}

#[allow(unused)]
pub(super) fn parse_album_search(json: String) -> anyhow::Result<Vec<Album>> {
    let value: Value = serde_json::from_str(&json)?;

    let code: i64 = json_val!(&value, "code")?;

    if code == 200 {
        let albums_value: &Vec<Value> = json_val!(&value, "result", "albums")?;

        let albums: Vec<Album> = albums_value
            .iter()
            .map(|album_value| -> Result<Album> {
                let (artist_id, artist_name): (u64, String) = {
                    let artist_value: &Value = json_val!(album_value, "artist")?;

                    (
                        json_val!(artist_value, "id").unwrap_or_default(),
                        json_val!(artist_value, "name").unwrap_or(String::from("未知Artist")),
                    )
                };

                Ok(Album {
                    id: json_val!(album_value, "id")?,
                    name: json_val!(album_value, "name")?,
                    artist_id,
                    artist_name,
                    description: json_val!(album_value, "description").unwrap_or_default(),
                    publish_time: json_val!(album_value, "publishTime").unwrap_or_default(),
                    pic_url: json_val!(album_value, "picUrl").unwrap_or_default(),
                    songs: Vec::new(),
                })
            })
            .collect::<Result<_, _>>()?;

        Ok(albums)
    } else {
        Err(anyhow!("api code 没有返回200"))
    }
}

#[allow(unused)]
pub(super) fn parse_playlist_search(json: String) -> anyhow::Result<Vec<PlayList>> {
    let value: Value = serde_json::from_str(&json)?;

    let code: i64 = json_val!(&value, "code")?;

    if code == 200 {
        let playlists_value: &Vec<Value> = json_val!(&value, "result", "playlists")?;

        let playlists: Vec<PlayList> = playlists_value
            .iter()
            .map(|playlist_value| -> Result<PlayList> {
                Ok(PlayList {
                    local: false,
                    id: json_val!(playlist_value, "id")?,
                    name: json_val!(playlist_value, "name").unwrap_or("未知歌单名".to_string()),
                    img_url: json_val!(playlist_value, "coverImgUrl").unwrap_or_default(),
                    track_count: json_val!(playlist_value, "trackCount")?,
                    songs: Vec::new(),
                    description: json_val!(playlist_value, "description").unwrap_or_default(),
                })
            })
            .collect::<Result<_, _>>()?;

        Ok(playlists)
    } else {
        Err(anyhow!("api code 没有返回200"))
    }
}

pub(super) fn parse_songs_in_album(json: String) -> anyhow::Result<Vec<Song>> {
    let value: Value = serde_json::from_str(&json)?;

    let code: i64 = json_val!(&value, "code")?;

    if code == 200 {
        let songs_value: &Vec<Value> = json_val!(&value, "songs")?;

        let songs: Vec<Song> = songs_value
            .iter()
            .map(|song_value| -> Result<Song> {
                let (artist_id, artist_name): (u64, String) = {
                    let artists_value: &Vec<Value> = json_val!(song_value, "ar")?;
                    if let Some(artist_value) = artists_value.first() {
                        (
                            json_val!(artist_value, "id").unwrap_or_default(),
                            json_val!(artist_value, "name").unwrap_or(String::from("未知artist")),
                        )
                    } else {
                        (0, String::new())
                    }
                };

                let (album_id, album_name, pic_url): (u64, String, String) = {
                    let album_value: &Value = json_val!(song_value, "al")?;

                    (
                        json_val!(album_value, "id").unwrap_or_default(),
                        json_val!(album_value, "name").unwrap_or(String::from("未知专辑名")),
                        json_val!(album_value, "picUrl").unwrap_or_default(),
                    )
                };

                Ok(Song {
                    id: json_val!(song_value, "id")?,
                    name: json_val!(song_value, "name").unwrap_or(String::from("未知歌名")),
                    artist_id,
                    artist_name,
                    album_id,
                    album_name,
                    pic_url,
                    song_url: String::new(),
                    local_path: None,
                    duration: json_val!(song_value, "dt")?,
                })
            })
            .collect::<Result<_, _>>()?;

        Ok(songs)
    } else {
        Err(anyhow!("api code 没有返回200"))
    }
}

pub(super) fn parse_song_urls(json: String) -> anyhow::Result<Vec<SongUrl>> {
    let value: Value = serde_json::from_str(&json)?;

    let code: i64 = json_val!(&value, "code")?;

    if code == 200 {
        let songs_value: &Vec<Value> = json_val!(&value, "data")?;

        let song_urls: Vec<SongUrl> = songs_value
            .iter()
            .map(|song_value| -> Result<SongUrl> {
                Ok(SongUrl {
                    id: json_val!(song_value, "id")?,
                    url: json_val!(song_value, "url")?,
                    rate: json_val!(song_value, "br")?,
                })
            })
            .collect::<Result<_, _>>()?;

        Ok(song_urls)
    } else {
        Err(anyhow!("api code 没有返回200"))
    }
}

pub(super) fn parse_login_info(json: String) -> anyhow::Result<LoginInfo> {
    dbg!(json);
    // let mut file = File::create("login_info.json").unwrap();
    // file.write_all(json.as_bytes()).unwrap();
    todo!()
}

pub(super) fn to_captcha(json: String) -> anyhow::Result<Message> {
    let value: Value = serde_json::from_str(&json)?;
    let code: i64 = json_val!(&value, "code")?;

    if code == 200 {
        Ok(Message { msg: None, code })
    } else {
        let msg: String = json_val!(&value, "message")?;
        Ok(Message {
            msg: Some(msg),
            code,
        })
    }
}

macro_rules! file_write {
    ($file_name : expr,$content : expr) => {
        let mut file = File::create($file_name).unwrap();
        file.write_all($content.as_bytes()).unwrap();
    };
}
pub(super) fn to_login_qr(json: String) -> anyhow::Result<LoginQrCode> {
    file_write!("login_qr.json", json);
    todo!()
}
