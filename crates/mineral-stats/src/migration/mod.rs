//! 数据库结构的版本化迁移。

mod m20260906_stats;
mod registry;
mod stats_schema;

pub(crate) use registry::Migrator;

#[cfg(test)]
pub(crate) mod test_support;
