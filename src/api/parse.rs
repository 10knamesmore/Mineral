use anyhow::{anyhow, Context, Ok, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::app::Song;

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

pub(super) fn parse_song_info(json: String) -> anyhow::Result<Vec<Song>> {
    let value: Value = serde_json::from_str(&json)?;

    let code: i64 = json_val!(&value, "code")?;

    if code == 200 {
        let songs_value: &Vec<Value> = json_val!(&value, "result", "songs")?;

        let songs: Vec<Song> = songs_value
            .iter()
            .map(|song_value| -> Result<Song> {
                let (artist_id, artist_name): (u64, String) = {
                    let artists: &Vec<Value> = json_val!(@as &Vec<Value>, song_value, "artists")?;

                    if let Some(artist_value) = artists.first() {
                        let id = json_val!(@as u64, artist_value, "id")?;
                        let name = json_val!(@as String, artist_value, "name")?;
                        (id, name)
                    } else {
                        // 默认值
                        (0, "未知".to_string())
                    }
                };

                let (album_id, album_name, pic_url): (u64, String, String) = {
                    let album_value: &Value = json_val!(song_value, "album")?;

                    (
                        json_val!(album_value, "id")?,
                        json_val!(album_value, "name").unwrap_or_default(),
                        json_val!(album_value, "picUrl").unwrap_or_default(),
                    )
                };

                Ok(Song {
                    id: json_val!(song_value, "id")?,
                    name: json_val!(song_value, "name")?,
                    // HACK: 对多名artists的支持
                    artist_name,
                    artist_id,
                    album_name,
                    album_id,
                    pic_url,
                    song_url: String::new(),
                    duration: json_val!(song_value, "duration")?,
                })
            })
            .collect::<Result<_, _>>()?;

        Ok(songs)
    } else {
        Err(anyhow!("api code 没有返回200"))
    }
}
