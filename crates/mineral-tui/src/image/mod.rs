//! 图片管线：异步抓取与解码、低清 preview、取色、终端图片编码与缓存、歌单拼贴、
//! Kitty shared memory 与统一渲染。终端能力和协议实现封装在本模块内，消费方只面向
//! [`ImageEngine`]、[`ImageContent`] 与 [`ImageRenderPhase`]。

pub(crate) mod collage;
pub(crate) mod colors;
mod encode;
pub(crate) mod fetch;
mod geometry;
pub(crate) mod graphics;

mod accent;
mod cache;
mod hub;
mod key;
mod kitty;
mod render;
mod resize;
mod terminal;

pub(crate) use geometry::square_cells;
#[cfg(test)]
pub(crate) use graphics::GraphicsProtocol;
#[cfg(test)]
pub use hub::CoverTransition;
pub use hub::ImageEngine;
pub(crate) use render::{BlendStyle, ImageContent, ImageRenderPhase};
