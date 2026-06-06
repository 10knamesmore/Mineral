//! daemon 侧脚本桥:装配件透传([`ScriptParts`])+ 双泵(脚本命令 →
//! player 执行面;脚本推送 → event hub)。
//!
//! 启动顺序(见 [`ScriptParts::spawn_runtime`]):脚本线程先于
//! [`Server`](crate::Server) 起(只需 VM + host),其投递句柄再喂给
//! Server 的事件出口 —— 无环。

use mineral_protocol::{DownloadTarget, Event};
use mineral_script::mlua::Lua;
use mineral_script::{ScriptCmd, ScriptHost, ScriptRuntime, WatchdogConfig};
use num_traits::ToPrimitive;
use tokio::sync::broadcast;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::player::PlayerCore;

/// daemon 入口(main)装配、`serve` 层消费的脚本部件包。
///
/// `vm` 为 `None` 表示无用户脚本(文件缺失 / eval 失败已降级),此时只有
/// 泵在跑(命令通道无生产者、推送通道无生产者,等于闲置)。
pub struct ScriptParts {
    /// eval 过用户脚本的 VM(无脚本为 `None`)。
    vm: Option<Lua>,

    /// 宿主句柄(与 VM 内 API 闭包共享注册表)。
    host: ScriptHost,

    /// 脚本 → daemon 的命令出口接收端。
    cmd_rx: UnboundedReceiver<ScriptCmd>,

    /// 脚本 → client 的推送出口接收端。
    push_rx: UnboundedReceiver<Event>,
}

impl ScriptParts {
    /// 打包装配件(由 daemon 入口在 `load_with_vm` 后构造)。
    ///
    /// # Params:
    ///   - `vm`: `load_with_vm` 交还的 VM
    ///   - `host`: 与 `install_api` 同一个宿主句柄
    ///   - `cmd_rx`: 命令通道接收端
    ///   - `push_rx`: 推送通道接收端
    #[must_use]
    pub fn new(
        vm: Option<Lua>,
        host: ScriptHost,
        cmd_rx: UnboundedReceiver<ScriptCmd>,
        push_rx: UnboundedReceiver<Event>,
    ) -> Self {
        Self {
            vm,
            host,
            cmd_rx,
            push_rx,
        }
    }

    /// 起脚本线程(若有 VM)。须在 [`Server::spawn`](crate::Server::spawn)
    /// **之前**调用,返回的 runtime 句柄由调用方持有到 daemon 退出
    /// (Drop = 停机 + join);spawn 失败降级无脚本(warn)。
    ///
    /// # Params:
    ///   - `watchdog`: 回调看门狗参数(配置 `script` 段派生)
    ///
    /// # Return:
    ///   `(runtime, rest)`:runtime 为 `None` 表示无脚本;rest 是泵接线件。
    #[must_use]
    pub fn spawn_runtime(self, watchdog: WatchdogConfig) -> (Option<ScriptRuntime>, ScriptPumps) {
        let runtime =
            self.vm.and_then(
                |lua| match ScriptRuntime::spawn(lua, self.host.clone(), watchdog) {
                    Ok(runtime) => Some(runtime),
                    Err(e) => {
                        mineral_log::warn!(
                            target: "script",
                            error = mineral_log::chain(&e),
                            "脚本线程启动失败,降级无脚本"
                        );
                        None
                    }
                },
            );
        (
            runtime,
            ScriptPumps {
                cmd_rx: self.cmd_rx,
                push_rx: self.push_rx,
            },
        )
    }
}

/// [`ScriptParts::spawn_runtime`] 拆出的泵接线件,等 Server 就绪后接上。
pub struct ScriptPumps {
    /// 命令通道接收端。
    cmd_rx: UnboundedReceiver<ScriptCmd>,

    /// 推送通道接收端。
    push_rx: UnboundedReceiver<Event>,
}

impl ScriptPumps {
    /// 接上两条泵:脚本命令 → player 执行面;脚本推送 → event hub。
    ///
    /// # Params:
    ///   - `player`: 命令执行面
    ///   - `sink`: event hub 发送端
    pub(crate) fn start(self, player: PlayerCore, sink: broadcast::Sender<Event>) {
        let Self {
            mut cmd_rx,
            mut push_rx,
        } = self;
        tokio::spawn(async move {
            while let Some(event) = push_rx.recv().await {
                // 无订阅者 send 失败即丢(advisory)。
                let _ = sink.send(event);
            }
        });
        tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                apply_cmd(&player, cmd);
            }
        });
    }
}

/// 把一条脚本命令落到 player 执行面(与 client Request 同一些方法)。
fn apply_cmd(player: &PlayerCore, cmd: ScriptCmd) {
    match cmd {
        ScriptCmd::Toggle => {
            if player.audio_snapshot().playing {
                player.audio().pause();
            } else {
                player.audio().resume();
            }
        }
        ScriptCmd::Next => player.next_song(),
        ScriptCmd::Prev => player.prev_or_restart(),
        ScriptCmd::Stop => player.stop_playback(),
        ScriptCmd::SeekRel(secs) => {
            let delta_ms = (secs * 1000.0).round().to_i64().unwrap_or(0);
            let pos = i64::try_from(player.audio_snapshot().position_ms).unwrap_or(i64::MAX);
            let target = pos.saturating_add(delta_ms).max(0);
            player.audio().seek(u64::try_from(target).unwrap_or(0));
        }
        ScriptCmd::SeekTo(secs) => {
            let target_ms = (secs * 1000.0).round().to_u64().unwrap_or(0);
            player.audio().seek(target_ms);
        }
        ScriptCmd::SetVolume(pct) => player.audio().set_volume(pct),
        ScriptCmd::SetMode(mode) => player.set_play_mode(mode),
        ScriptCmd::Play(id) => {
            let song = player.with_state(|st| st.queue.iter().find(|s| s.id == id).cloned());
            match song {
                Some(song) => player.play_song(&song),
                // 队列外跳播是后续能力;当前 warn 丢弃,不拉详情。
                None => mineral_log::warn!(
                    target: "script",
                    song_id = id.qualified(),
                    "play: 不在当前队列,忽略"
                ),
            }
        }
        ScriptCmd::Download(id) => {
            let song = player.with_state(|st| {
                st.queue
                    .iter()
                    .find(|s| s.id == id)
                    .or(st.current_song.as_ref())
                    .filter(|s| s.id == id)
                    .cloned()
            });
            match song {
                Some(song) => player.download(DownloadTarget::Song(Box::new(song))),
                None => mineral_log::warn!(
                    target: "script",
                    song_id = id.qualified(),
                    "download: 不在当前队列,忽略"
                ),
            }
        }
    }
}
