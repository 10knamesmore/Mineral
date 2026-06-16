//! 全屏布局层的统一契约 [`Page`]:页自管 view 状态、吃按键、把副作用作为意图返回。

use crossterm::event::KeyEvent;

/// 一个全屏布局层(当前 Search / Browse 两页,由 [`AppState::page_kind`](crate::runtime::state::AppState::page_kind)
/// 互斥选出)。浮层栈叠在页之上,正交,不走这个契约。
///
/// 契约:实现者**自管 view 状态**(光标 / 焦点 / 动画),只读地借入跨页上下文 [`Page::Ctx`],
/// 把"想让 App 做的副作用"作为 [`Page::Effect`] 意图**返回**——不反手持有 `App` / `client`,
/// 故可脱离 App 单测(喂 [`KeyEvent`]、断言返回的意图)。
pub(crate) trait Page {
    /// 吃完按键后吐给 App 的副作用意图集(各页自带枚举,App 侧各有 `apply`,不塞公共 god-enum)。
    type Effect;

    /// 决策所需的只读跨页上下文(props):各页所需不同(Search 要 `caps`、Browse 要 `library`),
    /// 故关联化而非强塞统一 struct——避免最小公倍数把不相关字段摊给每页。
    type Ctx<'a>;

    /// 吃一次按键:改自身 view 状态,返回副作用意图。
    ///
    /// # Params:
    ///   - `key`: 本次按键事件
    ///   - `ctx`: 只读跨页上下文(由 App 在调用点就地构造)
    ///
    /// # Return:
    ///   要 App 落地的副作用意图。
    fn on_key(&mut self, key: &KeyEvent, ctx: Self::Ctx<'_>) -> Self::Effect;
}
