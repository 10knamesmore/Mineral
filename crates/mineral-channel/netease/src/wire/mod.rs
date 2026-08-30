//! 网易云 API 的线上协议结构（serde 反序列化目标）。
//!
//! 这一层是「接收网易云原生 JSON 的形态」——字段名、类型按对方协议给，
//! 与上层 `mineral-model` 的规范化领域类型分离。各 API 模块通过 [`de::from_value`]
//! 反序列化并保留字段路径错误,再由 `convert` 模块显式映射到 model 类型。

#![allow(dead_code)]

pub mod artist;
pub mod common;
pub mod de;
pub mod playlist;
pub mod search;
pub mod song;
pub mod user;
