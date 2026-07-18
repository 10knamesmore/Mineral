//! Playlists 视图的深度搜索:搜索词穿透到歌单内歌曲(歌名 / 别名 / 艺人 / 专辑)。
//!
//! 全库 × 4 字段的 nucleo 打分开销大(千首级 × 每字段一次匹配),结果按
//! `(query, tracks 版本, 权重)` 缓存在 [`AppState::deep_search`],只在按键 /
//! 数据到达 / 配置热重载时重算,渲染帧只读。
//!
//! 覆盖范围 = `library.tracks` 已有的歌单;未拉到曲目的歌单只参与歌单名匹配
//! (全量补拉由进搜索态时的 [`crate::app::App`] 触发,结果渐进到达后经
//! `tracks_generation` 失效自然并入)。

use mineral_model::{PlaylistId, Song};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;

use crate::runtime::state::{LibraryData, SearchState};

/// 单个歌单的深度命中:加权分 + 行中部展示载荷。
///
/// 展示文本按段结构化交给渲染端拼装(`♪ <name>[ (<alias>)][ · <second>]`),
/// 命中下标是 [`Self::hit_field`] 所指字段**内**的 char 坐标,不做跨段平移——
/// 各段样式(别名括注暗调等)由渲染端决定。
#[derive(Clone, Debug)]
pub struct DeepHit {
    /// 歌单内最佳歌曲的加权分(字段权重已生效),与歌单名分取 max 排序用。
    pub score: f64,

    /// 最佳命中歌曲的 id(进歌单时定位光标用)。
    pub song_id: mineral_model::SongId,

    /// 主段:歌名。
    pub name: String,

    /// 括注别名(译名 / 副标题);有则恒展示,与歌单内曲目行的样式一致。
    pub alias: Option<String>,

    /// `·` 后次段:命中在歌名 / 别名时为首位艺人(无则省略),命中在艺人 / 专辑时为命中文本。
    pub second: Option<String>,

    /// [`Self::hits`] 落在哪个展示段。
    pub hit_field: HitField,

    /// 命中字段内的原文 char 下标,喂 highlight 渲染。
    pub hits: Vec<u32>,

    /// 该歌单内除最佳外还有几首命中(展示 `+n`;0 = 不显示)。
    pub extra: usize,
}

/// [`DeepHit::hits`] 的落点段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitField {
    /// 歌名主段。
    Name,

    /// 括注别名段。
    Alias,

    /// `·` 后的次段(艺人 / 专辑)。
    Second,
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

    /// `[name, alias, artist, album]` 权重位模式。
    weights: [u32; 4],

    /// 深度搜索总开关。
    deep: bool,
}

impl CacheKey {
    /// 以当前 state 算一份指纹。
    fn current(search: &SearchState, library: &LibraryData, cfg: &mineral_config::Config) -> Self {
        let scfg = cfg.tui().search().deep();
        Self {
            query: search.query().to_owned(),
            generation: library.tracks_generation,
            weights: weight_bits(scfg.weights()),
            deep: *scfg.enabled(),
        }
    }

    /// 与当前模型比对是否仍然有效(避免每帧 clone query 构 key)。
    fn matches(
        &self,
        search: &SearchState,
        library: &LibraryData,
        cfg: &mineral_config::Config,
    ) -> bool {
        let scfg = cfg.tui().search().deep();
        self.deep == *scfg.enabled()
            && self.generation == library.tracks_generation
            && self.weights == weight_bits(scfg.weights())
            && self.query == search.query()
    }
}

/// 四字段权重的位模式(`f32::to_bits`)——热重载改任一权重即指纹失配。
fn weight_bits(w: &mineral_config::DeepWeights) -> [u32; 4] {
    [
        w.name().to_bits(),
        w.alias().to_bits(),
        w.artist().to_bits(),
        w.album().to_bits(),
    ]
}

/// 保证缓存与当前 `(query, tracks 版本, 权重)` 一致,失配则重建。
///
/// 每帧可重复调用:命中指纹时只做几次整数 / 字符串比较。
pub fn ensure(search: &SearchState, library: &LibraryData, cfg: &mineral_config::Config) {
    if let Some(key) = &search.deep_cache.borrow().key
        && key.matches(search, library, cfg)
    {
        return;
    }
    let key = CacheKey::current(search, library, cfg);
    let hits = if key.deep && !key.query.is_empty() {
        build(search, library, cfg)
    } else {
        FxHashMap::default()
    };
    *search.deep_cache.borrow_mut() = DeepSearchCache {
        key: Some(key),
        hits,
    };
}

/// 全量重建:对 `library.tracks` 内每首歌按字段权重打分,每歌单留最佳一首 + 命中计数。
fn build(
    search: &SearchState,
    library: &LibraryData,
    cfg: &mineral_config::Config,
) -> FxHashMap<PlaylistId, DeepHit> {
    let weights = FieldWeights::from_cfg(cfg.tui().search().deep().weights());
    let mut out = FxHashMap::default();
    for (pid, tracks) in &library.tracks {
        let mut best: Option<SongHit<'_>> = None;
        let mut matched = 0usize;
        for sv in tracks {
            let Some(hit) = score_song(search, &sv.data, weights) else {
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

/// 深度搜索四字段的命中分折扣(已 clamp 到 `0.0..=1.0`),`0` = 该字段不参与。
#[derive(Clone, Copy)]
struct FieldWeights {
    /// 歌名。
    name: f64,

    /// 别名(译名 / 副标题)。
    alias: f64,

    /// 艺人(多艺人取最高)。
    artist: f64,

    /// 专辑。
    album: f64,
}

impl FieldWeights {
    /// 从配置读四字段权重并 clamp 到合法区间。
    fn from_cfg(w: &mineral_config::DeepWeights) -> Self {
        Self {
            name: f64::from(w.name().clamp(0.0, 1.0)),
            alias: f64::from(w.alias().clamp(0.0, 1.0)),
            artist: f64::from(w.artist().clamp(0.0, 1.0)),
            album: f64::from(w.album().clamp(0.0, 1.0)),
        }
    }
}

/// 一首歌的最佳字段命中(构建期中间态,借 song 的文本)。
struct SongHit<'a> {
    /// 加权分。
    score: f64,

    /// 命中歌曲(展示主段取 `name` + 括注 `alias`,定位取 `id`)。
    song: &'a Song,

    /// 展示次段:命中在歌名 / 别名时为首位艺人(无则省略),命中在艺人 / 专辑时为命中文本。
    second: Option<&'a str>,

    /// 命中下标落点段。
    hit_field: HitField,

    /// 命中字段内的原文 char 下标。
    hits: SmallVec<[u32; 8]>,
}

/// 对一首歌的四个字段分别打分取加权最高;全不命中(或权重全 0)返回 `None`。
///
/// 同分时按 歌名 > 别名 > 艺人 > 专辑 优先(评估顺序 + 严格大于才替换)。
fn score_song<'a>(search: &SearchState, song: &'a Song, w: FieldWeights) -> Option<SongHit<'a>> {
    let mut best: Option<SongHit<'a>> = None;
    if w.name > 0.0
        && let Some(m) = search.match_for(&song.name)
    {
        best = Some(SongHit {
            score: w.name * f64::from(m.score),
            song,
            second: song.artists.first().map(|a| a.name.as_str()),
            hit_field: HitField::Name,
            hits: m.hits,
        });
    }
    if w.alias > 0.0
        && let Some(alias) = &song.alias
        && let Some(m) = search.match_for(alias)
    {
        let score = w.alias * f64::from(m.score);
        if best.as_ref().is_none_or(|b| score > b.score) {
            best = Some(SongHit {
                score,
                song,
                second: song.artists.first().map(|a| a.name.as_str()),
                hit_field: HitField::Alias,
                hits: m.hits,
            });
        }
    }
    if w.artist > 0.0 {
        for artist in &song.artists {
            let Some(m) = search.match_for(&artist.name) else {
                continue;
            };
            let score = w.artist * f64::from(m.score);
            if best.as_ref().is_none_or(|b| score > b.score) {
                best = Some(SongHit {
                    score,
                    song,
                    second: Some(&artist.name),
                    hit_field: HitField::Second,
                    hits: m.hits,
                });
            }
        }
    }
    if w.album > 0.0
        && let Some(album) = &song.album
        && let Some(m) = search.match_for(&album.name)
    {
        let score = w.album * f64::from(m.score);
        if best.as_ref().is_none_or(|b| score > b.score) {
            best = Some(SongHit {
                score,
                song,
                second: Some(&album.name),
                hit_field: HitField::Second,
                hits: m.hits,
            });
        }
    }
    best
}

/// 组装展示载荷:借用态 [`SongHit`] 落成 owned [`DeepHit`],段结构原样携带,
/// 命中下标保持字段内坐标(拼装与平移都不在这里做)。
fn compose(b: &SongHit<'_>, extra: usize) -> DeepHit {
    DeepHit {
        score: b.score,
        song_id: b.song.id.clone(),
        name: b.song.name.clone(),
        alias: b.song.alias.clone(),
        second: b.second.map(str::to_owned),
        hit_field: b.hit_field,
        hits: b.hits.iter().copied().collect(),
        extra,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mineral_model::{PlaylistId, SourceKind};

    use super::HitField;
    use crate::runtime::state::AppState;
    use crate::runtime::view_model::SongView;
    use crate::test_support::{
        playlist_view, song, state_with_playlists, with_album, with_alias, with_artist, with_name,
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
    /// 载荷段 = 歌名「春日影」+ 次段首位艺人 CRYCHIC,命中落在歌名段(字段内坐标)。
    #[test]
    fn song_name_hit_surfaces_playlist() -> color_eyre::Result<()> {
        let mut s = state_with_deep_tracks()?;
        s.browse.search.set_query("春日");
        let names = s
            .filtered_playlists()
            .iter()
            .map(|p| p.data.name.clone())
            .collect::<Vec<String>>();
        assert_eq!(names, vec!["The Power of Failing".to_owned()], "仅 p2 命中");
        let hit = s
            .deep_hit_for(&p2())
            .ok_or_else(|| color_eyre::eyre::eyre!("p2 应有深度命中"))?;
        assert_eq!(hit.name, "春日影");
        assert_eq!(hit.second.as_deref(), Some("CRYCHIC"));
        assert_eq!(hit.hit_field, HitField::Name);
        assert_eq!(hit.hits, vec![0, 1], "春日 两字命中(歌名段内坐标)");
        assert_eq!(hit.extra, 0);
        Ok(())
    }

    /// 艺人命中:`mygo` 只落在「迷星叫」的艺人 MyGO!!!!! 上 → 命中落在次段
    /// (字段内坐标,无跨段平移)。
    #[test]
    fn artist_hit_lands_in_second_segment() -> color_eyre::Result<()> {
        let mut s = state_with_deep_tracks()?;
        s.browse.search.set_query("mygo");
        let _ = s.filtered_playlists();
        let hit = s
            .deep_hit_for(&p2())
            .ok_or_else(|| color_eyre::eyre::eyre!("p2 应有深度命中"))?;
        assert_eq!(hit.name, "迷星叫");
        assert_eq!(hit.second.as_deref(), Some("MyGO!!!!!"));
        assert_eq!(hit.hit_field, HitField::Second);
        assert_eq!(hit.hits, vec![0, 1, 2, 3], "MyGO 四字命中(次段内坐标)");
        Ok(())
    }

    /// 别名命中:歌名 / 艺人都不含「mayo」,但某曲别名 Mayoiuta 命中 → p2 被深度捞出,
    /// 命中落在括注别名段,次段回落首位艺人(与歌名命中同型:`♪ 歌名 (别名) · 艺人`)。
    #[test]
    fn alias_hit_lands_in_alias_segment() -> color_eyre::Result<()> {
        let mut s = state_with_playlists()?;
        let t = with_alias(
            with_artist(with_name(song("s2"), "迷星叫"), "MyGO!!!!!"),
            "Mayoiuta",
        );
        fill_tracks(&mut s, &p2(), vec![t]);
        s.browse.search.set_query("mayo");
        let _ = s.filtered_playlists();
        let hit = s
            .deep_hit_for(&p2())
            .ok_or_else(|| color_eyre::eyre::eyre!("p2 应有别名深度命中"))?;
        assert_eq!(hit.name, "迷星叫");
        assert_eq!(hit.alias.as_deref(), Some("Mayoiuta"));
        assert_eq!(hit.second.as_deref(), Some("MyGO!!!!!"), "次段回落首位艺人");
        assert_eq!(hit.hit_field, HitField::Alias);
        assert_eq!(hit.hits, vec![0, 1, 2, 3], "Mayo 四字命中(别名段内坐标)");
        Ok(())
    }

    /// 字段权重独立:alias 权重置 0 后,纯别名命中不再捞出歌单。
    #[test]
    fn zero_alias_weight_disables_alias() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.lua");
        std::fs::write(
            &path,
            "return { tui = { search = { deep = { weights = { alias = 0 } } } } }",
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
        let t = with_alias(with_name(song("s2"), "迷星叫"), "Mayoiuta");
        fill_tracks(&mut s, &p2(), vec![t]);
        s.browse.search.set_query("mayo");
        assert!(
            s.filtered_playlists().is_empty(),
            "alias 权重 0:纯别名命中不应捞出歌单"
        );
        assert!(s.deep_hit_for(&p2()).is_none());
        Ok(())
    }

    /// 多首命中计数:`迷` 同时命中「迷星叫」(歌名)与「春日影」(专辑〈迷途之子〉),
    /// 最佳一首之外 extra = 1。
    #[test]
    fn extra_counts_additional_matched_songs() -> color_eyre::Result<()> {
        let mut s = state_with_deep_tracks()?;
        s.browse.search.set_query("迷");
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
            "return { tui = { search = { deep = { weights = { artist = 0 } } } } }",
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
        s.browse.search.set_query("mygo");
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
        s.browse.search.set_query("春日影");
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
        s.browse.search.set_query("春日");
        assert_eq!(s.filtered_playlists().len(), 1, "初始仅 p2 命中");
        // p1 的曲目此刻到达,内含同名命中曲。
        let p1 = PlaylistId::new(SourceKind::NETEASE, "p1");
        fill_tracks(&mut s, &p1, vec![with_name(song("s9"), "春日影 (cover)")]);
        assert_eq!(s.filtered_playlists().len(), 2, "p1 到数后应并入命中集");
        Ok(())
    }
}
