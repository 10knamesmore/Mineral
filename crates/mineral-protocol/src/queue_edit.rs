//! 队列结构编辑的操作描述与回执。
//!
//! 编辑是「对已在队列里的条目做删除与重排」,不引入新歌——加歌走各自的插入 / 追加请求。

use mineral_model::SongId;
use serde::{Deserialize, Serialize};

/// 队列中一个条目的定位:下标 + 身份双保险。
///
/// 多 client 并发下,client 手里的下标可能已被别人的编辑顶掉。只带下标会让操作静默作用到
/// **另一首歌**上;附带 `song_id` 让 server 能校验出这种错位并拒绝(见
/// [`QueueEditOutcome::Stale`]),而不是执行一次用户没打算做的删除。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueAnchor {
    /// 条目在队列中的下标(client 看到的位置)。
    pub index: usize,

    /// 该下标上应当是哪首歌;对不上即判定 client 视图已过期。
    pub song_id: SongId,
}

impl QueueAnchor {
    /// 构造一个定位。
    ///
    /// # Params:
    ///   - `index`: 条目下标
    ///   - `song_id`: 该下标上的歌 id
    ///
    /// # Return:
    ///   定位。
    pub fn new(index: usize, song_id: SongId) -> Self {
        Self { index, song_id }
    }
}

/// 重排的目标位置。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueuePos {
    /// 上移一格。已在首位时不环绕(见 [`QueueEditOutcome::NoOp`])。
    Up,

    /// 下移一格。已在末位时不环绕。
    Down,

    /// 移到队首。
    Top,

    /// 移到队尾。
    Bottom,

    /// 移到当前曲之后——队列内重排,**不复制**。
    AfterCurrent,
}

/// 一次队列结构编辑。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueOp {
    /// 删除某条目。删的若是在播曲,声音不打断,播完再接下一首。
    Remove(QueueAnchor),

    /// 把某条目移到新位置。
    Move {
        /// 待移动的条目。
        at: QueueAnchor,

        /// 目标位置。
        to: QueuePos,
    },

    /// 清空锚点**之上**的全部条目,锚点自身保留。
    ClearAbove(QueueAnchor),

    /// 清空锚点**之下**的全部条目,锚点自身保留。
    ClearBelow(QueueAnchor),

    /// 应用一个脚本注册的具名变换(下标对应配置里 `queue.transforms` 的声明顺序)。
    ApplyTransform {
        /// 变换在配置数组中的下标。
        index: usize,

        /// 发起时的光标位置(0-based)。变换函数要能表达「以我看着的这一行为界」这类
        /// 操作,而光标只有 client 知道,server 无从推断,故随请求带来。
        selected: Option<usize>,
    },

    /// 撤销上一次编辑。单级,撤销后快照即清空。
    Undo,
}

/// 一次队列编辑的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueEditOutcome {
    /// 队列已变更,随后的队列同步会带上新内容。
    Applied,

    /// 锚点处的歌与 client 所报不符,未执行。
    ///
    /// client 收到后应重画并提示,**不要自动重试**——重试会在用户看不见的情况下作用到
    /// 另一首歌上。
    Stale,

    /// 执行了但队列没有变化(如上移已在首位的条目、变换返回原序)。
    NoOp,
}
