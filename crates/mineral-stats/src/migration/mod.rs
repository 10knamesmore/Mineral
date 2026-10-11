//! 数据库结构的版本化迁移。

mod m20260906_stats;
mod m20261007_saved_albums;
mod registry;

pub(crate) use registry::Migrator;

#[cfg(test)]
pub(crate) mod test_support;
