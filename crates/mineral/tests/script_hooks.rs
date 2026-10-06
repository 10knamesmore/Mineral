//! daemon.lua 的 setup、音乐 hook 与逐曲持久值的进程级 E2E。
//!
//! 每例使用隔离 XDG/socket 目录和 null 音频后端,不读取个人配置或执行 tui.lua。

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use color_eyre::eyre::{WrapErr, bail};
use mineral_client::Client;
use mineral_client::connection::ClientConfig;
use mineral_model::SongId;
use mineral_protocol::{
    Event, FailureNotice, QueueContextWire, SocketWire, StoreValue, Subscription, SubscriptionTopic,
};

/// 脚本命令提交与文件轮询的等待上限。
const WAIT: Duration = Duration::from_secs(10);

/// 一个隔离 daemon;停止后可保留数据目录供重启验证。
struct Daemon {
    /// daemon 子进程。
    child: Child,

    /// 隔离 XDG 根目录。
    root: PathBuf,

    /// 短 socket 目录,避免超过 Unix socket 路径上限。
    sock_dir: PathBuf,

    /// daemon socket。
    socket: PathBuf,

    /// Drop 时是否删除隔离目录。
    cleanup: bool,
}

impl Daemon {
    /// 分别写入独立 daemon/tui 文件,再启动真实 daemon 进程。
    fn spawn(tag: &str, daemon: Option<&str>, tui: Option<&str>) -> color_eyre::Result<Self> {
        let root = std::env::temp_dir().join(format!(
            "mineral-script-e2e-{tag}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        Self::spawn_in(root, daemon, tui)
    }

    /// None 保留既有文件,用于同一数据目录重启。
    fn spawn_in(
        root: PathBuf,
        daemon: Option<&str>,
        tui: Option<&str>,
    ) -> color_eyre::Result<Self> {
        let sock_dir =
            std::env::temp_dir().join(format!("mnls-{}-{}", std::process::id(), unique_suffix()));
        let cfg_dir = root.join("config/mineral");
        std::fs::create_dir_all(&cfg_dir).wrap_err("create config dir")?;
        for (name, source) in [("daemon.lua", daemon), ("tui.lua", tui)] {
            if let Some(source) = source {
                std::fs::write(cfg_dir.join(name), source).wrap_err("seed Lua source")?;
            }
        }
        let child = Command::new(env!("CARGO_BIN_EXE_mineral"))
            .arg("serve")
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("MINERAL_SOCKET_DIR", &sock_dir)
            .env("MINERAL_AUDIO_NULL", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .wrap_err("spawn mineral serve")?;
        let socket = sock_dir.join("mineral.sock");
        Ok(Self {
            child,
            root,
            sock_dir,
            socket,
            cleanup: true,
        })
    }

    /// 停止进程,保留配置和数据库。
    fn stop_keep_data(mut self) -> PathBuf {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.sock_dir);
        self.cleanup = false;
        self.root.clone()
    }

    /// 等 socket 可连接。
    fn wait_ready(&self) -> color_eyre::Result<()> {
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            if UnixStream::connect(&self.socket).is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        bail!("daemon did not become ready in time");
    }

    /// 创建真实 IPC 会话。
    async fn connect(&self) -> color_eyre::Result<Client> {
        let wire = SocketWire::connect(&self.socket).await?;
        Client::from_wire(Box::new(wire), "script_hooks", ClientConfig::cli())
            .await
            .map_err(color_eyre::Report::new)
    }

    /// 重写独立用户文件,由对应宿主决定是否加载。
    fn write_source(&self, name: &str, source: &str) -> color_eyre::Result<()> {
        std::fs::write(self.root.join("config/mineral").join(name), source)
            .wrap_err("rewrite Lua source")
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if self.cleanup {
            let _ = std::fs::remove_dir_all(&self.root);
            let _ = std::fs::remove_dir_all(&self.sock_dir);
        }
    }
}

/// 临时目录唯一后缀。
fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

/// 通过真实 IPC 等待 daemon 脚本的异步存储提交。
async fn wait_store_int(
    client: &Client,
    song: &SongId,
    key: &str,
    want: i64,
) -> color_eyre::Result<()> {
    let deadline = Instant::now() + WAIT;
    loop {
        let value = client.store_get(song.clone(), key).await;
        if value.into_success() == Some(StoreValue::Int(want)) {
            return Ok(());
        }
        if Instant::now() > deadline {
            bail!("store value for {key} did not become {want}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 等实际播放器状态变化,而不是仅检查 setup 文件通过加载。
async fn wait_volume(client: &Client, volume: u8) -> color_eyre::Result<()> {
    let deadline = Instant::now() + WAIT;
    while client.playback_snapshot().volume_pct != volume {
        if Instant::now() > deadline {
            bail!("setup volume did not become {volume}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}

/// daemon.lua 的 setup 接收音乐 API;本地 tui.lua 不参与 daemon 配置或执行。
#[tokio::test]
async fn daemon_setup_store_persists_and_ignores_tui_file() -> color_eyre::Result<()> {
    use mineral_model::SourceKind;
    let song = SongId::new(SourceKind::NETEASE, "42");
    let first = Daemon::spawn(
        "store",
        Some(
            r#"return {
                setup = function(api)
                    assert(_G.mineral == nil)
                    assert(api.ui == nil)
                    assert(type(api.player.stop) == "function")
                    api.player.set_volume(41)
                    api.store.get("netease:42", "plugin.saved", function(value, err)
                        if err then error(err) end
                        if value == nil then
                            api.store.set("netease:42", "plugin.saved", 41)
                        else
                            api.store.set("netease:42", "plugin.readback", value)
                        end
                    end)
                end
            }"#,
        ),
        Some(
            r#"return {
                setup = function(api) error("tui setup must not run in daemon") end,
                heartbeat_secs = 7,
            }"#,
        ),
    )?;
    first.wait_ready()?;
    let client = first.connect().await?;
    client.subscribe(SubscriptionTopic::Playback);
    wait_store_int(&client, &song, "plugin.saved", 41).await?;
    wait_volume(&client, 41).await?;
    // 即便 TUI 文件无法解析,daemon 重启仍只读取 daemon.lua。
    first.write_source("tui.lua", "this is not lua ((")?;
    drop(client);
    let root = first.stop_keep_data();
    let second = Daemon::spawn_in(root, None, None)?;
    second.wait_ready()?;
    let client = second.connect().await?;
    wait_store_int(&client, &song, "plugin.readback", 41).await?;
    Ok(())
}

/// 配置和 setup 来自同一份 daemon.lua;hook 写出其所属成功重载的版本。
fn hook_source(version: i64, name: &str, level: &str) -> String {
    format!(
        r#"return {{
            stats = {{ level = "{level}" }},
            queue = {{ transforms = {{
                {{ name = "{name}", transform = function(queue) return queue end }},
            }} }},
            setup = function(api)
                api.player.set_volume({volume})
                api.store.set("netease:42", "plugin.ready", {version})
                api.hook("before_stream", function(ctx)
                    api.store.set(ctx.song.id, "plugin.hook", {version})
                end)
            end
        }}"#,
        volume = 40 + version,
    )
}

/// 等 daemon 公开其实际可用操作,不读取私有配置树。
async fn wait_transform_name(client: &Client, name: &str) -> color_eyre::Result<()> {
    let deadline = Instant::now() + WAIT;
    loop {
        if client
            .service_info()
            .await
            .into_success()
            .is_some_and(|info| info.queue_transforms == [name])
        {
            return Ok(());
        }
        if Instant::now() > deadline {
            bail!("daemon did not publish the reloaded transform");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 没有该来源的 playback provider 时也运行 before_stream,无需网络或真实媒体。
async fn trigger_hook(client: &Client, id: &str, version: i64) -> color_eyre::Result<()> {
    let song = mineral_test::song(id);
    assert!(
        client
            .play_queue(vec![song.clone()], 0, QueueContextWire::Manual)?
            .outcome()
            .await
            .is_success()
    );
    wait_store_int(client, &song.id, "plugin.hook", version).await
}

/// daemon 文件重载替换 hook、能力与 setup 效果;失败 setup 不提交命令并保留旧 VM。
#[tokio::test]
async fn daemon_reload_keeps_old_state_when_setup_fails() -> color_eyre::Result<()> {
    let daemon = Daemon::spawn(
        "reload",
        Some(&hook_source(1, "Keep original order", "off")),
        None,
    )?;
    daemon.wait_ready()?;
    let client = daemon.connect().await?;
    client.subscribe(SubscriptionTopic::Playback);
    client.subscribe(SubscriptionTopic::Events(Subscription::Toast));
    let marker = SongId::new(mineral_model::SourceKind::NETEASE, "42");
    wait_store_int(&client, &marker, "plugin.ready", 1).await?;
    wait_transform_name(&client, "Keep original order").await?;
    trigger_hook(&client, "initial-hook", 1).await?;

    daemon.write_source("daemon.lua", &hook_source(2, "Keep current order", "core"))?;
    wait_store_int(&client, &marker, "plugin.ready", 2).await?;
    wait_transform_name(&client, "Keep current order").await?;
    wait_volume(&client, 42).await?;
    trigger_hook(&client, "reloaded-hook", 2).await?;

    daemon.write_source(
        "daemon.lua",
        r#"return {
            stats = { level = "off" },
            queue = { transforms = {
                { name = "Rejected operation", transform = function(queue) return queue end },
            } },
            setup = function(api)
                api.player.set_volume(1)
                api.store.set("netease:42", "plugin.rejected", 99)
                api.config.override("stats.level", "off")
                api.hook("before_stream", function(ctx)
                    api.store.set(ctx.song.id, "plugin.hook", 99)
                end)
                error("setup failed")
            end
        }"#,
    )?;
    let deadline = Instant::now() + WAIT;
    loop {
        if client.mirror().drain_events().into_iter().any(|event| {
            matches!(
                event,
                Event::Failure(FailureNotice::ScriptReloadFailed {
                    previous_kept: true
                })
            )
        }) {
            break;
        }
        if Instant::now() > deadline {
            bail!("failed setup reload was not reported");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // 先通过旧 hook 的持久提交,再检查失败 setup 没有留下任何命令效果。
    trigger_hook(&client, "preserved-hook", 2).await?;
    assert_eq!(
        client
            .store_get(marker, "plugin.rejected")
            .await
            .into_success(),
        Some(StoreValue::Nil)
    );
    assert_eq!(client.playback_snapshot().volume_pct, 42);
    let info = client
        .service_info()
        .await
        .into_success()
        .ok_or_else(|| color_eyre::eyre::eyre!("service info query failed"))?;
    assert_eq!(info.queue_transforms, vec!["Keep current order"]);
    assert!(info.play_counts.enabled);
    Ok(())
}
