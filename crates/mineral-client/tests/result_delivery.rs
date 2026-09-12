//! 不等待结果的请求仍记录业务失败，等待者接收自己的结论，两者均释放在途额度。

use std::io::{Read, Seek};
use std::time::Duration;

use color_eyre::eyre::eyre;
use mineral_client::Client;
use mineral_client::connection::ClientConfig;
use mineral_client::operation::Outcome;
use mineral_protocol::{
    FailureKind, MessageBatch, OperationFailure, OperationResult, Request, Response,
    SessionMessage, SessionResult,
};
use serde_json::Value;
use tokio::time::timeout;

mod common;

use common::server::{accept_handshake, request_ids};
use common::transport::TestTransport;

/// 不等待者的拒绝日志包含请求身份与原因，等待者仍得到失败，两种结果均释放额度。
/// 使用单线程 runtime，让会话任务共享本测试线程的日志 subscriber。
#[tokio::test(flavor = "current_thread")]
async fn fire_failure_is_logged_and_result_slots_are_released() -> color_eyre::Result<()> {
    for transport in TestTransport::ALL {
        let mut log = tempfile::tempfile()?;
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_ansi(/*ansi*/ false)
            .with_max_level(tracing::Level::WARN)
            .with_writer(log.try_clone()?)
            .finish();
        let _log_guard = tracing::subscriber::set_default(subscriber);

        let (client_wire, server_wire) = transport.pair(/*capacity*/ 8)?;
        let server = tokio::spawn(async move {
            let mut wire = server_wire;
            accept_handshake(&mut wire).await?;
            let batch = wire
                .recv()
                .await?
                .ok_or_else(|| eyre!("没有收到首批请求"))?;
            let mut ids = request_ids(&batch).into_iter();
            assert_eq!(ids.len(), 2, "{transport:?}: {batch:?}");
            let fire_id = ids.next().ok_or_else(|| eyre!("缺少不等待结果的请求"))?;
            let caller_id = ids.next().ok_or_else(|| eyre!("缺少等待结果的请求"))?;
            wire.send(MessageBatch::one(SessionMessage::Results(vec![
                SessionResult {
                    id: fire_id,
                    result: OperationResult::Failed(OperationFailure {
                        kind: FailureKind::Unavailable,
                        detail: "禁止修改播放状态".to_owned(),
                    }),
                },
                SessionResult {
                    id: caller_id,
                    result: OperationResult::Failed(OperationFailure {
                        kind: FailureKind::Unavailable,
                        detail: "诊断信息暂不可用".to_owned(),
                    }),
                },
            ])))
            .await?;

            let batch = wire
                .recv()
                .await?
                .ok_or_else(|| eyre!("没有收到后续请求"))?;
            let mut ids = request_ids(&batch).into_iter();
            assert_eq!(ids.len(), 2, "{transport:?}: {batch:?}");
            let stop_id = ids.next().ok_or_else(|| eyre!("缺少停止请求"))?;
            let query_id = ids.next().ok_or_else(|| eyre!("缺少诊断查询"))?;
            wire.send(MessageBatch::one(SessionMessage::Results(vec![
                SessionResult {
                    id: stop_id,
                    result: OperationResult::Applied,
                },
                SessionResult {
                    id: query_id,
                    result: OperationResult::Query(Box::new(Response::DaemonInfo {
                        pid: std::process::id(),
                    })),
                },
            ])))
            .await?;
            Ok::<_, color_eyre::Report>(fire_id)
        });
        let client = Client::from_wire(
            client_wire,
            "result_delivery",
            ClientConfig {
                max_in_flight: 2,
                ..ClientConfig::default()
            },
        )
        .await?;
        client.fire(Request::Pause);
        let outcome = timeout(Duration::from_secs(/*secs*/ 5), client.daemon_info()).await?;
        match outcome {
            Outcome::Failed { kind, detail } => {
                assert_eq!(kind, FailureKind::Unavailable);
                assert_eq!(detail, "诊断信息暂不可用");
            }
            other => {
                return Err(eyre!(
                    "{transport:?}: 等待者应收到自己的失败结论: {other:?}"
                ));
            }
        }

        // 再同时提交两条请求，确认两种结果都已归还在途额度。
        client.fire(Request::Stop);
        let outcome = timeout(Duration::from_secs(/*secs*/ 5), client.daemon_info()).await?;
        assert!(
            matches!(outcome, Outcome::Applied(pid) if pid == std::process::id()),
            "{transport:?}: 后续请求应成功: {outcome:?}"
        );
        let fire_id = server.await??;
        client.close();

        log.rewind()?;
        let mut output = String::new();
        log.read_to_string(&mut output)?;
        let events = output
            .lines()
            .map(serde_json::from_str::<Value>)
            .collect::<Result<Vec<_>, _>>()?;
        let failures = events
            .iter()
            .filter(|event| {
                event.pointer("/fields/message").and_then(Value::as_str) == Some("daemon 拒绝操作")
            })
            .collect::<Vec<_>>();
        assert_eq!(failures.len(), 1, "{transport:?}: {output}");
        let fields = failures
            .first()
            .and_then(|event| event.get("fields"))
            .ok_or_else(|| eyre!("{transport:?}: 缺少不等待请求的失败日志: {output}"))?;
        assert_eq!(fields.get("method").and_then(Value::as_str), Some("pause"));
        assert_eq!(
            fields.get("request_id").and_then(Value::as_u64),
            Some(fire_id.value())
        );
        assert_eq!(
            fields.get("kind").and_then(Value::as_str),
            Some("Unavailable")
        );
        assert_eq!(
            fields.get("detail").and_then(Value::as_str),
            Some("禁止修改播放状态")
        );
    }
    Ok(())
}
