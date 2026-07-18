//! 封面管线:原图取色出色板(colors)、协议编码缓存(encode)、异步抓取(fetch)、
//! 图床服务端缩放改写(cdn_scale)、聚合歌单拼贴合成(collage)、
//! kitty 图数据流式传输(kitty_transmit)。

pub mod cdn_scale;
pub mod collage;
pub mod colors;
pub mod encode;
pub mod fetch;
pub mod kitty_transmit;
