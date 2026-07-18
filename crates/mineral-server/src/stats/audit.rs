//! 埋点遗忘防线(编译期强制)。
//!
//! 两层穷尽 `match`(项目规约禁 `_ =>` 兜底):
//!
//! - **入口层**:[`Request`] / [`ScriptCmd`] / [`ChannelFetchKind`] / [`PlaylistWriteOp`]
//!   ——加变体不补这里 → 编译错误 → 逼开发者对新入口做出「记 / 不记」的显式决策。
//! - **事件层**:[`BehaviorEvent`] / [`SystemEvent`]——加变体(= 建新事件类型)不补发射
//!   点账本 → 编译错误 → 逼开发者声明**谁发它**,防「有表有变体但无发出站点」的恒零列。
//!
//! 本模块运行期从不被调用——价值全在编译期穷尽性 + 下方对账测试。即便无人调用,rustc
//! 仍完整类型检查函数体,故防线不因 `dead_code` 抑制而失效。防线只强制**决策存在**;
//! 「标了 [`TrackingDecision::Recorded`] 但挂接点忘调 recorder」与「不加任何变体的全新
//! 代码路径忘发既有事件」编译期兜不住,由触发链集成测试兜底。

// 编译期防线 + 测试对账,非运行期调用;穷尽性即价值,不因未调用而失效。
#![allow(dead_code)]

use mineral_protocol::Request;
use mineral_script::ScriptCmd;
use mineral_stats::{BehaviorEvent, SystemEvent};
use mineral_task::{ChannelFetchKind, PlaylistWriteOp};

/// 一个行为入口变体的埋点归属决策。
enum TrackingDecision {
    /// 已埋,落到哪张事件表(表名供对账测试核对真实存在)。
    Recorded(&'static str),

    /// 明确不是事件(轮询读 / 渲染流 / 纯控制流),附不记的理由。
    NotAnEvent(&'static str),
}

/// client 请求入口的埋点归属(穷尽;新增 [`Request`] 变体不补此处即编译失败)。
fn audit_request(req: &Request) -> TrackingDecision {
    use TrackingDecision::{NotAnEvent, Recorded};
    match req {
        Request::Play(..) => {
            NotAnEvent("低层音频 URL 播放指令;播放事实经 play_song 起播链落 plays")
        }
        Request::Pause => Recorded("pauses"),
        Request::Resume => Recorded("pauses"),
        Request::Stop => Recorded("plays"),
        Request::Seek(..) => Recorded("seeks"),
        Request::SetVolume(..) => Recorded("volume_changes"),
        Request::AudioSnapshot => NotAnEvent("轮询读:音频状态快照"),
        Request::SubmitTask(..) => {
            NotAnEvent("任务提交;取数事件在 task 终态记,见 audit_fetch_kind")
        }
        Request::CancelTasks(..) => Recorded("task_cancels"),
        Request::TaskSnapshot => NotAnEvent("轮询读:任务快照"),
        Request::PlaySong(..) => Recorded("plays"),
        Request::SetQueue { .. } => Recorded("queue_ops"),
        Request::QueueInsertNext { .. } => Recorded("queue_ops"),
        Request::QueueAppend { .. } => Recorded("queue_ops"),
        Request::ChannelCaps => NotAnEvent("读:channel 能力查询"),
        Request::CyclePlayMode => Recorded("mode_changes"),
        // 切上首=skip 记 plays;回曲首(超阈值)分支另记 seeks,主归属取 plays。
        Request::PrevOrRestart => Recorded("plays"),
        Request::NextSong => Recorded("plays"),
        Request::PlayerSync(..) => NotAnEvent("读:播放器版本同步"),
        Request::PullPcm(..) => NotAnEvent("读:拉 PCM 数据"),
        Request::DaemonInfo => NotAnEvent("读:daemon 信息"),
        Request::ToggleLove(..) => Recorded("love_changes"),
        Request::QuerySongStats(..) => NotAnEvent("读:单曲统计查询(改口读 stats.db)"),
        Request::Download(..) => Recorded("downloads"),
        Request::DownloadProgress => NotAnEvent("轮询读:下载进度"),
        Request::InvokeAction { .. } => Recorded("action_invocations"),
        Request::RenderCopyTemplate { .. } => Recorded("copy_renders"),
        Request::StoreGet { .. } => NotAnEvent("读:per-song KV 读"),
        Request::StoreSet { .. } => Recorded("store_writes"),
        Request::StoreInc { .. } => Recorded("store_writes"),
        Request::ScriptBinds => NotAnEvent("读:脚本键位绑定查询"),
        Request::TerminalState { .. } => Recorded("fullscreen_changes"),
        Request::Shutdown => Recorded("app_lifecycle"),
    }
}

/// 脚本命令入口的埋点归属(穷尽)。镜像用户动作的命令落各行为表(actor=script);读类
/// 与「触发取数 task」类不在此直接记(事件在 task 终态由 [`audit_fetch_kind`] 归属)。
fn audit_script_cmd(cmd: &ScriptCmd) -> TrackingDecision {
    use TrackingDecision::{NotAnEvent, Recorded};
    match cmd {
        ScriptCmd::Toggle => Recorded("pauses"),
        ScriptCmd::Next => Recorded("plays"),
        ScriptCmd::Prev => Recorded("plays"),
        ScriptCmd::Stop => Recorded("plays"),
        ScriptCmd::SeekRel(..) => Recorded("seeks"),
        ScriptCmd::SeekTo(..) => Recorded("seeks"),
        ScriptCmd::SetVolume(..) => Recorded("volume_changes"),
        ScriptCmd::SetMode(..) => Recorded("mode_changes"),
        ScriptCmd::Play(..) => Recorded("plays"),
        ScriptCmd::Download(..) => Recorded("downloads"),
        ScriptCmd::StoreGet { .. } => NotAnEvent("读:脚本 KV 读"),
        ScriptCmd::StoreSet { .. } => Recorded("store_writes"),
        ScriptCmd::StoreInc { .. } => Recorded("store_writes"),
        ScriptCmd::QueueList { .. } => NotAnEvent("读:队列查询"),
        ScriptCmd::LibraryPlaylists { .. } => {
            NotAnEvent("触发取数 task;fetches 在终态记,见 audit_fetch_kind")
        }
        ScriptCmd::LibraryTracks { .. } => NotAnEvent("触发取数 task;fetches 在终态记"),
        ScriptCmd::LibrarySearch { .. } => {
            NotAnEvent("触发搜索 task;searches 在终态记,见 audit_fetch_kind")
        }
        ScriptCmd::LibrarySongUrl { .. } => NotAnEvent("触发取链 task;url_resolutions 在终态记"),
        ScriptCmd::SetLoved { .. } => Recorded("love_changes"),
        ScriptCmd::Spawn { .. } => Recorded("spawns"),
        ScriptCmd::SpawnKill { .. } => {
            NotAnEvent("请求杀子进程;spawns 行在进程收束回调记(outcome=killed)")
        }
        ScriptCmd::ConfigOverride { .. } => Recorded("config_overrides"),
        ScriptCmd::WindowTitle { .. } => NotAnEvent("设置终端窗口标题,纯 UI 副作用,非事件"),
    }
}

/// 取数 task 入口的埋点归属(穷尽)。取链 / 搜索有专表,其余归 `fetches`。
fn audit_fetch_kind(kind: &ChannelFetchKind) -> TrackingDecision {
    use TrackingDecision::Recorded;
    match kind {
        ChannelFetchKind::Search { .. } => Recorded("searches"),
        ChannelFetchKind::SongUrl { .. } => Recorded("url_resolutions"),
        ChannelFetchKind::MyPlaylists { .. } => Recorded("fetches"),
        ChannelFetchKind::PlaylistDetail { .. } => Recorded("fetches"),
        ChannelFetchKind::Lyrics { .. } => Recorded("fetches"),
        ChannelFetchKind::RemotePlayCount { .. } => Recorded("fetches"),
        ChannelFetchKind::ArtistDetail { .. } => Recorded("fetches"),
        ChannelFetchKind::ArtistAlbums { .. } => Recorded("fetches"),
        ChannelFetchKind::AlbumDetail { .. } => Recorded("fetches"),
    }
}

/// 歌单写操作入口的埋点归属(穷尽)。全部落 `playlist_ops`(outcome/error_kind 在终态填)。
fn audit_playlist_op(op: &PlaylistWriteOp) -> TrackingDecision {
    use TrackingDecision::Recorded;
    match op {
        PlaylistWriteOp::Create { .. } => Recorded("playlist_ops"),
        PlaylistWriteOp::Delete { .. } => Recorded("playlist_ops"),
        PlaylistWriteOp::AddSongs { .. } => Recorded("playlist_ops"),
        PlaylistWriteOp::RemoveSongs { .. } => Recorded("playlist_ops"),
        PlaylistWriteOp::Rename { .. } => Recorded("playlist_ops"),
        PlaylistWriteOp::SetDescription { .. } => Recorded("playlist_ops"),
    }
}

/// 行为域事件的发射点账本(穷尽)。新增 [`BehaviorEvent`] 变体不补此处即编译失败——
/// 声明「谁发它」;串值是给读者的站点索引,漂移由触发链集成测试兜。
fn audit_behavior_emitters(event: &BehaviorEvent) -> &'static str {
    match event {
        BehaviorEvent::Search { .. } => "channel_fetch 终态 + script_bridge library.search",
        BehaviorEvent::Seek { .. } => "PlayerCore::seek_playback(client / 脚本 / 媒体键统一出口)",
        BehaviorEvent::Pause { .. } => {
            "PlayerCore::pause_playback / resume_playback(client / 脚本 / 媒体键统一出口)"
        }
        BehaviorEvent::VolumeChange { .. } => "PlayerCore::set_playback_volume(统一出口)",
        BehaviorEvent::ModeChange { .. } => {
            "PlayerCore::record_mode_change(cycle / 直设 / 脚本共用)"
        }
        BehaviorEvent::LoveChange { .. } => {
            "PlayerCore::record_love_change(set_favorite / toggle_favorite 共用)"
        }
        BehaviorEvent::QueueOp { .. } => "ClientHandle 的 set_queue / insert_next / append",
        BehaviorEvent::PlaylistOp { .. } => "playlist 写 task 终态(events.rs)",
        BehaviorEvent::Fetch { .. } => "channel_fetch 终态(events.rs)",
        BehaviorEvent::Download { .. } => "download.rs record_download(三种结局)",
        BehaviorEvent::TaskCancel { .. } => "ClientHandle::cancel_tasks",
        BehaviorEvent::CopyRender { .. } => "serve.rs RenderCopyTemplate",
        BehaviorEvent::ActionInvocation { .. } => "serve.rs InvokeAction",
        BehaviorEvent::ConfigOverride { .. } => "script_bridge ConfigOverride",
        BehaviorEvent::StoreWrite { .. } => "serve.rs / script_bridge 的 StoreSet / StoreInc",
        BehaviorEvent::Spawn { .. } => "script_bridge 子进程收束回调",
        BehaviorEvent::BusMessage { .. } => "script_bridge 事件总线",
        BehaviorEvent::FullscreenChange { .. } => "serve.rs TerminalState",
        BehaviorEvent::ConnectionReject { .. } => "client.rs record_connection_reject",
        BehaviorEvent::AppLifecycle { .. } => {
            "recorder.daemon_lifecycle + serve.rs client 生命周期"
        }
    }
}

/// 系统域事件的发射点账本(穷尽)。新增 [`SystemEvent`] 变体不补此处即编译失败。
fn audit_system_emitters(event: &SystemEvent) -> &'static str {
    match event {
        SystemEvent::UrlResolution { .. } => "events.rs handle_play_url_ready / 取链失败分支",
        SystemEvent::HookFire { .. } => "hook_bridge 各提交点裁决回来处",
        SystemEvent::GaplessBoundary { .. } => "gapless.rs check_advance 边界裁决",
        SystemEvent::Prefetch { .. } => "gapless.rs 预取武装 / 否决 / 失败各终态",
        SystemEvent::CacheHarvest { .. } => "download.rs spawn_harvest 收割终态",
        SystemEvent::CacheEviction { .. } => "media_cache 淘汰点",
        SystemEvent::ScriptLifecycle { .. } => "script_reload + script_bridge 生命周期回调",
        SystemEvent::ConfigReload => "config_host 重载成功处",
    }
}

#[cfg(test)]
mod tests {
    /// 真实事件表 + plays 事实表(与 `mineral-stats` migrations 建的 28 张事件表 + `plays`
    /// 同源;mineral-stats 侧有测试把那 28 张与 migrations 对账,此处复制其名做本 crate 侧
    /// 的表名核对)。
    const REAL_TABLES: &[&str] = &[
        "plays",
        "searches",
        "seeks",
        "pauses",
        "volume_changes",
        "mode_changes",
        "love_changes",
        "queue_ops",
        "playlist_ops",
        "fetches",
        "downloads",
        "task_cancels",
        "copy_renders",
        "action_invocations",
        "config_overrides",
        "store_writes",
        "spawns",
        "bus_messages",
        "fullscreen_changes",
        "connection_rejects",
        "app_lifecycle",
        "url_resolutions",
        "hook_fires",
        "gapless_boundaries",
        "prefetches",
        "cache_harvests",
        "cache_evictions",
        "script_lifecycle",
        "config_reloads",
    ];

    /// audit_* 四函数里出现过的每个 `Recorded` 表名。**新增 `Recorded` 归属须同步补进来**,
    /// 由下方测试兜住「Recorded 了张不存在的表」(拼错 / 漂移)。
    const AUDIT_TABLES: &[&str] = &[
        "plays",
        "pauses",
        "seeks",
        "volume_changes",
        "mode_changes",
        "love_changes",
        "queue_ops",
        "downloads",
        "store_writes",
        "config_overrides",
        "fullscreen_changes",
        "app_lifecycle",
        "action_invocations",
        "copy_renders",
        "task_cancels",
        "spawns",
        "searches",
        "url_resolutions",
        "fetches",
        "playlist_ops",
    ];

    /// §9.8 对账:audit 声称记入的每张表都真实存在于 migrations(防表名拼错 / 迁移漂移)。
    #[test]
    fn audit_recorded_tables_exist_in_migrations() {
        for table in AUDIT_TABLES {
            assert!(
                REAL_TABLES.contains(table),
                "audit Recorded 表 {table} 不在真实事件表集合(拼错或迁移漂移?)"
            );
        }
    }
}
