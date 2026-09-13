//! 搜索页面的输入处理与副作用意图。

mod effects;
mod panels;
mod prompt;

#[cfg(test)]
mod artist_albums_tests;
#[cfg(test)]
mod pagination_tests;

pub(crate) use effects::{SearchCtx, SearchEffect};
