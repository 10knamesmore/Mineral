use ratatui::{
    layout::{Alignment, Constraint},
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Cell, Row, Table},
};

use crate::{
    app::{Album, Artist, ImageState, PlayList, RenderCache},
    state::{HasDescription, HasId, Song},
    util::format::format_duration,
};

use super::{Selectable, SongList};

#[derive(Debug)]
pub(crate) struct MainPageState {
    pub(crate) now_state: MainPageSubState,
    playlist_state: TabList<PlayList>,
    album_state: TabList<Album>,
    artist_state: TabList<Artist>,
}

#[derive(Debug)]
pub(crate) struct TabList<T> {
    items: Vec<T>,
    selected_idx: Option<usize>,
    selected_id: Option<u64>,
}

impl<T> TabList<T> {
    pub(crate) fn new(items: Vec<T>) -> Self
    where
        T: HasId,
    {
        Self {
            selected_idx: if items.is_empty() { None } else { Some(0) },
            selected_id: if items.is_empty() {
                None
            } else {
                Some(items.first().unwrap().id())
            },
            items,
        }
    }

    fn extend(&mut self, items: Vec<T>) {
        self.items.extend(items);
    }

    pub(crate) fn to_rows<'a>(&'a self) -> Vec<Row<'a>>
    where
        Row<'a>: From<&'a T>, // 这里声明 Row 可以从 &T 转换
    {
        self.items.iter().map(Row::from).collect()
    }

    pub fn selected_id(&self) -> Option<u64> {
        self.selected_id
    }
}

impl SongList for TabList<Song> {
    fn songs(&self) -> &[Song] {
        &self.items
    }
}

impl<T> Selectable for TabList<T>
where
    T: HasId,
{
    type Item = T;

    fn items(&self) -> &[Self::Item] {
        &self.items
    }

    fn selected_index(&self) -> Option<usize> {
        self.selected_idx
    }

    fn _select(&mut self, index: usize) {
        self.selected_idx = Some(index);
        self.selected_id = Some(self.items.get(index).unwrap().id());
    }
}

#[derive(Default, Debug)]
pub(crate) enum MainPageTab {
    #[default]
    PlayList,
    FavoriteAlbum,
    FavoriteArtist,
}

#[derive(Debug)]
pub(crate) enum MainPageSubState {
    TabView(MainPageTab),
    ViewingPlayList(TabList<Song>),
    ViewingAlbum(TabList<Song>),
    ViewingArtist(TabList<Song>),
}

impl Default for MainPageSubState {
    fn default() -> Self {
        Self::TabView(MainPageTab::default())
    }
}

impl MainPageState {
    pub(crate) fn update_playlist<T>(&mut self, playlists: T)
    where
        T: Into<Vec<PlayList>>,
    {
        self.playlist_state.extend(playlists.into());
    }

    pub(crate) fn update_album<T>(&mut self, albums: T)
    where
        T: Into<Vec<Album>>,
    {
        self.album_state.extend(albums.into());
    }

    pub(crate) fn update_artist<T>(&mut self, artists: T)
    where
        T: Into<Vec<Artist>>,
    {
        self.artist_state.extend(artists.into());
    }

    // 当now_state的selected_idx为None的时候, 会返回NotRequested
    pub(crate) fn now_cover<'a>(&self, cache: &'a mut RenderCache) -> &'a ImageState {
        match &self.now_state {
            MainPageSubState::TabView(main_page_tab) => match main_page_tab {
                MainPageTab::PlayList => match self.playlist_state.selected_idx {
                    Some(_) => {
                        // 如果selected_idx存在, 那么应当保证selected_id也存在
                        // HACK: 粗暴判断如果浏览本地playlist就不渲染图片
                        if self.playlist_state.selected_item().unwrap().local {
                            cache.not_requested()
                        } else {
                            let id = self.playlist_state.selected_id.unwrap();
                            cache.playlist_cover(id)
                        }
                    }
                    None => cache.not_requested(),
                },
                MainPageTab::FavoriteAlbum => match self.album_state.selected_idx {
                    Some(_) => {
                        // 如果selected_idx存在, 那么应当保证selected_id也存在
                        let id = self.album_state.selected_id.unwrap();
                        cache.album_cover(id)
                    }
                    None => cache.not_requested(),
                },
                MainPageTab::FavoriteArtist => match self.artist_state.selected_idx {
                    Some(_) => {
                        // 如果selected_idx存在, 那么应当保证selected_id也存在
                        let id = self.artist_state.selected_id.unwrap();
                        cache.artist_cover(id)
                    }
                    None => cache.not_requested(),
                },
            },
            MainPageSubState::ViewingPlayList(_) => {
                // HACK:
                if self.playlist_state.selected_item().unwrap().local {
                    cache.not_requested()
                } else {
                    let id = self.playlist_state.selected_id.unwrap();
                    cache.playlist_cover(id)
                }
            }
            MainPageSubState::ViewingAlbum(_) => {
                let id = self.album_state.selected_id.unwrap();
                cache.album_cover(id)
            }
            MainPageSubState::ViewingArtist(_) => {
                let id = self.artist_state.selected_id.unwrap();
                cache.artist_cover(id)
            }
        }
    }

    pub(crate) fn new(playlists: Vec<PlayList>, albums: Vec<Album>, artists: Vec<Artist>) -> Self {
        Self {
            now_state: MainPageSubState::default(),
            playlist_state: TabList::new(playlists),
            album_state: TabList::new(albums),
            artist_state: TabList::new(artists),
        }
    }

    /// 根据目前的 tab 以及对应的 selected index 返回SongList
    pub(crate) fn song_list(&self) -> Option<&dyn SongList> {
        match &self.now_state {
            MainPageSubState::TabView(main_page_tab) => match main_page_tab {
                MainPageTab::PlayList => {
                    let index = self.playlist_state.selected_idx?;
                    Some(&self.playlist_state.items[index])
                }
                MainPageTab::FavoriteAlbum => {
                    let index = self.album_state.selected_idx?;
                    Some(&self.album_state.items[index])
                }
                MainPageTab::FavoriteArtist => {
                    let index = self.artist_state.selected_idx?;
                    Some(&self.artist_state.items[index])
                }
            },
            MainPageSubState::ViewingPlayList(play_list) => Some(play_list),
            MainPageSubState::ViewingAlbum(album) => Some(album),
            MainPageSubState::ViewingArtist(artist) => Some(artist),
        }
    }

    /// 根据目前的 tab 返回对应的 列表(playlist,album,artist)
    pub(crate) fn all_song_lists(&self) -> Option<Vec<&dyn SongList>> {
        match &self.now_state {
            MainPageSubState::TabView(main_page_tab) => match main_page_tab {
                MainPageTab::PlayList => Some(
                    self.playlist_state
                        .items
                        .iter()
                        .map(|a| a as &dyn SongList)
                        .collect(),
                ),
                MainPageTab::FavoriteAlbum => Some(
                    self.album_state
                        .items
                        .iter()
                        .map(|a| a as &dyn SongList)
                        .collect(),
                ),
                MainPageTab::FavoriteArtist => Some(
                    self.artist_state
                        .items
                        .iter()
                        .map(|a| a as &dyn SongList)
                        .collect(),
                ),
            },
            _ => None, // NOTE:  比如当处于浏览一个具体的Playlist的时候,
                       // 你不应该去获取当前的所有列表
        }
    }

    /// # 根据当前的state,返回对应的Rows
    ///  - 比如现在在浏览所有的PlayList, 就返回所有的playlist组成的row
    ///  - 比如现在在浏览某一个playlist, 就返回当前playlist里面的所有song组成的row
    pub(crate) fn now_tab_items(&self) -> Vec<Row> {
        match &self.now_state {
            MainPageSubState::TabView(main_page_tab) => match main_page_tab {
                MainPageTab::PlayList => self.playlist_state.to_rows(),
                MainPageTab::FavoriteAlbum => self.album_state.to_rows(),
                MainPageTab::FavoriteArtist => self.artist_state.to_rows(),
            },
            MainPageSubState::ViewingPlayList(play_list) => play_list.to_rows(),
            MainPageSubState::ViewingAlbum(album) => album.to_rows(),
            MainPageSubState::ViewingArtist(artist) => artist.to_rows(),
        }
    }

    /// 根据目前的 Tab 返回对应的详情页应有的内容
    ///
    /// 比如当前在 Tab 为PlayList, 就根据目前的 Selected, 返回选中的 PlayList
    /// 里面歌曲组成的列表摘要
    /// 如果是Playlist或Album等,就会返回None,因为在那些情况不应该使用这个函数
    pub(crate) fn selected_detail(&self) -> Option<Table> {
        match &self.now_state {
            MainPageSubState::TabView(main_page_tab) => match main_page_tab {
                MainPageTab::PlayList => match self.playlist_state.selected_idx {
                    Some(index) => {
                        let selected_list = self
                            .playlist_state
                            .selected_item()
                            .unwrap_or_else(|| panic!("程序内部错误! 对于当前Playlist,想获取idx: {} 的 detail, but selected index out of bounds",index));
                        tracing::debug!("{:?}", selected_list);
                        Some(Self::detail_from_songlist(selected_list))
                    }
                    None => None,
                },
                MainPageTab::FavoriteAlbum => match self.album_state.selected_idx {
                    Some(index) => {
                        let selected_list = self
                                    .album_state
                                    .selected_item()
                                    .unwrap_or_else(|| panic!("程序内部错误! 对于当前Album,想获取idx: {} 的 detail ,but selected index out of bounds",index));
                        Some(Self::detail_from_songlist(selected_list))
                    }
                    None => None,
                },
                MainPageTab::FavoriteArtist => match self.artist_state.selected_idx {
                    Some(index) => {
                        let selected_list = self
                                    .artist_state
                                    .selected_item()
                                    .unwrap_or_else(|| panic!("程序内部错误! 对于当前Album,想获取idx: {} 的 detail ,but selected index out of bounds",index));
                        Some(Self::detail_from_songlist(selected_list))
                    }
                    None => None,
                },
            },
            MainPageSubState::ViewingPlayList(_) => Some(Self::detail_from_introuction(
                self.playlist_state.selected_item()?,
            )),
            MainPageSubState::ViewingAlbum(_) => Some(Self::detail_from_introuction(
                self.album_state.selected_item()?,
            )),
            MainPageSubState::ViewingArtist(_) => Some(Self::detail_from_introuction(
                self.artist_state.selected_item()?,
            )),
        }
    }

    pub(crate) fn nav_forward(&mut self) {
        if let MainPageSubState::TabView(main_page_tab) = &self.now_state {
            match main_page_tab {
                MainPageTab::PlayList => {
                    // HACK: 直接clone太不优雅了, 以后要重构
                    let Some(item) = self.playlist_state.selected_item() else {
                        return;
                    };
                    let playlist = TabList::new(item.songs.clone());
                    self.now_state = MainPageSubState::ViewingPlayList(playlist);
                }
                MainPageTab::FavoriteAlbum => {
                    let Some(item) = self.album_state.selected_item() else {
                        return;
                    };
                    let album = TabList::new(item.songs.clone());
                    self.now_state = MainPageSubState::ViewingAlbum(album);
                }
                MainPageTab::FavoriteArtist => {
                    let Some(item) = self.artist_state.selected_item() else {
                        return;
                    };
                    let artist = TabList::new(item.songs.clone());
                    self.now_state = MainPageSubState::ViewingArtist(artist);
                }
            }
        }
    }

    pub(crate) fn nav_backward(&mut self) {
        match &self.now_state {
            MainPageSubState::TabView(_) => {}
            MainPageSubState::ViewingPlayList(_) => {
                self.now_state = MainPageSubState::TabView(MainPageTab::PlayList);
            }
            MainPageSubState::ViewingAlbum(_) => {
                self.now_state = MainPageSubState::TabView(MainPageTab::FavoriteAlbum);
            }
            MainPageSubState::ViewingArtist(_) => {
                self.now_state = MainPageSubState::TabView(MainPageTab::FavoriteArtist)
            }
        }
    }

    pub fn play(&mut self, song: &Song) {
        match &song.local_path {
            Some(path) => todo!(),
            None => todo!("远端api的歌曲"),
        }
    }

    // 解析传入的 SongList, 根据其内部信息返回对应组成的Rows
    fn detail_from_songlist<T>(songlist: &T) -> Table<'_>
    where
        T: SongList,
    {
        let songs_cell: Vec<Row> = songlist
            .songs()
            .iter()
            .enumerate()
            .map(|(i, song)| {
                let index = Text::from(
                    Line::from(Span::styled(
                        format!("{:>2}", i + 1),
                        Style::default().bold(),
                    ))
                    .alignment(Alignment::Left),
                );

                let song_name = Text::from(
                    Line::from(Span::styled(
                        &song.name,
                        Style::default().fg(Color::LightBlue),
                    ))
                    .alignment(Alignment::Center),
                );

                let duration = Text::from(
                    Line::from(Span::styled(
                        format_duration(song.duration),
                        Style::default().fg(Color::Gray),
                    ))
                    .alignment(Alignment::Right),
                );

                Row::new(vec![
                    Cell::from(index),
                    Cell::from(song_name).style(Style::default()),
                    Cell::from(duration).style(Style::default()),
                ])
            })
            .collect();

        let table = Table::default()
            .rows(songs_cell)
            .block(
                Block::default()
                    .title(" 详情 ")
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .widths(vec![
                Constraint::Percentage(20),
                Constraint::Percentage(40),
                Constraint::Percentage(40),
            ]);
        table
    }

    fn detail_from_introuction<T>(intro: &T) -> Table
    where
        T: HasDescription,
    {
        // HACK: 修改具体样式
        let desc = intro.description();
        let cell = vec![Cell::new(desc).style(Style::new())];
        let row = vec![Row::new(cell)];

        Table::default().rows(row)
    }

    pub(crate) fn playlist_state(&self) -> &TabList<PlayList> {
        &self.playlist_state
    }

    pub(crate) fn album_state(&self) -> &TabList<Album> {
        &self.album_state
    }

    pub(crate) fn artist_state(&self) -> &TabList<Artist> {
        &self.artist_state
    }

    pub(crate) fn now_tab_move_up_by(&mut self, n: usize) {
        match &mut self.now_state {
            MainPageSubState::TabView(main_page_tab) => match main_page_tab {
                MainPageTab::PlayList => self.playlist_state.move_up_by(n),
                MainPageTab::FavoriteAlbum => self.album_state.move_up_by(n),
                MainPageTab::FavoriteArtist => self.artist_state.move_up_by(n),
            },
            MainPageSubState::ViewingPlayList(play_list) => play_list.move_up_by(n),
            MainPageSubState::ViewingAlbum(album) => album.move_up_by(n),
            MainPageSubState::ViewingArtist(artist) => artist.move_up_by(n),
        }
    }

    pub(crate) fn now_tab_move_down_by(&mut self, n: usize) {
        match &mut self.now_state {
            MainPageSubState::TabView(main_page_tab) => match main_page_tab {
                MainPageTab::PlayList => self.playlist_state.move_down_by(n),
                MainPageTab::FavoriteAlbum => self.album_state.move_down_by(n),
                MainPageTab::FavoriteArtist => self.artist_state.move_down_by(n),
            },
            MainPageSubState::ViewingPlayList(play_list) => play_list.move_down_by(n),
            MainPageSubState::ViewingAlbum(album) => album.move_down_by(n),
            MainPageSubState::ViewingArtist(artist) => artist.move_down_by(n),
        }
    }
}
