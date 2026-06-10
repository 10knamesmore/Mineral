//! server 主动推送([`Event`])到通知层的翻译:Toast 进单行 flash、Card 进
//! 多行卡片(都带 id 顶替 / 无 id 堆叠),其余类别 TUI 未订阅、安全忽略。

use mineral_protocol::{Event, ToastKind};

use crate::components::toast::notifications::{Notifications, TextTint, tinted_spans_item};

/// 消费一条 server 推送:
///   - Toast 按 kind 上色进单行 flash(`id: Some` 顶替 / `None` 堆叠;
///     `ttl_secs: Some` 覆盖默认展示时长);
///   - Card 进多行卡片(同款 id 语义;`ttl_secs: Some` 到时自动退场,
///     `None` 驻留到用户显式关闭);
///   - 其余类别 TUI 未订阅,收到(订阅集将来变化)也安全忽略——轮询仍是权威值来源。
///
/// # Params:
///   - `notifications`: 通知层
///   - `event`: server 推送的事件
pub(crate) fn apply_event(notifications: &mut Notifications, event: Event) {
    match event {
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
        // ScriptReloaded / UiOverride 在 App::drain_push_events 已分流
        // (前者刷新 bind 键、后者落旋钮覆盖),这里只是穷尽兜底。
        // BusMessage 是用户自定义消息,内置 TUI 不订阅也不解释
        // (语义契约在用户脚本与其外部工具之间)。
        Event::PropertyChanged { .. }
        | Event::TrackFinished { .. }
        | Event::DownloadCompleted { .. }
        | Event::StoreChanged { .. }
        | Event::ScriptReloaded
        | Event::BusMessage { .. }
        | Event::UiOverride { .. } => {}
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
    use mineral_protocol::{Event, PropName, PropValue, TextSpan, ToastKind};

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
