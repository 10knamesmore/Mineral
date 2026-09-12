//! 脚本绑定、具名动作、复制模板与逐曲持久值。

use mineral_model::SongId;
use mineral_protocol::{FailureKind, OperationResult, ScriptBind};

use super::Client;
use crate::operation::{Outcome, Pending, SubmitError, decode_applied, decode_query};

impl Client {
    /// 拉取脚本绑定表。
    pub async fn script_binds(&self) -> Outcome<Vec<ScriptBind>> {
        self.request(
            mineral_protocol::Request::ScriptBinds,
            |result, request_name| {
                decode_query(result, request_name, |response| match response {
                    mineral_protocol::Response::ScriptBinds(binds) => Some(binds),
                    _ => None,
                })
            },
        )
        .await
    }

    /// 触发脚本具名动作(等待结论)。
    ///
    /// # Params:
    ///   - `name`: 动作名
    ///   - `ctx`: 按键上下文
    ///   - `args`: 位置实参(CLI)
    pub async fn invoke_action(
        &self,
        name: &str,
        ctx: Option<mineral_protocol::KeyContext>,
        args: Vec<String>,
    ) -> Outcome<()> {
        match self.invoke_action_pending(name, ctx, args) {
            Ok(pending) => pending.outcome().await,
            Err(error) => Outcome::Unknown {
                detail: format!("未提交:{error}"),
            },
        }
    }

    /// 触发脚本具名动作(不等待结论,结果句柄交给调用方 / 完成事件队列)。
    ///
    /// # Params:
    ///   - `name`: 动作名
    ///   - `ctx`: 按键上下文
    ///   - `args`: 位置实参
    ///
    /// # Errors
    /// 本地未提交。
    pub fn invoke_action_pending(
        &self,
        name: &str,
        ctx: Option<mineral_protocol::KeyContext>,
        args: Vec<String>,
    ) -> Result<Pending<()>, SubmitError> {
        self.submit(
            mineral_protocol::Request::InvokeAction {
                name: name.to_owned(),
                ctx,
                args,
            },
            decode_applied,
        )
    }

    /// 渲染复制模板。
    ///
    /// # Params:
    ///   - `index`: 模板下标
    ///   - `ctx`: 模板实体
    pub async fn render_copy_template(
        &self,
        index: usize,
        ctx: mineral_protocol::CopyTemplateCtx,
    ) -> Outcome<Result<String, String>> {
        self.request(
            mineral_protocol::Request::RenderCopyTemplate { index, ctx },
            decode_copy_template,
        )
        .await
    }

    /// 渲染复制模板(不等待结论,结果句柄交给调用方 / 完成事件队列)。
    ///
    /// # Params:
    ///   - `index`: 模板下标
    ///   - `ctx`: 模板实体
    ///
    /// # Errors
    /// 本地未提交。
    pub fn render_copy_template_pending(
        &self,
        index: usize,
        ctx: mineral_protocol::CopyTemplateCtx,
    ) -> Result<Pending<Result<String, String>>, SubmitError> {
        self.submit(
            mineral_protocol::Request::RenderCopyTemplate { index, ctx },
            decode_copy_template,
        )
    }

    /// 读 per-song 持久值。
    ///
    /// # Params:
    ///   - `song`: 目标歌
    ///   - `key`: 开放键
    pub async fn store_get(
        &self,
        song: SongId,
        key: &str,
    ) -> Outcome<mineral_protocol::StoreValue> {
        self.request(
            mineral_protocol::Request::StoreGet {
                song,
                key: key.to_owned(),
            },
            |result, request_name| {
                decode_query(result, request_name, |response| match response {
                    mineral_protocol::Response::StoreValue(value) => Some(value),
                    _ => None,
                })
            },
        )
        .await
    }

    /// 写 per-song 持久值。
    ///
    /// # Params:
    ///   - `song`: 目标歌
    ///   - `key`: 开放键
    ///   - `value`: 标量值
    pub async fn store_set(
        &self,
        song: SongId,
        key: &str,
        value: mineral_protocol::StoreValue,
    ) -> Outcome<()> {
        self.request(
            mineral_protocol::Request::StoreSet {
                song,
                key: key.to_owned(),
                value,
            },
            decode_applied,
        )
        .await
    }
}

/// 译码复制模板结论(daemon 的 `Ok` / `Err` 是业务结果,不是连接失败)。
fn decode_copy_template(
    result: OperationResult,
    request_name: &'static str,
) -> Outcome<Result<String, String>> {
    match result {
        OperationResult::Query(response) => match *response {
            mineral_protocol::Response::CopyText(text) => Outcome::Applied(text),
            other => Outcome::Failed {
                kind: FailureKind::Internal,
                detail: format!("{request_name} 收到意外应答 {other:?}"),
            },
        },
        OperationResult::Failed(failure) => Outcome::Failed {
            kind: failure.kind,
            detail: failure.detail,
        },
        _ => Outcome::Failed {
            kind: FailureKind::Internal,
            detail: format!("{request_name} 缺少结果载荷"),
        },
    }
}
