//! 搜索按键决策的只读上下文与副作用意图；提交任务、下载和播放均经后端句柄执行。
//!
//! 页面只修改自身视图状态并返回意图；应用就地借用能力、键表和配置，随后执行返回的意图。

use crossterm::event::KeyEvent;
use mineral_channel_core::ChannelCaps;
use mineral_model::{SearchKind, Song, SourceKind};
use mineral_protocol::DownloadTarget;
use mineral_task::{ChannelFetchKind, Priority, TaskKind};
use rustc_hash::FxHashMap;

use crate::app::App;
use crate::app::page::Page;
use crate::runtime::action::Action;
use crate::runtime::keymap::Keymap;

/// Page 决策所需的只读跨页上下文(= React 的 props)。借用而非拥有,故 App 侧必须在调用点
/// **就地构造**(写明 `&self.state.caps` 等字段路径),不能抽成 `self.page_ctx()` 方法——那会整借
/// `self.state`、与 `&mut self.state.channel_search` 冲突。
#[derive(Clone, Copy)]
pub(crate) struct SearchCtx<'a> {
    /// 各 source 能力声明(决定可搜 source / kind 列表)。
    pub caps: &'a FxHashMap<SourceKind, ChannelCaps>,

    /// 键位表(面板动词自查表;prompt 仍走裸键)。
    pub keymap: &'a Keymap,

    /// 结果列表与详情简介的滚动步长、结果分页预取半径。
    pub behavior: &'a mineral_config::BehaviorConfig,

    /// detail 下钻 / 返回滑动拍数(App 从动画配置预算好传入)。
    pub sweep_ticks: u16,
}

/// Search 页吃完按键后吐给 App 的副作用意图;[`App::apply_search_effect`] 落地。
pub(crate) enum SearchEffect {
    /// 原子替换队列并起播 exact target occurrence。
    PlayQueue {
        /// 替换进播放队列的曲目(整列结果 / 整列表)。
        queue: Vec<Song>,

        /// `queue` 内的 0-based exact target coordinate。
        target: usize,

        /// 队列语境(埋点 provenance:顶层结果记搜索词、详情面板记容器身份)。
        context: mineral_protocol::QueueContextWire,
    },

    /// 下载 Search 当前光标指向的歌曲。
    Download(Box<Song>),

    /// 提交一条首页搜索任务(User 优先级)。
    Submit {
        /// 目标 source。
        source: SourceKind,

        /// 目标 kind。
        kind: SearchKind,

        /// 查询词。
        query: String,
    },

    /// 预取下一页：结果桶已登记待收取页，消费成功或失败回包前不再生成同桶的续页意图。
    FetchMore {
        /// 目标 source。
        source: SourceKind,

        /// 目标 kind。
        kind: SearchKind,

        /// 查询词(与首页同词,续拉同一桶)。
        query: String,

        /// 待收取页的分页参数，页大小沿用首页。
        page: mineral_channel_core::Page,
    },

    /// Search 未接管的动词回落全局 dispatch(transport / 退出确认等照常生效)。
    Dispatch(Action),

    /// 纯状态改动,无副作用。
    None,
}

impl App {
    /// Search 布局态按键入口:就地构造只读 [`SearchCtx`]、交 Page 吃键、再落地它吐回的意图。
    pub(in crate::app) fn handle_channel_search_key(&mut self, key: &KeyEvent) {
        let anim = self.state.cfg.tui().animation();
        let sweep_ticks =
            crate::render::anim::ticks16_from_ms(*anim.sweep_ms(), *anim.frame_tick_ms());
        // SearchCtx 就地构造:channel_search / caps / cfg 是 self.state 三个不相交字段,keymap 在
        // self 上,全 disjoint,借用检查器放行(故不能抽成取 ctx 的方法,那会整借 self.state)。
        let eff = self.state.channel_search.on_key(
            key,
            SearchCtx {
                caps: &self.state.caps,
                keymap: &self.keymap,
                behavior: self.state.cfg.tui().behavior(),
                sweep_ticks,
            },
        );
        self.apply_search_effect(eff);
    }

    /// 落地 Search 页吐回的副作用意图。Page 只产意图、不碰 `client` / `notifications`,全在此收口。
    pub(super) fn apply_search_effect(&mut self, eff: SearchEffect) {
        match eff {
            SearchEffect::PlayQueue {
                queue,
                target,
                context,
            } => self.play_queue(queue, target, context),
            SearchEffect::Download(song) => {
                self.client.download(DownloadTarget::Song(song));
            }
            SearchEffect::Submit {
                source,
                kind,
                query,
            } => {
                self.client.submit_task(
                    TaskKind::ChannelFetch(ChannelFetchKind::Search {
                        source,
                        kind,
                        query,
                        page: mineral_channel_core::Page::default(),
                    }),
                    Priority::User,
                );
                // 首页在飞：结果区显 searching spinner，到货（含 0 条）由 apply_page 清。
                self.state.channel_search.mark_loading(kind);
            }
            SearchEffect::FetchMore {
                source,
                kind,
                query,
                page,
            } => {
                mineral_log::debug!(
                    target: "tui",
                    source = ?source,
                    kind = ?kind,
                    offset = page.offset,
                    limit = page.limit,
                    "提交搜索结果分页预取"
                );
                self.client.submit_task(
                    TaskKind::ChannelFetch(ChannelFetchKind::Search {
                        source,
                        kind,
                        query,
                        page,
                    }),
                    Priority::User,
                );
            }
            SearchEffect::Dispatch(action) => self.dispatch(action),
            SearchEffect::None => {}
        }
    }
}
