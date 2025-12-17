use crate::{state::HasId, util::format::format_duration};
use std::{fmt::Debug, path::PathBuf};

use lofty::{
    file::{AudioFile, TaggedFileExt},
    tag::Accessor,
};
use ratatui::{
    style::{Color, Style, Stylize},
    text::{Line, Text},
    widgets::{Cell, Row},
};

#[derive(Debug, Clone)]
pub struct Song {
    pub id: u64,

    pub name: String,

    // TODO: 对多名artists的支持
    pub artist_id: u64,
    pub artist_name: String,

    pub album_id: u64,
    pub album_name: String,

    pub pic_url: String,

    pub song_url: String,

    pub local_path: Option<String>,

    pub duration: u64, // 毫秒
}

impl HasId for Song {
    fn id(&self) -> u64 {
        self.id
    }
}

impl<'a> From<&'a Song> for Row<'a> {
    fn from(song: &'a Song) -> Self {
        let text_style = Style::default().fg(Color::DarkGray).bold();

        let name_block = Text::from(vec![Line::styled(&song.name, text_style)]);

        let artist_block = Text::from(vec![Line::styled(&song.name, text_style)]);

        let album_block = Text::from(vec![Line::styled(&song.album_name, text_style)]);

        let duration_block = Text::from(vec![Line::styled(
            format_duration(song.duration),
            text_style,
        )]);

        Row::new(vec![
            Cell::from(name_block),
            Cell::from(artist_block),
            Cell::from(album_block),
            Cell::from(duration_block),
        ])
    }
}

impl Song {
    pub fn from_path(path: &PathBuf) -> anyhow::Result<Song> {
        use crate::util::fs;

        let name = fs::name_from_path(path);
        let id = fs::hash_path(path);

        let (artist_name, album_name, duration): (String, String, u64) = {
            match lofty::read_from_path(path) {
                Ok(tagged_file) => {
                    // 尝试获取 primary_tag 或 fallback 到 first_tag，若都没则返回 None
                    let tag_opt = tagged_file
                        .primary_tag()
                        .or_else(|| tagged_file.first_tag());

                    if let Some(tag) = tag_opt {
                        let artist_name = tag
                            .artist()
                            .as_deref()
                            .unwrap_or_else(|| {
                                tracing::info!("文件 {:?} 标签中没有 artist", path);
                                "unknown"
                            })
                            .to_string();

                        let album_name = tag
                            .album()
                            .as_deref()
                            .unwrap_or_else(|| {
                                tracing::info!("文件 {:?} 标签中没有 album", path);
                                "unknown"
                            })
                            .to_string();

                        let duration = tagged_file.properties().duration().as_millis() as u64;

                        (artist_name, album_name, duration)
                    } else {
                        tracing::warn!("文件 {:?} 没有任何标签", path);
                        (String::default(), String::default(), 0)
                    }
                }
                Err(e) => {
                    tracing::warn!("获取 {:?} 标签时出错: {:?}", path, e);
                    (String::default(), String::default(), 0)
                }
            }
        };

        Ok(Song {
            id,
            name,
            artist_id: 0,
            artist_name,
            album_id: 0,
            album_name,
            pic_url: String::default(),
            song_url: String::default(),
            local_path: Some(path.to_string_lossy().to_string()),
            duration,
        })
    }
}
