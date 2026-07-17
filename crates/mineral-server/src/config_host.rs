//! 有效配置宿主:daemon 是配置的唯一 watch 点与合成者,client 只消费推送。
//!
//! 状态 = 合成底树(default + user,加载管线产物)+ session 覆盖表
//! (`mineral.config.override`,daemon 重启即清)+ 窗口标题覆盖(高频直通,
//! **不**参与合成)。任一变更 → 重算有效树 → 落型校验 → 广播
//! [`Event::ConfigChanged`](mineral_protocol::Event::ConfigChanged);坏覆盖按
//! 报错路径剔除并警告,有效树永远是校验通过的那份。新 client 握手时经
//! [`PlayerCore::effective_config`] 重放当前有效配置。

use mineral_protocol::BusValue;
use mineral_script::ConfigOverrideOp;
use parking_lot::Mutex;

use crate::player::PlayerCore;

/// 宿主本体(挂在 [`Inner`](crate::player) 上)。
pub(crate) struct ConfigHost {
    /// 内部状态(底树 / 覆盖表 / 有效树 / 标题覆盖)。
    state: Mutex<HostState>,
}

/// [`ConfigHost`] 的内部状态。
struct HostState {
    /// 合成底树(default + user;文件重载时整树替换)。
    base: serde_json::Value,

    /// session 覆盖表(插入序;同 path 重写就地替换)。
    overlay: Vec<(String, BusValue)>,

    /// 当前有效树(base + overlay 合成、已过落型校验)。
    effective: serde_json::Value,

    /// 窗口标题覆盖(渲染产物直通,不进合成)。
    window_title: Option<String>,
}

/// 一条被剔除的坏覆盖(供日志 / toast 报告)。
struct EvictedOverride {
    /// 覆盖的配置路径。
    path: String,

    /// 落型报错详情。
    detail: String,
}

impl ConfigHost {
    /// 以加载管线产出的合成底树建宿主(底树已过落型校验,直接作为初始有效树)。
    ///
    /// # Params:
    ///   - `base`: 合成底树
    pub(crate) fn new(base: serde_json::Value) -> Self {
        Self {
            state: Mutex::new(HostState {
                effective: base.clone(),
                base,
                overlay: Vec::new(),
                window_title: None,
            }),
        }
    }
}

/// 重算有效树:base + overlay 逐条叠加 → 落型校验;失败按报错路径剔除肇事
/// 覆盖再试(每轮至少剔一条,收敛)。overlay 就地收缩,剔除清单返回给调用方
/// 报告。
///
/// # Params:
///   - `base`: 合成底树(生产路径已过校验)
///   - `overlay`: session 覆盖表(坏条目被就地剔除)
///
/// # Return:
///   `(有效树, 剔除清单)`
fn recompute(
    base: &serde_json::Value,
    overlay: &mut Vec<(String, BusValue)>,
) -> (serde_json::Value, Vec<EvictedOverride>) {
    let mut evicted = Vec::<EvictedOverride>::new();
    loop {
        let mut tree = base.clone();
        for (path, value) in overlay.iter() {
            tree = mineral_config::merge_tree(
                tree,
                mineral_config::nest_path(path, value.clone().into_json()),
            );
        }
        let warning = match mineral_config::from_tree(&tree) {
            Ok(_config) => return (tree, evicted),
            Err(warning) => warning,
        };
        if overlay.is_empty() {
            // 底树自身落型失败:生产路径不可达(管线已校验),测试注入坏底树
            // 时兜底——有效树退回底树,不无限重试。
            mineral_log::error!(
                target: "config",
                warning = %warning,
                "配置底树落型失败(不该发生),有效树退回底树"
            );
            return (base.clone(), evicted);
        }
        let (err_path, detail) = match &warning {
            mineral_config::ConfigWarning::Deserialize { path, detail } => {
                (path.clone(), detail.clone())
            }
            other => (String::new(), other.to_string()),
        };
        let before = overlay.len();
        overlay.retain(|(path, _value)| {
            let hit = err_path.is_empty() || covers(path, &err_path);
            if hit {
                evicted.push(EvictedOverride {
                    path: path.clone(),
                    detail: detail.clone(),
                });
            }
            !hit
        });
        if overlay.len() == before {
            // 报错路径与所有覆盖都对不上(不该发生):整表清空兜底,防死循环。
            evicted.extend(overlay.drain(..).map(|(path, _value)| EvictedOverride {
                path,
                detail: detail.clone(),
            }));
        }
    }
}

/// 覆盖 path 与落型报错路径是否互为段前缀(`tui.lyrics` 覆盖整段时报错可能
/// 深入到 `tui.lyrics.gap`;unknown field 反过来报在父段)。
fn covers(overlay_path: &str, err_path: &str) -> bool {
    segment_prefix_of(overlay_path, err_path) || segment_prefix_of(err_path, overlay_path)
}

/// `short` 是否为 `long` 的段边界前缀(下一字符是 `.` / `[` 或串尾)。
fn segment_prefix_of(short: &str, long: &str) -> bool {
    long.strip_prefix(short)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.') || rest.starts_with('['))
}

impl PlayerCore {
    /// 换配置底树(配置文件热重载成功后调):重算有效树,变了才广播。
    ///
    /// # Params:
    ///   - `tree`: 新合成底树(加载管线产物)
    pub(crate) fn set_config_base(&self, tree: serde_json::Value) {
        // 埋点:配置从磁盘重载(config_reloads;此入口仅由 mtime 重载回调驱动,故一次
        // 调用 = 一次真重载,即便有效树最终未变——「重读了文件」本身是要记的事实)。
        self.inner.stats.event(mineral_stats::StatsEvent::System(
            mineral_stats::SystemEvent::ConfigReload,
        ));
        let (changed, effective, evicted) = {
            let mut guard = self.inner.config_host.state.lock();
            let state = &mut *guard;
            state.base = tree;
            let (effective, evicted) = recompute(&state.base, &mut state.overlay);
            let changed = effective != state.effective;
            state.effective = effective.clone();
            (changed, effective, evicted)
        };
        self.report_evicted_overrides(&evicted);
        if changed {
            self.reapply_stats(&effective);
            self.notify().config_changed(BusValue::from_json(effective));
        }
    }

    /// 落一批脚本配置覆盖:逐条更新覆盖表 → **一次**重算校验 → 有效树变了
    /// 才广播**一帧**(表对象形一次调用拍出多条叶子;字符串形 = 长度 1)。
    ///
    /// 同值重写 / 撤销不存在的 path 不算 touch(脚本常在 observe 回调里无脑
    /// 重设,diff 掉无谓的下推),整批没 touch 直接返回;坏 path / 坏值按叶子
    /// 剔除并警告,好叶子不受殃及。
    ///
    /// # Params:
    ///   - `ops`: 叶子覆盖(`None` 值 = 撤销)
    pub(crate) fn apply_config_overrides(&self, ops: Vec<ConfigOverrideOp>) {
        let (changed, effective, evicted) = {
            let mut guard = self.inner.config_host.state.lock();
            let state = &mut *guard;
            let mut touched = false;
            for ConfigOverrideOp { path, value } in ops {
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
            let (effective, evicted) = recompute(&state.base, &mut state.overlay);
            let changed = effective != state.effective;
            state.effective = effective.clone();
            (changed, effective, evicted)
        };
        self.report_evicted_overrides(&evicted);
        if changed {
            self.reapply_stats(&effective);
            self.notify().config_changed(BusValue::from_json(effective));
        }
    }

    /// 当前有效配置(新 client 握手订阅 `Config` 时重放一帧)。
    pub(crate) fn effective_config(&self) -> BusValue {
        BusValue::from_json(self.inner.config_host.state.lock().effective.clone())
    }

    /// 配置重算后把 stats 采集侧旋钮折算给 recorder 热更(level / gap / exclude 等,
    /// 只影响后续采集;`report` 口径不进此处,报告装配时现读)。落型失败保持旧参数。
    ///
    /// # Params:
    ///   - `effective`: 新有效配置树
    fn reapply_stats(&self, effective: &serde_json::Value) {
        match serde_json::from_value::<mineral_config::Config>(effective.clone()) {
            Ok(config) => self
                .inner
                .stats
                .set_params(crate::params_from_config(config.stats())),
            Err(e) => {
                mineral_log::warn!(target: "stats", error = mineral_log::chain(&e), "配置落型失败,保持旧采集参数");
            }
        }
    }

    /// 落窗口标题覆盖(渲染产物直通:不进合成、不触发重算,10fps 级高频友好)。
    ///
    /// # Params:
    ///   - `text`: 覆盖文本;`None` = 撤销
    pub(crate) fn apply_window_title_override(&self, text: Option<String>) {
        let changed = {
            let mut state = self.inner.config_host.state.lock();
            if state.window_title == text {
                false
            } else {
                state.window_title.clone_from(&text);
                true
            }
        };
        if changed {
            self.notify().window_title_override(text);
        }
    }

    /// 当前窗口标题覆盖(新 client 握手订阅 `WindowTitle` 时重放;无覆盖不发)。
    pub(crate) fn window_title_override(&self) -> Option<String> {
        self.inner.config_host.state.lock().window_title.clone()
    }

    /// 报告被剔除的坏覆盖:warn 日志 + toast 提示(脚本作者的诊断出口)。
    fn report_evicted_overrides(&self, evicted: &[EvictedOverride]) {
        for e in evicted {
            mineral_log::warn!(
                target: "config",
                path = e.path,
                detail = e.detail,
                "配置覆盖无效,已撤销"
            );
            self.notify().toast(
                mineral_protocol::ToastKind::Warn,
                format!("配置覆盖无效已撤销:{}({})", e.path, e.detail),
            );
        }
    }
}
