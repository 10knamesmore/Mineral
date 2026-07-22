//! 平铺基本层:占满主屏的各面板,按渲染归属分档——browse 浏览态专属面、search 搜索态专属
//! 面、两态共用 / 通用组件、跨两态的 page morph 封面飞行层。布局几何与整屏几何变换归共用档。

pub mod browse;
pub mod flight;
pub mod search;
pub mod shared;
