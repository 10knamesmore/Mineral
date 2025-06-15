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
        Page, PopupState, Selectable,
        main_page::{MainPageState, MainPageSubState, MainPageTab},
    },
    ui::render_ui,
    util::notification::{Notification, NotifyUrgency},
};
use data_generator::test_render_cache;
use ratatui::{
    DefaultTerminal,
    crossterm::event::{self},
    style::Color,
    widgets::{Row, Table},
};
use ratatui_image::{picker::Picker, protocol::StatefulProtocol};
use std::{
    collections::{HashMap, VecDeque},
    io::{self},
};
use std::{io::Cursor, path::Path};
use tokio::{
    fs,
    sync::mpsc::{self},
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

// ############### 图片缓存相关 ###################

/// 图片缓存类型
enum ImageCacheType {
    Playlist,
    Album,
    Artist,
}

// OPTIM: 尝试改为RefCell<StatefulProtocol>以更好的保证enum本身不可变,但内部变量可变
// 详情见,not_requested(), get_now_cover()
pub enum ImageState {
    NotRequested,
    Loading,
    Loaded(StatefulProtocol),
    Failed(String),
}

pub struct ImageLoadRequest {
    image_type: ImageCacheType,
    id: u64,
}

struct ImageloadResult {
    image_type: ImageCacheType,
    id: u64,
    result: Result<StatefulProtocol, String>,
}

/// 图片缩略图缓存
pub(crate) struct RenderCache {
    not_requested_placeholder: ImageState,
    playlist_cover: HashMap<u64, ImageState>,
    album_cover: HashMap<u64, ImageState>,
    artist_cover: HashMap<u64, ImageState>,

    load_request_sender: mpsc::UnboundedSender<ImageLoadRequest>,
    load_result_receiver: mpsc::UnboundedReceiver<ImageloadResult>,
}

// #############################################

impl RenderCache {
    /// 创建新的 RenderCache
    pub fn new(picker: Picker, cache_path: String) -> Self {
        let (load_request_sender, load_request_receiver) = mpsc::unbounded_channel();
        let (load_result_sender, load_result_receiver) = mpsc::unbounded_channel();

        // let runtime_handle = tokio::runtime::Handle::current();

        let cache_path_cloned = cache_path.clone();

        tokio::spawn(async move {
            Self::image_loader_task(
                load_request_receiver,
                load_result_sender,
                cache_path_cloned,
                picker,
            )
            .await;
        });

        Self {
            not_requested_placeholder: ImageState::NotRequested,
            playlist_cover: HashMap::new(),
            album_cover: HashMap::new(),
            artist_cover: HashMap::new(),
            load_request_sender,
            load_result_receiver,
        }
    }

    pub(crate) fn not_requested(&mut self) -> &mut ImageState {
        &mut self.not_requested_placeholder
    }

    pub(crate) fn get_playlist_cover(&mut self, id: u64) -> &mut ImageState {
        self.poll_image_results();

        if let std::collections::hash_map::Entry::Vacant(entry) = self.playlist_cover.entry(id) {
            entry.insert(ImageState::Loading);
            self.request_image_load(ImageCacheType::Playlist, id);
        }

        self.playlist_cover.get_mut(&id).unwrap()
    }
    pub(crate) fn get_artist_cover(&mut self, id: u64) -> &mut ImageState {
        self.poll_image_results();

        if let std::collections::hash_map::Entry::Vacant(entry) = self.artist_cover.entry(id) {
            entry.insert(ImageState::Loading);
            self.request_image_load(ImageCacheType::Artist, id);
        }

        self.artist_cover.get_mut(&id).unwrap()
    }
    pub(crate) fn get_album_cover(&mut self, id: u64) -> &mut ImageState {
        self.poll_image_results();

        if let std::collections::hash_map::Entry::Vacant(entry) = self.album_cover.entry(id) {
            entry.insert(ImageState::Loading);
            self.request_image_load(ImageCacheType::Album, id);
        }

        self.album_cover.get_mut(&id).unwrap()
    }

    fn request_image_load(&self, image_type: ImageCacheType, id: u64) {
        let request = ImageLoadRequest { image_type, id };

        if let Err(e) = self.load_request_sender.send(request) {
            eprintln!("Failed to send image load request: {}", e)
        }
    }

    // 轮询, 更新load结果到self
    fn poll_image_results(&mut self) {
        while let Ok(result) = self.load_result_receiver.try_recv() {
            let state = match result.result {
                Ok(image) => ImageState::Loaded(image),
                Err(e) => ImageState::Failed(e),
            };

            match result.image_type {
                ImageCacheType::Playlist => {
                    self.playlist_cover.insert(result.id, state);
                }
                ImageCacheType::Album => {
                    self.album_cover.insert(result.id, state);
                }
                ImageCacheType::Artist => {
                    self.artist_cover.insert(result.id, state);
                }
            }
        }
    }

    // 等待接收加载图片的请求
    async fn image_loader_task(
        mut request_receiver: mpsc::UnboundedReceiver<ImageLoadRequest>,
        result_sender: mpsc::UnboundedSender<ImageloadResult>,
        cache_path: String,
        picker: Picker,
    ) {
        while let Some(request) = request_receiver.recv().await {
            let result = Self::load_image_async(&cache_path, &picker, &request).await;

            let load_result = ImageloadResult {
                image_type: request.image_type,
                id: request.id,
                result,
            };

            if let Err(e) = result_sender.send(load_result) {
                eprintln!("Failed to send image load result: {}", e);
                break;
            }
        }
    }

    // 异步加载图片
    async fn load_image_async(
        cache_path: &str,
        picker: &Picker,
        request: &ImageLoadRequest,
    ) -> Result<StatefulProtocol, String> {
        let (image_type, id) = (&request.image_type, request.id);

        match Self::try_get_img_path_from_disk(cache_path, image_type, id).await {
            Ok(file_path_opt) => match file_path_opt {
                Some(file_path) => {
                    let data = tokio::fs::read(&file_path)
                        .await
                        .map_err(|e| e.to_string())?;

                    let format = image::guess_format(&data)
                        .map_err(|e| format!("无法识别 {} 文件格式 {}", &file_path, e))?;
                    let cursor = Cursor::new(data);

                    let decoded_image = image::ImageReader::with_format(cursor, format)
                        .decode()
                        .map_err(|e| format!("文件 {} 解码时发生错误 {}", &file_path, e))?;

                    let image = picker.new_resize_protocol(decoded_image);
                    Ok(image)
                }
                None => match Self::try_get_img_path_from_net(cache_path, picker, image_type, id)
                    .await
                {
                    Ok(_) => todo!("从net获取图片尚未实现"),
                    Err(_) => todo!("从net获取图片尚未实现"),
                },
            },
            Err(e) => Err(format!("读取图片的时候发生IO错误: {}", e)),
        }
    }

    async fn try_get_img_path_from_net(
        cache_path: &str,
        picker: &Picker,
        image_type: &ImageCacheType,
        id: u64,
    ) -> io::Result<StatefulProtocol> {
        // TODO: 尝试通过api获取网络图片,并保存到本地,直接返回StatefulProtocol
        todo!("从net获取图片尚未实现")
    }

    /// 尝试查找type为image_type的图片在磁盘上是否存在, 如果存在, 返回图片的路径
    ///
    /// # 参数
    /// - `image_type`: 图片类型
    /// - `id`: image_type 对应类型的 ID
    ///
    /// # 返回
    /// - `Err(e)`: 发生io错误
    /// - `Option<String>`: 返回图片的路径，如果不存在则返回 None
    async fn try_get_img_path_from_disk(
        cache_path: &str,
        image_type: &ImageCacheType,
        id: u64,
    ) -> io::Result<Option<String>> {
        // TODO: 对非UTF-8编码的文件系统的支持
        let subdir = match image_type {
            ImageCacheType::Playlist => "playlist",
            ImageCacheType::Album => "album",
            ImageCacheType::Artist => "artist",
        };

        let dir_path = Path::new(cache_path).join("images").join(subdir);

        let prefix = format!("{}.", id);
        let mut rd = fs::read_dir(&dir_path).await?;

        while let Some(entry) = rd.next_entry().await? {
            // 获取文件名，并检查是否为有效 UTF-8
            let file_name_os = entry.file_name();

            if let Some(file_name_str) = file_name_os.to_str() {
                if file_name_str.starts_with(&prefix) {
                    let full_path = entry.path();

                    // 检查整个文件路径是否为有效UTF-8
                    if let Some(path_str) = full_path.to_str() {
                        return Ok(Some(path_str.to_string()));
                    } else {
                        // 文件路径不是有效 UTF-8 字符串
                        continue;
                    }
                }
            }
        }

        Ok(None)
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
        // HACK: 正式运行更改
        let test_picker = Picker::from_query_stdio().unwrap();
        let cache_dir = String::from("/home/wanger/Pictures/ncm_tui/");
        let mut cache = RenderCache::new(test_picker, cache_dir);
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
    pub(crate) fn get_main_tab_items_as_row(&self) -> Vec<Row> {
        self.main_page.get_now_tab_items()
    }

    /// 获取主页面表格选中项
    pub(crate) fn get_main_tab_selected_index(&self) -> Option<usize> {
        // match &self.main_page.now_state {
        //     MainPageTab::PlayList => self.main_page.playlist_state().selected_index(),
        //     MainPageTab::FavoriteAlbum => self.main_page.album_state().selected_index(),
        //     MainPageTab::FavoriteArtist => self.main_page.artist_state().selected_index(),
        // }
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
    pub(super) fn get_selected_detail(&self) -> Option<Table> {
        match &self.now_page {
            Page::Main => self.main_page.get_selected_detail(),
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
            Page, PopupState, Song,
            main_page::{MainPageState, playlist::PlayList},
        },
    };
    use rand::{Rng, seq::SliceRandom};
    use ratatui_image::picker::Picker;

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

    fn gen_unique_ids(amount: usize, rng: &mut impl Rng) -> Vec<u64> {
        let pool: Vec<u64> = (0..100).collect();
        pool.choose_multiple(rng, amount).cloned().collect()
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
    fn gen_playlist(id: u64, name: &str, rng: &mut impl Rng) -> PlayList {
        let song_count = rng.gen_range(10..=20);
        let ids = gen_unique_ids(song_count, rng);

        let songs: Vec<Song> = ids.into_iter().map(|id| gen_song(id, rng)).collect();

        PlayList {
            name: format!("{} - 测试歌单 ID:{}", name, id),
            track_count: songs.len(),
            songs,
            id,
        }
    }

    // 生成所有歌单
    fn gen_playlists() -> Vec<PlayList> {
        let mut rng = rand::thread_rng();
        let playlist_names = rand_playlist_names(&mut rng, 12);
        let ids = gen_unique_ids(playlist_names.len(), &mut rng);

        playlist_names
            .iter()
            .zip(ids)
            .map(|(name, id)| gen_playlist(id, name, &mut rng))
            .collect()
    }

    #[cfg(debug_assertions)]
    pub(crate) fn test_struct_app() -> App {
        use std::collections::VecDeque;

        use crate::state::main_page::{album::Album, artist::Artist};

        let play_list = gen_playlists();

        App {
            should_quit: false,
            now_page: Page::Main,
            main_page: MainPageState::new(
                play_list,
                vec![Album::default()],
                vec![Artist::default()],
            ),
            notifications: VecDeque::new(),
            popup_state: PopupState::None,
            colors: TableColors::new(),
        }
    }

    pub(crate) fn test_render_cache() -> RenderCache {
        let test_picker = Picker::from_query_stdio().unwrap();
        let cache_dir = String::from("/home/wanger/Pictures/ncm_tui/");

        RenderCache::new(test_picker, cache_dir)
    }
}
