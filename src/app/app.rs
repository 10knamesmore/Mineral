use crate::{
    app::Signals,
    event_handler,
    state::{
        main_page::{MainPageState, MainPageSubState, MainPageTab},
        Page, PopupState, Selectable,
    },
    ui::render_ui,
    util::notification::{Notification, NotifyUrgency},
};
use data_generator::test_render_cache;
use ratatui::{
    style::Color,
    widgets::{Row, Table},
    DefaultTerminal,
};
use std::{
    collections::VecDeque,
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
    signals: Signals,
}

impl App {
    pub(crate) async fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        // HACK: 正式运行更改
        let mut cache = test_render_cache();
        loop {
            if self.should_quit {
                return Ok(());
            }

            terminal.draw(|frame| {
                render_ui(self, frame, &mut cache);
            })?;

            if let Some(event) = self.signals.rx.recv().await {
                event_handler::handle_event(self, event);
            }
        }
    }

    /// 获取当前页面
    pub(crate) fn now_page(&self) -> Page {
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
}

/// 测试数据与测试渲染缓存生成工具
pub mod data_generator {
    use crate::{
        app::{RenderCache, TableColors},
        state::{
            main_page::{playlist::PlayList, MainPageState},
            Introduction, Page, PopupState, Song,
        },
        App,
    };
    use rand::{seq::IndexedRandom, Rng};
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

    fn rand_album_intro(rng: &mut impl Rng) -> String {
        let intros = [
            "一段穿越时空的声音旅程，唤起你内心最深的回忆。",
            "用旋律讲述生活的细腻与浪漫，每一首都是心情的注脚。",
            "融合多元风格，开启听觉新世界。",
            "献给孤独时光的你，陪你走过每个夜晚。",
            "在节奏中释放压力，在旋律中寻找自我。",
            "捕捉城市的脉动，描绘喧嚣中的宁静。",
            "一张专属于清晨与黄昏之间的音乐写真。",
            "当代声音实验，挑战你的听觉边界。",
            "民谣与电子的对话，传统与未来的交融。",
            "低保真质感，记录最真实的情绪。",
            "灵感来自旅途的每一次偶遇与别离。",
            "用音乐串联记忆，构建属于你的声音档案馆。",
            "轻盈旋律，宛如夏日微风轻拂心头。",
            "节拍与情感交织，一场声波的深度潜行。",
            "在每个不眠夜里，与你心灵相通。",
            "静谧与激荡并存，一次音乐与灵魂的对话。",
            "从过去到未来，用音符写下不朽篇章。",
            "疗愈旋律抚慰心灵，找回内在的平静。",
            "探索未知的声音维度，开启感官新篇。",
            "音符跳动如心跳，是你未说出口的情绪。",
        ];
        intros.choose(rng).unwrap().to_string()
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
            duration: rng.random_range(120..=320),
        }
    }

    // 生成一个歌单
    fn gen_playlist(id: u64, name: &str, rng: &mut impl Rng) -> PlayList {
        let song_count = rng.random_range(10..=20);
        let ids = gen_unique_ids(song_count, rng);

        let songs: Vec<Song> = ids.into_iter().map(|id| gen_song(id, rng)).collect();
        let introduction = Introduction::new(rand_album_intro(rng));

        PlayList {
            name: format!("{} - 测试歌单 ID:{}", name, id),
            track_count: songs.len(),
            songs,
            id,
            introduction,
        }
    }

    // 生成所有歌单
    fn gen_playlists() -> Vec<PlayList> {
        let mut rng = rand::rng();
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

        use crate::{
            app::Signals,
            state::main_page::{album::Album, artist::Artist},
        };

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
            signals: Signals::start().unwrap(),
        }
    }

    pub(crate) fn test_render_cache() -> RenderCache {
        let test_picker = Picker::from_query_stdio().unwrap();
        let cache_dir = String::from("/home/wanger/Pictures/ncm_tui/");

        RenderCache::new(test_picker, cache_dir)
    }
}
