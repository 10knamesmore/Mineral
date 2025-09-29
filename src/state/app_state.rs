use crate::{
    app::{PlayList, Song, TableColors},
    event_handler::AppEvent,
    state::{
        main_page::{MainPageState, MainPageSubState, MainPageTab},
        Page, PopupState, Selectable,
    },
    util::notification::Notification,
};
use ratatui::widgets::{Row, Table};
use std::{any, collections::VecDeque, path::PathBuf};

pub struct AppState {
    now_page: Page,
    main_page: MainPageState,
    popup_state: PopupState,
    notifications: VecDeque<Notification>,

    pub(crate) colors: TableColors,
}

impl AppState {
    pub fn notify(&mut self, notification: Notification) {
        self.popup(PopupState::Notificacion);
        self.notifications.push_back(notification);
    }

    pub(crate) fn mut_main_page(&mut self) -> &mut MainPageState {
        &mut self.main_page
    }

    /// 获取当前页面
    pub(crate) fn now_page(&self) -> Page {
        self.now_page
    }

    /// 切换当前页面
    pub(crate) fn change_now_page(&mut self, target_page: Page) {
        self.now_page = target_page;
    }

    /// 是否弹窗
    pub(crate) fn should_popup(&self) -> PopupState {
        self.popup_state
    }

    /// 设置弹窗状态
    pub(crate) fn popup(&mut self, popup_state: PopupState) {
        self.popup_state = popup_state;
        AppEvent::Render.emit();
    }

    /// 获取主页面表格数据
    pub(crate) fn main_tab_items_as_row(&self) -> Vec<Row<'_>> {
        self.main_page.now_tab_items()
    }

    /// 获取主页面表格选中项
    // TODO: 改掉这坨屎, 分到main_page里面
    pub(crate) fn main_tab_selected_index(&self) -> Option<usize> {
        match &self.main_page.now_state {
            MainPageSubState::TabView(main_page_tab) => match main_page_tab {
                MainPageTab::PlayList => self.main_page.playlist_state().selected_index(),
                MainPageTab::FavoriteAlbum => self.main_page.album_state().selected_index(),
                MainPageTab::FavoriteArtist => self.main_page.artist_state().selected_index(),
            },
            MainPageSubState::ViewingPlayList(playlist) => playlist.selected_index(),
            MainPageSubState::ViewingAlbum(album) => album.selected_index(),
            MainPageSubState::ViewingArtist(artist) => artist.selected_index(),
        }
    }

    /// 获取 MainPageState 的引用
    pub(crate) fn main_page(&self) -> &MainPageState {
        &self.main_page
    }

    /// 获取选中项详情
    /// 根据当前的 Page, 交给对应
    pub(crate) fn selected_detail(&self) -> Option<Table<'_>> {
        match &self.now_page {
            Page::Main => self.main_page.selected_detail(),
            Page::Search => todo!(),
        }
    }

    pub(crate) fn first_notification(&self) -> Option<&Notification> {
        self.notifications.front()
    }

    pub(crate) fn consume_first_notification(&mut self) {
        self.notifications.pop_front();
        if self.notifications.is_empty() {
            self.popup(PopupState::None);
        }
    }

    pub fn load_musics(&mut self, music_dirs: &'static [PathBuf]) {
        use std::fs;

        let mut playlists = Vec::new();

        for music_dir in music_dirs {
            let read_dir = match fs::read_dir(music_dir) {
                Ok(rd) => rd,
                Err(e) => {
                    tracing::warn!("读取目录 {:?} 出错: {:?}", music_dir, e);
                    continue;
                }
            };

            for entry in read_dir.filter_map(Result::ok) {
                let path = entry.path();
                match PlayList::from_path(&path) {
                    Ok(playlist) => playlists.push(playlist),
                    Err(e) => {
                        tracing::warn!("加载播放列表 {:?} 时出错: {:?}", path, e);
                    }
                }
            }
        }

        self.main_page.update_playlist(playlists);
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            now_page: Page::Main,
            main_page: MainPageState::new(Vec::new(), Vec::new(), Vec::new()),
            notifications: VecDeque::default(),
            popup_state: PopupState::None,

            colors: TableColors::default(),
        }
    }
}
