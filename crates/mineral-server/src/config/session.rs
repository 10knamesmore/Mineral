//! Daemon 私有配置：合成文件底树与 session 覆盖，重配统计和下载策略。
//!
//! Client 只收到实际服务能力的变化，不接收配置树。坏覆盖按落型报错路径剔除并记录。

use std::sync::Arc;

use crate::config::DaemonConfig;
use mineral_protocol::{BusValue, PlayCountAvailability, ServiceInfo};
use mineral_script::ConfigOverrideOp;
use parking_lot::Mutex;

use crate::player::PlayerCore;

/// Daemon 配置底树、session 覆盖与已校验的有效配置。
pub(crate) struct ConfigHost {
    /// 文件重载与脚本覆盖共用的配置状态。
    state: Mutex<HostState>,
}

/// 配置宿主的内部状态；有效树与落型结果始终成对替换。
struct HostState {
    /// 默认值与 daemon.lua 合成的底树。
    base: serde_json::Value,

    /// 进程内覆盖表；相同路径就地替换，重载文件不会清空。
    overlay: Vec<(String, BusValue)>,

    /// 底树加覆盖后通过校验的有效树。
    effective: serde_json::Value,

    /// 有效树的落型结果，用于执行策略与对外服务能力。
    config: DaemonConfig,

    /// 已激活脚本的有效队列变换名称；运行时替换时随配置一起提交。
    queue_transforms: Vec<String>,
}

/// 被剔除的无效覆盖及其诊断。
struct EvictedOverride {
    /// 无效覆盖的配置路径。
    path: String,

    /// 一次落型失败可归属多条覆盖。
    warning: Arc<mineral_config::ConfigWarning>,
}

impl ConfigHost {
    /// 校验 daemon 底树并创建宿主；非法底树不能成为运行时配置。
    pub(crate) fn new(
        base: serde_json::Value,
        script_available: bool,
    ) -> Result<Self, mineral_config::ConfigWarning> {
        let config = crate::config::daemon_from_tree(&base)?;
        let queue_transforms = available_transforms(&config, script_available);
        Ok(Self {
            state: Mutex::new(HostState {
                effective: base.clone(),
                base,
                overlay: Vec::new(),
                config,
                queue_transforms,
            }),
        })
    }
}

/// 仅运行时可用时发布具名队列变换；保留声明顺序。
fn available_transforms(config: &DaemonConfig, script_available: bool) -> Vec<String> {
    if script_available {
        config
            .queue()
            .transforms()
            .iter()
            .map(|operation| operation.name().clone())
            .collect::<Vec<String>>()
    } else {
        Vec::<String>::new()
    }
}

/// 合成底树与覆盖；按失败路径剔除坏覆盖，返回校验成功的有效树与配置。
fn recompute(
    base: &serde_json::Value,
    overlay: &mut Vec<(String, BusValue)>,
) -> Result<(serde_json::Value, DaemonConfig, Vec<EvictedOverride>), mineral_config::ConfigWarning>
{
    let mut evicted = Vec::<EvictedOverride>::new();
    loop {
        let mut tree = base.clone();
        for (path, value) in overlay.iter() {
            tree = mineral_config::merge_tree(
                tree,
                mineral_config::nest_path(path, value.clone().into_json()),
            );
        }
        let warning = match crate::config::daemon_from_tree(&tree) {
            Ok(config) => return Ok((tree, config, evicted)),
            Err(warning) => warning,
        };
        if overlay.is_empty() {
            return Err(warning);
        }
        let warning = Arc::new(warning);
        let err_path = match warning.as_ref() {
            mineral_config::ConfigWarning::Deserialize { path, .. } => path.as_deref(),
            _ => None,
        };
        let before = overlay.len();
        overlay.retain(|(path, _value)| {
            let hit = err_path.is_none_or(|err_path| covers(path, err_path));
            if hit {
                evicted.push(EvictedOverride {
                    path: path.clone(),
                    warning: Arc::clone(&warning),
                });
            }
            !hit
        });
        if overlay.len() == before {
            evicted.extend(overlay.drain(..).map(|(path, _value)| EvictedOverride {
                path,
                warning: Arc::clone(&warning),
            }));
        }
    }
}

/// 覆盖路径与错误路径是否互为段前缀；整段覆盖可在子字段校验失败。
fn covers(overlay_path: &str, err_path: &str) -> bool {
    segment_prefix_of(overlay_path, err_path) || segment_prefix_of(err_path, overlay_path)
}

/// 检查段边界前缀，不把 download.dir 与 download.directory 视为同一路径。
fn segment_prefix_of(short: &str, long: &str) -> bool {
    long.strip_prefix(short)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.') || rest.starts_with('['))
}

impl PlayerCore {
    /// 文件重载成功后替换底树，保留 session 覆盖，重配 daemon 策略。
    pub(crate) fn set_config_base(&self, tree: serde_json::Value) {
        self.inner.stats.event(mineral_stats::StatsEvent::System(
            mineral_stats::SystemEvent::ConfigReload,
        ));
        let previous_info = self.service_info();
        let script_available = self
            .script_sender()
            .is_some_and(|sender| sender.is_attached());
        let result = {
            let mut guard = self.inner.config_host.state.lock();
            let state = &mut *guard;
            recompute(&tree, &mut state.overlay).map(|(effective, config, evicted)| {
                let changed = effective != state.effective;
                state.base = tree;
                state.queue_transforms = available_transforms(&config, script_available);
                state.effective = effective;
                state.config = config.clone();
                (changed, config, evicted)
            })
        };
        self.apply_recomputed_config(result, &previous_info);
    }

    /// 原子合成一批 daemon 配置覆盖；同值重写或撤销不存在路径不触发重配。
    pub(crate) fn apply_config_overrides(&self, ops: Vec<ConfigOverrideOp>) {
        let previous_info = self.service_info();
        let script_available = self
            .script_sender()
            .is_some_and(|sender| sender.is_attached());
        let result = {
            let mut guard = self.inner.config_host.state.lock();
            let state = &mut *guard;
            let mut touched = false;
            for ConfigOverrideOp { path, value } in ops {
                if value.as_ref().is_some_and(|value| {
                    mineral_config::nest_path(&path, value.clone().into_json())
                        .pointer("/queue/transforms")
                        .is_some()
                }) {
                    mineral_log::warn!(target: "config", path, "队列变换须随 daemon.lua 的函数定义一起加载,拒绝覆盖描述表");
                    self.notify()
                        .failure(mineral_protocol::FailureNotice::ConfigOverrideRejected { path });
                    continue;
                }
                touched |= match value {
                    Some(new_value) => match state.overlay.iter_mut().find(|(p, _)| *p == path) {
                        Some((_, existing)) if *existing == new_value => false,
                        Some((_, existing)) => {
                            *existing = new_value;
                            true
                        }
                        None => {
                            state.overlay.push((path, new_value));
                            true
                        }
                    },
                    None => {
                        let before = state.overlay.len();
                        state.overlay.retain(|(p, _)| *p != path);
                        state.overlay.len() != before
                    }
                };
            }
            if !touched {
                return;
            }
            recompute(&state.base, &mut state.overlay).map(|(effective, config, evicted)| {
                let changed = effective != state.effective;
                state.queue_transforms = available_transforms(&config, script_available);
                state.effective = effective;
                state.config = config.clone();
                (changed, config, evicted)
            })
        };
        self.apply_recomputed_config(result, &previous_info);
    }

    /// 当前 daemon 可执行的队列变换与统计采集策略，供请求与订阅重放使用。
    pub(crate) fn service_info(&self) -> ServiceInfo {
        let guard = self.inner.config_host.state.lock();
        ServiceInfo {
            queue_transforms: guard.queue_transforms.clone(),
            play_counts: PlayCountAvailability {
                enabled: self.inner.stats.play_counts_enabled(),
                excluded_sources: guard.config.stats().exclude_sources().clone(),
            },
        }
    }

    /// 重配统计和下载；只有对外能力变化才广播，不暴露 daemon 配置细节。
    fn apply_recomputed_config(
        &self,
        result: Result<(bool, DaemonConfig, Vec<EvictedOverride>), mineral_config::ConfigWarning>,
        previous_info: &ServiceInfo,
    ) {
        let (changed, config, evicted) = match result {
            Ok(result) => result,
            Err(error) => {
                mineral_log::error!(target: "config", error = mineral_log::chain(&error), "daemon 配置底树无效,保持旧策略");
                return;
            }
        };
        self.report_evicted_overrides(&evicted);
        if changed {
            self.inner
                .stats
                .set_params(crate::params_from_config(config.stats()));
            self.inner.downloads.set_config(
                *config.download().quality(),
                *config.download().max_concurrent(),
            );
            mineral_log::info!(target: "config", "daemon 运行时配置已更新");
        }
        let info = self.service_info();
        if &info != previous_info {
            self.notify().service_info_changed(info);
        }
    }

    /// 记录无效覆盖的完整诊断，client 仅收到路径和失败类别。
    fn report_evicted_overrides(&self, evicted: &[EvictedOverride]) {
        for e in evicted {
            mineral_log::warn!(
                target: "config",
                path = e.path,
                error = mineral_log::chain(e.warning.as_ref()),
                "daemon 配置覆盖无效,已撤销"
            );
            self.notify()
                .failure(mineral_protocol::FailureNotice::ConfigOverrideRejected {
                    path: e.path.clone(),
                });
        }
    }
}
