//! 封面管线的 client 端状态:原图缓存(字节预算 LRU)、色板、在飞集合、已编码协议。

mod cache;
mod hub;
mod protocols;

pub use hub::CoverHub;
