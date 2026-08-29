//! 图片引擎的完整解码图、低清 preview 与终端成品缓存。

mod decoded;
mod terminal;

pub(crate) use decoded::CoverCache;
pub(crate) use terminal::TerminalImageCache;
