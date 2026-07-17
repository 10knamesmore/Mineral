//! Search 结果桶:某 (source, kind) 已拉取页的累积、结果列光标、翻页游标与选中实体的
//! 详情栈。翻页 append 同变体载荷、`exhausted` 由源显式信号或短页推断;光标真移动才把
//! 详情栈复位到新选中实体。

use mineral_channel_core::ArtistSections;
use mineral_model::{Album, AlbumId, Artist, ArtistId, PlaylistId, Song};
use mineral_task::SearchPayload;

use crate::runtime::scroll::list::ScrollList;

use super::super::detail::{DetailFetch, DetailStack, EntityRef};

/// 某个 (source, kind) 的一桶搜索结果：累积页 + 光标 + 翻页游标 + 该实体的详情栈。
///
/// 「累积页」= 翻页 append 进同一 [`SearchPayload`] 变体；`next_offset` 是下一页起点、
/// `exhausted` 由源的显式 `has_more` 或短页推断置真后停止自动翻页。
pub struct KindResults {
    /// 已拉取并累积的结果（翻页 append 同变体）。
    pub results: SearchPayload,

    /// 结果列光标 + 视口滚动（nvim 手感:offset 跨帧持久 + scrolloff + 缓动平移）。
    list: ScrollList,

    /// 下一页 offset,**页对齐**（= 已请求页数 × limit,而非已加载条数）；翻页提交带它。
    /// 别改回按实际条数累加——页码型源（如 B 站）每页实际条数与 limit 无关,余数会让
    /// channel 侧 offset→页号换算折回/跳页。
    pub next_offset: u32,

    /// 是否已榨干:源显式说没有下一页（`has_more == Some(false)`）,或源不知道时按
    /// 短页（返回条数 < 页大小）推断。置真后停止自动翻页,底标 `n/∞` → `n/n`。
    pub exhausted: bool,

    /// 当前选中实体的详情栈（root 随 `sel` 复位、下钻 push/pop）。
    pub detail: DetailStack,

    /// 本桶所属源的 artist 可用分区（`caps.artist_sections`；一桶同源故桶级缓存）。`None` = caps
    /// 尚未落定。由 [`Self::apply_sections`] 在首页到货后按 caps 落定,`set_sel` 复位新 root 时复用
    /// （无需再查 caps）——新建 / 复位的歌手 root 帧据此把分区收到首个可用区。
    sections: Option<ArtistSections>,
}

/// 榨干判定:源显式表态（`has_more`）优先,`None` 回退「短页即榨干」推断。
///
/// # Params:
///   - `loaded`: 本页实际返回条数
///   - `limit`: 请求页大小
///   - `has_more`: 源的显式翻页信号
fn page_exhausts(loaded: u32, limit: u32, has_more: Option<bool>) -> bool {
    match has_more {
        Some(more) => !more,
        None => loaded < limit,
    }
}

impl KindResults {
    /// 首页结果（`offset == 0`）：替换、光标归零、判 `exhausted`、detail root 落首项。
    ///
    /// # Params:
    ///   - `payload`: 首页载荷
    ///   - `limit`: 页大小（`next_offset` 按它页对齐推进）
    ///   - `has_more`: 源的显式翻页信号（`None` 回退短页推断）
    pub(super) fn first_page(payload: SearchPayload, limit: u32, has_more: Option<bool>) -> Self {
        let count = payload_len(&payload);
        let detail = EntityRef::from_payload(&payload, 0)
            .map_or_else(DetailStack::empty, DetailStack::rooted);
        let loaded = u32::try_from(count).unwrap_or(u32::MAX);
        Self {
            results: payload,
            list: ScrollList::new(),
            next_offset: limit,
            exhausted: page_exhausts(loaded, limit, has_more),
            detail,
            sections: None,
        }
    }

    /// 按 caps 落定本桶所属源的 artist 可用分区（首页到货后由上层调,持 caps）。立即把当前歌手
    /// root 帧的分区收到首个可用区,并让后续 `set_sel` 复位复用此判定。
    pub fn apply_sections(&mut self, sections: ArtistSections) {
        self.sections = Some(sections);
        self.apply_sections_to_root();
    }

    /// 把桶级分区声明落到当前 root 帧（歌手帧才有意义;非歌手帧 / 未落定 → 不动）。
    fn apply_sections_to_root(&mut self) {
        if let Some(sections) = self.sections.clone()
            && let Some(frame) = self.detail.current_mut()
            && matches!(frame.entity, EntityRef::Artist(_))
        {
            frame.apply_sections(sections);
        }
    }

    /// 翻页 append（`offset > 0`）：拼接同变体、`next_offset` 页对齐推进、判 `exhausted`。
    pub(super) fn append_page(
        &mut self,
        payload: SearchPayload,
        limit: u32,
        has_more: Option<bool>,
    ) {
        let added = u32::try_from(payload_len(&payload)).unwrap_or(u32::MAX);
        extend_payload(&mut self.results, payload);
        self.next_offset = self.next_offset.saturating_add(limit);
        if page_exhausts(added, limit, has_more) {
            self.exhausted = true;
        }
    }

    /// 当前结果条数。
    pub fn len(&self) -> usize {
        payload_len(&self.results)
    }

    /// 结果列光标下标。
    pub fn sel(&self) -> usize {
        self.list.sel()
    }

    /// 结果列光标 + 视口滚动态（渲染 / 锚点反推读取）。
    pub(crate) fn list(&self) -> &ScrollList {
        &self.list
    }

    /// 移动结果列光标到 `idx`（钳末行）；**真的移动了**才把 detail 栈复位到新选中实体
    /// （边界钳制不动则保留下钻栈）。
    pub fn set_sel(&mut self, idx: usize) {
        let clamped = idx.min(self.len().saturating_sub(1));
        if clamped == self.list.sel() {
            return;
        }
        self.list.set_sel(clamped);
        if let Some(entity) = EntityRef::from_payload(&self.results, clamped) {
            self.detail.reset_to(entity);
            // 新 root 帧默认建;歌手帧沿用桶级分区声明把分区收到首个可用区(无热门曲的源即 Albums)。
            self.apply_sections_to_root();
        }
    }

    /// AlbumDetail 回包落当前帧——仅当栈顶帧正等这张专辑（歌曲帧拉所属专辑、专辑帧拉自身）。
    /// 帧已切走（移光标 / 下钻 / 切 source）则丢弃。
    ///
    /// 顺带**回填结果列**:搜索投影的 Album 曲目数未知(列表画 `-`),下钻拿到真值后写回同 id 的
    /// 列表行,回列表即显真数、不再是 `-`(帧已切走仍回填,列表值与详情一致)。
    pub fn fill_album_detail(&mut self, id: &AlbumId, album: Box<Album>) {
        if let SearchPayload::Albums(albums) = &mut self.results {
            for listed in albums.iter_mut().filter(|a| a.id == *id) {
                listed.track_count = album.track_count;
            }
        }
        let Some(frame) = self.detail.current_mut() else {
            return;
        };
        if frame.entity.fetch() == Some(DetailFetch::AlbumDetail(id.clone())) {
            frame.set_album_detail(album);
        }
    }

    /// PlaylistDetail 回包落当前帧（栈顶正等这个歌单时）。
    pub fn fill_playlist_tracks(&mut self, id: &PlaylistId, tracks: Vec<Song>) {
        let Some(frame) = self.detail.current_mut() else {
            return;
        };
        if frame.entity.fetch() == Some(DetailFetch::PlaylistDetail(id.clone())) {
            frame.set_tracks(tracks);
        }
    }

    /// ArtistDetail 回包（热门曲那一路）落当前帧（栈顶正等这个歌手时）。
    pub fn fill_artist_detail(&mut self, id: &ArtistId, artist: Box<Artist>) {
        let Some(frame) = self.detail.current_mut() else {
            return;
        };
        if frame.entity.fetch() == Some(DetailFetch::Artist(id.clone())) {
            frame.set_artist_detail(artist);
        }
    }

    /// ArtistAlbums 回包（专辑列表那一路）落当前帧（栈顶正等这个歌手时）。
    pub fn fill_artist_albums(&mut self, id: &ArtistId, albums: Vec<Album>) {
        let Some(frame) = self.detail.current_mut() else {
            return;
        };
        if frame.entity.fetch() == Some(DetailFetch::Artist(id.clone())) {
            frame.set_artist_albums(albums);
        }
    }
}

/// 一页结果载荷的条数（任意变体）。
fn payload_len(payload: &SearchPayload) -> usize {
    match payload {
        SearchPayload::Songs(v) => v.len(),
        SearchPayload::Albums(v) => v.len(),
        SearchPayload::Playlists(v) => v.len(),
        SearchPayload::Artists(v) => v.len(),
    }
}

/// 把 `src` 拼到 `dst`（同变体才拼；变体不符——不该发生——静默忽略）。
fn extend_payload(dst: &mut SearchPayload, src: SearchPayload) {
    match (dst, src) {
        (SearchPayload::Songs(d), SearchPayload::Songs(s)) => d.extend(s),
        (SearchPayload::Albums(d), SearchPayload::Albums(s)) => d.extend(s),
        (SearchPayload::Playlists(d), SearchPayload::Playlists(s)) => d.extend(s),
        (SearchPayload::Artists(d), SearchPayload::Artists(s)) => d.extend(s),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use mineral_channel_core::Page;
    use mineral_model::{SearchKind, SourceKind};
    use mineral_task::SearchPayload;

    use crate::test_support::endserenading;

    use crate::runtime::state::detail::EntityRef;

    use super::super::SearchSession;

    /// 5 条歌曲一页（配 limit=5 即满页；endserenading 上限 10 条）。
    fn full_page() -> SearchPayload {
        SearchPayload::Songs(endserenading(5))
    }

    /// 造一张专辑（测试 helper）。
    fn album(raw: &str) -> mineral_model::Album {
        mineral_model::Album::builder()
            .id(mineral_model::AlbumId::new(SourceKind::NETEASE, raw))
            .name(format!("album {raw}"))
            .build()
    }

    /// 首页满页（= limit）不判榨干、next_offset 页对齐推到 limit；detail root 落首项。
    #[test]
    fn first_page_full_not_exhausted() -> color_eyre::Result<()> {
        let mut s = SearchSession::new(SearchKind::Song);
        s.apply_page(
            SearchKind::Song,
            full_page(),
            Page {
                offset: 0,
                limit: 5,
            },
            /*has_more*/ None,
        );
        let kr = s
            .kind_results()
            .ok_or_else(|| color_eyre::eyre::eyre!("首页应入桶"))?;
        assert_eq!(kr.len(), 5, "5 条入桶");
        assert!(!kr.exhausted, "满页（= limit）不判榨干");
        assert_eq!(kr.next_offset, 5, "下一页从 limit 起(页对齐)");
        assert_eq!(kr.detail.depth(), 0, "detail root 落首项、无下钻");
        Ok(())
    }

    /// 显式 `has_more` 优先于短页推断:短页 + `Some(true)` 不榨干(页码型源每页实际条数
    /// 与 limit 无关);满页 + `Some(false)` 也榨干。next_offset 恒页对齐,与实际条数无关。
    #[test]
    fn explicit_has_more_overrides_short_page_inference() -> color_eyre::Result<()> {
        // 短页(2 < limit 5)但源明说还有下一页 → 不榨干,next_offset 仍推到 limit。
        let mut s = SearchSession::new(SearchKind::Song);
        s.apply_page(
            SearchKind::Song,
            SearchPayload::Songs(endserenading(2)),
            Page {
                offset: 0,
                limit: 5,
            },
            /*has_more*/ Some(true),
        );
        let kr = s
            .kind_results()
            .ok_or_else(|| color_eyre::eyre::eyre!("首页应入桶"))?;
        assert!(!kr.exhausted, "源显式 has_more=true → 短页不判榨干");
        assert_eq!(
            kr.next_offset, 5,
            "next_offset 页对齐推进,与实际条数(2)无关"
        );

        // 满页(= limit)但源明说没有下一页 → 榨干。
        let mut s = SearchSession::new(SearchKind::Song);
        s.apply_page(
            SearchKind::Song,
            full_page(),
            Page {
                offset: 0,
                limit: 5,
            },
            /*has_more*/ Some(false),
        );
        let kr = s
            .kind_results()
            .ok_or_else(|| color_eyre::eyre::eyre!("首页应入桶"))?;
        assert!(kr.exhausted, "源显式 has_more=false → 满页也榨干");
        Ok(())
    }

    /// 短页（< 30）判榨干。
    #[test]
    fn short_page_marks_exhausted() -> color_eyre::Result<()> {
        let mut s = SearchSession::new(SearchKind::Song);
        s.apply_page(
            SearchKind::Song,
            SearchPayload::Songs(endserenading(5)),
            Page::default(),
            /*has_more*/ None,
        );
        let kr = s
            .kind_results()
            .ok_or_else(|| color_eyre::eyre::eyre!("首页应入桶"))?;
        assert!(kr.exhausted, "短页判榨干");
        Ok(())
    }

    /// 翻页 append：第二页拼到第一页后、next_offset 页对齐累加(与实际条数无关)；短二页判榨干。
    #[test]
    fn append_page_accumulates_and_exhausts() -> color_eyre::Result<()> {
        let mut s = SearchSession::new(SearchKind::Song);
        s.apply_page(
            SearchKind::Song,
            full_page(),
            Page {
                offset: 0,
                limit: 5,
            },
            /*has_more*/ None,
        );
        s.apply_page(
            SearchKind::Song,
            SearchPayload::Songs(endserenading(2)),
            Page {
                offset: 5,
                limit: 5,
            },
            /*has_more*/ None,
        );
        let kr = s
            .kind_results()
            .ok_or_else(|| color_eyre::eyre::eyre!("桶应在"))?;
        assert_eq!(kr.len(), 7, "两页累积");
        assert_eq!(kr.next_offset, 10, "offset 页对齐推到 2 × limit");
        assert!(kr.exhausted, "短二页榨干");
        Ok(())
    }

    /// set_sel 真的移动才复位 detail 栈（下钻后移光标 → 回 root）；钳制不动则保留。
    #[test]
    fn set_sel_resets_detail_only_on_real_move() -> color_eyre::Result<()> {
        let mut s = SearchSession::new(SearchKind::Album);
        s.apply_page(
            SearchKind::Album,
            SearchPayload::Albums(vec![album("a1"), album("a2")]),
            Page::default(),
            /*has_more*/ None,
        );
        let kr = s
            .kind_results_mut()
            .ok_or_else(|| color_eyre::eyre::eyre!("桶应在"))?;
        kr.detail
            .push(EntityRef::Album(Box::new(album("drill"))), 1);
        assert_eq!(kr.detail.depth(), 1, "已下钻一层");
        kr.set_sel(1);
        assert_eq!(kr.detail.depth(), 0, "移光标 → detail 复位到新 root");
        kr.set_sel(9); // 越界钳到末行(idx1)==当前，不动
        assert_eq!(kr.sel(), 1, "钳制在末行");
        Ok(())
    }

    /// fill_album_detail 回填结果列:搜索投影的 Album 曲目数 `None`(列表画 `-`),下钻到货后
    /// 把真值写回同 id 的列表行,其它行不动。
    #[test]
    fn fill_album_detail_backfills_list_track_count() -> color_eyre::Result<()> {
        use mineral_model::{Album, AlbumId};

        let mut s = SearchSession::new(SearchKind::Album);
        s.apply_page(
            SearchKind::Album,
            SearchPayload::Albums(vec![album("a1"), album("a2")]),
            Page::default(),
            /*has_more*/ None,
        );
        let kr = s
            .kind_results_mut()
            .ok_or_else(|| color_eyre::eyre::eyre!("桶应在"))?;
        let SearchPayload::Albums(before) = &kr.results else {
            color_eyre::eyre::bail!("应是 Albums 桶");
        };
        assert_eq!(
            before.first().and_then(|a| a.track_count),
            None,
            "投影列表曲目数未知"
        );

        let detailed = Album::builder()
            .id(AlbumId::new(SourceKind::NETEASE, "a1"))
            .name("album a1".to_owned())
            .track_count(Some(11))
            .build();
        kr.fill_album_detail(&AlbumId::new(SourceKind::NETEASE, "a1"), Box::new(detailed));

        let SearchPayload::Albums(after) = &kr.results else {
            color_eyre::eyre::bail!("应是 Albums 桶");
        };
        assert_eq!(
            after.first().and_then(|a| a.track_count),
            Some(11),
            "同 id 行回填真值"
        );
        assert_eq!(after.get(1).and_then(|a| a.track_count), None, "其它行不动");
        Ok(())
    }
}
