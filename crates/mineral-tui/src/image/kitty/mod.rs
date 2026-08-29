//! Kitty graphics protocol 的 shared memory、命令、探测、成品与 placement。

mod command;
mod image;
mod placement;
mod probe;
mod shared_memory;

pub(crate) use image::KittyImage;
pub(crate) use probe::probe_shared_memory;
