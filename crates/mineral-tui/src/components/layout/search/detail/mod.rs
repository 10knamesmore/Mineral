//! Search detail 面板：实体详情栈顶帧的页头式渲染——左正方头图 + 右元数据，其下曲目/
//! 专辑列表；artist 帧多一行热门曲/专辑双区 Tab。数据未到画占位骨架。
//!
//! 稳态直接画在主帧；下钻/返回滑动期，出发帧与目标帧各渲染到离屏 Buffer，图片统一
//! 使用 halfblock，再按 sweep 进度逐列合成。

mod body;
mod description;
mod frame;
mod geometry;
mod meta;
mod panel;
mod placeholder;
mod sweep;
mod title;
mod track_table;
mod transition;

pub(crate) use geometry::{detail_list_area, header_cover_area};
pub use panel::draw;
pub(super) use track_table::highlight_style;
