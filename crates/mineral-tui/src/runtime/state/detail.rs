//! detail 面板的实体详情栈：结果列选中实体为栈底，下钻 push 一帧、back pop 一帧。
//!
//! 一种机制覆盖所有「看详情」：歌曲/专辑/歌单看曲目、artist 看热门曲与专辑、artist 专辑区
//! 进某专辑看曲目。栈帧携带已有的完整实体（头部立即可画），补充的列表/详情由对应
//! fetch 任务回填。`frames[0]` 即 root（对应结果列选中行），其上是下钻帧。

use std::cell::Cell;

use mineral_channel_core::{ArtistSectionKind, ArtistSections};
use mineral_model::{
    Album, AlbumId, Artist, ArtistId, MediaUrl, Playlist, PlaylistId, SearchKind, Song, SourceKind,
};
use mineral_task::SearchPayload;

use crate::render::anim::{Toggle, Transition};
use crate::runtime::scroll::list::ScrollList;

/// 一帧详情要补拉的内容（携带目标 id，供派发与回包配对）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DetailFetch {
    /// 专辑/歌曲帧 → 同专辑曲目（`AlbumDetail` 任务）。
    AlbumDetail(AlbumId),

    /// 歌单帧 → 曲目（`PlaylistDetail` 任务）。
    PlaylistDetail(PlaylistId),

    /// artist 帧 → 详情(热门曲) + 专辑列表（`ArtistDetail` + `ArtistAlbums` 两任务）。
    Artist(ArtistId),
}

impl DetailFetch {
    /// 去重键（per 实体一份，跨类型不碰撞）：驻留派发用它防重复发同一实体的拉取。
    pub fn dedup_key(&self) -> String {
        match self {
            Self::AlbumDetail(id) => format!("album:{}", id.qualified()),
            Self::PlaylistDetail(id) => format!("playlist:{}", id.qualified()),
            Self::Artist(id) => format!("artist:{}", id.qualified()),
        }
    }

    /// 该拉取目标的来源 source（id namespace 派生）：封面搭车投递时定来源用。
    pub fn source(&self) -> SourceKind {
        match self {
            Self::AlbumDetail(id) => id.namespace(),
            Self::PlaylistDetail(id) => id.namespace(),
            Self::Artist(id) => id.namespace(),
        }
    }
}

/// 栈帧指向的实体，携带结果列/上钻已有的完整数据（头部不必等 fetch）。
#[derive(Clone)]
pub enum EntityRef {
    /// 歌曲：详情等同其所属专辑（头图=专辑封面、列表=同专辑曲目）。
    Song(Box<Song>),

    /// 专辑。
    Album(Box<Album>),

    /// artist。
    Artist(Box<Artist>),

    /// 歌单。
    Playlist(Box<Playlist>),
}

impl EntityRef {
    /// 从一页结果载荷的第 `idx` 项构造（越界返回 `None`）。
    ///
    /// # Params:
    ///   - `payload`: 结果载荷（单一实体类型）
    ///   - `idx`: 结果列光标下标
    ///
    /// # Return:
    ///   对应实体的 `EntityRef`；`idx` 越界为 `None`。
    pub fn from_payload(payload: &SearchPayload, idx: usize) -> Option<Self> {
        match payload {
            SearchPayload::Songs(v) => v.get(idx).cloned().map(Box::new).map(Self::Song),
            SearchPayload::Albums(v) => v.get(idx).cloned().map(Box::new).map(Self::Album),
            SearchPayload::Artists(v) => v.get(idx).cloned().map(Box::new).map(Self::Artist),
            SearchPayload::Playlists(v) => v.get(idx).cloned().map(Box::new).map(Self::Playlist),
        }
    }

    /// 头图源：歌曲/专辑/歌单取 `cover_url`、artist 取 `avatar_url`；无图为 `None`。
    pub fn cover(&self) -> Option<&MediaUrl> {
        match self {
            Self::Song(s) => s.cover_url.as_ref(),
            Self::Album(a) => a.cover_url.as_ref(),
            Self::Artist(a) => a.avatar_url.as_ref(),
            Self::Playlist(p) => p.cover_url.as_ref(),
        }
    }

    /// 展示名（头部标题）。
    pub fn name(&self) -> &str {
        match self {
            Self::Song(s) => &s.name,
            Self::Album(a) => &a.name,
            Self::Artist(a) => &a.name,
            Self::Playlist(p) => &p.name,
        }
    }

    /// 该实体对应的 [`SearchKind`]（与结果列 tab 同一套）——detail 顶栏 title 据此取图标/类型词。
    pub fn kind(&self) -> SearchKind {
        match self {
            Self::Song(_) => SearchKind::Song,
            Self::Album(_) => SearchKind::Album,
            Self::Artist(_) => SearchKind::Artist,
            Self::Playlist(_) => SearchKind::Playlist,
        }
    }

    /// 该实体的详情要拉什么；歌曲无所属专辑（单曲）返回 `None`（降级：只画歌曲卡片）。
    pub fn fetch(&self) -> Option<DetailFetch> {
        match self {
            Self::Song(s) => s
                .album
                .as_ref()
                .map(|a| DetailFetch::AlbumDetail(a.id.clone())),
            Self::Album(a) => Some(DetailFetch::AlbumDetail(a.id.clone())),
            Self::Artist(a) => Some(DetailFetch::Artist(a.id.clone())),
            Self::Playlist(p) => Some(DetailFetch::PlaylistDetail(p.id.clone())),
        }
    }
}

/// 一帧补拉到的数据。
#[derive(Clone)]
pub enum DetailData {
    /// 曲目列表（歌单帧）。
    Tracks(Vec<Song>),

    /// 专辑完整详情（元信息 + 曲目）——album 帧、以及 song 帧（看其所属专辑）。
    Album(Box<Album>),

    /// artist 帧两路（详情含热门曲 + 专辑列表），分别到货。
    Artist {
        /// artist 详情（`songs` 为热门曲），`None` = 未到。
        detail: Option<Box<Artist>>,

        /// 专辑列表，`None` = 未到。
        albums: Option<Vec<Album>>,
    },
}

/// artist 帧的面板内分区（`[` / `]` 切换）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ArtistSection {
    /// 热门曲。
    #[default]
    Hot,

    /// 专辑。
    Albums,
}

impl ArtistSection {
    /// 映射到 caps 的分区种类（[`ArtistSectionKind`]）：渲染分区与能力声明的桥。
    fn kind(self) -> ArtistSectionKind {
        match self {
            Self::Hot => ArtistSectionKind::TopSongs,
            Self::Albums => ArtistSectionKind::Albums,
        }
    }

    /// 从 caps 的分区种类映射回渲染分区。
    fn from_kind(kind: ArtistSectionKind) -> Self {
        match kind {
            ArtistSectionKind::TopSongs => Self::Hot,
            ArtistSectionKind::Albums => Self::Albums,
        }
    }
}

/// detail 栈的一帧：实体 + 补拉数据 + 面板内导航位置。
#[derive(Clone)]
pub struct DetailFrame {
    /// 这一帧展示的实体（头部直接从它渲染）。
    pub entity: EntityRef,

    /// 补拉到的列表/详情，`None` = 未到（渲染占位骨架）。
    pub data: Option<DetailData>,

    /// artist 帧当前分区（非 artist 帧忽略）。
    pub section: ArtistSection,

    /// 该 artist 源的可用分区（`caps.artist_sections`）。`None` = caps 尚未落定的瞬态（建帧到
    /// [`Self::apply_sections`] 之间，同一事件内 apply 先于 render 故不被观测）。落定后：默认区落
    /// 首个可用区、`[`/`]` 只在两区皆有时切、渲染按可用区决定画几个 tab。非 artist 帧忽略。
    artist_sections: Option<ArtistSections>,

    /// artist 双区切换的横向滑动（off=Top Songs / on=Albums）。`None` = 从未切过（恒 Top Songs，
    /// 无动画）；首次切换按 sweep 拍数懒构造，复用 browse view-sweep 同款 [`Toggle`]。
    section_anim: Option<Toggle>,

    /// 面板内列表光标 + 视口滚动(nvim 手感:offset 跨帧持久 + scrolloff + 缓动平移)。
    /// artist 帧 Hot/Albums 双区共用此一份(切区时 [`Self::cycle_section`] 瞬时归位)。
    list: ScrollList,

    /// 头部简介的滚动 offset（可视行）。render 端折行后把它钳进内容边界并写回（渲染走
    /// `&self`，故内部可变）；C-d/u/b/f 经 [`Self::nudge_description`] 平移。
    desc_scroll: Cell<u16>,

    /// 这一帧是否已派过 detail 拉取（防同帧重复派；新帧 / 移光标后复位为可再派）。
    requested: bool,
}

impl DetailFrame {
    /// 新帧：光标归零、未拉数据、分区回热门曲、简介滚回顶、未派拉取。
    fn new(entity: EntityRef) -> Self {
        Self {
            entity,
            data: None,
            section: ArtistSection::default(),
            artist_sections: None,
            section_anim: None,
            list: ScrollList::new(),
            desc_scroll: Cell::new(0),
            requested: false,
        }
    }

    /// 该帧 artist 源的可用分区（`None` = caps 未落定;渲染据此决定画哪些 tab）。非 artist 帧无意义。
    pub fn artist_sections(&self) -> Option<&ArtistSections> {
        self.artist_sections.as_ref()
    }

    /// 落定 artist 源的可用分区(`caps.artist_sections`),并把当前分区收到首个可用区。由持 caps 的
    /// 上层在建 / 复位 artist root 帧后调用(幂等):如 B站只有 Albums,分区从默认的 Hot 收到 Albums。
    pub fn apply_sections(&mut self, sections: ArtistSections) {
        if let Some(first) = sections.kinds().first() {
            self.section = ArtistSection::from_kind(*first);
        }
        self.artist_sections = Some(sections);
    }

    /// 面板内列表的光标 + 视口滚动态(渲染 / 选中读取)。
    pub(crate) fn list(&self) -> &ScrollList {
        &self.list
    }

    /// 面板内列表态(可变)：按键路径移光标 / 翻页;数据到货后定位。
    pub(crate) fn list_mut(&mut self) -> &mut ScrollList {
        &mut self.list
    }

    /// 头部简介滚动 offset 句柄：render 端钳进内容边界并写回。
    pub fn description_scroll(&self) -> &Cell<u16> {
        &self.desc_scroll
    }

    /// 简介滚动平移 `delta` 行（向下为正；下界钳 0，上界由 render 端按内容高度钳）。
    /// C-d/u/b/f 在 detail 焦点经此（与列表光标 `j/k` 互不干扰）。
    pub fn nudge_description(&self, delta: i64) {
        let next = i64::from(self.desc_scroll.get())
            .saturating_add(delta)
            .max(0);
        self.desc_scroll
            .set(u16::try_from(next).unwrap_or(u16::MAX));
    }

    /// 沿本源可用分区列表前进一格并 arm 横向过渡（复用 browse view-sweep 同款 [`Toggle`]）。
    /// 光标归零；`ticks` 为切换拍数（由 sweep 配置折算，与下钻滑动同源）。
    ///
    /// 只有一个可用区（如 B站仅 Albums）或 caps 未落定 → no-op：无处可切。
    pub fn cycle_section(&mut self, ticks: u16) {
        let Some(kinds) = self.artist_sections.as_ref().map(ArtistSections::kinds) else {
            return;
        };
        if kinds.len() < 2 {
            return;
        }
        let here = kinds
            .iter()
            .position(|k| *k == self.section.kind())
            .unwrap_or(0);
        let next = (here + 1) % kinds.len();
        let Some(&target) = kinds.get(next) else {
            return;
        };
        self.section = ArtistSection::from_kind(target);
        // 各区共用一份列表态:切区瞬时归位顶端(无平移),区间长度不同也不残留旧 offset。
        self.list.place(0, 0);
        // 动画 [`Toggle`] 二态(off=列表首区 / on=次区);当前每源至多两区,>2 时仅首两区间平滑。
        self.section_anim
            .get_or_insert_with(|| Toggle::new(ticks.max(1)))
            .set(next == 1);
    }

    /// 双区切换的横向滑动进度（`0` = Top Songs、满值 = Albums）；未在动画 → `None`（渲染单区）。
    pub fn section_eased(&self) -> Option<u16> {
        self.section_anim
            .as_ref()
            .filter(|t| !t.settled())
            .map(Toggle::eased_in_out)
    }

    /// 推进双区切换动画一拍（由 [`DetailStack::tick`] 对栈顶帧调用）。
    fn tick_section(&mut self) {
        if let Some(anim) = &mut self.section_anim {
            anim.tick();
        }
    }

    /// 是否还需派 detail 拉取：未派过且数据未到（防风暴；失败后换帧即可重试）。
    pub fn needs_fetch(&self) -> bool {
        !self.requested && self.data.is_none()
    }

    /// 标记已派 detail 拉取（同帧不重复发）。
    pub fn mark_requested(&mut self) {
        self.requested = true;
    }

    /// 当前区列表长度（曲目 / 专辑 / artist 热门曲 / artist 专辑，按 section + data；未到为 0）。
    /// detail 焦点的 `j`/`k` 据此钳光标。
    pub fn list_len(&self) -> usize {
        match (&self.entity, self.section, &self.data) {
            (
                EntityRef::Artist(_),
                ArtistSection::Hot,
                Some(DetailData::Artist {
                    detail: Some(a), ..
                }),
            ) => a.songs.len(),
            (
                EntityRef::Artist(_),
                ArtistSection::Albums,
                Some(DetailData::Artist {
                    albums: Some(albs), ..
                }),
            ) => albs.len(),
            (EntityRef::Artist(_), _, _) => 0,
            (_, _, Some(DetailData::Album(a))) => a.songs.len(),
            (_, _, Some(DetailData::Tracks(songs))) => songs.len(),
            _ => 0,
        }
    }

    /// 当前区列表选中行对应的实体（detail 焦点的行级菜单 `y`/`o` 据此构造项）：歌单/专辑/
    /// 歌曲帧曲目行、artist Hot 区热门曲 → `Song`；artist Albums 区 → `Album`（容器，菜单走
    /// 播放全部）；数据未到 / 越界 → `None`。与 [`Self::list_len`] 同一套
    /// `(entity, section, data)` 分流——`j`/`k` 钳的行与菜单取的行必须是同一行。
    pub(crate) fn row_entity(&self) -> Option<EntityRef> {
        let sel = self.list.sel();
        match (&self.entity, self.section, &self.data) {
            (
                EntityRef::Artist(_),
                ArtistSection::Hot,
                Some(DetailData::Artist {
                    detail: Some(a), ..
                }),
            ) => a.songs.get(sel).cloned().map(Box::new).map(EntityRef::Song),
            (
                EntityRef::Artist(_),
                ArtistSection::Albums,
                Some(DetailData::Artist {
                    albums: Some(albs), ..
                }),
            ) => albs.get(sel).cloned().map(Box::new).map(EntityRef::Album),
            (EntityRef::Artist(_), _, _) => None,
            (_, _, Some(DetailData::Album(a))) => {
                a.songs.get(sel).cloned().map(Box::new).map(EntityRef::Song)
            }
            (_, _, Some(DetailData::Tracks(songs))) => {
                songs.get(sel).cloned().map(Box::new).map(EntityRef::Song)
            }
            _ => None,
        }
    }

    /// 当前区作为「歌曲列表」的视图（`Play` 的队列上下文 = 这一列整列，语义同 activate
    /// 起播）：歌单/专辑/歌曲帧曲目、artist Hot 区热门曲；artist Albums 区（行是专辑容器，不入
    /// 队）/ 数据未到 → 空。与 [`Self::row_entity`] 取 `Song` 的几路一一对应。
    pub(crate) fn song_list(&self) -> Vec<Song> {
        match (&self.entity, self.section, &self.data) {
            (
                EntityRef::Artist(_),
                ArtistSection::Hot,
                Some(DetailData::Artist {
                    detail: Some(a), ..
                }),
            ) => a.songs.clone(),
            (EntityRef::Artist(_), _, _) => Vec::new(),
            (_, _, Some(DetailData::Album(a))) => a.songs.clone(),
            (_, _, Some(DetailData::Tracks(songs))) => songs.clone(),
            _ => Vec::new(),
        }
    }

    /// 从这一帧起播时的队列语境（埋点 provenance）：专辑帧 / 歌曲帧看所属专辑 → `Album`、
    /// artist Hot 区 → `Artist`、歌单帧 → `Playlist`；数据未到 / 无可播容器（artist Albums 区行是
    /// 专辑容器，不在此起单曲队列）→ `None`。与 [`Self::song_list`] 取整列的几路一一对应——
    /// 起播的那一列曲目归属哪个容器，语境就报哪个。
    pub(crate) fn play_context(&self) -> Option<mineral_protocol::QueueContextWire> {
        use mineral_protocol::QueueContextWire;
        match (&self.entity, self.section, &self.data) {
            (
                EntityRef::Artist(a),
                ArtistSection::Hot,
                Some(DetailData::Artist {
                    detail: Some(_), ..
                }),
            ) => Some(QueueContextWire::Artist {
                id: a.id.clone(),
                name: Some(a.name.clone()),
            }),
            (EntityRef::Artist(_), _, _) => None,
            (_, _, Some(DetailData::Album(a))) => Some(QueueContextWire::Album {
                id: a.id.clone(),
                name: Some(a.name.clone()),
            }),
            (EntityRef::Playlist(p), _, Some(DetailData::Tracks(_))) => {
                Some(QueueContextWire::Playlist {
                    id: p.id.clone(),
                    name: Some(p.name.clone()),
                })
            }
            _ => None,
        }
    }

    /// artist 帧头部 meta 该用哪份 artist：优先 fetch 回来的完整 detail（channel 已聚合成
    /// 字段齐全的 `Artist`——fans/计数/简介都有），未到货退回结果列 entity 占位（仅 name/fans）。
    /// 非 artist 帧 → `None`。渲染层据此读字段，不必关心数据来自哪个端点。
    pub fn artist_meta(&self) -> Option<&Artist> {
        let EntityRef::Artist(entity) = &self.entity else {
            return None;
        };
        let detail = match &self.data {
            Some(DetailData::Artist { detail, .. }) => detail.as_deref(),
            _ => None,
        };
        Some(detail.unwrap_or(&**entity))
    }

    /// album 帧头部 meta 该用哪份 album：优先 fetch 回来的完整 detail（含简介 / 发行信息），
    /// 未到货退回结果列 entity 占位（搜索那份,简介常缺）。非 album 帧 → `None`。
    pub fn album_meta(&self) -> Option<&Album> {
        let EntityRef::Album(entity) = &self.entity else {
            return None;
        };
        let detail = match &self.data {
            Some(DetailData::Album(a)) => Some(a.as_ref()),
            _ => None,
        };
        Some(detail.unwrap_or(&**entity))
    }

    /// 当前区列表选中项的封面（artist 帧：Hot→歌的专辑封面 / Albums→专辑封面）；非 artist
    /// 帧 / 数据未到 / 选中项无封面 → `None`。供右栏副头图渲染与 prefetch 搭车共用。
    pub fn selected_cover(&self) -> Option<&MediaUrl> {
        let (EntityRef::Artist(_), Some(DetailData::Artist { detail, albums })) =
            (&self.entity, &self.data)
        else {
            return None;
        };
        match self.section {
            ArtistSection::Hot => detail
                .as_ref()
                .and_then(|a| a.songs.get(self.list.sel()))
                .and_then(|s| s.cover_url.as_ref()),
            ArtistSection::Albums => albums
                .as_ref()
                .and_then(|v| v.get(self.list.sel()))
                .and_then(|al| al.cover_url.as_ref()),
        }
    }

    /// 落 `Tracks` 数据（歌单帧）。
    pub fn set_tracks(&mut self, songs: Vec<Song>) {
        self.data = Some(DetailData::Tracks(songs));
    }

    /// 落专辑完整详情（album 帧 / song 帧）。song 帧顺手把列表光标落到这首歌在专辑里的位置——
    /// 高亮 + `j/k` 起点一次对齐，让「选中歌在其所属专辑中」一目了然。
    pub fn set_album_detail(&mut self, album: Box<Album>) {
        if let EntityRef::Song(s) = &self.entity
            && let Some(idx) = album.songs.iter().position(|t| t.id == s.id)
        {
            // 视口瞬时定位到这首歌(无平移);渲染端首帧按 scrolloff 钳,选中歌即带上下文出现。
            self.list.place(idx, 0);
        }
        self.data = Some(DetailData::Album(album));
    }

    /// 落 artist 详情（热门曲那一路）；与 `albums` 那一路合并，先到的不被覆盖。
    pub fn set_artist_detail(&mut self, artist: Box<Artist>) {
        match &mut self.data {
            Some(DetailData::Artist { detail, .. }) => *detail = Some(artist),
            _ => {
                self.data = Some(DetailData::Artist {
                    detail: Some(artist),
                    albums: None,
                });
            }
        }
    }

    /// 落 artist 专辑列表那一路；与 `detail` 那一路合并。
    pub fn set_artist_albums(&mut self, albums: Vec<Album>) {
        match &mut self.data {
            Some(DetailData::Artist { albums: slot, .. }) => *slot = Some(albums),
            _ => {
                self.data = Some(DetailData::Artist {
                    detail: None,
                    albums: Some(albums),
                });
            }
        }
    }
}

/// detail 面板内的实体详情栈：`frames[0]` 是 root（结果列选中行），其上是下钻帧。
/// 下钻/返回带横向滑动过渡（push 右入、pop 左入），与左栏 playlist↔tracks 同机制。
pub struct DetailStack {
    /// 栈帧，底为 root；空 = 无选中实体（渲染空态）。
    frames: Vec<DetailFrame>,

    /// 下钻/返回的横向滑动过渡（settled = 无滑动，渲染直接画当前帧含真图头图）。
    transition: Transition,

    /// 过渡中的「出发帧」+ 方向（`true` = push 右入、`false` = pop 左入）；settled 后清空。
    /// 滑动期离屏合成此帧与当前帧，头图走程序化占位（不上 kitty 真图）。
    sweep_from: Option<(Box<DetailFrame>, bool)>,
}

impl DetailStack {
    /// 空栈（无选中实体）。
    pub fn empty() -> Self {
        Self {
            frames: Vec::new(),
            transition: Transition::new(1),
            sweep_from: None,
        }
    }

    /// 以 `entity` 为 root 的栈（结果列选中即建）。
    pub fn rooted(entity: EntityRef) -> Self {
        Self {
            frames: vec![DetailFrame::new(entity)],
            transition: Transition::new(1),
            sweep_from: None,
        }
    }

    /// 当前帧（栈顶）；空栈为 `None`。
    pub fn current(&self) -> Option<&DetailFrame> {
        self.frames.last()
    }

    /// 当前帧（可变）；空栈为 `None`。
    pub fn current_mut(&mut self) -> Option<&mut DetailFrame> {
        self.frames.last_mut()
    }

    /// 下钻一帧（push），arm `ticks` 拍右入滑动。空栈时忽略（无 root 不该下钻）。
    pub fn push(&mut self, entity: EntityRef, ticks: u16) {
        let Some(top) = self.frames.last() else {
            return;
        };
        self.sweep_from = Some((Box::new(top.clone()), true));
        self.frames.push(DetailFrame::new(entity));
        self.transition = Transition::expanding(ticks.max(1));
    }

    /// 退一帧（pop），arm `ticks` 拍左入滑动；弹掉返回 `true`，已在 root 返回 `false`。
    pub fn pop(&mut self, ticks: u16) -> bool {
        if self.frames.len() > 1 {
            if let Some(popped) = self.frames.pop() {
                self.sweep_from = Some((Box::new(popped), false));
                self.transition = Transition::expanding(ticks.max(1));
            }
            true
        } else {
            false
        }
    }

    /// 换 root 实体并清空下钻栈（结果列选中变化时调；不滑动）。
    pub fn reset_to(&mut self, entity: EntityRef) {
        self.frames = vec![DetailFrame::new(entity)];
        self.transition = Transition::new(1);
        self.sweep_from = None;
    }

    /// 下钻深度：0 = 只看 root（或空栈）、N = root 之上压了 N 帧。
    pub fn depth(&self) -> usize {
        self.frames.len().saturating_sub(1)
    }

    /// 每一帧的 `(类型, 名)` 链，root→top——供 detail 顶栏 breadcrumb title 组装。
    /// 空栈 → 空 `Vec`（无实体可标，调用方回退固定标题）。
    pub fn title_crumbs(&self) -> Vec<(SearchKind, &str)> {
        self.frames
            .iter()
            .map(|f| (f.entity.kind(), f.entity.name()))
            .collect()
    }

    /// 推进滑动一拍；过渡 settle 后清出发帧。同时推进栈顶帧的双区切换动画。
    pub fn tick(&mut self) {
        self.transition.tick();
        if self.transition.settled() {
            self.sweep_from = None;
        }
        if let Some(frame) = self.frames.last_mut() {
            frame.tick_section();
        }
    }

    /// 滑动渲染参数：`(出发帧, 目标帧, ease-in-out 进度, is_push)`；未过渡为 `None`（渲染直接
    /// 画当前帧）。进度走 ease-in-out（与 artist 双区切换 / 左栏视图切换同曲线，两端减速、打断
    /// 反向连续），不用单向 ease-out。
    pub fn sweep_frames(&self) -> Option<(&DetailFrame, &DetailFrame, u16, bool)> {
        if self.transition.settled() {
            return None;
        }
        let (from, is_push) = self.sweep_from.as_ref()?;
        let to = self.frames.last()?;
        Some((from, to, self.transition.eased_in_out(), *is_push))
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::{
        Album, AlbumId, AlbumRef, Artist, ArtistId, MediaUrl, Playlist, PlaylistId, Song, SongId,
        SourceKind,
    };
    use mineral_task::SearchPayload;

    use mineral_channel_core::{ArtistSectionKind, ArtistSections};

    use super::{ArtistSection, DetailFetch, DetailStack, EntityRef};

    /// 两区皆有的 artist 分区(音乐源形态测试夹具)。
    fn both_sections() -> ArtistSections {
        ArtistSections::new(vec![ArtistSectionKind::TopSongs, ArtistSectionKind::Albums])
    }

    /// 造一首歌；`album` 给所属专辑 id（`None` = 单曲）。
    fn song(raw: &str, album: Option<&str>) -> Song {
        Song::builder()
            .id(SongId::new(SourceKind::NETEASE, raw))
            .name(format!("song {raw}"))
            .album(album.map(|a| AlbumRef {
                id: AlbumId::new(SourceKind::NETEASE, a),
                name: format!("album {a}"),
            }))
            .duration_ms(Some(1000))
            .build()
    }

    /// 造一张专辑。
    fn album(raw: &str) -> Album {
        Album::builder()
            .id(AlbumId::new(SourceKind::NETEASE, raw))
            .name(format!("album {raw}"))
            .build()
    }

    /// 造一个 artist（带头像，验证 `cover` 取 avatar）。
    fn artist(raw: &str) -> Artist {
        Artist::builder()
            .id(ArtistId::new(SourceKind::NETEASE, raw))
            .name(format!("artist {raw}"))
            .avatar_url(MediaUrl::remote("https://avatar/a.jpg").ok())
            .build()
    }

    /// 造一个歌单。
    fn playlist(raw: &str) -> Playlist {
        Playlist::builder()
            .id(PlaylistId::new(SourceKind::NETEASE, raw))
            .name(format!("playlist {raw}"))
            .build()
    }

    /// `from_payload` 按类型取第 idx 项；越界为 `None`。
    #[test]
    fn from_payload_picks_indexed_entity() -> color_eyre::Result<()> {
        let payload = SearchPayload::Albums(vec![album("a1"), album("a2")]);
        let Some(EntityRef::Album(got)) = EntityRef::from_payload(&payload, 1) else {
            color_eyre::eyre::bail!("应取到第 2 张专辑");
        };
        assert_eq!(got.name, "album a2");
        assert!(
            EntityRef::from_payload(&payload, 9).is_none(),
            "越界应为 None"
        );
        Ok(())
    }

    /// `fetch`：歌曲带专辑 → AlbumDetail(专辑 id)；单曲 → None。
    #[test]
    fn song_fetch_targets_album_or_none() {
        let with = EntityRef::Song(Box::new(song("s1", Some("al"))));
        assert_eq!(
            with.fetch(),
            Some(DetailFetch::AlbumDetail(AlbumId::new(
                SourceKind::NETEASE,
                "al"
            ))),
            "歌曲详情拉所属专辑曲目"
        );
        let single = EntityRef::Song(Box::new(song("s2", None)));
        assert_eq!(single.fetch(), None, "单曲无专辑 → 不拉、降级");
    }

    /// `fetch`：专辑→AlbumDetail(自身)、歌单→PlaylistDetail、artist→Artist。
    #[test]
    fn fetch_per_entity_kind() {
        assert_eq!(
            EntityRef::Album(Box::new(album("al"))).fetch(),
            Some(DetailFetch::AlbumDetail(AlbumId::new(
                SourceKind::NETEASE,
                "al"
            )))
        );
        assert_eq!(
            EntityRef::Playlist(Box::new(playlist("pl"))).fetch(),
            Some(DetailFetch::PlaylistDetail(PlaylistId::new(
                SourceKind::NETEASE,
                "pl"
            )))
        );
        assert_eq!(
            EntityRef::Artist(Box::new(artist("ar"))).fetch(),
            Some(DetailFetch::Artist(ArtistId::new(
                SourceKind::NETEASE,
                "ar"
            )))
        );
    }

    /// artist `cover` 取 avatar_url（非 cover_url）。
    #[test]
    fn artist_cover_is_avatar() {
        let a = EntityRef::Artist(Box::new(artist("ar")));
        assert!(a.cover().is_some(), "artist 头图取 avatar_url");
    }

    /// dedup_key 跨类型不碰撞、同实体稳定。
    #[test]
    fn dedup_key_is_per_entity() {
        let al = DetailFetch::AlbumDetail(AlbumId::new(SourceKind::NETEASE, "x"));
        let pl = DetailFetch::PlaylistDetail(PlaylistId::new(SourceKind::NETEASE, "x"));
        assert_ne!(al.dedup_key(), pl.dedup_key(), "裸值同但类型不同 → 键不同");
        assert_eq!(al.dedup_key(), al.clone().dedup_key(), "同实体键稳定");
    }

    /// 栈：rooted→current=root depth0；push→depth1 current=新帧；pop→回 root true；root 处 pop=false。
    #[test]
    fn stack_push_pop_keeps_root() {
        let mut st = DetailStack::rooted(EntityRef::Artist(Box::new(artist("ar"))));
        assert_eq!(st.depth(), 0, "只 root");
        assert_eq!(st.current().map(entity_name), Some("artist ar".to_owned()));

        st.push(EntityRef::Album(Box::new(album("al"))), 1);
        assert_eq!(st.depth(), 1, "下钻一层");
        assert_eq!(st.current().map(entity_name), Some("album al".to_owned()));

        assert!(st.pop(1), "弹出下钻帧");
        assert_eq!(st.depth(), 0, "回到 root");
        assert_eq!(st.current().map(entity_name), Some("artist ar".to_owned()));

        assert!(!st.pop(1), "root 处不再弹");
        assert!(st.current().is_some(), "root 仍在");
    }

    /// reset_to 换 root + 清下钻栈；空栈 push 被忽略。
    #[test]
    fn reset_clears_drill_and_empty_ignores_push() {
        let mut st = DetailStack::rooted(EntityRef::Artist(Box::new(artist("ar"))));
        st.push(EntityRef::Album(Box::new(album("al"))), 1);
        st.reset_to(EntityRef::Playlist(Box::new(playlist("pl"))));
        assert_eq!(st.depth(), 0, "reset 清下钻栈");
        assert_eq!(
            st.current().map(entity_name),
            Some("playlist pl".to_owned())
        );

        let mut empty = DetailStack::empty();
        empty.push(EntityRef::Album(Box::new(album("al"))), 1);
        assert!(empty.current().is_none(), "空栈 push 忽略");
    }

    /// 单区源(仅 Albums,如 B站):apply_sections 把分区收到唯一可用区,cycle_section 为 no-op。
    #[test]
    fn single_section_defaults_and_no_cycle() {
        let mut frame = super::DetailFrame::new(EntityRef::Artist(Box::new(artist("ar"))));
        frame.apply_sections(ArtistSections::new(vec![ArtistSectionKind::Albums]));
        assert_eq!(
            frame.section,
            ArtistSection::Albums,
            "单区源默认落到唯一可用区(专辑),不是默认的 Hot"
        );
        frame.cycle_section(/*ticks*/ 4);
        assert_eq!(
            frame.section,
            ArtistSection::Albums,
            "单区无处可切,cycle no-op"
        );
        assert!(frame.section_eased().is_none(), "单区不 arm 滑动");
    }

    /// selected_cover：Hot 区取选中歌封面、Albums 区取选中专辑封面、非 artist 帧为 None。
    #[test]
    fn selected_cover_follows_section() -> color_eyre::Result<()> {
        use mineral_model::{Album, AlbumId, Artist, ArtistId, MediaUrl, Song, SongId};

        let detail = Artist::builder()
            .id(ArtistId::new(SourceKind::NETEASE, "ar"))
            .name("ar".to_owned())
            .songs(vec![
                Song::builder()
                    .id(SongId::new(SourceKind::NETEASE, "s0"))
                    .name("s0".to_owned())
                    .cover_url(Some(MediaUrl::remote("https://c/song.jpg")?))
                    .build(),
            ])
            .build();
        let album0 = Album::builder()
            .id(AlbumId::new(SourceKind::NETEASE, "al0"))
            .name("al0".to_owned())
            .cover_url(Some(MediaUrl::remote("https://c/album.jpg")?))
            .build();

        let mut frame = super::DetailFrame::new(EntityRef::Artist(Box::new(artist("ar"))));
        frame.set_artist_detail(Box::new(detail));
        frame.set_artist_albums(vec![album0]);

        frame.section = ArtistSection::Hot;
        assert_eq!(
            frame.selected_cover(),
            Some(&MediaUrl::remote("https://c/song.jpg")?),
            "Hot 区 → 选中歌封面"
        );
        frame.section = ArtistSection::Albums;
        assert_eq!(
            frame.selected_cover(),
            Some(&MediaUrl::remote("https://c/album.jpg")?),
            "Albums 区 → 选中专辑封面"
        );

        let other = super::DetailFrame::new(EntityRef::Album(Box::new(album("al"))));
        assert!(other.selected_cover().is_none(), "非 artist 帧无副头图");
        Ok(())
    }

    /// artist_meta：fetch 到货整份用聚合 detail（fans/计数/简介都来自它），未到货退回 entity 占位。
    #[test]
    fn artist_meta_prefers_fetched_detail() -> color_eyre::Result<()> {
        use mineral_model::{Artist, ArtistId};

        // 结果列 entity 份：搜索来的，只有 name + fans。
        let entity = Artist::builder()
            .id(ArtistId::new(SourceKind::NETEASE, "ar"))
            .name("CF".to_owned())
            .follower_count(Some(176_393))
            .build();
        let mut frame = super::DetailFrame::new(EntityRef::Artist(Box::new(entity)));

        // 未到货：退回 entity（fans 占位、无计数 / 简介）。
        let before = frame
            .artist_meta()
            .ok_or_else(|| color_eyre::eyre::eyre!("artist 帧应有 meta"))?;
        assert_eq!(before.follower_count, Some(176_393));
        assert_eq!(before.song_count, None, "entity 份无计数");
        assert!(before.description.is_empty(), "entity 份无简介");

        // fetch 到货：整份用聚合 detail（fans 非 0、计数 / 简介齐）。
        frame.set_artist_detail(Box::new(
            Artist::builder()
                .id(ArtistId::new(SourceKind::NETEASE, "ar"))
                .name("CF".to_owned())
                .follower_count(Some(176_396))
                .album_count(Some(8))
                .song_count(Some(43))
                .description("emo band".to_owned())
                .build(),
        ));
        let after = frame
            .artist_meta()
            .ok_or_else(|| color_eyre::eyre::eyre!("artist 帧应有 meta"))?;
        assert_eq!(after.follower_count, Some(176_396), "fans 取聚合 detail");
        assert_eq!(after.album_count, Some(8));
        assert_eq!(after.song_count, Some(43));
        assert_eq!(after.description, "emo band");
        Ok(())
    }

    /// 非 artist 帧 → artist_meta 为 None。
    #[test]
    fn artist_meta_none_for_non_artist() {
        let frame = super::DetailFrame::new(EntityRef::Album(Box::new(album("al"))));
        assert!(frame.artist_meta().is_none());
    }

    /// album_meta：fetch 到货整份用聚合 detail（含简介），未到货退回 entity 占位（无简介）。
    #[test]
    fn album_meta_prefers_fetched_detail() -> color_eyre::Result<()> {
        use mineral_model::{Album, AlbumId};

        // 结果列 entity 份：搜索来的,有 track_count、无简介。
        let entity = Album::builder()
            .id(AlbumId::new(SourceKind::NETEASE, "al"))
            .name("CF".to_owned())
            .track_count(Some(13))
            .build();
        let mut frame = super::DetailFrame::new(EntityRef::Album(Box::new(entity)));

        let before = frame
            .album_meta()
            .ok_or_else(|| color_eyre::eyre::eyre!("album 帧应有 meta"))?;
        assert!(before.description.is_empty(), "entity 份无简介");
        assert_eq!(before.track_count, Some(13));

        // fetch 到货:整份用详情(简介齐)。
        frame.set_album_detail(Box::new(
            Album::builder()
                .id(AlbumId::new(SourceKind::NETEASE, "al"))
                .name("CF".to_owned())
                .track_count(Some(13))
                .description("emo album".to_owned())
                .build(),
        ));
        let after = frame
            .album_meta()
            .ok_or_else(|| color_eyre::eyre::eyre!("album 帧应有 meta"))?;
        assert_eq!(after.description, "emo album", "简介取 fetch detail");

        // 非 album 帧 → None。
        let other = super::DetailFrame::new(EntityRef::Artist(Box::new(artist("ar"))));
        assert!(other.album_meta().is_none());
        Ok(())
    }

    /// push 后处于滑动过渡(sweep_frames Some、is_push),推满后 settle 为 None。
    #[test]
    fn push_arms_sweep_until_settled() -> color_eyre::Result<()> {
        let mut st = DetailStack::rooted(EntityRef::Artist(Box::new(artist("ar"))));
        assert!(st.sweep_frames().is_none(), "root 无滑动");
        st.push(EntityRef::Album(Box::new(album("al"))), 3);
        let Some((_, _, _, is_push)) = st.sweep_frames() else {
            color_eyre::eyre::bail!("push 后应处于滑动中");
        };
        assert!(is_push, "push 方向 = 右入");
        for _ in 0..3 {
            st.tick();
        }
        assert!(st.sweep_frames().is_none(), "推满后 settle、无滑动");
        Ok(())
    }

    /// sweep_frames 的进度走 ease-in-out（与 artist 双区切换、左栏视图切换同曲线），
    /// 不再是单向 ease-out——下钻/返回过渡与其余 sweep 对齐的回归守卫。
    #[test]
    fn sweep_uses_ease_in_out_curve() -> color_eyre::Result<()> {
        use crate::render::anim::Transition;

        let mut st = DetailStack::rooted(EntityRef::Artist(Box::new(artist("ar"))));
        st.push(EntityRef::Album(Box::new(album("al"))), /*ticks*/ 6);
        st.tick();
        let Some((_, _, eased, _)) = st.sweep_frames() else {
            color_eyre::eyre::bail!("push 后应处于滑动中");
        };
        // 同参数参照：推进同样拍数，取 ease-in-out 应一致、取单向 ease-out 应不同。
        let mut reference = Transition::expanding(6);
        reference.tick();
        assert_eq!(
            eased,
            reference.eased_in_out(),
            "下钻 sweep 取 ease-in-out 曲线"
        );
        assert_ne!(eased, reference.eased(), "不再是单向 ease-out");
        Ok(())
    }

    /// EntityRef::kind 把四变体映射到对应 SearchKind（与结果列 tab 同一套）。
    #[test]
    fn entity_ref_kind_maps_variants() {
        use mineral_model::SearchKind;
        assert_eq!(
            EntityRef::Song(Box::new(song("s", None))).kind(),
            SearchKind::Song
        );
        assert_eq!(
            EntityRef::Album(Box::new(album("al"))).kind(),
            SearchKind::Album
        );
        assert_eq!(
            EntityRef::Artist(Box::new(artist("ar"))).kind(),
            SearchKind::Artist
        );
        assert_eq!(
            EntityRef::Playlist(Box::new(playlist("pl"))).kind(),
            SearchKind::Playlist
        );
    }

    /// title_crumbs 给出 root→top 的 (kind, name) 链，下钻一层多一节；空栈为空链。
    #[test]
    fn title_crumbs_walk_root_to_top() {
        use mineral_model::SearchKind;
        let mut st = DetailStack::rooted(EntityRef::Artist(Box::new(artist("ar"))));
        assert_eq!(st.title_crumbs(), vec![(SearchKind::Artist, "artist ar")]);

        st.push(EntityRef::Album(Box::new(album("al"))), 1);
        assert_eq!(
            st.title_crumbs(),
            vec![
                (SearchKind::Artist, "artist ar"),
                (SearchKind::Album, "album al"),
            ]
        );

        assert!(
            DetailStack::empty().title_crumbs().is_empty(),
            "空栈无实体可标 → 空链"
        );
    }

    /// cycle_section：立即翻转 section 并 arm 横向过渡；动画中 section_eased Some、
    /// settle 后 None；可来回切。
    #[test]
    fn cycle_section_arms_and_settles() {
        let mut frame = super::DetailFrame::new(EntityRef::Artist(Box::new(artist("ar"))));
        frame.apply_sections(both_sections());
        assert_eq!(
            frame.section,
            ArtistSection::Hot,
            "两区源默认落首区 Top Songs"
        );
        assert!(frame.section_eased().is_none(), "未切过 → 无动画");

        frame.cycle_section(/*ticks*/ 4);
        assert_eq!(frame.section, ArtistSection::Albums, "立即翻到 Albums");
        assert!(frame.section_eased().is_some(), "切换中有滑动进度");

        for _ in 0..4 {
            frame.tick_section();
        }
        assert!(frame.section_eased().is_none(), "推满后 settle、无动画");
        assert_eq!(frame.section, ArtistSection::Albums);

        frame.cycle_section(4);
        assert_eq!(frame.section, ArtistSection::Hot, "再切回 Top Songs");
        assert!(frame.section_eased().is_some(), "反向切换同样有滑动");
    }

    /// 非 artist 帧不该被 cycle_section 误用，但即便调用也只是切 section + arm，不 panic；
    /// 这里验证 DetailStack::tick 会推进栈顶帧的 section 动画。
    #[test]
    fn stack_tick_advances_top_section_anim() {
        let mut st = DetailStack::rooted(EntityRef::Artist(Box::new(artist("ar"))));
        if let Some(frame) = st.current_mut() {
            frame.apply_sections(both_sections());
            frame.cycle_section(4);
        }
        assert!(
            st.current()
                .and_then(super::DetailFrame::section_eased)
                .is_some(),
            "切换后栈顶帧动画在进行"
        );
        for _ in 0..4 {
            st.tick();
        }
        assert!(
            st.current()
                .and_then(super::DetailFrame::section_eased)
                .is_none(),
            "DetailStack::tick 推进并 settle 栈顶帧 section 动画"
        );
    }

    /// nudge_description：向下累加、下界钳 0（上界由 render 端按内容高度钳，不在此）。
    #[test]
    fn nudge_description_clamps_lower() {
        let frame = super::DetailFrame::new(EntityRef::Album(Box::new(album("al"))));
        assert_eq!(frame.description_scroll().get(), 0, "新帧简介滚回顶");
        frame.nudge_description(5);
        assert_eq!(frame.description_scroll().get(), 5, "向下平移 +5");
        frame.nudge_description(-100);
        assert_eq!(frame.description_scroll().get(), 0, "下界钳 0、不为负");
    }

    /// 取栈顶实体名（测试 helper）。
    fn entity_name(f: &super::DetailFrame) -> String {
        f.entity.name().to_owned()
    }

    /// 造一张带 `n` 首曲目的专辑（曲目名 `t0..tn`）。
    fn album_with_tracks(raw: &str, n: usize) -> Album {
        Album::builder()
            .id(AlbumId::new(SourceKind::NETEASE, raw))
            .name(format!("album {raw}"))
            .songs(
                (0..n)
                    .map(|i| {
                        Song::builder()
                            .id(SongId::new(SourceKind::NETEASE, format!("t{i}")))
                            .name(format!("t{i}"))
                            .duration_ms(Some(1000))
                            .build()
                    })
                    .collect::<Vec<Song>>(),
            )
            .build()
    }

    /// row_entity：专辑帧曲目行 → Song（按当前光标）；越界 → None。
    #[test]
    fn row_entity_album_track_follows_cursor() -> color_eyre::Result<()> {
        let mut frame = super::DetailFrame::new(EntityRef::Album(Box::new(album("al"))));
        assert!(frame.row_entity().is_none(), "数据未到 → None");
        frame.set_album_detail(Box::new(album_with_tracks("al", 3)));
        frame.list_mut().set_sel(2);
        let Some(EntityRef::Song(s)) = frame.row_entity() else {
            color_eyre::eyre::bail!("专辑帧曲目行应取到 Song");
        };
        assert_eq!(s.name, "t2", "取第 3 行曲目");
        frame.list_mut().set_sel(9);
        assert!(frame.row_entity().is_none(), "越界行 → None");
        Ok(())
    }

    /// row_entity：歌单帧（Tracks）曲目行 → Song。
    #[test]
    fn row_entity_playlist_track() -> color_eyre::Result<()> {
        let mut frame = super::DetailFrame::new(EntityRef::Playlist(Box::new(playlist("pl"))));
        frame.set_tracks(vec![song("s0", None), song("s1", None)]);
        frame.list_mut().set_sel(1);
        let Some(EntityRef::Song(s)) = frame.row_entity() else {
            color_eyre::eyre::bail!("歌单帧曲目行应取到 Song");
        };
        assert_eq!(s.name, "song s1");
        Ok(())
    }

    /// row_entity：artist 帧 Hot 区行 → Song、Albums 区行 → Album（容器）。
    #[test]
    fn row_entity_artist_by_section() -> color_eyre::Result<()> {
        use mineral_model::{Artist, ArtistId};
        let detail = Artist::builder()
            .id(ArtistId::new(SourceKind::NETEASE, "ar"))
            .name("ar".to_owned())
            .songs(vec![song("h0", None), song("h1", None)])
            .build();
        let mut frame = super::DetailFrame::new(EntityRef::Artist(Box::new(artist("ar"))));
        frame.set_artist_detail(Box::new(detail));
        frame.set_artist_albums(vec![album("a0"), album("a1")]);

        frame.section = ArtistSection::Hot;
        frame.list_mut().set_sel(1);
        let Some(EntityRef::Song(s)) = frame.row_entity() else {
            color_eyre::eyre::bail!("Hot 区行应取到 Song");
        };
        assert_eq!(s.name, "song h1");

        frame.section = ArtistSection::Albums;
        frame.list_mut().set_sel(0);
        let Some(EntityRef::Album(a)) = frame.row_entity() else {
            color_eyre::eyre::bail!("Albums 区行应取到 Album");
        };
        assert_eq!(a.name, "album a0");
        Ok(())
    }

    /// song_list：专辑帧 = 全曲、artist Hot 区 = 热门曲、Albums 区（容器行）= 空、未到 = 空。
    #[test]
    fn song_list_per_section() -> color_eyre::Result<()> {
        use mineral_model::{Artist, ArtistId};
        let mut album_frame = super::DetailFrame::new(EntityRef::Album(Box::new(album("al"))));
        assert!(album_frame.song_list().is_empty(), "未到货 → 空");
        album_frame.set_album_detail(Box::new(album_with_tracks("al", 4)));
        assert_eq!(album_frame.song_list().len(), 4, "专辑帧 = 全曲");

        let detail = Artist::builder()
            .id(ArtistId::new(SourceKind::NETEASE, "ar"))
            .name("ar".to_owned())
            .songs(vec![song("h0", None), song("h1", None)])
            .build();
        let mut artist_frame = super::DetailFrame::new(EntityRef::Artist(Box::new(artist("ar"))));
        artist_frame.set_artist_detail(Box::new(detail));
        artist_frame.set_artist_albums(vec![album("a0")]);
        artist_frame.section = ArtistSection::Hot;
        assert_eq!(artist_frame.song_list().len(), 2, "Hot 区 = 热门曲");
        artist_frame.section = ArtistSection::Albums;
        assert!(
            artist_frame.song_list().is_empty(),
            "Albums 区行是专辑容器,不作歌曲队列上下文"
        );
        Ok(())
    }
}
