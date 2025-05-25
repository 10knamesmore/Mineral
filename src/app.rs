//! # app.rs
//!
//! 本模块为 ncm-tui 的应用主入口，包含全局 App 状态、渲染缓存 RenderCache、主页面状态管理、图片缓存与加载、测试数据生成等核心功能。
//!
//! ## 主要结构体
//! - [`App`]：应用全局状态，负责页面切换、弹窗、主页面状态、表格配色等。
//! - [`RenderCache`]：图片缩略图缓存，支持歌单、专辑、艺人等多种类型的图片按需加载与缓存。
//! - [`TableColors`]：表格配色方案。
//!
//! ## 主要功能
//! - 页面切换与主页面状态管理
//! - 表格数据与选中项管理
//! - 图片缩略图的本地/网络加载与缓存
//! - 测试数据与测试渲染缓存的生成
//!
//! ## 相关模块
//! - [`state`]：应用状态定义
//! - [`ui`]：UI 渲染
//! - [`data_generator`]：测试数据生成工具

use crate::{
    state::{
        Page, PopupState,
        main_page::{MainPageState, MainPageTab},
        selectable::Selectable,
    },
    ui::render_ui,
    util::notification::{Notification, NotifyUrgency},
};
use data_generator::test_render_cache;
use ratatui::{
    DefaultTerminal,
    crossterm::event::{self},
    style::Color,
    widgets::Row,
};
use ratatui_image::{picker::Picker, protocol::StatefulProtocol};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    io::{self},
};

/// 表格配色方案
pub(crate) struct TableColors {
    pub(crate) buffer_bg: Color,
    pub(crate) row_fg: Color,
    pub(crate) selected_row_style_fg: Color,
    pub(crate) normal_row_color: Color,
    pub(crate) alt_row_color: Color,
}

impl TableColors {
    /// 默认配色
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

/// 应用全局状态
pub(crate) struct App {
    should_quit: bool,
    now_page: Page,
    main_page: MainPageState,
    popup_state: PopupState,
    notifications: VecDeque<Notification>,
    pub(crate) colors: TableColors,
}

/// 图片缩略图缓存
pub(crate) struct RenderCache {
    picker: Picker,
    cache_path: String,
    playlist_cover: HashMap<u64, StatefulProtocol>,
    album_cover: HashMap<u64, StatefulProtocol>,
    artist_cover: HashMap<u64, StatefulProtocol>,
}

/// 图片缓存类型
enum ImageCacheType {
    PlaylistCover,
    AlbumCover,
    ArtistCover,
}

impl RenderCache {
    /// 创建新的 RenderCache
    fn new() -> Self {
        let picker = Picker::from_query_stdio().unwrap();
        RenderCache {
            picker,
            cache_path: String::new(),
            playlist_cover: HashMap::new(),
            album_cover: HashMap::new(),
            artist_cover: HashMap::new(),
        }
    }

    /// 创建默认 RenderCache，带错误处理
    fn default() -> io::Result<Self> {
        let picker = Picker::from_query_stdio()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("Picker error: {}", e)))?;

        Ok(RenderCache {
            picker,
            cache_path: String::new(),
            playlist_cover: HashMap::new(),
            album_cover: HashMap::new(),
            artist_cover: HashMap::new(),
        })
    }

    /// 获取歌单封面，自动缓存
    ///
    /// # 参数
    /// - `id`: play_list ID
    ///
    /// # 返回
    /// - `Option<&mut StatefulProtocol>`: 返回一个可变引用，指向缓存的 StatefulProtocol
    pub(crate) fn get_playlist_cover(&mut self, id: u64) -> Option<&mut StatefulProtocol> {
        let image_type = ImageCacheType::PlaylistCover;

        // 尝试获取缓存（短生命周期）
        if self.playlist_cover.contains_key(&id) {
            return Some(self.playlist_cover.get_mut(&id).unwrap());
        }

        self.try_load_image(image_type, id)
            .map(|image| self.playlist_cover.entry(id).or_insert(image))
    }

    /// 获取专辑封面，自动缓存
    ///
    /// # 参数
    /// - `id`: album ID
    ///
    /// # 返回
    /// - `Option<&mut StatefulProtocol>`: 返回一个可变引用，指向缓存的 StatefulProtocol
    pub(crate) fn get_album_cover(&mut self, id: u64) -> Option<&mut StatefulProtocol> {
        let image_type = ImageCacheType::AlbumCover;

        // 尝试获取缓存（短生命周期）
        if self.album_cover.contains_key(&id) {
            return self.album_cover.get_mut(&id);
        }

        self.try_load_image(image_type, id)
            .map(|image| self.album_cover.entry(id).or_insert(image))
    }

    /// 获取艺人封面，自动缓存
    ///
    /// # 参数
    /// - `id`: artist ID
    ///
    /// # 返回
    /// - `Option<&mut StatefulProtocol>`: 返回一个可变引用，指向缓存的 StatefulProtocol
    pub(crate) fn get_artist_cover(&mut self, id: u64) -> Option<&mut StatefulProtocol> {
        let image_type = ImageCacheType::ArtistCover;

        // 尝试获取缓存（短生命周期）
        if self.artist_cover.contains_key(&id) {
            return self.artist_cover.get_mut(&id);
        }

        self.try_load_image(image_type, id)
            .map(|image| self.artist_cover.entry(id).or_insert(image))
    }

    /// 尝试加载图片（本地优先，失败则尝试网络）
    /// 缓存会优先在本地查找，本地存在并解码成功, 则直接缓存到内存并返回.
    /// 如果不存在则尝试从网络加载。
    ///
    /// # 参数
    /// - `image_type`: 图片类型
    /// - `id`: image_type 对应类型的 ID
    ///
    /// # 返回
    /// - `Option<StatefulProtocol>`: 返回一个 StatefulProtocol，如果加载成功则返回 Some，否则返回 None
    fn try_load_image(&mut self, image_type: ImageCacheType, id: u64) -> Option<StatefulProtocol> {
        if let Some(path) = self.if_image_cache_in_disk(&image_type, id) {
            match self.load_image(path) {
                Ok(image) => {
                    // 将加载的图片添加到缓存中
                    Some(image)
                }

                // 说明本地图片存在,但是加载失败
                Err(_) => None,
            }
        } else {
            self.try_load_image_from_net(&image_type, id)
        }
    }

    /// 尝试从网络加载图片（未实现）
    fn try_load_image_from_net(
        &mut self,
        image_type: &ImageCacheType,
        id: u64,
    ) -> Option<StatefulProtocol> {
        unimplemented!("尝试用ncm_api从网络加载图片到磁盘");
    }

    /// 尝试查找type为image_type的图片在磁盘上是否存在, 如果存在, 返回图片的路径
    ///
    /// # 参数
    /// - `image_type`: 图片类型
    /// - `id`: image_type 对应类型的 ID
    ///
    /// # 返回
    /// - `Option<String>`: 返回图片的路径，如果不存在则返回 None
    fn if_image_cache_in_disk(&self, image_type: &ImageCacheType, id: u64) -> Option<String> {
        let path = match image_type {
            ImageCacheType::PlaylistCover => format!("{}images/playlist/", self.cache_path),
            ImageCacheType::AlbumCover => format!("{}images/album/", self.cache_path),
            ImageCacheType::ArtistCover => format!("{}images/artist/", self.cache_path),
        };

        let prefix = format!("{}.", id);
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let file_name = entry.file_name();
                let file_name = file_name.to_string_lossy();

                if file_name.starts_with(&prefix) {
                    return Some(entry.path().to_string_lossy().to_string());
                }
            }
        }

        None
    }

    /// 加载图片并转为 StatefulProtocol
    ///
    /// # 参数
    /// - `path`: 图片路径
    ///
    /// # 返回
    /// - `io::Result<StatefulProtocol>`: 返回一个 StatefulProtocol，如果加载成功则返回 Ok，否则返回 Err
    fn load_image(&self, path: String) -> io::Result<StatefulProtocol> {
        let decoded_image = image::ImageReader::open(path)
            .map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("加载图片时发生错误: {}", e))
            })?
            .decode()
            .map_err(|e| {
                io::Error::new(io::ErrorKind::Other, format!("解码图片时发生错误: {}", e))
            })?;
        let image = self.picker.new_resize_protocol(decoded_image);

        Ok(image)
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
        let mut cache = test_render_cache();
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

    /// 运行正式模式
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

    /// 获取当前页面
    pub(crate) fn get_now_page(&self) -> Page {
        self.now_page
    }

    /// 切换当前页面
    pub(crate) fn change_now_page(&mut self, target_page: Page) {
        self.now_page = target_page;
    }

    /// 是否退出
    pub(crate) fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// 设置退出
    pub(crate) fn quit(&mut self) {
        self.should_quit = true
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
    pub(crate) fn get_main_tab_items(&self) -> Vec<Row> {
        match &self.main_page.now_tab {
            MainPageTab::PlayList(state) => state.items().iter().map(Row::from).collect(),
            MainPageTab::FavoriteAlbum(state) => state.items().iter().map(Row::from).collect(),
            MainPageTab::FavoriteArtist(state) => state.items().iter().map(Row::from).collect(),
        }
    }

    /// 获取主页面表格选中项
    pub(crate) fn get_main_tab_selected_index(&self) -> Option<usize> {
        match &self.main_page.now_tab {
            MainPageTab::PlayList(state) => state.selected_index(),
            MainPageTab::FavoriteAlbum(state) => state.selected_index(),
            MainPageTab::FavoriteArtist(state) => state.selected_index(),
        }
    }

    /// 表格上移
    pub(crate) fn table_move_up_by(&mut self, gap: usize) {
        match self.now_page {
            Page::Main => match &mut self.main_page.now_tab {
                MainPageTab::PlayList(state) => state.move_up_by(gap),
                MainPageTab::FavoriteAlbum(state) => state.move_up_by(gap),
                MainPageTab::FavoriteArtist(state) => state.move_up_by(gap),
            },
            Page::Search => todo!(),
        }
    }

    /// 表格下移
    pub(crate) fn table_move_down_by(&mut self, gap: usize) {
        match self.now_page {
            Page::Main => match &mut self.main_page.now_tab {
                MainPageTab::PlayList(state) => state.move_down_by(gap),
                MainPageTab::FavoriteAlbum(state) => state.move_down_by(gap),
                MainPageTab::FavoriteArtist(state) => state.move_down_by(gap),
            },
            Page::Search => todo!(),
        }
    }

    /// 获取主页面状态
    pub(crate) fn main_page(&self) -> &MainPageState {
        &self.main_page
    }

    /// 获取选中项详情
    pub(super) fn get_selected_detail(&self) -> Vec<Row> {
        match &self.now_page {
            Page::Main => self.main_page.now_tab.get_selected_detail(),
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

    fn notify_internal(&mut self, title: &str, msg: &str, urgency: NotifyUrgency) {
        self.popup(PopupState::Notificacion);
        self.notifications
            .push_back(Notification::new(title, msg, urgency));
    }

    // 分别暴露四个等级的接口
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
}

/// 测试数据与测试渲染缓存生成工具
pub(super) mod data_generator {
    use super::RenderCache;
    use crate::{
        App,
        app::TableColors,
        state::{
            Page, PopupState,
            main_page::{
                MainPageState, MainPageTab,
                playlist::{PlayList, PlayListState},
            },
            song::Song,
        },
    };
    use rand::{Rng, seq::SliceRandom};
    use ratatui_image::picker::Picker;
    use std::collections::HashMap;

    fn rand_artist_name(rng: &mut impl Rng) -> String {
        let pool = [
            "测试歌手",
            "流行歌手",
            "电子艺人",
            "古典大师",
            "治愈歌者",
            "说唱先锋",
            "爵士灵魂",
            "摇滚之星",
            "民族歌者",
            "实验派",
            "独立创作人",
            "地下说唱者",
            "现场之王",
            "空灵女声",
            "男低音传说",
            "合成器大亨",
            "街头诗人",
            "空灵作曲家",
            "蓝调大师",
            "乡村风情",
        ];
        pool.choose(rng).unwrap().to_string()
    }

    fn rand_album_name(rng: &mut impl Rng) -> String {
        let pool = [
            "测试专辑",
            "流行精选",
            "电子风暴",
            "古典之声",
            "疗愈之旅",
            "午夜旋律",
            "摇滚记忆",
            "爵士年代",
            "实验空间",
            "民族风采",
            "夜色节拍",
            "孤独之旅",
            "晨光微露",
            "时光胶囊",
            "声音博物馆",
            "流浪计划",
            "街角故事",
            "故乡原声",
            "虚拟梦境",
            "城市脉搏",
        ];
        pool.choose(rng).unwrap().to_string()
    }

    fn rand_playlist_names(rng: &mut impl Rng, amount: usize) -> Vec<String> {
        let pool = [
            "我的最爱",
            "运动节奏",
            "经典回忆",
            "工作伴侣",
            "电子狂欢",
            "深夜咖啡",
            "早安元气",
            "情绪低谷",
            "上班不累",
            "睡前冥想",
            "开车必备",
            "雨天听歌",
            "快乐加倍",
            "专注模式",
            "世界音乐",
            "校园时代",
            "独处时光",
            "情人节精选",
            "一人食",
            "异国风情",
        ];
        pool.choose_multiple(rng, amount)
            .map(|s| s.to_string())
            .collect()
    }

    // 生成随机歌曲
    fn gen_song(id: u64, rng: &mut impl Rng) -> Song {
        Song {
            id,
            name: format!("测试歌曲ID:{}", id),
            artist: rand_artist_name(rng),
            album: rand_album_name(rng),
            duration: rng.gen_range(120..=320),
        }
    }

    // 生成一个歌单
    fn gen_playlist(i: usize, name: &str, rng: &mut impl Rng) -> PlayList {
        let song_count = rng.gen_range(10..=20);
        let ids = gen_unique_ids(song_count, rng);

        let songs: Vec<Song> = ids.into_iter().map(|id| gen_song(id, rng)).collect();

        PlayList {
            name: name.to_string(),
            track_count: songs.len(),
            songs,
            id: i as u64,
        }
    }

    fn gen_unique_ids(song_count: usize, rng: &mut impl Rng) -> Vec<u64> {
        let pool: Vec<u64> = (0..100).collect();
        pool.choose_multiple(rng, song_count).cloned().collect()
    }

    // 生成所有歌单
    fn gen_playlists() -> Vec<PlayList> {
        let mut rng = rand::thread_rng();
        let playlist_names = rand_playlist_names(&mut rng, 12);

        playlist_names
            .iter()
            .enumerate()
            .map(|(index, name)| gen_playlist(index, name, &mut rng))
            .collect()
    }

    #[cfg(debug_assertions)]
    pub(crate) fn test_struct_app() -> App {
        let play_list = gen_playlists();

        App {
            should_quit: false,
            now_page: Page::Main,
            main_page: MainPageState {
                now_tab: MainPageTab::PlayList(PlayListState {
                    items: play_list,
                    selected: Some(0),
                }),
            },
            popup_state: PopupState::None,
            colors: TableColors::new(),
        }
    }

    pub(crate) fn test_render_cache() -> RenderCache {
        let test_picker = Picker::from_query_stdio().unwrap();

        RenderCache {
            picker: test_picker,
            cache_path: String::from("/home/wanger/Pictures/ncm_tui/"),
            playlist_cover: HashMap::new(),
            album_cover: HashMap::new(),
            artist_cover: HashMap::new(),
        }
    }
}
