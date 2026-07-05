//! favorite(♥)编排:本地 persist 是事实来源,channel 只做可选远端镜像 / 导入。
//!
//! 三条路径:toggle(锁内读改写+推,镜像锁外)、set(显式设值,同上)、sync(先推本地 canonical,
//! 锁外拉远端红心,锁内 add-only 导入+推)。本地为准:远端只进不出——导入从不删本地独有的收藏,
//! 取消收藏只由本地 toggle/set 触发。
//!
//! 串行:所有 persist 读改写 + canonical 推送都在 [`Inner::favorites_lock`](crate::player) 内,
//! 让 toggle 与 connect 期并发的 sync 互斥(否则陈旧远端快照会复活刚取消的收藏、或乐观收藏被整源
//! 桶替换清掉);远端网络调用(镜像 / fetch)一律在锁外,不阻塞其它收藏操作。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::{FutureExt, StreamExt};
use mineral_channel_core::MusicChannel;
use mineral_model::{Song, SongId, SourceKind};
use mineral_task::TaskEvent;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::player::PlayerCore;

/// 聚合收藏补 meta 后台任务的状态 + 节流旋钮。
///
/// 状态是单飞用的两个 atomic:`running` 保证同时只一个 worker;`pending` 让"worker 运行中又被
/// 触发"不丢——收尾看到它再扫一轮(coalesce sync 分源晚到的那批导入)。旋钮由 config 注入。
pub(crate) struct Backfill {
    /// 单飞闸:`true` = 已有 worker 在跑。
    running: AtomicBool,

    /// 待办标志:运行中被再次触发时置位,worker 收尾据此决定是否再扫一轮。
    pending: AtomicBool,

    /// 每次 `songs_detail` 调用处理的 id 数(刷新粒度 + 限住单次调用时长)。
    chunk_size: usize,

    /// 并行 `songs_detail` 调用数上限(并发即节流)。
    max_concurrent: usize,
}

impl Backfill {
    /// 新建:旋钮由 `sources.mineral.backfill` 注入,状态置初值(未在跑、无待办)。
    ///
    /// # Params:
    ///   - `chunk_size`: 每次 `songs_detail` 调用处理的 id 数
    ///   - `max_concurrent`: 并行 `songs_detail` 调用数上限
    ///
    /// # Return:
    ///   初值 [`Backfill`]。
    pub(crate) fn new(chunk_size: usize, max_concurrent: usize) -> Self {
        Self {
            running: AtomicBool::new(false),
            pending: AtomicBool::new(false),
            chunk_size,
            max_concurrent,
        }
    }
}

/// 导入守卫:某远端红心 id 是否应写回 persist。
///
/// fetch 期间被本地显式取消(fetch 前 `before` 有、导入时 `now` 无)的 id **不**重加——否则陈旧
/// 远端快照会复活用户刚取消的收藏、污染事实来源。其余(本地新增的远端红心、或仍在的)照常导入。
///
/// # Params:
///   - `id`: 远端红心里的一个歌曲 id
///   - `before`: fetch 发起前的本地 favorited 快照
///   - `now`: 导入时刻的本地 favorited 快照
///
/// # Return:
///   `true` = 应导入;`false` = fetch 期间被本地取消,跳过。
fn should_import(id: &SongId, before: &FxHashSet<SongId>, now: &FxHashSet<SongId>) -> bool {
    let removed_during_fetch = before.contains(id) && !now.contains(id);
    !removed_during_fetch
}

impl PlayerCore {
    /// 设/取消一首歌的收藏(♥)为显式值:锁内**写 persist(事实来源,所有源通用)+ 推 canonical**,
    /// 锁外尽力镜像远端。
    ///
    /// # Params:
    ///   - `id`: 目标歌曲;namespace 决定 persist scope 与远端 channel。
    ///   - `loved`: `true` 收藏、`false` 取消。
    pub(crate) async fn set_favorite(&self, id: &SongId, loved: bool) -> color_eyre::Result<()> {
        let ns = id.namespace();
        {
            let _guard = self.inner.favorites_lock.lock().await;
            self.persist().scope(ns).set_loved(id, loved).await?;
            self.push_current_favorited_ids(ns).await;
        }
        // script/CLI love 只握 id(不像 TUI toggle 自带整首 Song):触发后台补 meta,让缺 meta
        // 的这首(及其它待补的)渐进进聚合视图。取消收藏无需 meta。
        if loved {
            self.spawn_meta_backfill();
        }
        self.refresh_aggregate_favorites().await;
        self.mirror_remote_favorite(id, loved).await;
        Ok(())
    }

    /// 切换一首歌的收藏(♥):锁内**读当前态 → 翻转 → 写 persist → 推 canonical**(原子,防与
    /// sync 交错),锁外尽力镜像远端。
    ///
    /// 携带整首 [`Song`]:落 love 的同时顺手 `upsert_meta`,跨源聚合视图(全源收藏)才能
    /// 离线重建歌名 / 艺人。meta 写失败只 warn 不阻断——love 是主操作,meta 是装饰。
    ///
    /// # Params:
    ///   - `song`: 目标歌曲(id 的 namespace 决定 persist scope 与远端 channel)。
    ///
    /// # Return:
    ///   切换后的新 loved 态。
    pub(crate) async fn toggle_favorite(&self, song: &Song) -> color_eyre::Result<bool> {
        let ns = song.id.namespace();
        let new = {
            let _guard = self.inner.favorites_lock.lock().await;
            let scope = self.persist().scope(ns);
            if let Err(e) = scope.upsert_meta(song).await {
                mineral_log::warn!(
                    target: "favorites",
                    song = song.id.value(),
                    error = mineral_log::chain(&e),
                    "收藏顺手写 meta 失败(love 照常)"
                );
            }
            let new = !scope.is_loved(&song.id).await?;
            scope.set_loved(&song.id, new).await?;
            self.push_current_favorited_ids(ns).await;
            new
        };
        self.refresh_aggregate_favorites().await;
        self.mirror_remote_favorite(&song.id, new).await;
        Ok(new)
    }

    /// 同步某源的收藏(connect 时触发):先按本地 persist 推一次 canonical(所有源立即出 ♥,含无
    /// 远端能力的源);若该 channel 有远端红心,拉取(锁外)并 add-only 导入 persist(守卫见
    /// [`should_import`]),再推合并 canonical。远端不支持 / 未登录 / 失败:本地那次已推,静默返回。
    ///
    /// # Params:
    ///   - `source`: 目标源。
    ///   - `channel`: 该源 channel(远端红心来源)。
    pub(crate) async fn sync_favorites(&self, source: SourceKind, channel: Arc<dyn MusicChannel>) {
        // step1(锁内):记 before 快照 + 立即推本地 canonical。
        let before = {
            let _guard = self.inner.favorites_lock.lock().await;
            let ids = self.load_favorited_ids(source).await;
            self.emit_favorited_ids(source, ids.clone());
            ids
        };
        // step2(锁外):拉远端红心(数百 ms 网络往返,不占锁)。
        let remote = match channel.liked_song_ids().await {
            Ok(remote) => remote,
            Err(e) => {
                mineral_log::debug!(
                    target: "favorites",
                    source = source.name(),
                    error = mineral_log::chain(&e),
                    "无远端红心可导入(不支持 / 未登录 / 失败)"
                );
                return;
            }
        };
        // step3(锁内):add-only 导入(守卫跳过 fetch 期间被本地取消的)+ 推合并 canonical。
        let imported = {
            let _guard = self.inner.favorites_lock.lock().await;
            let now = self.load_favorited_ids(source).await;
            let scope = self.persist().scope(source);
            let mut imported = 0_usize;
            for id in &remote {
                if !should_import(id, &before, &now) {
                    continue;
                }
                match scope.set_loved(id, /*loved*/ true).await {
                    Ok(()) => imported = imported.saturating_add(1),
                    Err(e) => {
                        mineral_log::warn!(
                            target: "favorites",
                            source = source.name(),
                            song = id.value(),
                            error = mineral_log::chain(&e),
                            "远端红心导入 persist 失败"
                        );
                    }
                }
            }
            self.push_current_favorited_ids(source).await;
            imported
        };
        // 导入改变了收藏集才值得重合成聚合歌单(幂等导入也会走到,代价可接受:
        // 一次本地 SQL + 一次事件推送)。
        if imported > 0 {
            self.refresh_aggregate_favorites().await;
        }
        // 导入的红心只有 id、无 meta,聚合视图重建不出;触发后台补 meta 逐步填满(单飞,
        // 多源 sync 只跑一个 worker,它扫全部缺 meta 的收藏)。
        self.spawn_meta_backfill();
    }

    /// 收藏集变化后重合成聚合歌单(mineral 源)并重推,两条出口各司其职:
    ///
    /// - **曲目集合**走 [`TaskEvent::PlaylistDetailFetched`]:client 直接替换 `library.tracks`,
    ///   正在看聚合歌单时收藏行实时增删。
    /// - **歌单列表(track_count)**走 [`PlayerCore::library_concluded`](crate::library) 的出口
    ///   管线,与其它源同路合并成 `LibrarySnapshot`——sidebar 计数只认这条刷新。**不能**直推
    ///   `PlaylistsFetched`:那是 scheduler 内部事件,client 侧 apply 按约定丢弃(等价 no-op)。
    ///
    /// 读 persist + 推 detail 全程持 [`Inner::favorites_lock`](crate::player):并发收藏写下,各
    /// refresh 依次读当时 persist 态、按获锁序推 detail,最后获锁者读到最终态并最后推,detail
    /// 收敛。(持锁只做本地 SQL,无网络。)列表 count 经出口管线异步落地,与其它源同款最终一致。
    /// 未注册聚合 channel(裁剪构建 / 纯 mock 测试核)时静默跳过。
    async fn refresh_aggregate_favorites(&self) {
        let Some(channel) = self.channel_for(SourceKind::MINERAL) else {
            return;
        };
        let channel = Arc::clone(channel);
        let playlists = {
            let _guard = self.inner.favorites_lock.lock().await;
            let playlists = match channel.my_playlists().await {
                Ok(playlists) => playlists,
                Err(e) => {
                    mineral_log::debug!(
                        target: "favorites",
                        error = mineral_log::chain(&e),
                        "聚合歌单列表重合成失败,跳过重推"
                    );
                    return;
                }
            };
            for p in &playlists {
                match channel.playlist_detail(&p.id).await {
                    Ok(playlist) => {
                        self.inner
                            .client_events
                            .lock()
                            .push(TaskEvent::PlaylistDetailFetched {
                                id: p.id.clone(),
                                playlist: Box::new(playlist),
                            });
                    }
                    Err(e) => {
                        mineral_log::debug!(
                            target: "favorites",
                            playlist = p.id.qualified(),
                            error = mineral_log::chain(&e),
                            "聚合歌单 detail 重合成失败,跳过该张"
                        );
                    }
                }
            }
            playlists
        };
        // 歌单列表进出口管线(与其它源同路),产出 client 认得的 LibrarySnapshot 刷 sidebar 计数。
        self.library_concluded(SourceKind::MINERAL, Some(playlists));
    }

    /// 触发后台补 meta(单飞):sync 导入的红心先只有 id,后台扫全部缺 meta 的 loved 歌,逐源
    /// 分块拉 `songs_detail` 回填 persist,渐进填满聚合面。已有 worker 在跑则只置 `pending`,由它
    /// 收尾再扫一轮(coalesce sync 分源晚到的导入)。fire-and-forget,daemon 退出随 runtime 收。
    pub(crate) fn spawn_meta_backfill(&self) {
        self.inner.backfill.pending.store(true, Ordering::SeqCst);
        if self.inner.backfill.running.swap(true, Ordering::SeqCst) {
            return; // 已有 worker;它会看到 pending 再扫一轮
        }
        let player = self.clone();
        tokio::spawn(async move {
            loop {
                player.inner.backfill.pending.store(false, Ordering::SeqCst);
                player.run_meta_backfill().await;
                player.inner.backfill.running.store(false, Ordering::SeqCst);
                // 收尾后若又有 pending,重新抢占续跑;抢不到(别的触发已接手)就退,避免双 worker。
                if !player.inner.backfill.pending.load(Ordering::SeqCst)
                    || player.inner.backfill.running.swap(true, Ordering::SeqCst)
                {
                    break;
                }
            }
        });
    }

    /// 扫一遍缺 meta 的 loved 歌,逐源分块拉 `songs_detail` 回填 persist,每块补完刷一次聚合面
    /// (渐进填充)。**source-neutral**:按各歌 namespace 走各自 channel、`buffer_unordered` 限并发,
    /// 不假设 `songs_detail` 是批量还是逐个。best-effort:无该源 channel / 不支持 / 失败都只 debug。
    async fn run_meta_backfill(&self) {
        let missing = match self.persist().missing_meta_loved_ids().await {
            Ok(ids) => ids,
            Err(e) => {
                mineral_log::debug!(
                    target: "favorites",
                    error = mineral_log::chain(&e),
                    "查缺 meta 收藏失败,跳过补全"
                );
                return;
            }
        };
        if missing.is_empty() {
            return;
        }
        // 逐源分组 → 每源分块 → 拼 (channel, chunk) 作业列表。
        let mut by_source = FxHashMap::<SourceKind, Vec<SongId>>::default();
        for id in missing {
            by_source.entry(id.namespace()).or_default().push(id);
        }
        let chunk_size = self.inner.backfill.chunk_size.max(1);
        let mut jobs = Vec::<(Arc<dyn MusicChannel>, Vec<SongId>)>::new();
        for (source, ids) in by_source {
            let Some(channel) = self.channel_for(source) else {
                continue;
            };
            for chunk in ids.chunks(chunk_size) {
                jobs.push((Arc::clone(channel), chunk.to_vec()));
            }
        }
        let max_concurrent = self.inner.backfill.max_concurrent.max(1);
        // 在迭代器上把每块 map 成 `boxed` future,再交给 stream::iter/buffer_unordered——直接在
        // stream 上 map 捕获 `Arc<dyn MusicChannel>` 会撞 HRTB lifetime 推断(闭包不够泛型)。
        let futures = jobs
            .into_iter()
            .map(|(channel, chunk)| async move { channel.songs_detail(&chunk).await }.boxed())
            .collect::<Vec<_>>();
        let stream = futures_util::stream::iter(futures).buffer_unordered(max_concurrent);
        let mut stream = std::pin::pin!(stream);
        while let Some(result) = stream.next().await {
            let songs = match result {
                Ok(songs) => songs,
                Err(e) => {
                    mineral_log::debug!(
                        target: "favorites",
                        error = mineral_log::chain(&e),
                        "补 meta 拉 detail 失败,跳过该块"
                    );
                    continue;
                }
            };
            if songs.is_empty() {
                continue;
            }
            for song in &songs {
                if let Err(e) = self.persist().scope(song.id.namespace()).upsert_meta(song).await {
                    mineral_log::debug!(
                        target: "favorites",
                        song = song.id.value(),
                        error = mineral_log::chain(&e),
                        "补 meta 写入失败"
                    );
                }
            }
            // 补完一块就刷聚合面,渐进填充(而非全补完才一次性出现)。
            self.refresh_aggregate_favorites().await;
        }
    }

    /// 远端镜像(best-effort,**锁外**):把新态同步到该源远端(如网易云红心)。无该源 channel /
    /// 不支持 / 未登录 / 网络失败都无害——本地已写。
    async fn mirror_remote_favorite(&self, id: &SongId, loved: bool) {
        let ns = id.namespace();
        if let Some(channel) = self.channel_for(ns)
            && let Err(e) = channel.set_loved(id, loved).await
        {
            mineral_log::debug!(
                target: "favorites",
                source = ns.name(),
                song = id.value(),
                error = mineral_log::chain(&e),
                "远端 favorite 镜像不可用(本地已写)"
            );
        }
    }

    /// 读 persist 的 canonical favorited 集;读失败静默 warn + 返回空(装饰非致命)。
    async fn load_favorited_ids(&self, source: SourceKind) -> FxHashSet<SongId> {
        match self.persist().scope(source).loved_ids().await {
            Ok(ids) => ids,
            Err(e) => {
                mineral_log::warn!(
                    target: "favorites",
                    source = source.name(),
                    error = mineral_log::chain(&e),
                    "读 persist favorited 集失败"
                );
                FxHashSet::default()
            }
        }
    }

    /// 把给定 favorited 集推进 client_events(供 client 装饰)。
    fn emit_favorited_ids(&self, source: SourceKind, ids: FxHashSet<SongId>) {
        self.inner
            .client_events
            .lock()
            .push(TaskEvent::LikedSongIdsFetched { source, ids });
    }

    /// 读 persist canonical + 推。**调用方须持 [`Inner::favorites_lock`](crate::player)**,
    /// 保证推出的集与刚写入的 persist 一致。
    async fn push_current_favorited_ids(&self, source: SourceKind) {
        let ids = self.load_favorited_ids(source).await;
        self.emit_favorited_ids(source, ids);
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::{SongId, SourceKind};
    use rustc_hash::FxHashSet;

    use super::should_import;

    /// 造一个 NETEASE namespace 的 SongId。
    fn nid(v: &str) -> SongId {
        SongId::new(SourceKind::NETEASE, v)
    }

    /// should_import 守卫:本地新增的远端红心导入;fetch 期间被本地取消的(before 有、now 无)
    /// 跳过,免陈旧快照复活;仍在的照常(幂等)。
    #[test]
    fn should_import_skips_removed_during_fetch() {
        let a = nid("A");
        let b = nid("B");
        let empty = FxHashSet::<SongId>::default();
        let with_a: FxHashSet<SongId> = [a.clone()].into_iter().collect();

        // 远端有、本地 fetch 前后都没有 → 新导入。
        assert!(
            should_import(&b, /*before*/ &empty, /*now*/ &empty),
            "远端新红心应导入"
        );
        // fetch 前有、导入时仍有 → 幂等导入。
        assert!(
            should_import(&a, /*before*/ &with_a, /*now*/ &with_a),
            "仍在的应照常(幂等)"
        );
        // fetch 前有、导入时已无(fetch 期间被本地取消)→ 跳过,不复活。
        assert!(
            !should_import(&a, /*before*/ &with_a, /*now*/ &empty),
            "fetch 期间被本地取消的不重加"
        );
    }
}
