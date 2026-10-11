//! 将歌单位置升级为区分类型的曲库位置。

mod entity;
mod migration;

pub(crate) use migration::Migration;
