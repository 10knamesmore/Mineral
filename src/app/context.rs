use crate::{
    app::{
        models::{Album, Artist, PlayList},
        style::TableColors,
    },
    state::{
        main_page::{MainPageState, MainPageSubState, MainPageTab},
        Page, PopupState, Selectable,
    },
    util::notification::{Notification, NotifyUrgency},
};
use ratatui::widgets::{Row, Table};
use std::collections::VecDeque;

pub struct Context {
    now_page: Page,
    main_page: MainPageState,
    popup_state: PopupState,
    notifications: VecDeque<Notification>,

    pub(crate) colors: TableColors,
}

impl Context {
    fn notify_internal(&mut self, title: &str, msg: &str, urgency: NotifyUrgency) {
        self.popup(PopupState::Notificacion);
        self.notifications
            .push_back(Notification::new(title, msg, urgency));
    }

    pub(crate) fn notify_debug(&mut self, title: &str, msg: &str) {
        self.notify_internal(title, msg, NotifyUrgency::Debug);
    }

    pub(crate) fn notify_info(&mut self, title: &str, msg: &str) {
        self.notify_internal(title, msg, NotifyUrgency::Info);
    }

    pub(crate) fn notify_warning(&mut self, title: &str, msg: &str) {
        self.notify_internal(title, msg, NotifyUrgency::Warning);
    }

    pub(crate) fn notify_error(&mut self, title: &str, msg: &str) {
        self.notify_internal(title, msg, NotifyUrgency::Error);
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
    }

    /// 获取主页面表格数据
    pub(crate) fn main_tab_items_as_row(&self) -> Vec<Row> {
        self.main_page.now_tab_items()
    }

    /// 获取主页面表格选中项
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

    /// 表格上移
    pub(crate) fn table_move_up_by(&mut self, n: usize) {
        match self.now_page {
            Page::Main => {
                self.main_page.now_tab_move_up_by(n);
            }
            Page::Search => todo!(),
        }
    }

    /// 表格下移
    pub(crate) fn table_move_down_by(&mut self, n: usize) {
        match self.now_page {
            Page::Main => {
                self.main_page.now_tab_move_down_by(n);
            }
            Page::Search => todo!(),
        }
    }

    /// 获取 MainPageState 的引用
    pub(crate) fn main_page(&self) -> &MainPageState {
        &self.main_page
    }

    /// 获取选中项详情
    /// 根据当前的 Page, 交给对应
    pub(crate) fn selected_detail(&self) -> Option<Table> {
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

    pub(crate) fn nav_forward(&mut self) {
        match self.now_page {
            Page::Main => self.main_page.nav_forward(),
            Page::Search => todo!(),
        }
    }

    pub(crate) fn nav_backward(&mut self) {
        match self.now_page {
            Page::Main => self.main_page.nav_backward(),
            Page::Search => todo!(),
        }
    }
}

impl Default for Context {
    fn default() -> Self {
        Self {
            now_page: Page::Main,
            main_page: MainPageState::new(
                vec![PlayList::default()],
                vec![Album::default()],
                vec![Artist::default()],
            ),
            notifications: VecDeque::default(),
            popup_state: PopupState::None,

            colors: TableColors::default(),
        }
    }
}
