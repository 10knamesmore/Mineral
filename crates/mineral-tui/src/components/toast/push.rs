//! server 主动推送([`Event`])到通知层的翻译:脚本通知保留 Toast/Card 载荷,
//! 后台失败由 TUI 按结构化类别生成文案与生命周期。

use mineral_protocol::{Event, FailureNotice, ToastKind};

use crate::components::toast::card::{plain_body, plain_line};
use crate::components::toast::notifications::{
    Notifications, TextTint, tinted_spans_item, tinted_text_item,
};

/// 脚本回调失败的顶替键，连续失败只保留一条提示。
const SCRIPT_ERROR_TOAST_ID: &str = "script.error";

/// 脚本重载提示的顶替键，与原脚本重载通知一致。
const SCRIPT_RELOAD_TOAST_ID: &str = "script.reload";

/// 配置重载警告卡的顶替键，干净重载的 DismissToast 使用同一 id。
const CONFIG_RELOAD_CARD_ID: &str = "config.reload";

/// 消费一条 server 推送:
///   - Toast 按 kind 上色进单行 flash(`id: Some` 顶替 / `None` 堆叠;
///     `ttl_secs: Some` 覆盖默认展示时长);
///   - Card 进多行卡片(同款 id 语义;`ttl_secs: Some` 到时自动退场,
///     `None` 驻留到用户显式关闭);
///   - Failure 按类别生成本地提示，配置警告卡与干净重载撤卡共用 id；
///   - 其余类别已由 App 分流，或不属于内置 TUI 的通知面。
///
/// # Params:
///   - `notifications`: 通知层
///   - `event`: server 推送的事件
pub(crate) fn apply_event(notifications: &mut Notifications, event: Event) {
    match event {
        Event::Failure(failure) => apply_failure(notifications, failure),
        Event::Toast {
            kind,
            content,
            id,
            ttl_secs,
        } => {
            let item = tinted_spans_item(content, tint_of(kind));
            let ttl = ttl_secs.map(std::time::Duration::from_secs);
            match id {
                Some(key) => notifications.flash_keyed_for(key, item, ttl),
                None => notifications.flash_for(item, ttl),
            }
        }
        Event::Card {
            kind,
            id,
            title,
            body,
            ttl_secs,
        } => {
            notifications.push_card(
                tint_of(kind),
                title,
                body,
                id,
                ttl_secs.map(std::time::Duration::from_secs),
            );
        }
        // daemon 主动撤驻留通知(坏配置修好后撤警告卡等)。
        Event::DismissToast { id } => notifications.dismiss_card_by_id(&id),
        // Task / ScriptReloaded / ConfigChanged / WindowTitleOverride / TrackFinished 在
        // App::drain_push_events 已分流(apply 数据 / 刷新 bind 键 / 应用有效
        // 配置 / 落标题覆盖 / 更新本地完播 cache),这里只是穷尽兜底。BusMessage 是用户自定义消息,
        // 内置 TUI 不订阅也不解释(语义契约在用户脚本与其外部工具之间)。
        Event::PropertyChanged { .. }
        | Event::TrackFinished { .. }
        | Event::DownloadCompleted { .. }
        | Event::StoreChanged { .. }
        | Event::ScriptReloaded
        | Event::BusMessage { .. }
        | Event::ConfigChanged { .. }
        | Event::WindowTitleOverride { .. }
        | Event::Task(_) => {}
    }
}

/// 按失败类别在本地生成通知；诊断详情只写入 daemon 日志。
fn apply_failure(notifications: &mut Notifications, failure: FailureNotice) {
    match failure {
        FailureNotice::ConfigOverrideRejected { path } => notifications.flash(tinted_text_item(
            format!("Could not apply config override at {path}"),
            TextTint::Warn,
        )),
        FailureNotice::ScriptReloadFailed { previous_kept } => {
            let message = if previous_kept {
                "Script reload failed; previous script is still running"
            } else {
                "Script reload failed; scripts are unavailable"
            };
            notifications.flash_keyed_for(
                SCRIPT_RELOAD_TOAST_ID.to_owned(),
                tinted_text_item(message.to_owned(), TextTint::Error),
                /*ttl*/ None,
            );
        }
        FailureNotice::ConfigRejected { fields } => {
            let mut lines = if fields.is_empty() {
                vec!["config.lua could not be applied".to_owned()]
            } else {
                fields
                    .into_iter()
                    .map(|field| format!("config.lua: invalid {field}"))
                    .collect::<Vec<String>>()
            };
            lines.push("keeping current config; see logs for details".to_owned());
            notifications.push_card(
                TextTint::Warn,
                plain_line("config.lua warnings"),
                plain_body(lines),
                Some(CONFIG_RELOAD_CARD_ID.to_owned()),
                /*ttl*/ None,
            );
        }
        FailureNotice::ScriptCallbackFailed { callback } => notifications.flash_keyed_for(
            SCRIPT_ERROR_TOAST_ID.to_owned(),
            tinted_text_item(
                format!("Script {callback} callback failed; see logs"),
                TextTint::Error,
            ),
            /*ttl*/ None,
        ),
        FailureNotice::QueueTransformFailed => notifications.flash(tinted_text_item(
            "Could not transform queue".to_owned(),
            TextTint::Warn,
        )),
        FailureNotice::QueueTransformInvalidSong => notifications.flash(tinted_text_item(
            "Queue transform returned an invalid song".to_owned(),
            TextTint::Warn,
        )),
        FailureNotice::PlaylistDownloadFailed { id } => notifications.flash(tinted_text_item(
            format!("Could not download playlist {}", id.qualified()),
            TextTint::Warn,
        )),
    }
}

/// 协议视觉级别 → 通知层语义级别。
fn tint_of(kind: ToastKind) -> TextTint {
    match kind {
        ToastKind::Info => TextTint::Normal,
        ToastKind::Warn => TextTint::Warn,
        ToastKind::Error => TextTint::Error,
    }
}

#[cfg(test)]
mod tests {
    use mineral_protocol::{Event, FailureNotice, PropName, PropValue, TextSpan, ToastKind};

    use super::apply_event;
    use crate::components::toast::notifications::Notifications;

    /// 以默认旋钮构造通知管理器(对照 default.lua:flash_ttl_secs=4 / 6 拍动画)。
    fn notifications() -> Notifications {
        Notifications::new(/*flash_ttl_secs*/ 4, /*toast_anim_ticks*/ 6)
    }

    /// 一条带 id 的 Toast 事件。
    fn toast(content: &str, id: Option<&str>) -> Event {
        Event::Toast {
            kind: ToastKind::Info,
            content: vec![TextSpan::plain(content)],
            id: id.map(str::to_owned),
            ttl_secs: None,
        }
    }

    /// Card 事件进卡片区:同 id 顶替、无 id 堆叠。
    #[test]
    fn card_event_reaches_card_area_with_id_semantics() {
        let card = |id: Option<&str>| Event::Card {
            kind: ToastKind::Warn,
            id: id.map(str::to_owned),
            title: vec![TextSpan::plain("要点")],
            body: vec![vec![TextSpan::plain("第一行")]],
            ttl_secs: None,
        };
        let mut n = notifications();
        apply_event(&mut n, card(Some("release")));
        apply_event(&mut n, card(Some("release")));
        assert_eq!(n.card_count(), 1, "同 id 应顶替");
        apply_event(&mut n, card(/*id*/ None));
        assert_eq!(n.card_count(), 2, "无 id 应堆叠");
        assert_eq!(n.entry_count(), 0, "卡片不该混进单行区");
    }

    /// Toast 的 id 语义:同 id 顶替为一条、无 id 堆叠、不同 id 各自一条。
    #[test]
    fn toast_id_replaces_anonymous_stacks() {
        let mut n = notifications();
        apply_event(&mut n, toast("音量 31", Some("vol")));
        apply_event(&mut n, toast("音量 32", Some("vol")));
        assert_eq!(n.entry_count(), 1, "同 id 应顶替");

        apply_event(&mut n, toast("一次性", None));
        apply_event(&mut n, toast("一次性", None));
        assert_eq!(n.entry_count(), 3, "无 id 应堆叠");

        apply_event(&mut n, toast("shuffle", Some("mode")));
        assert_eq!(n.entry_count(), 4, "不同 id 各自一条");
    }

    /// 同一脚本失败会顶替旧提示；配置失败驻留到对应撤卡事件。
    #[test]
    fn failure_events_keep_notice_lifecycles() {
        let mut n = notifications();
        for callback in ["track_started", "track_finished"] {
            apply_event(
                &mut n,
                Event::Failure(FailureNotice::ScriptCallbackFailed {
                    callback: callback.to_owned(),
                }),
            );
        }
        assert_eq!(n.entry_count(), 1, "回调错误使用相同顶替键");
        apply_event(
            &mut n,
            Event::Failure(FailureNotice::ConfigRejected {
                fields: vec!["tui.behavior.volume_step".to_owned()],
            }),
        );
        assert!(n.has_live_card("config.reload"));
        apply_event(
            &mut n,
            Event::DismissToast {
                id: "config.reload".to_owned(),
            },
        );
        assert!(!n.has_live_card("config.reload"));
    }

    /// 未订阅类别(PropertyChanged 等)被安全忽略,不进通知层。
    #[test]
    fn non_toast_events_are_ignored() {
        let mut n = notifications();
        apply_event(
            &mut n,
            Event::PropertyChanged {
                prop: PropName::PLAYER_VOLUME,
                value: PropValue::Int(42),
            },
        );
        assert_eq!(n.entry_count(), 0, "非 Toast 推送不该产生通知");
    }
}
