//! 组件依赖比较、保留画布和区域提交；更新与图片保活不依赖绘制次数。

mod component_id;
mod compositor;
mod damage;
mod plan;
mod surface;
#[cfg(test)]
mod tests;

pub(crate) use compositor::Compositor;
pub(crate) use plan::{Dependencies, FramePlan, PaintCache, Placement};
