//! m20260906_stats 的固定建表结构与迁移实现。

mod entity;
mod indexes;
mod migration;
mod tables;

pub(super) use migration::Migration;
