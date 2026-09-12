//! 检查类型定义中相邻成员之间的空行。

mod rule;
#[cfg(test)]
mod tests;

pub(crate) use rule::{MINERAL_MEMBER_SPACING, MemberSpacing};
