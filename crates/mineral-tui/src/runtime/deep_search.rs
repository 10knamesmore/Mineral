//! Playlists 视图的深度搜索:搜索词穿透到歌单内歌曲(歌名 / 艺人 / 专辑)。
//!
//! 全库 × 3 字段的 nucleo 打分开销大(千首级 × 每字段一次匹配),结果按
//! `(query, tracks 版本, 权重)` 缓存在 [`AppState::deep_search`],只在按键 /
//! 数据到达 / 配置热重载时重算,渲染帧只读。
//!
//! 覆盖范围 = `library.tracks` 已有的歌单;未拉到曲目的歌单只参与歌单名匹配
//! (全量补拉由进搜索态时的 [`crate::app::App`] 触发,结果渐进到达后经
//! `tracks_generation` 失效自然并入)。

use mineral_model::{PlaylistId, Song};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;

use crate::runtime::state::AppState;

/// 单个歌单的深度命中:加权分 + 行中部展示载荷。
#[derive(Clone, Debug)]
pub struct DeepHit {
    /// 歌单内最佳歌曲的加权分(字段权重已生效),与歌单名分取 max 排序用。
    pub score: f64,

    /// 最佳命中歌曲的 id(进歌单时定位光标用)。
    pub song_id: mineral_model::SongId,

    /// 行中部展示文本,如 `♪ 春日影 · MyGO!!!!!`;次段随最佳命中字段取艺人或专辑。
    pub line: String,

    /// `line` 内的命中 char 下标(已按展示前缀平移),喂 highlight 渲染。
    pub hits: Vec<u32>,

    /// 该歌单内除最佳外还有几首命中(展示 `+n`;0 = 不显示)。
    pub extra: usize,
}

/// 深度搜索结果缓存。key 失配(query / tracks 版本 / 权重任一变化)时整体重建。
#[derive(Default)]
pub struct DeepSearchCache {
    /// 上次构建时的输入指纹;`None` = 还没建过。
    key: Option<CacheKey>,

    /// 歌单 id → 命中载荷;不在 map 里 = 该歌单无歌曲命中(或曲目未拉到)。
    hits: FxHashMap<PlaylistId, DeepHit>,
}

impl DeepSearchCache {
    /// 某歌单的加权深度分;无命中返回 `None`。
    pub fn score_of(&self, id: &PlaylistId) -> Option<f64> {
        self.hits.get(id).map(|h| h.score)
    }

    /// 某歌单的命中载荷引用;无命中返回 `None`。
    pub fn hit_of(&self, id: &PlaylistId) -> Option<&DeepHit> {
        self.hits.get(id)
    }

    /// 是否存在任何深度命中(渲染端据此决定 match 列要不要占位)。
    pub fn has_hits(&self) -> bool {
        !self.hits.is_empty()
    }
}

/// 缓存输入指纹。权重比位模式(`f32::to_bits`)而非数值,热重载改权重即失效。
#[derive(PartialEq, Eq)]
struct CacheKey {
    /// 构建时的搜索词。
    query: String,

    /// 构建时的 `library.tracks` 内容版本。
    generation: u64,

    /// `[name, artist, album]` 权重位模式。
    weights: [u32; 3],

    /// 深度搜索总开关。
    deep: bool,
}

impl CacheKey {
    /// 以当前 state 算一份指纹。
    fn current(state: &AppState) -> Self {
        let cfg = state.cfg.tui().search();
        let w = cfg.deep_weights();
        Self {
            query: state.search.query().to_owned(),
            generation: state.library.tracks_generation,
            weights: [
                w.name().to_bits(),
                w.artist().to_bits(),
                w.album().to_bits(),
            ],
            deep: *cfg.deep(),
        }
    }

    /// 与当前 state 比对是否仍然有效(避免每帧 clone query 构 key)。
    fn matches(&self, state: &AppState) -> bool {
        let cfg = state.cfg.tui().search();
        let w = cfg.deep_weights();
        self.deep == *cfg.deep()
            && self.generation == state.library.tracks_generation
            && self.weights
                == [
                    w.name().to_bits(),
                    w.artist().to_bits(),
                    w.album().to_bits(),
                ]
            && self.query == state.search.query()
    }
}

/// 保证缓存与当前 `(query, tracks 版本, 权重)` 一致,失配则重建。
///
/// 每帧可重复调用:命中指纹时只做几次整数 / 字符串比较。
pub fn ensure(state: &AppState) {
    if let Some(key) = &state.search.deep_cache.borrow().key
        && key.matches(state)
    {
        return;
    }
    let key = CacheKey::current(state);
    let hits = if key.deep && !key.query.is_empty() {
        build(state)
    } else {
        FxHashMap::default()
    };
    *state.search.deep_cache.borrow_mut() = DeepSearchCache {
        key: Some(key),
        hits,
    };
}

/// 全量重建:对 `library.tracks` 内每首歌按字段权重打分,每歌单留最佳一首 + 命中计数。
fn build(state: &AppState) -> FxHashMap<PlaylistId, DeepHit> {
    let w = state.cfg.tui().search().deep_weights();
    let wn = f64::from(w.name().clamp(0.0, 1.0));
    let wa = f64::from(w.artist().clamp(0.0, 1.0));
    let wal = f64::from(w.album().clamp(0.0, 1.0));
    let mut out = FxHashMap::default();
    for (pid, tracks) in &state.library.tracks {
        let mut best: Option<SongHit<'_>> = None;
        let mut matched = 0usize;
        for sv in tracks {
            let Some(hit) = score_song(state, &sv.data, wn, wa, wal) else {
                continue;
            };
            matched = matched.saturating_add(1);
            if best.as_ref().is_none_or(|b| hit.score > b.score) {
                best = Some(hit);
            }
        }
        if let Some(b) = best {
            out.insert(pid.clone(), compose(&b, matched.saturating_sub(1)));
        }
    }
    out
}

/// 一首歌的最佳字段命中(构建期中间态,借 song 的文本)。
struct SongHit<'a> {
    /// 加权分。
    score: f64,

    /// 命中歌曲(展示主段取 `name`,定位取 `id`)。
    song: &'a Song,

    /// 展示次段:命中在歌名时为首位艺人(无则省略),命中在艺人 / 专辑时为命中文本。
    second: Option<&'a str>,

    /// 命中下标落在次段(艺人 / 专辑)而非歌名段。
    hits_in_second: bool,

    /// 命中字段内的原文 char 下标。
    hits: SmallVec<[u32; 8]>,
}

/// 对一首歌的三个字段分别打分取加权最高;全不命中(或权重全 0)返回 `None`。
///
/// 同分时按 歌名 > 艺人 > 专辑 优先(评估顺序 + 严格大于才替换)。
fn score_song<'a>(
    state: &AppState,
    song: &'a Song,
    wn: f64,
    wa: f64,
    wal: f64,
) -> Option<SongHit<'a>> {
    let mut best: Option<SongHit<'a>> = None;
    if wn > 0.0
        && let Some(m) = state.search.match_for(&song.name)
    {
        best = Some(SongHit {
            score: wn * f64::from(m.score),
            song,
            second: song.artists.first().map(|a| a.name.as_str()),
            hits_in_second: false,
            hits: m.hits,
        });
    }
    if wa > 0.0 {
        for artist in &song.artists {
            let Some(m) = state.search.match_for(&artist.name) else {
                continue;
            };
            let score = wa * f64::from(m.score);
            if best.as_ref().is_none_or(|b| score > b.score) {
                best = Some(SongHit {
                    score,
                    song,
                    second: Some(&artist.name),
                    hits_in_second: true,
                    hits: m.hits,
                });
            }
        }
    }
    if wal > 0.0
        && let Some(album) = &song.album
        && let Some(m) = state.search.match_for(&album.name)
    {
        let score = wal * f64::from(m.score);
        if best.as_ref().is_none_or(|b| score > b.score) {
            best = Some(SongHit {
                score,
                song,
                second: Some(&album.name),
                hits_in_second: true,
                hits: m.hits,
            });
        }
    }
    best
}

/// 组装展示载荷:`♪ <歌名>[ · <次段>]`,把字段内命中下标平移到 line 坐标。
fn compose(b: &SongHit<'_>, extra: usize) -> DeepHit {
    // '♪' + ' ' = 2 char;" · " = 3 char。偏移按 char 数算(highlight 以 char 为单位)。
    let mut line = String::from("♪ ");
    line.push_str(&b.song.name);
    let mut second_start = 2u32;
    if let Some(second) = b.second {
        second_start = second_start
            .saturating_add(u32::try_from(b.song.name.chars().count()).unwrap_or(u32::MAX))
            .saturating_add(3);
        line.push_str(" · ");
        line.push_str(second);
    }
    let offset = if b.hits_in_second { second_start } else { 2 };
    let hits = b
        .hits
        .iter()
        .map(|h| h.saturating_add(offset))
        .collect::<Vec<u32>>();
    DeepHit {
        score: b.score,
        song_id: b.song.id.clone(),
        line,
        hits,
        extra,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mineral_model::{PlaylistId, SourceKind};

    use crate::runtime::state::AppState;
    use crate::runtime::view_model::SongView;
    use crate::test_support::{
        playlist_view, song, state_with_playlists, with_album, with_artist, with_name,
    };

    /// p2 的歌单 id(与 [`state_with_playlists`] 对齐)。
    fn p2() -> PlaylistId {
        PlaylistId::new(SourceKind::NETEASE, "p2")
    }

    /// 把裸 Song 列表包成 SongView 塞进某歌单的 library.tracks 并 bump 版本。
    fn fill_tracks(s: &mut AppState, id: &PlaylistId, tracks: Vec<mineral_model::Song>) {
        let views = tracks
            .into_iter()
            .map(|data| SongView {
                data,
                loved: false,
                plays: None,
            })
            .collect::<Vec<SongView>>();
        s.library.tracks.insert(id.clone(), views);
        s.library.tracks_generation = s.library.tracks_generation.wrapping_add(1);
    }

    /// 标准夹具:[`state_with_playlists`] 的 p2 塞两首歌——
    /// 「春日影 · CRYCHIC · 专辑〈迷途之子〉」与「迷星叫 · MyGO!!!!!(无专辑)」。
    fn state_with_deep_tracks() -> color_eyre::Result<AppState> {
        let mut s = state_with_playlists()?;
        let t1 = with_album(
            with_artist(with_name(song("s1"), "春日影"), "CRYCHIC"),
            "迷途之子",
        );
        let t2 = with_artist(with_name(song("s2"), "迷星叫"), "MyGO!!!!!");
        fill_tracks(&mut s, &p2(), vec![t1, t2]);
        Ok(s)
    }

    /// 歌名命中:歌单名都不含「春日」,但 p2 内有「春日影」→ p2 被深度命中捞出,
    /// 中部载荷 = `♪ 春日影 · CRYCHIC`,高亮落在歌名段(前缀 2 char 偏移)。
    #[test]
    fn song_name_hit_surfaces_playlist() -> color_eyre::Result<()> {
        let mut s = state_with_deep_tracks()?;
        s.search.set_query("春日");
        let names = s
            .filtered_playlists()
            .iter()
            .map(|p| p.data.name.clone())
            .collect::<Vec<String>>();
        assert_eq!(names, vec!["The Power of Failing".to_owned()], "仅 p2 命中");
        let hit = s
            .deep_hit_for(&p2())
            .ok_or_else(|| color_eyre::eyre::eyre!("p2 应有深度命中"))?;
        assert_eq!(hit.line, "♪ 春日影 · CRYCHIC");
        assert_eq!(hit.hits, vec![2, 3], "春日 两字高亮(+2 前缀偏移)");
        assert_eq!(hit.extra, 0);
        Ok(())
    }

    /// 艺人命中:`mygo` 只落在「迷星叫」的艺人 MyGO!!!!! 上 → 次段高亮
    /// (偏移 = 前缀 2 + 歌名 3 + 分隔 3 = 8)。
    #[test]
    fn artist_hit_highlights_second_segment() -> color_eyre::Result<()> {
        let mut s = state_with_deep_tracks()?;
        s.search.set_query("mygo");
        let _ = s.filtered_playlists();
        let hit = s
            .deep_hit_for(&p2())
            .ok_or_else(|| color_eyre::eyre::eyre!("p2 应有深度命中"))?;
        assert_eq!(hit.line, "♪ 迷星叫 · MyGO!!!!!");
        assert_eq!(hit.hits, vec![8, 9, 10, 11], "MyGO 四字高亮(次段偏移 8)");
        Ok(())
    }

    /// 多首命中计数:`迷` 同时命中「迷星叫」(歌名)与「春日影」(专辑〈迷途之子〉),
    /// 最佳一首之外 extra = 1。
    #[test]
    fn extra_counts_additional_matched_songs() -> color_eyre::Result<()> {
        let mut s = state_with_deep_tracks()?;
        s.search.set_query("迷");
        let _ = s.filtered_playlists();
        let hit = s
            .deep_hit_for(&p2())
            .ok_or_else(|| color_eyre::eyre::eyre!("p2 应有深度命中"))?;
        assert_eq!(hit.extra, 1, "除最佳外还有一首命中");
        Ok(())
    }

    /// 字段权重独立:artist 权重置 0 后,纯艺人命中(`mygo`)不再捞出 p2。
    #[test]
    fn zero_weight_disables_field() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.lua");
        std::fs::write(
            &path,
            "return { tui = { search = { deep_weights = { artist = 0 } } } }",
        )?;
        let (cfg, warnings) = mineral_config::load(&path)?;
        assert!(warnings.is_empty(), "测试配置不应有 warning");
        let mut s = AppState::new(Arc::new(cfg));
        s.library.playlists = vec![playlist_view(
            "p2",
            "The Power of Failing",
            SourceKind::NETEASE,
            8,
        )];
        let t = with_artist(with_name(song("s2"), "迷星叫"), "MyGO!!!!!");
        fill_tracks(&mut s, &p2(), vec![t]);
        s.search.set_query("mygo");
        assert!(
            s.filtered_playlists().is_empty(),
            "artist 权重 0:纯艺人命中不应捞出歌单"
        );
        assert!(s.deep_hit_for(&p2()).is_none());
        Ok(())
    }

    /// 排序合约:同一文本,歌单名直接命中(无折扣)排在歌内命中(0.6 折扣)之前。
    #[test]
    fn name_match_outranks_weighted_song_match() -> color_eyre::Result<()> {
        let mut s = AppState::test_default()?;
        s.library.playlists = vec![
            // 故意把「歌内命中」的歌单放在前面,排序若不生效会按原序输出。
            playlist_view("inner", "other", SourceKind::NETEASE, 1),
            playlist_view("named", "春日影", SourceKind::NETEASE, 1),
        ];
        let inner_id = PlaylistId::new(SourceKind::NETEASE, "inner");
        fill_tracks(&mut s, &inner_id, vec![with_name(song("s1"), "春日影")]);
        s.search.set_query("春日影");
        let names = s
            .filtered_playlists()
            .iter()
            .map(|p| p.data.name.clone())
            .collect::<Vec<String>>();
        assert_eq!(
            names,
            vec!["春日影".to_owned(), "other".to_owned()],
            "名字直接命中应排在加权歌内命中之前"
        );
        Ok(())
    }

    /// 数据到达失效:曲目落 cache(版本 bump)后,同一 query 的下一次过滤并入新歌单。
    #[test]
    fn new_tracks_invalidate_cache() -> color_eyre::Result<()> {
        let mut s = state_with_deep_tracks()?;
        s.search.set_query("春日");
        assert_eq!(s.filtered_playlists().len(), 1, "初始仅 p2 命中");
        // p1 的曲目此刻到达,内含同名命中曲。
        let p1 = PlaylistId::new(SourceKind::NETEASE, "p1");
        fill_tracks(&mut s, &p1, vec![with_name(song("s9"), "春日影 (cover)")]);
        assert_eq!(s.filtered_playlists().len(), 2, "p1 到数后应并入命中集");
        Ok(())
    }
}
