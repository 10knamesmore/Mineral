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

use mineral_channel_core::MusicChannel;
use mineral_model::{SongId, SourceKind};
use mineral_task::TaskEvent;
use rustc_hash::FxHashSet;

use crate::player::PlayerCore;

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
        self.mirror_remote_favorite(id, loved).await;
        Ok(())
    }

    /// 切换一首歌的收藏(♥):锁内**读当前态 → 翻转 → 写 persist → 推 canonical**(原子,防与
    /// sync 交错),锁外尽力镜像远端。
    ///
    /// # Params:
    ///   - `id`: 目标歌曲。
    ///
    /// # Return:
    ///   切换后的新 loved 态。
    pub(crate) async fn toggle_favorite(&self, id: &SongId) -> color_eyre::Result<bool> {
        let ns = id.namespace();
        let new = {
            let _guard = self.inner.favorites_lock.lock().await;
            let scope = self.persist().scope(ns);
            let new = !scope.is_loved(id).await?;
            scope.set_loved(id, new).await?;
            self.push_current_favorited_ids(ns).await;
            new
        };
        self.mirror_remote_favorite(id, new).await;
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
        let _guard = self.inner.favorites_lock.lock().await;
        let now = self.load_favorited_ids(source).await;
        let scope = self.persist().scope(source);
        for id in &remote {
            if !should_import(id, &before, &now) {
                continue;
            }
            if let Err(e) = scope.set_loved(id, /*loved*/ true).await {
                mineral_log::warn!(
                    target: "favorites",
                    source = source.name(),
                    song = id.value(),
                    error = mineral_log::chain(&e),
                    "远端红心导入 persist 失败"
                );
            }
        }
        self.push_current_favorited_ids(source).await;
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
