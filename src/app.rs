use crate::{
    state::{
        Page, PopupState,
        main_page::{
            MainPageState, MainPageTab,
            playlist::{PlayList, PlayListState},
        },
        selectable::Selectable,
    },
    ui::render_ui,
};
use ratatui::{
    DefaultTerminal,
    crossterm::event::{self},
    style::Color,
    widgets::Row,
};
use ratatui_image::{
    picker::{self, Picker},
    protocol::StatefulProtocol,
};
use std::{
    io::{self},
    vec,
};

pub(crate) struct TableColors {
    pub(crate) buffer_bg: Color,
    pub(crate) row_fg: Color,
    pub(crate) selected_row_style_fg: Color,
    pub(crate) normal_row_color: Color,
    pub(crate) alt_row_color: Color,
}

impl TableColors {
    const fn new() -> Self {
        TableColors {
            buffer_bg: Color::Black,
            row_fg: Color::White,
            selected_row_style_fg: Color::Yellow,
            normal_row_color: Color::Gray,
            alt_row_color: Color::DarkGray,
        }
    }
}

pub(crate) struct App {
    should_quit: bool,
    now_page: Page,
    main_page: MainPageState,
    popup_state: PopupState,
    pub(crate) colors: TableColors,
}

pub(crate) struct RenderCache {
    picker: Picker,
    pub(crate) image: Vec<StatefulProtocol>,
}

impl RenderCache {
    fn default() -> io::Result<Self> {
        let picker = Picker::from_query_stdio()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Picker error: {}", e)))?;

        Ok(RenderCache {
            picker,
            image: Vec::new(),
        })
    }
    fn test_data() -> Self {
        let test_picker = Picker::from_query_stdio().unwrap();
        let test_image = test_picker.new_resize_protocol(
            image::ImageReader::open("/home/wanger/Pictures/pic2.png")
                .unwrap()
                .decode()
                .unwrap(),
        );

        RenderCache {
            picker: test_picker,
            image: vec![test_image],
        }
    }
}

impl App {
    // pub(crate) fn default() -> Self {
    //     let picker = Picker::from_query_stdio().unwrap();
    //
    //     let mut image =
    //         picker.new_resize_protocol(image::ImageReader::open("/home/wanger/Pictures/pic1.jpg"));
    //     App {
    //         should_quit: false,
    //         main_page: MainPageState {
    //             tab: MainPageTab::PlayList(PlayListState {
    //                 items: Vec::new(),
    //                 selected: None, // 当前选中项
    //             }),
    //         },
    //         now_page: Page::Main,
    //         popup_state: PopupState::None,
    //         colors: TableColors::new(),
    //         image: image::ImageReader::open("/home/wanger/Pictures/pic1.jpg")
    //             .unwrap()
    //             .decode()
    //             .unwrap(),
    //     }
    // }
    pub(crate) fn test_run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let mut cache = RenderCache::test_data();
        loop {
            if self.should_quit {
                return Ok(());
            }

            terminal.draw(|frame| {
                render_ui(self, frame, &mut cache);
            })?;

            let event = event::read()?;
            crate::event_handler::handle_event(self, event);
        }
    }

    pub(crate) fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let mut cache = RenderCache::default()?;
        loop {
            if self.should_quit {
                return Ok(());
            }

            terminal.draw(|frame| {
                render_ui(self, frame, &mut cache);
            })?;

            let event = event::read()?;
            crate::event_handler::handle_event(self, event);
        }
    }

    pub(crate) fn get_now_page(&self) -> Page {
        self.now_page
    }

    pub(crate) fn change_now_page(&mut self, target_page: Page) {
        self.now_page = target_page;
    }

    pub(crate) fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub(crate) fn quit(&mut self) {
        self.should_quit = true
    }

    pub(crate) fn should_popup(&self) -> PopupState {
        self.popup_state
    }

    pub(crate) fn popup(&mut self, popup_state: PopupState) {
        self.popup_state = popup_state;
    }

    pub(crate) fn get_main_tab_items(&self) -> Vec<Row> {
        match &self.main_page.tab {
            MainPageTab::PlayList(state) => state.items().iter().map(Row::from).collect(),
            MainPageTab::FavoriteAlbum(state) => state.items().iter().map(Row::from).collect(),
            MainPageTab::FavoriteArtist(state) => state.items().iter().map(Row::from).collect(),
        }
    }

    pub(crate) fn get_main_tab_selected(&self) -> Option<usize> {
        match &self.main_page.tab {
            MainPageTab::PlayList(state) => state.selected_index(),
            MainPageTab::FavoriteAlbum(state) => state.selected_index(),
            MainPageTab::FavoriteArtist(state) => state.selected_index(),
        }
    }

    pub(crate) fn table_move_up_by(&mut self, gap: usize) {
        match self.now_page {
            Page::Main => match &mut self.main_page.tab {
                MainPageTab::PlayList(state) => state.move_up_by(gap),
                MainPageTab::FavoriteAlbum(state) => state.move_up_by(gap),
                MainPageTab::FavoriteArtist(state) => state.move_up_by(gap),
            },
            Page::Search => todo!(),
        }
    }

    pub(crate) fn table_move_down_by(&mut self, gap: usize) {
        match self.now_page {
            Page::Main => match &mut self.main_page.tab {
                MainPageTab::PlayList(state) => state.move_down_by(gap),
                MainPageTab::FavoriteAlbum(state) => state.move_down_by(gap),
                MainPageTab::FavoriteArtist(state) => state.move_down_by(gap),
            },
            Page::Search => todo!(),
        }
    }

    pub(crate) fn main_page(&self) -> &MainPageState {
        &self.main_page
    }

    pub(super) fn get_selected_detail(&self) {
        todo!()
    }
}

#[cfg(debug_assertions)]
pub fn test_data() -> App {
    use crate::state::song::Song;

    let base_dir = "/home/wanger/Pictures/";
    let test_songs = vec![
        Song {
            id: 1,
            title: "测试歌曲A".to_string(),
            artist: "测试歌手".to_string(),
            album: "测试专辑".to_string(),
            duration: 210,
        },
        Song {
            id: 2,
            title: "测试歌曲B".to_string(),
            artist: "测试歌手".to_string(),
            album: "测试专辑".to_string(),
            duration: 185,
        },
        Song {
            id: 3,
            title: "测试歌曲C".to_string(),
            artist: "测试歌手".to_string(),
            album: "测试专辑".to_string(),
            duration: 210,
        },
    ];

    let play_list = vec![
        PlayList {
            name: String::from("我喜欢的音乐"),
            track_count: 42,
            cover_path: format!("{}{}", base_dir, "pic1.jpg"),
            songs: test_songs.clone(),
        },
        PlayList {
            name: String::from("跑步歌单"),
            track_count: 15,
            cover_path: format!("{}{}", base_dir, "pic2.jpg"),
            songs: test_songs.clone(),
        },
        PlayList {
            name: String::from("经典老歌"),
            track_count: 30,
            cover_path: format!("{}{}", base_dir, "pic3.jpg"),
            songs: test_songs.clone(),
        },
        PlayList {
            name: String::from("工作专注"),
            track_count: 28,
            cover_path: format!("{}{}", base_dir, "pic4.jpg"),
            songs: test_songs.clone(),
        },
        PlayList {
            name: String::from("电子音乐"),
            track_count: 53,
            cover_path: format!("{}{}", base_dir, "pic5.jpg"),
            songs: test_songs.clone(),
        },
        PlayList {
            name: String::from("流行金曲"),
            track_count: 37,
            cover_path: format!("{}{}", base_dir, "pic6.jpg"),
            songs: test_songs.clone(),
        },
        PlayList {
            name: String::from("轻音乐疗愈"),
            track_count: 22,
            cover_path: format!("{}{}", base_dir, "pic7.jpg"),
            songs: test_songs.clone(),
        },
        PlayList {
            name: String::from("古典音乐精选"),
            track_count: 19,
            cover_path: format!("{}{}", base_dir, "pic8.jpg"),
            songs: test_songs.clone(),
        },
    ];
    App {
        should_quit: false,
        now_page: Page::Main,
        main_page: MainPageState {
            tab: MainPageTab::PlayList(PlayListState {
                items: play_list,
                selected: Some(0),
            }),
        },
        popup_state: PopupState::None,
        colors: TableColors::new(),
    }
}
