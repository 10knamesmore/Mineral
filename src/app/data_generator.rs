use crate::{
    app::{PlayList, RenderCache, Song},
    state::Introduction,
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
        artist_id: 0,
        album: rand_album_name(rng),
        album_id: 0,
        pic_url: String::new(),
        song_url: String::new(),
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
    use crate::app::{Context, Signals};

    let playlists = gen_playlists();

    let mut ctx = Context::default();
    ctx.mut_main_page().update_playlist(playlists);

    App {
        ctx,
        signals: Signals::start().unwrap(),
    }
}

pub(crate) fn test_render_cache() -> RenderCache {
    let test_picker = Picker::from_query_stdio().unwrap();
    let cache_dir = String::from("/home/wanger/Pictures/ncm_tui/");

    RenderCache::new(test_picker, cache_dir)
}
