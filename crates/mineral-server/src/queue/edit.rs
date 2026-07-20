//! 队列结构编辑:删除 / 重排 / 批量清理 / 整表重排 / 撤销。

use mineral_model::SongId;
use mineral_protocol::{PlayCursor, QueueAnchor, QueueEditOutcome, QueueOp, QueuePos};

use crate::state::{QueueSnapshot, State};

/// 执行一次队列编辑。
///
/// [`QueueOp::ApplyTransform`] **不**在此处理:它要跨线程跑脚本,由调用方拿到新序后走
/// [`apply_order`]。
///
/// # Params:
///   - `st`: 播放状态(改 queue / cursor / original_queue / queue_undo)
///   - `op`: 待执行的操作
///
/// # Return:
///   本次编辑的结果。
pub(crate) fn apply(st: &mut State, op: &QueueOp) -> QueueEditOutcome {
    if matches!(op, QueueOp::Undo) {
        return undo(st);
    }
    // 每个操作都表达成「保留哪些原下标、以什么顺序」。游标跟随由此自动落地:重排后当前曲
    // 的新位置 = 它的原下标在这个序列里的位置,不必逐 op 手算。
    let Some(order) = plan(st, op) else {
        return QueueEditOutcome::Stale;
    };
    commit(st, &order)
}

/// 把一个操作翻译成保留下来的原下标序列。定位过期返回 `None`。
fn plan(st: &State, op: &QueueOp) -> Option<Vec<usize>> {
    let len = st.queue.len();
    let identity = || (0..len).collect::<Vec<_>>();
    match op {
        QueueOp::Remove(at) => {
            let target = resolve(st, at)?;
            Some((0..len).filter(|&i| i != target).collect())
        }
        QueueOp::Move { at, to } => {
            let target = resolve(st, at)?;
            let mut order = identity();
            let dest = destination(st, target, *to)?;
            order.remove(target);
            order.insert(dest.min(order.len()), target);
            Some(order)
        }
        QueueOp::ClearAbove(at) => {
            let target = resolve(st, at)?;
            Some((target..len).collect())
        }
        QueueOp::ClearBelow(at) => {
            let target = resolve(st, at)?;
            Some((0..=target).collect())
        }
        // 变换要跨线程跑脚本,新序由调用方经 apply_order 落地。
        QueueOp::ApplyTransform { .. } | QueueOp::Undo => Some(identity()),
    }
}

/// 校验定位并取出下标:下标处不是所报的那首歌即判定 client 视图已过期。
fn resolve(st: &State, at: &QueueAnchor) -> Option<usize> {
    st.queue
        .get(at.index)
        .filter(|s| s.id == at.song_id)
        .map(|_| at.index)
}

/// 重排的目标下标。端点不环绕:首项上移 / 末项下移返回原位(commit 据此判 NoOp)。
fn destination(st: &State, target: usize, to: QueuePos) -> Option<usize> {
    let last = st.queue.len().checked_sub(1)?;
    Some(match to {
        QueuePos::Up => target.saturating_sub(1),
        QueuePos::Down => (target + 1).min(last),
        QueuePos::Top => 0,
        QueuePos::Bottom => last,
        QueuePos::AfterCurrent => super::nav::after_current_pos(st),
    })
}

/// 落地一个原下标序列:重建队列、跟随游标、同步原序、记撤销快照、推版本。
fn commit(st: &mut State, order: &[usize]) -> QueueEditOutcome {
    if order.len() == st.queue.len() && order.iter().enumerate().all(|(at, &i)| at == i) {
        return QueueEditOutcome::NoOp;
    }
    let before = snapshot(st);
    let rebuilt = order
        .iter()
        .filter_map(|&i| st.queue.get(i).cloned())
        .collect::<Vec<_>>();
    st.cursor = follow_cursor(st.cursor, order, rebuilt.len());
    st.queue = rebuilt;
    sync_original(st);
    st.queue_undo = Some(before);
    st.bump_queue();
    QueueEditOutcome::Applied
}

/// 重排后游标的落点。
///
/// 当前曲若仍在保留序列里就跟着它走;被删掉则转悬空,接续点取「原下标之后第一个仍留存的
/// 条目」的新位置——没有这样的条目就落新队列长度,即播完即停。
fn follow_cursor(cursor: PlayCursor, order: &[usize], new_len: usize) -> PlayCursor {
    let survivor_after = |from: usize| {
        order
            .iter()
            .enumerate()
            .filter(|&(_, &original)| original >= from)
            .min_by_key(|&(_, &original)| original)
            .map_or(new_len, |(at, _)| at)
    };
    match cursor {
        PlayCursor::InQueue(cur) => order.iter().position(|&i| i == cur).map_or(
            PlayCursor::Detached {
                resume_at: survivor_after(cur + 1),
            },
            PlayCursor::InQueue,
        ),
        PlayCursor::Detached { resume_at } => PlayCursor::Detached {
            resume_at: survivor_after(resume_at),
        },
    }
}

/// 同步 shuffle 原序:摘掉新队列里已彻底不存在的歌。
///
/// 只同步删除,**不**同步重排与复制——原序快照的职责只是「退出 shuffle 时还原原始顺序」,
/// 它需要保证不复活已删的歌;把洗牌视图内的手动重排回写进去,反倒让这份快照失去意义。
fn sync_original(st: &mut State) {
    let Some(orig) = st.original_queue.as_mut() else {
        return;
    };
    let live = st
        .queue
        .iter()
        .map(|s| s.id.clone())
        .collect::<rustc_hash::FxHashSet<_>>();
    orig.retain(|s| live.contains(&s.id));
}

/// 拍一张编辑前的队列全貌。
fn snapshot(st: &State) -> QueueSnapshot {
    QueueSnapshot {
        queue: st.queue.clone(),
        original_queue: st.original_queue.clone(),
        cursor: st.cursor,
    }
}

/// 撤销上一次编辑;无快照可用时 no-op。
fn undo(st: &mut State) -> QueueEditOutcome {
    let Some(before) = st.queue_undo.take() else {
        return QueueEditOutcome::NoOp;
    };
    st.queue = before.queue;
    st.original_queue = before.original_queue;
    st.cursor = before.cursor;
    st.bump_queue();
    QueueEditOutcome::Applied
}

/// 按给定的 id 序列整表重排队列(脚本变换 / `mineral.queue.set` 的落地口)。
///
/// # Params:
///   - `st`: 播放状态
///   - `ids`: 新的队列顺序;每个 id 必须在原队列里出现过,次数不限
///
/// # Return:
///   本次编辑的结果;含未知 id 时整体拒绝,返回 [`QueueEditOutcome::Stale`]。
pub(crate) fn apply_order(st: &mut State, ids: &[SongId]) -> QueueEditOutcome {
    // 只认 id、从原队列取回权威实体:脚本手里的 song 表是有损投影(艺人 / 专辑只有名字),
    // 让它直接构造 Song 会造出半残实体。同一 id 出现多次即复制该条目,零额外成本。
    let mut order = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(at) = st.queue.iter().position(|s| s.id == *id) else {
            mineral_log::warn!(
                target: "player",
                id = id.qualified(),
                "queue transform returned an unknown song, rejecting the whole order"
            );
            return QueueEditOutcome::Stale;
        };
        order.push(at);
    }
    commit(st, &order)
}

#[cfg(test)]
mod tests {
    use mineral_protocol::{
        PlayCursor, PlayMode, QueueAnchor, QueueEditOutcome, QueueOp, QueuePos,
    };
    use mineral_test::song;
    use pretty_assertions::assert_eq;

    use super::{apply, apply_order};
    use crate::state::State;

    /// 造队列 a/b/c/d,当前曲附着在 `sel`。
    fn state(sel: usize) -> State {
        let mut st = State::empty();
        st.queue = vec![song("a"), song("b"), song("c"), song("d")];
        st.cursor = PlayCursor::InQueue(sel);
        st.current_song = st.queue.get(sel).cloned();
        st
    }

    /// 取队列各歌 id(原序)。
    fn ids(st: &State) -> Vec<&str> {
        st.queue.iter().map(|s| s.id.as_str()).collect()
    }

    /// 造一个指向 `index` 处那首歌的定位。
    fn anchor(st: &State, index: usize) -> QueueAnchor {
        let id = st
            .queue
            .get(index)
            .map(|s| s.id.clone())
            .unwrap_or_else(|| song("missing").id);
        QueueAnchor::new(index, id)
    }

    /// 删除非在播条目:该行消失,游标仍附着在原来那首歌上(下标随之前移)。
    #[test]
    fn remove_non_current_keeps_cursor_on_same_song() {
        let mut st = state(2); // 当前 c
        let at = anchor(&st, 0); // 删 a
        assert_eq!(
            apply(&mut st, &QueueOp::Remove(at)),
            QueueEditOutcome::Applied
        );
        assert_eq!(ids(&st), vec!["b", "c", "d"]);
        assert_eq!(st.cursor, PlayCursor::InQueue(1), "c 前移到下标 1");
    }

    /// 删在播曲:游标转为悬空,接续点指向原来的下一首。声音由音频引擎自然播完。
    #[test]
    fn remove_current_detaches_cursor_to_successor() {
        let mut st = state(1); // 当前 b
        let at = anchor(&st, 1);
        assert_eq!(
            apply(&mut st, &QueueOp::Remove(at)),
            QueueEditOutcome::Applied
        );
        assert_eq!(ids(&st), vec!["a", "c", "d"]);
        assert_eq!(
            st.cursor,
            PlayCursor::Detached { resume_at: 1 },
            "接续点应指向原下一首 c(新下标 1)"
        );
    }

    /// 删在播的末位曲:无后继,接续点落队列长度 = 播完即停。
    #[test]
    fn remove_current_at_tail_stops_after_finish() {
        let mut st = state(3); // 当前 d(末位)
        let at = anchor(&st, 3);
        assert_eq!(
            apply(&mut st, &QueueOp::Remove(at)),
            QueueEditOutcome::Applied
        );
        assert_eq!(ids(&st), vec!["a", "b", "c"]);
        assert_eq!(st.cursor, PlayCursor::Detached { resume_at: 3 });
    }

    /// 定位过期(下标处不是所报的那首歌)→ 拒绝执行,队列一字不动。
    #[test]
    fn stale_anchor_is_rejected() {
        let mut st = state(0);
        let bogus = QueueAnchor::new(1, song("zzz").id);
        assert_eq!(
            apply(&mut st, &QueueOp::Remove(bogus)),
            QueueEditOutcome::Stale
        );
        assert_eq!(ids(&st), vec!["a", "b", "c", "d"], "拒绝后队列不变");
    }

    /// 上移 / 下移一格,游标跟随歌曲而非停在原下标。
    #[test]
    fn move_up_and_down_follows_the_song() {
        let mut st = state(2); // 当前 c
        let at = anchor(&st, 2);
        assert_eq!(
            apply(
                &mut st,
                &QueueOp::Move {
                    at,
                    to: QueuePos::Up
                }
            ),
            QueueEditOutcome::Applied
        );
        assert_eq!(ids(&st), vec!["a", "c", "b", "d"]);
        assert_eq!(st.cursor, PlayCursor::InQueue(1), "游标跟着 c 上移");

        let at = anchor(&st, 1);
        assert_eq!(
            apply(
                &mut st,
                &QueueOp::Move {
                    at,
                    to: QueuePos::Down
                }
            ),
            QueueEditOutcome::Applied
        );
        assert_eq!(ids(&st), vec!["a", "b", "c", "d"]);
        assert_eq!(st.cursor, PlayCursor::InQueue(2));
    }

    /// 端点不环绕:首项上移 / 末项下移都是 NoOp。
    #[test]
    fn move_at_edges_does_not_wrap() {
        let mut st = state(0);
        let first = anchor(&st, 0);
        assert_eq!(
            apply(
                &mut st,
                &QueueOp::Move {
                    at: first,
                    to: QueuePos::Up
                }
            ),
            QueueEditOutcome::NoOp
        );
        let last = anchor(&st, 3);
        assert_eq!(
            apply(
                &mut st,
                &QueueOp::Move {
                    at: last,
                    to: QueuePos::Down
                }
            ),
            QueueEditOutcome::NoOp
        );
        assert_eq!(ids(&st), vec!["a", "b", "c", "d"]);
    }

    /// 移到队首 / 队尾。
    #[test]
    fn move_to_top_and_bottom() {
        let mut st = state(0);
        let at = anchor(&st, 2); // c
        assert_eq!(
            apply(
                &mut st,
                &QueueOp::Move {
                    at,
                    to: QueuePos::Top
                }
            ),
            QueueEditOutcome::Applied
        );
        assert_eq!(ids(&st), vec!["c", "a", "b", "d"]);
        assert_eq!(st.cursor, PlayCursor::InQueue(1), "当前曲 a 被 c 挤后一位");

        let at = anchor(&st, 0); // c
        assert_eq!(
            apply(
                &mut st,
                &QueueOp::Move {
                    at,
                    to: QueuePos::Bottom
                }
            ),
            QueueEditOutcome::Applied
        );
        assert_eq!(ids(&st), vec!["a", "b", "d", "c"]);
    }

    /// 移到当前曲之后 = 队列内重排,**不复制**(队列长度不变)。
    #[test]
    fn move_after_current_relocates_without_copying() {
        let mut st = state(0); // 当前 a
        let at = anchor(&st, 3); // d
        assert_eq!(
            apply(
                &mut st,
                &QueueOp::Move {
                    at,
                    to: QueuePos::AfterCurrent
                }
            ),
            QueueEditOutcome::Applied
        );
        assert_eq!(ids(&st), vec!["a", "d", "b", "c"], "长度不变,只是换位");
        assert_eq!(st.cursor, PlayCursor::InQueue(0));
    }

    /// 清空锚点之上:锚点自身保留。
    #[test]
    fn clear_above_keeps_the_anchor() {
        let mut st = state(0); // 当前 a,会被清掉
        let at = anchor(&st, 2); // c
        assert_eq!(
            apply(&mut st, &QueueOp::ClearAbove(at)),
            QueueEditOutcome::Applied
        );
        assert_eq!(ids(&st), vec!["c", "d"]);
        assert_eq!(
            st.cursor,
            PlayCursor::Detached { resume_at: 0 },
            "在播的 a 被清掉 → 悬空,播完接 c"
        );
    }

    /// 清空锚点之下:锚点自身保留。
    #[test]
    fn clear_below_keeps_the_anchor() {
        let mut st = state(0); // 当前 a,保留
        let at = anchor(&st, 1); // b
        assert_eq!(
            apply(&mut st, &QueueOp::ClearBelow(at)),
            QueueEditOutcome::Applied
        );
        assert_eq!(ids(&st), vec!["a", "b"]);
        assert_eq!(st.cursor, PlayCursor::InQueue(0));
    }

    /// 清空首项之上 = 无可清 → NoOp。
    #[test]
    fn clear_above_first_is_noop() {
        let mut st = state(0);
        let at = anchor(&st, 0);
        assert_eq!(
            apply(&mut st, &QueueOp::ClearAbove(at)),
            QueueEditOutcome::NoOp
        );
        assert_eq!(ids(&st), vec!["a", "b", "c", "d"]);
    }

    /// 撤销把队列与游标一并还原;单级,撤销后再撤销无快照可用。
    #[test]
    fn undo_restores_queue_and_cursor() {
        let mut st = state(1); // 当前 b
        let at = anchor(&st, 1);
        assert_eq!(
            apply(&mut st, &QueueOp::Remove(at)),
            QueueEditOutcome::Applied
        );
        assert_eq!(st.cursor, PlayCursor::Detached { resume_at: 1 });

        assert_eq!(apply(&mut st, &QueueOp::Undo), QueueEditOutcome::Applied);
        assert_eq!(ids(&st), vec!["a", "b", "c", "d"]);
        assert_eq!(st.cursor, PlayCursor::InQueue(1), "游标从悬空变回附着");

        assert_eq!(
            apply(&mut st, &QueueOp::Undo),
            QueueEditOutcome::NoOp,
            "单级撤销:没有第二份快照"
        );
    }

    /// 被拒绝的编辑不留快照,不会让下一次撤销退到更早的状态。
    #[test]
    fn rejected_edit_leaves_no_undo_snapshot() {
        let mut st = state(0);
        let bogus = QueueAnchor::new(1, song("zzz").id);
        assert_eq!(
            apply(&mut st, &QueueOp::Remove(bogus)),
            QueueEditOutcome::Stale
        );
        assert_eq!(apply(&mut st, &QueueOp::Undo), QueueEditOutcome::NoOp);
    }

    /// shuffle 下删除条目必须同步 original_queue,否则退出 shuffle 时被删的歌会复活。
    #[test]
    fn removal_syncs_original_queue() {
        let mut st = state(0);
        st.play_mode = PlayMode::Shuffle;
        st.queue = vec![song("c"), song("a"), song("d"), song("b")];
        st.cursor = PlayCursor::InQueue(0);
        st.current_song = Some(song("c"));
        st.original_queue = Some(vec![song("a"), song("b"), song("c"), song("d")]);

        let at = anchor(&st, 3); // 洗牌视图里的 b
        assert_eq!(
            apply(&mut st, &QueueOp::Remove(at)),
            QueueEditOutcome::Applied
        );
        assert_eq!(
            st.original_queue
                .as_ref()
                .map(|q| q.iter().map(|s| s.id.as_str()).collect::<Vec<_>>()),
            Some(vec!["a", "c", "d"]),
            "b 应一并从原序里摘掉"
        );
    }

    /// shuffle 下的重排**不**回写 original_queue——原序快照的职责只是还原原始顺序。
    #[test]
    fn reorder_does_not_pollute_original_queue() {
        let mut st = state(0);
        st.play_mode = PlayMode::Shuffle;
        st.queue = vec![song("c"), song("a"), song("d"), song("b")];
        st.cursor = PlayCursor::InQueue(0);
        st.current_song = Some(song("c"));
        st.original_queue = Some(vec![song("a"), song("b"), song("c"), song("d")]);

        let at = anchor(&st, 3);
        assert_eq!(
            apply(
                &mut st,
                &QueueOp::Move {
                    at,
                    to: QueuePos::Top
                }
            ),
            QueueEditOutcome::Applied
        );
        assert_eq!(
            st.original_queue
                .as_ref()
                .map(|q| q.iter().map(|s| s.id.as_str()).collect::<Vec<_>>()),
            Some(vec!["a", "b", "c", "d"]),
            "原序不受洗牌视图内重排影响"
        );
    }

    /// 整表重排:接受原队列的任意重排与删减。
    #[test]
    fn apply_order_accepts_permutation_and_subset() {
        let mut st = state(1); // 当前 b
        let order = vec![song("d").id, song("b").id];
        assert_eq!(apply_order(&mut st, &order), QueueEditOutcome::Applied);
        assert_eq!(ids(&st), vec!["d", "b"]);
        assert_eq!(st.cursor, PlayCursor::InQueue(1), "当前曲跟到新位置");
    }

    /// 整表重排:允许同一首歌出现多于原有次数(复制已在队列的歌零成本)。
    #[test]
    fn apply_order_allows_duplicates_beyond_original_count() {
        let mut st = state(0);
        let order = vec![song("a").id, song("a").id, song("a").id];
        assert_eq!(apply_order(&mut st, &order), QueueEditOutcome::Applied);
        assert_eq!(ids(&st), vec!["a", "a", "a"]);
    }

    /// 整表重排:混入原队列里没有的 id → 整体拒绝,队列不变。
    #[test]
    fn apply_order_rejects_unknown_id() {
        let mut st = state(0);
        let order = vec![song("a").id, song("zzz").id];
        assert_eq!(apply_order(&mut st, &order), QueueEditOutcome::Stale);
        assert_eq!(ids(&st), vec!["a", "b", "c", "d"]);
    }

    /// 整表重排返回原序 → NoOp(不白白 bump 版本让 client 重拉整队列)。
    #[test]
    fn apply_order_identity_is_noop() {
        let mut st = state(0);
        let order: Vec<_> = st.queue.iter().map(|s| s.id.clone()).collect();
        assert_eq!(apply_order(&mut st, &order), QueueEditOutcome::NoOp);
    }
}
