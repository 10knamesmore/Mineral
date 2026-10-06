//! 服务能力与实际队列操作的接线：按名称跨重载执行，不暴露 daemon 配置。

use std::path::Path;

use mineral_protocol::{Event, QueueEditOutcome, QueueOp, Subscription};
use mineral_script::{ScriptHost, ScriptRuntime, ScriptSender, WatchdogConfig, install_daemon_api};
use mineral_test::song;

use super::fixtures::core_with_events;
use crate::media_cache::MediaCache;
use crate::persistence::ServerStore;

/// 与 daemon 入口相同的加载路径；文件 helper 不在 async 测试体内阻塞。
fn load_script(
    path: &Path,
    source: &str,
    host: &ScriptHost,
) -> color_eyre::Result<crate::config::DaemonLoad> {
    std::fs::write(path, source)?;
    let loaded = crate::config::load_daemon_with_vm(path, |lua| install_daemon_api(lua, host))?;
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    Ok(loaded)
}

/// 两个操作声明可交换顺序；调用标识始终是名称。
fn transform_source(reversed: bool) -> String {
    let first = r#"{name = "keep first", transform = function(songs) return {songs[1]} end}"#;
    let second = r#"{name = "keep second", transform = function(songs) return {songs[2]} end}"#;
    let operations = if reversed {
        format!("{second}, {first}")
    } else {
        format!("{first}, {second}")
    };
    format!("return {{queue = {{transforms = {{{operations}}}}}}}")
}

/// 具名操作可从服务查询、订阅重放和变更获得；重排声明不会错执行另一操作。
#[tokio::test]
async fn service_names_dispatch_stably_across_reload_and_disappear_on_detach()
-> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("daemon.lua");
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (push_tx, _push_rx) = tokio::sync::mpsc::unbounded_channel();
    let sender = ScriptSender::detached();
    let watchdog = WatchdogConfig::builder()
        .instruction_interval(10_000)
        .soft_wall(std::time::Duration::from_millis(200))
        .hard_wall(std::time::Duration::from_secs(1))
        .build();
    let host = ScriptHost::new(cmd_tx.clone(), push_tx.clone());
    let loaded = load_script(&path, &transform_source(false), &host)?;
    let lua = loaded
        .vm
        .ok_or_else(|| color_eyre::eyre::eyre!("daemon VM unavailable"))?;
    let runtime = ScriptRuntime::spawn(lua, host, watchdog, &sender)?;
    let (events, mut updates) = tokio::sync::broadcast::channel(16);
    let core = core_with_events(
        Vec::new(),
        ServerStore::disabled(),
        None,
        MediaCache::disabled(),
        events,
        Some(sender.clone()),
    )?;
    core.set_config_base(loaded.tree);
    let info = core.service_info();
    assert_eq!(info.queue_transforms, vec!["keep first", "keep second"]);
    assert_eq!(
        updates.try_recv()?,
        Event::ServiceInfoChanged { info: info.clone() }
    );
    let client = crate::ClientHandle::new(core.clone());
    assert_eq!(
        client.replay_frames(&[Subscription::ServiceInfo]).await,
        vec![Event::ServiceInfoChanged { info }]
    );

    let host = ScriptHost::new(cmd_tx, push_tx);
    let reloaded = load_script(&path, &transform_source(true), &host)?;
    let lua = reloaded
        .vm
        .ok_or_else(|| color_eyre::eyre::eyre!("reloaded daemon VM unavailable"))?;
    let new_runtime = ScriptRuntime::spawn(lua, host, watchdog, &sender)?;
    drop(runtime);
    core.set_config_base(reloaded.tree);
    assert_eq!(
        core.service_info().queue_transforms,
        vec!["keep second", "keep first"]
    );
    assert_eq!(
        updates.try_recv()?,
        Event::ServiceInfoChanged {
            info: core.service_info()
        }
    );
    core.replace_queue(
        vec![song("a"), song("b")],
        0,
        mineral_stats::QueueContext::Unknown,
    )?;
    assert_eq!(
        client
            .queue_edit_async(QueueOp::ApplyTransform {
                name: "keep second".to_owned(),
                selected: None
            })
            .await,
        QueueEditOutcome::Applied
    );
    assert_eq!(
        core.with_state(|state| state.queue.clone()),
        vec![song("b")]
    );

    let available = core.service_info();
    core.apply_config_overrides(vec![mineral_script::ConfigOverrideOp {
        path: "queue.transforms".to_owned(),
        value: Some(mineral_protocol::BusValue::Array(vec![
            mineral_protocol::BusValue::Map(vec![(
                "name".to_owned(),
                mineral_protocol::BusValue::Str("not registered".to_owned()),
            )]),
        ])),
    }]);
    assert_eq!(core.service_info(), available);
    assert_eq!(
        updates.try_recv()?,
        Event::Failure(mineral_protocol::FailureNotice::ConfigOverrideRejected {
            path: "queue.transforms".to_owned(),
        })
    );
    core.apply_config_overrides(vec![mineral_script::ConfigOverrideOp {
        path: "queue.transforms".to_owned(),
        value: None,
    }]);
    assert!(updates.try_recv().is_err());

    sender.detach();
    drop(new_runtime);
    core.set_config_base(crate::config::default_daemon_tree()?);
    assert!(core.service_info().queue_transforms.is_empty());
    assert_eq!(
        updates.try_recv()?,
        Event::ServiceInfoChanged {
            info: core.service_info()
        }
    );
    Ok(())
}
