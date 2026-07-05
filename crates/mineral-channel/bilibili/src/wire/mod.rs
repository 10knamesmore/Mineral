//! 哔哩哔哩 API 的线上协议结构(serde 反序列化目标)。
//!
//! 这一层是「接收 B站原生 JSON 的形态」——字段名、类型按对方协议给,与上层
//! `mineral-model` 的规范化领域类型分离。各端点拿到 `serde_json::Value` 后先经
//! [`de::from_value`] 反序列化成这里的 DTO,再交给 `convert` 映射成 model 类型。

#![allow(dead_code)]

pub mod de;
pub mod fav;
pub mod nav;
pub mod playurl;
pub mod search;
pub mod view;
