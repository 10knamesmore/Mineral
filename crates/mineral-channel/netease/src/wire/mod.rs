//! 网易云 API 的线上协议结构（serde 反序列化目标）。
//!
//! 这一层是「接收网易云原生 JSON 的形态」——字段名、类型按对方协议给，
//! 与上层 `mineral-model` 的规范化领域类型分离。各 `api/*.rs` 拿到 `serde_json::Value`
//! 后，先 `serde_json::from_value::<wire::T>(...)` 反序列化，再 `into()` 成 `model::T`。

#![allow(dead_code)]

pub mod common;
pub mod search;
pub mod song;
pub mod user;
