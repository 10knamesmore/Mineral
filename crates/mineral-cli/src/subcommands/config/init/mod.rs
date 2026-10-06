//! 协调两个宿主写出用户配置与完整 LuaLS 元数据。

mod assets;

pub(super) use assets::run_init;

#[cfg(test)]
mod tests;
