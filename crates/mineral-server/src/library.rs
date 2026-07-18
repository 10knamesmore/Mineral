//! 用户歌单库聚合态:各源原始列表的唯一事实源 + curate 出口变换管线。
//!
//! 原则:**原始数据进聚合态,transform 只在出口**——脚本函数改了重启即对已
//! 有数据生效,被藏的歌单永不丢失。管线经串行锁逐结论推进,消除并发重算
//! 竞态;任何 transform 失败都透传原列表(fail-open)。

use mineral_model::{Playlist, PlaylistId, SourceKind};
use mineral_script::{CurateOutcome, CuratedEntry, PlaylistBrief, QueryId, ResolveValue};
use mineral_task::{ChannelFetchKind, Priority, TaskEvent, TaskKind, TaskOutcome};
use parking_lot::Mutex;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::player::PlayerCore;

/// 用户歌单库聚合态(daemon 内唯一事实源)。
pub(crate) struct Library {
    /// 可变态(原始列表 / curate 缓存 / 结论位 / 停靠 query / 快照)。
    state: Mutex<LibraryInner>,

    /// 管线串行锁:结论逐个过 transform(锁内 await 脚本往返),消除并发重算竞态。
    pipeline: tokio::sync::Mutex<()>,

    /// channel 注册序——合并 concat 的确定顺序(跨源顺序从此有唯一权威)。
    order: Vec<SourceKind>,
}

/// [`Library`] 的可变态。
struct LibraryInner {
    /// 各源**原始**列表(键存在 = 该源已结论;失败 / 不支持 = 空贡献)。
    raw: FxHashMap<SourceKind, Vec<Playlist>>,

    /// per-source curate 后的缓存(原始数据更新时失效,管线内重算)。
    curated: FxHashMap<SourceKind, Vec<Playlist>>,

    /// 尚未首次结论的源;空 = 初始完备。
    pending: FxHashSet<SourceKind>,

    /// 停靠的脚本 query(初始完备时刻统一 resolve)。
    parked: Vec<QueryId>,

    /// 最新合并快照(出口变换后;client 推送与脚本查询共用)。
    snapshot: Vec<Playlist>,
}

impl Library {
    /// 建聚合态;`order` 即 channel 注册序,启动时全部源计入在途。
    pub(crate) fn new(order: Vec<SourceKind>) -> Self {
        let pending = order.iter().copied().collect::<FxHashSet<SourceKind>>();
        Self {
            state: Mutex::new(LibraryInner {
                raw: FxHashMap::default(),
                curated: FxHashMap::default(),
                pending,
                parked: Vec::new(),
                snapshot: Vec::new(),
            }),
            pipeline: tokio::sync::Mutex::new(()),
            order,
        }
    }

    /// 落一次源结论。`Some` = 新原始列表(顺带失效该源 curate 缓存);
    /// `None` = 拉取失败 / 不支持——只兜空贡献,**不清已有数据**(重拉失败
    /// 不能让上次成功的列表消失)。
    fn conclude(&self, source: SourceKind, raw: Option<Vec<Playlist>>) {
        let mut st = self.state.lock();
        match raw {
            Some(list) => {
                st.raw.insert(source, list);
                st.curated.remove(&source);
            }
            None => {
                st.raw.entry(source).or_default();
            }
        }
        st.pending.remove(&source);
    }

    /// 初始完备:全部源都结论过一次(生产路径由 `snapshot_or_park` /
    /// `commit_snapshot` 锁内自判,此 accessor 仅供测试断言)。
    #[cfg(test)]
    fn is_complete(&self) -> bool {
        self.state.lock().pending.is_empty()
    }

    /// 某源当前原始列表(克隆;未结论为空)。仅供测试断言。
    #[cfg(test)]
    fn raw_of(&self, source: SourceKind) -> Vec<Playlist> {
        self.state
            .lock()
            .raw
            .get(&source)
            .cloned()
            .unwrap_or_default()
    }

    /// 本轮需要(重)算 per-source curate 的源:已有原始数据但缓存缺席。
    fn stale_sources(&self) -> Vec<(SourceKind, Vec<Playlist>)> {
        let st = self.state.lock();
        self.order
            .iter()
            .filter(|s| st.raw.contains_key(s) && !st.curated.contains_key(s))
            .map(|s| (*s, st.raw.get(s).cloned().unwrap_or_default()))
            .collect::<Vec<(SourceKind, Vec<Playlist>)>>()
    }

    /// 写入某源的 per-source curate 结果。
    fn set_curated(&self, source: SourceKind, list: Vec<Playlist>) {
        self.state.lock().curated.insert(source, list);
    }

    /// 跨源合并输入:按注册序 concat 各源 curate 结果。
    fn merged_input(&self) -> Vec<Playlist> {
        let st = self.state.lock();
        self.order
            .iter()
            .filter_map(|s| st.curated.get(s))
            .flat_map(|list| list.iter().cloned())
            .collect::<Vec<Playlist>>()
    }

    /// 提交合并快照;初始完备时顺带取走全部停靠 query(同锁,无漏 resolve 窗口)。
    fn commit_snapshot(&self, snapshot: Vec<Playlist>) -> Vec<QueryId> {
        let mut st = self.state.lock();
        st.snapshot = snapshot;
        if st.pending.is_empty() {
            std::mem::take(&mut st.parked)
        } else {
            Vec::new()
        }
    }

    /// 脚本查询入口:已完备立即给快照;未完备停靠(判定与停靠同锁,不与
    /// [`Self::commit_snapshot`] 的取走竞争出孤儿 query)。
    fn snapshot_or_park(&self, query: QueryId) -> Option<Vec<Playlist>> {
        let mut st = self.state.lock();
        if st.pending.is_empty() {
            Some(st.snapshot.clone())
        } else {
            st.parked.push(query);
            None
        }
    }

    /// 缓存快照(新 client 接入即时下发用);还没有任何源结论时为 `None`。
    pub(crate) fn cached_snapshot(&self) -> Option<Vec<Playlist>> {
        let st = self.state.lock();
        if st.raw.is_empty() {
            None
        } else {
            Some(st.snapshot.clone())
        }
    }
}

/// 把 curate 采纳条目按 qualified id 对回真实 [`Playlist`]:省略 = 隐藏,
/// 顺序 = 展示序,`name` / `description` 覆盖;未知 id warn 丢弃,重复 id
/// 取首见(map 取走后二次命中自然落到未知分支)。
///
/// # Params:
///   - `originals`: transform 前的原列表
///   - `entries`: 脚本采纳结果
///
/// # Return:
///   对回后的展示列表。
fn apply_curated(originals: Vec<Playlist>, entries: Vec<CuratedEntry>) -> Vec<Playlist> {
    let mut by_id = originals
        .into_iter()
        .map(|p| (p.id.clone(), p))
        .collect::<FxHashMap<PlaylistId, Playlist>>();
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(mut playlist) = by_id.remove(&entry.id) else {
            mineral_log::warn!(
                target: "library",
                id = entry.id.qualified(),
                "curate 返回未知或重复 id,丢弃该条"
            );
            continue;
        };
        if let Some(name) = entry.name {
            playlist.name = name;
        }
        if let Some(description) = entry.description {
            playlist.description = description;
        }
        out.push(playlist);
    }
    out
}

impl PlayerCore {
    /// 提交一次某源的 `MyPlaylists` 拉取,并挂失败观察者:任务终态非 Ok 时
    /// 以空贡献落结论——初始完备不因失败源卡死(成功结论走 `PlaylistsFetched`
    /// 事件路,见 [`crate::events`])。
    pub(crate) fn submit_my_playlists(&self, source: SourceKind) {
        let handle = self.inner.scheduler.submit(
            TaskKind::ChannelFetch(ChannelFetchKind::MyPlaylists { source }),
            Priority::User,
        );
        let player = self.clone();
        tokio::spawn(async move {
            if !matches!(handle.done().await, TaskOutcome::Ok) {
                player.library_concluded(source, /*raw*/ None);
            }
        });
    }

    /// 某源歌单结论到达(`Some` = 成功原始列表 / `None` = 失败空贡献):
    /// spawn 出口变换管线,不卡调用方(player tick / 任务观察者)。
    pub(crate) fn library_concluded(&self, source: SourceKind, raw: Option<Vec<Playlist>>) {
        let player = self.clone();
        tokio::spawn(async move { player.run_library_pipeline(source, raw).await });
    }

    /// 脚本 `library.playlists` 查询:完备即回快照;未完备停靠,初始完备
    /// 时刻由管线统一 resolve。
    pub(crate) fn library_snapshot_or_park(&self, query: QueryId) -> Option<Vec<Playlist>> {
        self.library().snapshot_or_park(query)
    }

    /// 新 client 接入的即时快照(有缓存才推,避免连接瞬间显示空库假象)。
    pub(crate) fn push_cached_library_snapshot(&self) {
        if let Some(playlists) = self.library().cached_snapshot() {
            self.notify()
                .task_event(TaskEvent::LibrarySnapshot { playlists });
        }
    }

    /// 出口变换管线:落结论 → 补算 per-source curate → 注册序 concat → 跨源
    /// curate → 提交快照 + 推 client + resolve 停靠 query。整体在管线串行锁内。
    async fn run_library_pipeline(&self, source: SourceKind, raw: Option<Vec<Playlist>>) {
        let library = self.library();
        let _serial = library.pipeline.lock().await;
        library.conclude(source, raw);
        for (src, raw_list) in library.stale_sources() {
            let curated = self.curate_via_script(Some(src), raw_list).await;
            library.set_curated(src, curated);
        }
        let merged = library.merged_input();
        let snapshot = self.curate_via_script(/*source*/ None, merged).await;
        let parked = library.commit_snapshot(snapshot.clone());
        self.notify().task_event(TaskEvent::LibrarySnapshot {
            playlists: snapshot.clone(),
        });
        if parked.is_empty() {
            return;
        }
        let Some(sender) = self.script_sender() else {
            return;
        };
        let briefs = snapshot
            .iter()
            .map(PlaylistBrief::from)
            .collect::<Vec<PlaylistBrief>>();
        for query in parked {
            sender.resolve(query, ResolveValue::Playlists(briefs.clone()));
        }
    }

    /// 跑一级 curate(经脚本线程);无脚本 / Identity / 失败 → 原列表透传。
    async fn curate_via_script(
        &self,
        source: Option<SourceKind>,
        list: Vec<Playlist>,
    ) -> Vec<Playlist> {
        let Some(sender) = self
            .script_sender()
            .filter(mineral_script::ScriptSender::is_attached)
        else {
            return list;
        };
        let briefs = list
            .iter()
            .map(PlaylistBrief::from)
            .collect::<Vec<PlaylistBrief>>();
        match sender
            .curate_playlists(source, briefs, self.hook_timeout())
            .await
        {
            CurateOutcome::Identity => list,
            CurateOutcome::Curated(entries) => apply_curated(list, entries),
        }
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::{Playlist, PlaylistId, SourceKind};
    use mineral_script::CuratedEntry;
    use pretty_assertions::assert_eq;

    use super::{Library, apply_curated};

    /// 极简歌单(netease 源,名字即 id 便于断言)。
    fn pl(id: &str, name: &str) -> Playlist {
        Playlist::builder()
            .id(PlaylistId::new(SourceKind::NETEASE, id))
            .name(name.to_owned())
            .build()
    }

    /// 采纳条目(可选 name 覆盖)。
    fn entry(id: &str, name: Option<&str>) -> CuratedEntry {
        CuratedEntry {
            id: PlaylistId::new(SourceKind::NETEASE, id),
            name: name.map(str::to_owned),
            description: None,
        }
    }

    /// id 对回:省略 = 隐藏,顺序 = 展示序,name/description 覆盖生效。
    #[test]
    fn apply_curated_maps_ids_back_in_order() {
        let originals = vec![pl("a", "甲"), pl("b", "乙"), pl("c", "丙")];
        let entries = vec![entry("c", None), entry("a", Some("甲改"))];
        let out = apply_curated(originals, entries);
        let got = out
            .iter()
            .map(|p| (p.id.value(), p.name.as_str()))
            .collect::<Vec<(&str, &str)>>();
        assert_eq!(
            got,
            vec![("c", "丙"), ("a", "甲改")],
            "b 被省略即隐藏;顺序按返回序;name 覆盖"
        );
    }

    /// 未知 id 丢弃(warn),重复 id 取首见——两者都不炸、不复制。
    #[test]
    fn apply_curated_drops_unknown_and_duplicate_ids() {
        let originals = vec![pl("a", "甲"), pl("b", "乙")];
        let entries = vec![
            entry("a", None),
            entry("ghost", None),
            entry("a", Some("重复")),
            entry("b", None),
        ];
        let out = apply_curated(originals, entries);
        let got = out
            .iter()
            .map(|p| (p.id.value(), p.name.as_str()))
            .collect::<Vec<(&str, &str)>>();
        assert_eq!(
            got,
            vec![("a", "甲"), ("b", "乙")],
            "未知 id 与重复 id 都被丢弃,首见生效"
        );
    }

    /// 初始完备:全部源结论过一次才算;parked 只在完备时刻取走。
    #[test]
    fn complete_only_after_all_sources_concluded() {
        let lib = Library::new(vec![SourceKind::NETEASE, SourceKind::BILIBILI]);
        assert!(!lib.is_complete());
        lib.conclude(SourceKind::NETEASE, Some(vec![pl("a", "甲")]));
        assert!(!lib.is_complete(), "还有源在途");
        lib.conclude(SourceKind::BILIBILI, /*raw*/ None);
        assert!(lib.is_complete(), "失败/不支持也是结论(空贡献)");
    }

    /// 重拉失败(None 结论)不得清掉已有原始数据。
    #[test]
    fn failure_conclusion_keeps_existing_raw() {
        let lib = Library::new(vec![SourceKind::NETEASE]);
        lib.conclude(SourceKind::NETEASE, Some(vec![pl("a", "甲")]));
        lib.conclude(SourceKind::NETEASE, /*raw*/ None);
        let raws = lib.raw_of(SourceKind::NETEASE);
        assert_eq!(raws.len(), 1, "失败结论只是空贡献兜底,不清已有数据");
    }
}
