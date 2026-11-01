//! 搜索按键决策的只读上下文与副作用意图；提交任务、下载和播放均经后端句柄执行。
//!
//! 页面只修改自身视图状态并返回意图；应用就地借用能力、键表和配置，随后执行返回的意图。

use crossterm::event::KeyEvent;
use mineral_channel_core::ChannelCaps;
use mineral_model::{ArtistId, SearchKind, Song, SourceKind};
use mineral_protocol::DownloadTarget;
use mineral_task::Priority;
use rustc_hash::FxHashMap;

use crate::app::App;
use crate::app::page::Page;
use crate::runtime::action::Action;
use crate::runtime::keymap::Keymap;
use crate::runtime::prefetch::{request_artist_albums, request_search};

/// Page 决策所需的只读跨页上下文(= React 的 props)。借用而非拥有,故 App 侧必须在调用点
/// **就地构造**(写明 `&self.state.models.caps` 等字段路径),不能抽成 `self.page_ctx()` 方法——那会整借
/// `self.state`、与 `&mut self.state.ui.channel_search` 冲突。
#[derive(Clone, Copy)]
pub(crate) struct SearchCtx<'a> {
    /// 各 source 能力声明(决定可搜 source / kind 列表)。
    pub caps: &'a FxHashMap<SourceKind, ChannelCaps>,

    /// 键位表(面板动词自查表;prompt 仍走裸键)。
    pub keymap: &'a Keymap,

    /// 结果列表与详情简介的滚动步长、结果分页预取半径。
    pub behavior: &'a crate::config::BehaviorConfig,

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

    /// 创建搜索订阅并请求首批。
    Submit {
        /// 目标 source。
        source: SourceKind,

        /// 目标 kind。
        kind: SearchKind,

        /// 查询词。
        query: String,
    },

    /// 推进已有搜索订阅，成功提交后登记待收取批次。
    FetchMore {
        /// 目标 source。
        source: SourceKind,

        /// 目标 kind。
        kind: SearchKind,

        /// 查询词(与首页同词,续拉同一桶)。
        query: String,

        /// 本次搜索中待收取的批次序号。
        batch: u64,
    },

    /// 推进艺人专辑订阅，成功提交后登记待收取批次。
    FetchArtistAlbums {
        /// 艺人完整身份，回包据此更新保留的详情帧。
        id: ArtistId,

        /// 本次读取中待收取的批次序号。
        batch: u64,
    },

    /// Search 未接管的动词回落全局 dispatch(transport / 退出确认等照常生效)。
    Dispatch(Action),

    /// 纯状态改动,无副作用。
    None,
}

impl App {
    /// Search 布局态按键入口:就地构造只读 [`SearchCtx`]、交 Page 吃键、再落地它吐回的意图。
    pub(in crate::app) fn handle_channel_search_key(&mut self, key: &KeyEvent) {
        let anim = self.state.cfg.animation();
        let sweep_ticks =
            crate::render::anim::ticks16_from_ms(*anim.sweep_ms(), *anim.frame_tick_ms());
        // SearchCtx 就地构造:channel_search / caps / cfg 是 self.state 三个不相交字段,keymap 在
        // self 上,全 disjoint,借用检查器放行(故不能抽成取 ctx 的方法,那会整借 self.state)。
        let eff = self.state.ui.channel_search.on_key(
            key,
            SearchCtx {
                caps: &self.state.models.caps,
                keymap: &self.keymap,
                behavior: self.state.cfg.behavior(),
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
                if let Err(error) = request_search(
                    self.client.as_ref(),
                    &mut self.state.ui.channel_search,
                    source,
                    kind,
                    query,
                    0,
                ) {
                    self.report_submit_error(error);
                }
            }
            SearchEffect::FetchMore {
                source,
                kind,
                query,
                batch,
            } => {
                mineral_log::debug!(
                    target: "tui",
                    source = ?source,
                    kind = ?kind,
                    batch,
                    "推进搜索结果"
                );
                if let Err(error) = request_search(
                    self.client.as_ref(),
                    &mut self.state.ui.channel_search,
                    source,
                    kind,
                    query,
                    batch,
                ) {
                    self.report_submit_error(error);
                }
            }
            SearchEffect::FetchArtistAlbums { id, batch } => {
                if let Err(error) = request_artist_albums(
                    self.client.as_ref(),
                    &mut self.state,
                    &id,
                    batch,
                    Priority::User,
                ) {
                    self.report_submit_error(error);
                }
            }
            SearchEffect::Dispatch(action) => self.dispatch(action),
            SearchEffect::None => {}
        }
    }
}
