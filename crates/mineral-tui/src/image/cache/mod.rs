//! 图片引擎的解码图与终端成品缓存。

mod decoded;
mod terminal;

pub(crate) use decoded::CoverCache;
pub(crate) use terminal::TerminalImageCache;
