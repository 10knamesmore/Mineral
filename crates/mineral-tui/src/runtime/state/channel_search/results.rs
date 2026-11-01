//! Search 结果桶保存累计条目、待收取批次、列表光标与详情栈。
//! 来源明确报告是否读完；光标真正移动时才把详情栈复位到新实体。

use mineral_channel_core::ArtistSections;
use mineral_model::{Album, AlbumId, Artist, ArtistId, PlaylistId};
use mineral_task::SearchPayload;

use crate::runtime::scroll::list::ScrollList;

use super::super::ArtistAlbums;
use super::super::detail::{DetailFetch, DetailStack, EntityRef};

/// 某个 (source, kind) 的结果、批次进度、光标及详情栈。
///
/// 续页按请求顺序追加同一 [`SearchPayload`] 变体；收到成功或失败回包前，同一桶只请求一页。
pub struct KindResults {
    /// 已拉取并累积的结果（翻页 append 同变体）。
    pub results: SearchPayload,

    /// 结果列光标 + 视口滚动（nvim 手感:offset 跨帧持久 + scrolloff + 缓动平移）。
    list: ScrollList,

    /// 本列表选中标题的滚动状态。
    pub(crate) title: crate::runtime::marquee::Marquee,

    /// 下一批的本地序号；None 表示来源已读完。
    next_batch: Option<u64>,

    /// 已成功提交、尚未收到结果的批次。
    pending_batch: Option<u64>,

    /// 当前选中实体的详情栈（root 随 `sel` 复位、下钻 push/pop）。
    pub detail: DetailStack,

    /// 本桶所属源的 artist 可用分区（`caps.artist_sections`；一桶同源故桶级缓存）。`None` = caps
    /// 尚未落定。由 [`Self::apply_sections`] 在首页到货后按 caps 落定,`set_sel` 复位新 root 时复用
    /// （无需再查 caps）——新建 / 复位的 artist root 帧据此把分区收到首个可用区。
    sections: Option<ArtistSections>,
}

impl KindResults {
    /// 首批替换结果并建立详情根；完整性直接采用来源结论。
    pub(super) fn first_page(payload: SearchPayload, complete: bool) -> Self {
        let detail = EntityRef::from_payload(&payload, 0)
            .map_or_else(DetailStack::empty, DetailStack::rooted);
        Self {
            results: payload,
            list: ScrollList::new(),
            title: crate::runtime::marquee::Marquee::default(),
            next_batch: (!complete).then_some(1),
            pending_batch: None,
            detail,
            sections: None,
        }
    }

    /// 按 caps 落定本桶所属源的 artist 可用分区（首页到货后由上层调,持 caps）。立即把当前 artist
    /// root 帧的分区收到首个可用区,并让后续 `set_sel` 复位复用此判定。
    pub fn apply_sections(&mut self, sections: ArtistSections) {
        self.sections = Some(sections);
        self.apply_sections_to_root();
    }

    /// 把桶级分区声明落到当前 root 帧（artist 帧才有意义;非 artist 帧 / 未落定 → 不动）。
    fn apply_sections_to_root(&mut self) {
        if let Some(sections) = self.sections.clone()
            && let Some(frame) = self.detail.current_mut()
            && matches!(frame.entity, EntityRef::Artist(_))
        {
            frame.apply_sections(sections);
        }
    }

    /// 生成续读意图；成功提交后再登记 pending。
    pub(crate) fn next_batch(&self) -> Option<u64> {
        self.next_batch.filter(|_| self.pending_batch.is_none())
    }

    pub(crate) fn batch_requested(&mut self, batch: u64) {
        self.pending_batch = Some(batch);
    }

    /// 只追加当前待收取批次；重复或未请求的回包不改变结果与进度。
    /// 返回是否接收，供上层跳过被丢弃回包的其它副作用。
    pub(super) fn append_page(
        &mut self,
        payload: SearchPayload,
        batch: u64,
        complete: bool,
    ) -> bool {
        if self.pending_batch != Some(batch) {
            return false;
        }
        self.pending_batch = None;
        self.next_batch = (!complete).then_some(batch + 1);
        extend_payload(&mut self.results, payload);
        true
    }

    /// 失败只释放对应的待收取批次；下次近底导航重试同一批。
    pub(super) fn fail_page(&mut self, batch: u64) {
        if self.pending_batch == Some(batch) {
            self.pending_batch = None;
        }
    }

    /// 是否已收齐结果，供底标决定是否在已加载数量后显示 `+`。
    pub fn exhausted(&self) -> bool {
        self.next_batch.is_none()
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

    /// 列表准备入口更新视口，不改变搜索结果或选中实体。
    pub(crate) fn list_mut(&mut self) -> &mut ScrollList {
        &mut self.list
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
            // 新 root 帧默认建;artist 帧沿用桶级分区声明把分区收到首个可用区(无热门曲的源即 Albums)。
            self.apply_sections_to_root();
        }
    }

    /// 将专辑详情填入所有匹配的保留帧，并回填结果列同 ID 专辑的曲目数。
    /// 歌曲帧按所属专辑匹配；移动结果光标后已销毁的帧不再接收。
    pub fn fill_album_detail(&mut self, id: &AlbumId, album: &Album, fresh: bool) {
        if let SearchPayload::Albums(albums) = &mut self.results {
            for listed in albums.iter_mut().filter(|a| a.id == *id) {
                listed.track_count = album.track_count;
            }
        }
        for frame in self
            .detail
            .matching_frames_mut(DetailFetch::AlbumDetail(id.clone()))
        {
            frame.set_album_detail(Box::new(album.clone()), fresh);
        }
    }

    /// 将歌单曲目填入所有匹配的保留帧。
    pub fn fill_playlist_entries(
        &mut self,
        id: &PlaylistId,
        entries: &[mineral_model::PlaylistEntry],
        complete: bool,
        fresh: bool,
    ) {
        for frame in self
            .detail
            .matching_frames_mut(DetailFetch::PlaylistDetail(id.clone()))
        {
            frame.set_playlist_entries(entries.to_vec(), complete, fresh);
        }
    }

    /// 将艺人详情填入所有匹配的保留帧，与已收到的专辑列表合并。
    pub fn fill_artist_detail(&mut self, id: &ArtistId, artist: &Artist) {
        for frame in self
            .detail
            .matching_frames_mut(DetailFetch::Artist(id.clone()))
        {
            frame.set_artist_detail(Box::new(artist.clone()));
        }
    }

    /// 将共享专辑列表的展示快照交给匹配的真实详情帧。
    pub fn fill_artist_albums(&mut self, id: &ArtistId, albums: &ArtistAlbums) {
        for frame in self
            .detail
            .matching_frames_mut(DetailFetch::Artist(id.clone()))
        {
            frame.set_artist_albums(albums.clone());
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
    use futures_util::StreamExt;
    use mineral_channel_core::{PageResult, page_batches};
    use mineral_model::{SearchKind, SourceKind};
    use mineral_task::SearchPayload;

    use crate::test_support::endserenading;

    use super::super::SearchSession;

    /// 一批五首歌曲。
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

    /// 未完成首批建立详情根，下一批序号不取决于结果条数。
    #[test]
    fn first_page_full_not_exhausted() -> color_eyre::Result<()> {
        let mut s = SearchSession::new(SearchKind::Song);
        s.apply_page(SearchKind::Song, full_page(), 0, false);
        let kr = s
            .kind_results()
            .ok_or_else(|| color_eyre::eyre::eyre!("首页应入桶"))?;
        assert_eq!(kr.len(), 5, "5 条入桶");
        assert!(!kr.exhausted(), "满页（= limit）不判榨干");
        assert_eq!(kr.next_batch(), Some(1), "下一次读取第二批");
        assert_eq!(kr.detail.depth(), 0, "detail root 落首项、无下钻");
        Ok(())
    }

    /// 完整性采用来源结论，不从本批条数推测。
    #[test]
    fn explicit_has_more_overrides_short_page_inference() -> color_eyre::Result<()> {
        let mut s = SearchSession::new(SearchKind::Song);
        s.apply_page(
            SearchKind::Song,
            SearchPayload::Songs(endserenading(2)),
            0,
            false,
        );
        let kr = s
            .kind_results()
            .ok_or_else(|| color_eyre::eyre::eyre!("首页应入桶"))?;
        assert!(!kr.exhausted(), "源显式 has_more=true → 短页不判榨干");
        assert_eq!(kr.next_batch(), Some(1), "本批只有两条也能继续读取");

        let mut s = SearchSession::new(SearchKind::Song);
        s.apply_page(SearchKind::Song, full_page(), 0, true);
        let kr = s
            .kind_results()
            .ok_or_else(|| color_eyre::eyre::eyre!("首页应入桶"))?;
        assert!(kr.exhausted(), "源显式 has_more=false → 满页也榨干");
        Ok(())
    }

    /// offset 来源的短页推断留在适配器中，TUI 只消费完成标记。
    #[tokio::test]
    async fn short_page_marks_exhausted() -> color_eyre::Result<()> {
        let mut stream = page_batches(|_page| async { Ok(PageResult::from(endserenading(5))) });
        let batch = stream
            .next()
            .await
            .ok_or_else(|| color_eyre::eyre::eyre!("missing batch"))??;
        let mut s = SearchSession::new(SearchKind::Song);
        s.apply_page(
            SearchKind::Song,
            SearchPayload::Songs(batch.items),
            0,
            batch.complete,
        );
        let kr = s
            .kind_results()
            .ok_or_else(|| color_eyre::eyre::eyre!("首页应入桶"))?;
        assert!(kr.exhausted(), "短页判榨干");
        Ok(())
    }

    /// 后续批次追加，来源完成后停止续读。
    #[test]
    fn append_page_accumulates_and_exhausts() -> color_eyre::Result<()> {
        let mut s = SearchSession::new(SearchKind::Song);
        s.apply_page(SearchKind::Song, full_page(), 0, false);
        assert_eq!(
            s.kind_results().and_then(super::KindResults::next_batch),
            Some(1)
        );
        s.batch_requested(SearchKind::Song, 1);
        s.apply_page(
            SearchKind::Song,
            SearchPayload::Songs(endserenading(2)),
            1,
            true,
        );
        let kr = s
            .kind_results()
            .ok_or_else(|| color_eyre::eyre::eyre!("桶应在"))?;
        assert_eq!(kr.len(), 7, "两页累积");
        assert_eq!(kr.next_batch(), None, "完整结果没有下一批");
        assert!(kr.exhausted(), "短二页榨干");
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
            0,
            true,
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
        kr.fill_album_detail(&AlbumId::new(SourceKind::NETEASE, "a1"), &detailed, true);

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
