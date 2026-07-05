//! WBI 请求签名。
//!
//! wbi 端点要求 query 带 `wts`(unix 秒)+ `w_rid`(签名)。`w_rid` 由 `img_key`/`sub_key`
//! (从 `nav` 端点拉取的两个文件名)重排出 `mixin_key` 后,对字典序排好的 query 取 md5 得到。

pub mod wbi;
